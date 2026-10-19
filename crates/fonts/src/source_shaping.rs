//! Shaping an explicitly selected source/document face, with OpenType features and UAX #9.
use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId};
use unicode_bidi::{Level, ParagraphBidiInfo};
use unicode_script::{Script, UnicodeScript};

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    pub position: [f64; 2],
    pub text: String,
    /// Byte range in the logical input; shared by every glyph of a shaping cluster.
    pub cluster: std::ops::Range<usize>,
    pub advance: f64,
}
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    pub advance: f64,
}

/// Complex scripts/marks require shaping instead of independently encoding characters.
pub fn required(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(u32::from(c), 0x0300..=0x036f | 0x0590..=0x0fff | 0x1780..=0x17ff | 0x200c..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069))
}

pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    shape_with_direction(bytes, text, unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl)
}

/// Shape a wrapped row with its paragraph's base direction, including rows whose first
/// strong character has a different direction from the start of the paragraph.
pub fn shape_with_direction(bytes: &[u8], text: &str, rtl: bool) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0 });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    if text.chars().any(|c| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x08a0..=0x0fff | 0x1780..=0x17ff)) && face.gsub().is_err() {
        return Err("the source subset has no contextual substitution tables; select its full font".into());
    }
    let data = ShaperData::new(&face);
    let shaper = data.shaper(&face).build();
    let em = f64::from(shaper.units_per_em().max(1));
    let base = if rtl { Level::rtl() } else { Level::ltr() };
    let bidi = ParagraphBidiInfo::new(text, Some(base));
    let (levels, runs) = bidi.visual_runs(0..text.len());
    let mut glyphs = Vec::new();
    let mut x = 0.0;
    for run in runs {
        let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        let mut scripts = script_runs(text, run);
        if rtl {
            scripts.reverse();
        }
        for run in scripts {
            let Some(input) = text.get(run.clone()) else { continue };
            let mut buffer = UnicodeBuffer::new();
            buffer.push_str(input);
            buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
            buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
            buffer.guess_segment_properties();
            let output = shaper.shape(buffer, ShapeOptions::new());
            let mut starts: Vec<_> = output.glyph_infos().iter().map(|g| g.cluster as usize).collect();
            starts.sort_unstable();
            starts.dedup();
            let mut seen = std::collections::HashSet::new();
            for (g, p) in output.glyph_infos().iter().zip(output.glyph_positions()) {
                let start = g.cluster as usize;
                if g.glyph_id == 0 {
                    let missing =
                        input.get(start..).and_then(|s| s.chars().next()).map(|c| format!(" “{c}” (U+{:04X})", u32::from(c))).unwrap_or_default();
                    return Err(format!("the selected font lacks a required glyph{missing}; load a complete font with the required glyphs"));
                }
                let end = starts.get(starts.partition_point(|i| *i <= start)).copied().unwrap_or(input.len());
                let logical = if seen.insert(start) { input.get(start..end).unwrap_or_default().to_string() } else { String::new() };
                glyphs.push(ShapedGlyph {
                    id: GlyphId::new(g.glyph_id),
                    position: [x + f64::from(p.x_offset) / em, f64::from(p.y_offset) / em],
                    text: logical,
                    cluster: run.start + start..run.start + end,
                    advance: f64::from(p.x_advance) / em,
                });
                x += f64::from(p.x_advance) / em;
            }
        }
    }
    if !x.is_finite() || glyphs.len() > 65_534 {
        return Err("shaped row exceeds its geometry budget".into());
    }
    Ok(ShapedRun { glyphs, advance: x })
}

/// OpenType shaping runs must have one script as well as one bidi level. Common and
/// inherited characters stay with their neighbours so combining marks never lose their base.
fn script_runs(text: &str, range: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    let mut result = Vec::new();
    let mut start = range.start;
    let mut current = None;
    for (at, c) in text.get(range.clone()).unwrap_or_default().char_indices() {
        let script = c.script();
        if matches!(script, Script::Common | Script::Inherited | Script::Unknown) {
            continue;
        }
        if current.is_some_and(|s| s != script) {
            result.push(start..range.start + at);
            start = range.start + at;
        }
        current = Some(script);
    }
    result.push(start..range.end);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_itemization_keeps_marks_with_their_base() {
        let text = "Hello น้ำ e\u{301} Ελληνικά";
        let runs = script_runs(text, 0..text.len());
        let segments: Vec<_> = runs.iter().map(|r| text.get(r.clone()).unwrap()).collect();
        assert_eq!(segments, ["Hello ", "น้ำ ", "e\u{301} ", "Ελληνικά"]);
        assert_eq!(segments.concat(), text);
    }

    #[test]
    fn shaped_clusters_partition_logical_unicode_once() {
        let text = "Привет e\u{301} office Ελληνικά";
        let run = shape(crate::INTER_REGULAR, text).unwrap();
        let mut clusters = std::collections::BTreeMap::new();
        for glyph in &run.glyphs {
            assert!(text.get(glyph.cluster.clone()).is_some());
            if !glyph.text.is_empty() {
                clusters.insert(glyph.cluster.start, glyph.text.clone());
            }
            assert!(glyph.position.iter().all(|p| p.is_finite()));
        }
        assert_eq!(clusters.into_values().collect::<String>(), text);
        assert!(shape(crate::INTER_REGULAR, "日").is_err());
        assert!(shape(crate::INTER_REGULAR, &"a".repeat(262_145)).is_err());
        assert!(shape(crate::INTER_REGULAR, "a\nb").is_err());
    }
}
