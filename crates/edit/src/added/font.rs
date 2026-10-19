//! Add Text uses the same OpenType shaper and Unicode font writer as Edit Text.
use super::*;
use pdfcraft_fonts::{
    embedding,
    source_shaping::{ShapedRun, ShapingFont},
};
use std::sync::Arc;

pub(super) struct Layout {
    pub rows: Vec<Row>,
    pub natural_width: f64,
    name: String,
}

struct CachedLayout {
    text: String,
    font: Arc<Vec<u8>>,
    width: f64,
    result: Result<Arc<Layout>, String>,
}

thread_local! {
    // One bounded layout per worker/UI thread. Font ownership makes pointer comparison safe;
    // colour, alignment and box translation can reuse shaping. Errors are cached as well.
    static LAST: std::cell::RefCell<Option<CachedLayout>> = const { std::cell::RefCell::new(None) };
}

pub(super) struct Row {
    pub text: String,
    pub rtl: bool,
    shaped: ShapedRun,
}

/// Reuse the last bounded layout when text, normalized width and font owner match.
pub(super) fn layout(t: &AddedText, bytes: &[u8]) -> Result<Arc<Layout>, EditError> {
    if t.text.len() > 262_144 {
        return Err(EditError::Invalid("use at most 262144 UTF-8 bytes in one text box".into()));
    }
    let width = (t.rect[2] - t.rect[0]).abs().max(t.size) / t.size;
    let program = t.font_program();
    LAST.with(|last| {
        if let Some(old) = last.borrow().as_ref()
            && old.text == t.text
            && old.width == width
            && program.as_ref().is_some_and(|font| Arc::ptr_eq(&old.font, font))
        {
            return old.result.clone().map_err(EditError::Invalid);
        }
        let result = prepare(t, bytes, width).map(Arc::new).map_err(|e| e.to_string());
        if let Some(font) = program {
            *last.borrow_mut() = Some(CachedLayout { text: t.text.clone(), font, width, result: result.clone() });
        }
        result.map_err(EditError::Invalid)
    })
}

/// Shape paragraphs once and wrap only at cluster boundaries, with mapping limits checked.
fn prepare(t: &AddedText, bytes: &[u8], width: f64) -> Result<Layout, EditError> {
    let (_, name) = embedding::names(bytes).map_err(EditError::Invalid)?;
    let shaper = ShapingFont::new(bytes).map_err(EditError::Invalid)?;
    let mut rows = Vec::new();
    let mut natural_width: f64 = 0.0;
    let text = t.text.replace("\r\n", "\n").replace('\r', "\n");
    for para in text.split('\n') {
        let rtl = unicode_bidi::get_base_direction(para) == unicode_bidi::Direction::Rtl;
        let shaped = shaper.shape(para, rtl).map_err(EditError::Invalid)?;
        natural_width = natural_width.max(shaped.advance);
        if shaped.advance <= width {
            rows.push(Row { text: para.to_string(), rtl, shaped });
            continue;
        }
        // Logical cluster boundaries include ligatures and stacked marks. Never wrap inside
        // one, even when the box is narrower than that cluster. Sorting also handles bidi.
        let mut clusters = std::collections::BTreeMap::new();
        for glyph in &shaped.glyphs {
            let entry = clusters.entry(glyph.cluster.start).or_insert((glyph.cluster.end, 0.0));
            entry.1 += glyph.advance;
        }
        let (mut start, mut end, mut used) = (0, 0, 0.0);
        let mut break_at = None;
        let mut spans = Vec::new();
        for (at, (next, advance)) in clusters {
            if used + advance > width && end > start {
                let cut = break_at.filter(|cut| *cut > start).unwrap_or(at);
                spans.push(start..cut);
                start = cut;
                // A word carried to the next row is measured with its actual context.
                used = shaper.shape(para.get(start..at).unwrap_or_default(), rtl).map_err(EditError::Invalid)?.advance;
                break_at = None;
            }
            used += advance;
            end = next;
            if para.get(at..next).is_some_and(|s| s.ends_with(' ')) {
                break_at = Some(next);
            }
        }
        spans.push(start..para.len());
        for span in spans {
            let text = para.get(span).unwrap_or_default().to_string();
            let shaped = shaper.shape(&text, rtl).map_err(EditError::Invalid)?;
            rows.push(Row { text, rtl, shaped });
        }
    }
    let mut mappings = HashSet::new();
    for glyph in rows.iter().flat_map(|row| &row.shaped.glyphs) {
        mappings.insert((glyph.text.as_str(), glyph.id.to_u32()));
        if glyph.text.len() > 4096 || mappings.len() > 65_534 {
            return Err(EditError::Invalid("shaped font mapping exceeds its budget; use a shorter text box".into()));
        }
    }
    Ok(Layout { rows, natural_width, name })
}

