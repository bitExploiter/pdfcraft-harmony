//! Japanese in text PdfCraft draws with the standard fonts (text-box comments and Fill & Sign
//! text). Characters WinAnsiEncoding lacks but the craft-fonts Japanese Gothic face has are drawn
//! as filled outlines of that face; the rest stays text in the standard font. The typed text is
//! kept where it always was (a comment's `/Contents`), so search and copy are unchanged.

use std::collections::HashMap;
use std::sync::OnceLock;

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

use crate::{CraftFont, GlyphOutline, helvetica_width, literal, win_ansi, win_ansi_encodable};

/// The face Japanese is drawn with next to Helvetica: BIZ UDPGothic Regular when the build has it.
fn face() -> Option<&'static CraftFont> {
    crate::document_japanese_font_for_style(false, false)
}

fn font() -> Option<&'static FontRef<'static>> {
    static FONT: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::new(face()?.bytes).ok()).as_ref()
}

/// The advance of `c` in em in the Japanese face, if the face has it.
fn advance(c: char) -> Option<f64> {
    let font = font()?;
    let gid = font.charmap().map(c)?;
    let loc = LocationRef::default();
    let scale = 1.0 / f64::from(font.metrics(Size::unscaled(), loc).units_per_em.max(1));
    let width = f64::from(font.glyph_metrics(Size::unscaled(), loc).advance_width(gid)?) * scale;
    (width.is_finite() && width > 0.0 && width <= 2.0).then_some(width)
}

/// Whether `c` is drawn with the Japanese face: the standard fonts can't, and the face can.
pub fn japanese_fallback(c: char) -> bool {
    !matches!(c, '\n' | '\r') && !win_ansi_encodable(c) && advance(c).is_some()
}

/// Whether any of `s` is drawn with the Japanese face ([`japanese_fallback`]).
pub fn needs_japanese(s: &str) -> bool {
    s.chars().any(japanese_fallback)
}

/// [`crate::first_undrawable`] for text that may also use the Japanese face: the first character
/// neither the standard fonts nor the face can draw.
pub fn first_undrawable_with_japanese(s: &str) -> Option<char> {
    s.chars().find(|c| !matches!(c, '\n' | '\r') && !win_ansi_encodable(*c) && advance(*c).is_none())
}

/// Advance of `s` at `size` points: Helvetica for what it draws (whole runs, so text without
/// Japanese measures exactly as [`helvetica_width`]), the Japanese face for the rest.
pub fn helvetica_japanese_width(s: &str, size: f64) -> f64 {
    let (mut width, mut latin) = (0.0, String::new());
    for c in s.chars() {
        match advance(c).filter(|_| !win_ansi_encodable(c)) {
            Some(w) => {
                width += helvetica_width(&latin, size) + w * size;
                latin.clear();
            }
            None => latin.push(c),
        }
    }
    width + helvetica_width(&latin, size)
}

/// Draw `line` with its baseline at (`x`, `y`) and `size` points: runs the standard fonts can draw
/// as text in the font resource `font` (Helvetica, WinAnsiEncoding), Japanese as filled outlines.
/// Written outside a text object; the fill colour must already be set. `outlines` caches glyphs
/// across the lines of one appearance.
pub fn helvetica_japanese_line(line: &str, font: &str, size: f64, x: f64, y: f64, outlines: &mut HashMap<char, Option<GlyphOutline>>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pen = x;
    let mut latin = String::new();
    let flush = |latin: &mut String, pen: &mut f64, out: &mut Vec<u8>| {
        if latin.is_empty() {
            return;
        }
        out.extend(format!("BT /{font} {} Tf 1 0 0 1 {} {} Tm ", num(size), num(*pen), num(y)).bytes());
        out.extend(literal(&win_ansi(latin)));
        out.extend_from_slice(b" Tj ET\n");
        *pen += helvetica_width(latin, size);
        latin.clear();
    };
    for c in line.chars() {
        let glyph = if win_ansi_encodable(c) {
            None
        } else {
            outlines.entry(c).or_insert_with(|| face().and_then(|f| crate::japanese_glyph_from(f, c).ok())).as_ref()
        };
        let Some(g) = glyph else {
            latin.push(c);
            continue;
        };
        flush(&mut latin, &mut pen, &mut out);
        if !g.contours.is_empty() {
            out.extend(format!("q {} 0 0 {} {} {} cm\n", num(size), num(size), num(pen), num(y)).bytes());
            for contour in &g.contours {
                for (j, p) in contour.iter().enumerate() {
                    out.extend(format!("{} {} {}\n", num(p[0]), num(p[1]), if j == 0 { "m" } else { "l" }).bytes());
                }
                out.extend_from_slice(b"h\n");
            }
            out.extend_from_slice(b"f Q\n");
        }
        pen += g.width * size;
    }
    flush(&mut latin, &mut pen, &mut out);
    out
}

/// A number for a content stream: at most four decimals, no trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{:.4}", if v.is_finite() { v } else { 0.0 });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face_or_skip() -> bool {
        if face().is_none() {
            eprintln!("skipping the Japanese fallback checks: built without craft-fonts (set CRAFT_FONTS_DIR)");
            return false;
        }
        true
    }

    #[test]
    fn latin_stays_text_and_japanese_becomes_outlines() {
        if !face_or_skip() {
            return;
        }
        assert!(needs_japanese("署名 Ada"));
        assert!(!needs_japanese("Café 5€"));
        assert_eq!(first_undrawable_with_japanese("日本語のテスト、カタカナ。"), None);
        let mut cache = HashMap::new();
        let ops = String::from_utf8_lossy(&helvetica_japanese_line("Hi 日本", "Helv", 10.0, 5.0, 20.0, &mut cache)).into_owned();
        assert!(ops.starts_with("BT /Helv 10 Tf 1 0 0 1 5 20 Tm (Hi ) Tj ET\n"), "{ops}");
        assert_eq!(ops.matches(" f Q").count() + ops.matches("\nf Q").count(), 2, "{ops}");
        assert!(!ops.contains('?'));
        assert!(helvetica_japanese_width("日本", 10.0) > helvetica_width("??", 10.0));
    }
}
