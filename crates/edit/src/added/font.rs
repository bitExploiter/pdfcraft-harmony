//! Add Text uses the same OpenType shaper and Unicode font writer as Edit Text.
use super::*;
use pdfcraft_fonts::{
    embedding,
    source_shaping::{self, ShapedRun},
};

pub(super) struct Row {
    pub text: String,
    pub rtl: bool,
    shaped: ShapedRun,
}

pub(super) fn layout(t: &AddedText, bytes: &[u8]) -> Result<Vec<Row>, EditError> {
    embedding::names(bytes).map_err(EditError::Invalid)?;
    if t.text.len() > 262_144 {
        return Err(EditError::Invalid("use at most 262144 UTF-8 bytes in one text box".into()));
    }
    let width = (t.rect[2] - t.rect[0]).abs().max(t.size) / t.size;
    let mut rows = Vec::new();
    for para in t.text.split('\n') {
        let shaped = source_shaping::shape(bytes, para).map_err(EditError::Invalid)?;
        let rtl = unicode_bidi::get_base_direction(para) == unicode_bidi::Direction::Rtl;
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
                used = source_shaping::shape(bytes, para.get(start..at).unwrap_or_default()).map_err(EditError::Invalid)?.advance;
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
            let shaped = source_shaping::shape_with_direction(bytes, &text, rtl).map_err(EditError::Invalid)?;
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
    Ok(rows)
}

pub(super) fn draw(doc: &mut Document, t: &AddedText, bytes: &[u8], taken: &Dict, out: &mut Vec<u8>) -> Result<Dict, EditError> {
    let rows = layout(t, bytes)?;
    let (_, name) = embedding::names(bytes).map_err(EditError::Invalid)?;
    let mut fonts = taken.clone();
    let mut used = Dict::new();
    // One mapping per item, shared by every row; duplicate cluster/glyph pairs use the same
    // CID. Preflight every row before allocating PDF objects, including late bad clusters.
    let mut mappings = Vec::new();
    let mut codes = HashMap::new();
    let mut row_codes = Vec::new();
    for row in &rows {
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
    let resource = embedding::embed_mapping(doc, &mut fonts, bytes, &name, &mappings).map_err(EditError::Invalid)?;
    if let Some(face) = fonts.get(resource.as_bytes()) {
        used.set(resource.as_bytes().to_vec(), face.clone());
    }
    let r = norm(t.rect);
    let [cr, cg, cb] = t.color.map(|v| v.clamp(0.0, 1.0));
    out.extend(format!("BT {} {} {} rg 0 Tw\n", n(cr), n(cg), n(cb)).bytes());
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
        for (glyph, code) in row.shaped.glyphs.iter().zip(encoded) {
            out.extend(
                format!("1 0 0 1 {} {} Tm <{code:04X}> Tj\n", n(x + glyph.position[0] * t.size + spacing), n(y + glyph.position[1] * t.size)).bytes(),
            );
            spacing += glyph.text.matches(' ').count() as f64 * extra;
        }
        out.extend_from_slice(b"EMC\n");
    }
    out.extend_from_slice(b"ET\n");
    Ok(used)
}