/// Embed a validated layout with logical ActualText and precisely positioned TJ batches.
pub(super) fn draw(doc: &mut Document, t: &AddedText, bytes: &[u8], taken: &Dict, out: &mut Vec<u8>) -> Result<Dict, EditError> {
    let layout = layout(t, bytes)?;
    let rows = &layout.rows;
    let mut fonts = taken.clone();
    let mut used = Dict::new();
    // One mapping per item, shared by every row; duplicate cluster/glyph pairs use the same
    // CID. Preflight every row before allocating PDF objects, including late bad clusters.
    let mut mappings = Vec::new();
    let mut codes = HashMap::new();
    let mut row_codes = Vec::new();
    for row in rows {
        let mut encoded = Vec::new();
        for glyph in &row.shaped.glyphs {
            let key = (glyph.text.clone(), glyph.id.to_u32());
            let code = if let Some(code) = codes.get(&key) {
                *code
            } else {
                let code = u16::try_from(mappings.len().saturating_add(1)).map_err(|_| EditError::Invalid("shaped glyph limit".into()))?;
                mappings.push((glyph.text.clone(), glyph.id));
                codes.insert(key, code);
                code
            };
            encoded.push(code);
        }
        row_codes.push(encoded);
    }
    let resource = embedding::embed_mapping(doc, &mut fonts, bytes, &layout.name, &mappings).map_err(EditError::Invalid)?;
    if let Some(face) = fonts.get(resource.as_bytes()) {
        used.set(resource.as_bytes().to_vec(), face.clone());
    }
    let r = norm(t.rect);
    let [cr, cg, cb] = t.color.map(|v| v.clamp(0.0, 1.0));
    out.extend(format!("BT {} {} {} rg 0 Tc 0 Tw 100 Tz\n", n(cr), n(cg), n(cb)).bytes());
    for (row_index, (row, encoded)) in rows.iter().zip(&row_codes).enumerate() {
        let logical: String = row.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
        out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n/{resource} {} Tf\n", n(t.size)).bytes());
        let w = row.shaped.advance * t.size;
        let spaces = row.text.matches(' ').count();
        let extra =
            if t.align == Align::Justify && row_index + 1 < rows.len() && spaces > 0 { ((r[2] - r[0]) - w).max(0.0) / spaces as f64 } else { 0.0 };
        let x = match t.align {
            Align::Justify if row.rtl => r[2] - w - extra * spaces as f64,
            Align::Left | Align::Justify => r[0],
            Align::Center => r[0] + (r[2] - r[0] - w) / 2.0,
            Align::Right => r[2] - w,
        };
        let y = r[3] - (row_index as f64 * 1.2 + 0.95) * t.size;
        let mut spacing = 0.0;
        let mut baseline = None;
        let mut cursor = 0.0;
        for (glyph, code) in row.shaped.glyphs.iter().zip(encoded) {
            let gx = glyph.position[0] + spacing / t.size;
            // Batch a baseline as one TJ array. Vertical mark offsets start a new array;
            // adjustments retain kerning, bidi order and the shaper's precise positions.
            if baseline != Some(glyph.position[1]) {
                if baseline.is_some() {
                    out.extend_from_slice(b"] TJ\n");
                }
                out.extend(format!("1 0 0 1 {} {} Tm [", n(x + gx * t.size), n(y + glyph.position[1] * t.size)).bytes());
                baseline = Some(glyph.position[1]);
            } else {
                let adjustment = (cursor - gx) * 1000.0;
                if adjustment.abs() > 0.000_001 {
                    out.extend(format!("{} ", matrix_number(adjustment)).bytes());
                }
            }
            out.extend(format!("<{code:04X}> ").bytes());
            cursor = gx + glyph.nominal_advance;
            spacing += glyph.text.matches(' ').count() as f64 * extra;
        }
        if baseline.is_some() {
            out.extend_from_slice(b"] TJ\n");
        }
        out.extend_from_slice(b"EMC\n");
    }
    out.extend_from_slice(b"ET\n");
    Ok(used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_batches_preserve_every_shaped_glyph_position() {
        let mut cases = vec![(pdfcraft_fonts::inter_regular_shared(), "AV Привет office e\u{301}")];
        for (variable, text) in [("PDFCRAFT_TEST_THAI_FONT", "Hello น้ำ ผู้ใช้ ปู่"), ("PDFCRAFT_TEST_OTF_FONT", "AV Привет office Ελληνικά")]
        {
            if let Some(path) = std::env::var_os(variable) {
                cases.push((Arc::new(std::fs::read(path).unwrap()), text));
            }
        }
        for (font, text) in cases {
            let t = AddedText { text: text.into(), rect: [10.0, 100.0, 500.0, 300.0], size: 17.0, font: Some(font), ..Default::default() };
            let bytes = t.font_bytes().unwrap();
            let prepared = layout(&t, bytes).unwrap();
            let expected: Vec<_> = prepared
                .rows
                .iter()
                .enumerate()
                .flat_map(|(row, r)| {
                    r.shaped
                        .glyphs
                        .iter()
                        .map(move |g| [10.0 + g.position[0] * 17.0, 300.0 - (row as f64 * 1.2 + 0.95) * 17.0 + g.position[1] * 17.0])
                })
                .collect();
            let mut doc = Document::new_empty();
            let mut output = Vec::new();
            let fonts = draw(&mut doc, &t, bytes, &Dict::new(), &mut output).unwrap();
            let face = doc.dict(fonts.iter().next().unwrap().1).unwrap();
            let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &face);
            let mut position = [0.0; 2];
            let mut actual = Vec::new();
            for op in pdfcraft_content::parse(&output).ops {
                if op.is("Tm") {
                    position = [op.num(4).unwrap(), op.num(5).unwrap()];
                }
                if op.is("TJ") {
                    for entry in op.operands.first().unwrap().as_array().unwrap() {
                        if let Some(value) = entry.as_f64() {
                            position[0] -= value / 1000.0 * 17.0;
                        }
                        if let Some(string) = entry.as_string() {
                            for (code, _) in metrics.codes(&string.bytes) {
                                actual.push(position);
                                position[0] += metrics.width(code) * 17.0;
                            }
                        }
                    }
                }
            }
            assert_eq!(actual.len(), expected.len());
            for (a, b) in actual.iter().zip(expected) {
                assert!((a[0] - b[0]).abs() < 0.002 && (a[1] - b[1]).abs() < 0.002, "{text}: {a:?} != {b:?}");
            }
        }
    }

    #[test]
    fn added_layout_is_shared_by_validation_measurement_and_drawing() {
        for font in [None, Some(pdfcraft_fonts::inter_regular_shared())] {
            let t = AddedText { text: "Привет office".into(), rect: [10.0, 100.0, 400.0, 300.0], font, ..Default::default() };
            let bytes = t.font_bytes().unwrap();
            let prepared = layout(&t, bytes).unwrap();
            validate(&Content::Text(t.clone())).unwrap();
            text_width(&t);
            text_rect(&t);
            let mut doc = Document::new_empty();
            draw(&mut doc, &t, bytes, &Dict::new(), &mut Vec::new()).unwrap();
            assert!(Arc::ptr_eq(&prepared, &layout(&t, bytes).unwrap()));
            let restyled = AddedText { color: [1.0, 0.0, 0.0], align: Align::Right, ..t.clone() };
            assert!(Arc::ptr_eq(&prepared, &layout(&restyled, bytes).unwrap()));
            for changed in [
                AddedText { text: "Другое".into(), ..t.clone() },
                AddedText { rect: [10.0, 100.0, 30.0, 300.0], ..t.clone() },
                AddedText { font: Some(Arc::new(include_bytes!("../../../../assets/fonts/JetBrainsMono-Regular.ttf").to_vec())), ..t.clone() },
            ] {
                assert!(!Arc::ptr_eq(&prepared, &layout(&changed, changed.font_bytes().unwrap()).unwrap()));
            }
        }
    }
}
