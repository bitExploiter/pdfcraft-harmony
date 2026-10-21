//! Greek letters and common mathematical signs, drawn from the standard Symbol font.
//!
//! WinAnsiEncoding, which the twelve text fonts are written in, has no Greek. The Symbol font is
//! one of the standard 14, so every viewer has it without an embedded program, and its built-in
//! encoding (ISO 32000-2 Annex D.5) holds the Greek alphabet and a set of mathematical signs.
//! This table maps those characters from Unicode to Symbol's character codes. The glyph names the
//! codes stand for are Adobe's, whose Unicode values come from the Adobe Glyph List (`phi` is
//! U+03C6, `phi1` U+03D5, `theta1` U+03D1, `omega1` U+03D6, `sigma1` U+03C2).

use crate::Std14;

/// (Unicode character, code in Symbol's built-in encoding). Where two characters draw the same
/// glyph, the first row is the one a viewer reports when text is copied (see [`symbol_to_unicode`]).
const SYMBOL: &[(char, u8)] = &[
    // Capital Greek letters.
    ('Α', 0x41),
    ('Β', 0x42),
    ('Χ', 0x43),
    ('Δ', 0x44),
    ('Ε', 0x45),
    ('Φ', 0x46),
    ('Γ', 0x47),
    ('Η', 0x48),
    ('Ι', 0x49),
    ('Κ', 0x4B),
    ('Λ', 0x4C),
    ('Μ', 0x4D),
    ('Ν', 0x4E),
    ('Ο', 0x4F),
    ('Π', 0x50),
    ('Θ', 0x51),
    ('Ρ', 0x52),
    ('Σ', 0x53),
    ('Τ', 0x54),
    ('Υ', 0x55),
    ('Ω', 0x57),
    ('Ξ', 0x58),
    ('Ψ', 0x59),
    ('Ζ', 0x5A),
    // Small Greek letters.
    ('α', 0x61),
    ('β', 0x62),
    ('χ', 0x63),
    ('δ', 0x64),
    ('ε', 0x65),
    ('φ', 0x6A), // Symbol's `phi1` glyph is the looped form U+03C6 shows in text fonts
    ('γ', 0x67),
    ('η', 0x68),
    ('ι', 0x69),
    ('κ', 0x6B),
    ('λ', 0x6C),
    ('μ', 0x6D),
    ('ν', 0x6E),
    ('ο', 0x6F),
    ('π', 0x70),
    ('θ', 0x71),
    ('ρ', 0x72),
    ('σ', 0x73),
    ('τ', 0x74),
    ('υ', 0x75),
    ('ω', 0x77),
    ('ξ', 0x78),
    ('ψ', 0x79),
    ('ζ', 0x7A),
    // Variant forms.
    ('ϑ', 0x4A),        // theta1
    ('ς', 0x56),        // sigma1, the final sigma
    ('ϕ', 0x66),        // Symbol's `phi` glyph is the closed form, U+03D5
    ('ϖ', 0x76),        // omega1, the pi-shaped omega
    ('ϒ', 0xA1),        // Upsilon1
    ('\u{2126}', 0x57), // OHM SIGN draws the capital omega
    // Mathematical signs.
    ('∀', 0x22),
    ('∃', 0x24),
    ('∋', 0x27),
    ('−', 0x2D),
    ('≅', 0x40),
    ('∴', 0x5C),
    ('⊥', 0x5E),
    ('∼', 0x7E),
    ('′', 0xA2),
    ('≤', 0xA3),
    ('∞', 0xA5),
    ('↔', 0xAB),
    ('←', 0xAC),
    ('↑', 0xAD),
    ('→', 0xAE),
    ('↓', 0xAF),
    ('″', 0xB2),
    ('≥', 0xB3),
    ('∝', 0xB5),
    ('∂', 0xB6),
    ('≠', 0xB9),
    ('≡', 0xBA),
    ('≈', 0xBB),
    ('ℵ', 0xC0),
    ('⊗', 0xC4),
    ('⊕', 0xC5),
    ('∅', 0xC6),
    ('∩', 0xC7),
    ('∪', 0xC8),
    ('⊃', 0xC9),
    ('⊇', 0xCA),
    ('⊄', 0xCB),
    ('⊂', 0xCC),
    ('⊆', 0xCD),
    ('∈', 0xCE),
    ('∉', 0xCF),
    ('∠', 0xD0),
    ('∇', 0xD1),
    ('∏', 0xD5),
    ('√', 0xD6),
    ('⋅', 0xD7),
    ('∧', 0xD9),
    ('∨', 0xDA),
    ('⇔', 0xDB),
    ('⇐', 0xDC),
    ('⇑', 0xDD),
    ('⇒', 0xDE),
    ('⇓', 0xDF),
    ('∑', 0xE5),
    ('∫', 0xF2),
];

/// The code of `c` in the Symbol font's built-in encoding, if the font has it.
///
/// A character WinAnsiEncoding already has (`°`, `±`, `×`, `÷`, `µ`, `¬`) is not in this table:
/// it is drawn in the item's own font ([`crate::win_ansi_byte`]).
pub fn symbol_byte(c: char) -> Option<u8> {
    SYMBOL.iter().find(|(u, _)| *u == c).map(|(_, code)| *code)
}

/// Whether the Symbol font draws `c` (and the text fonts don't, so it is the font that does).
pub fn symbol_encodable(c: char) -> bool {
    symbol_byte(c).is_some()
}

/// `s` in the Symbol font's codes; a character it has no code for becomes `?` (check with
/// [`symbol_encodable`] first).
pub fn symbol_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| symbol_byte(c).unwrap_or(b'?')).collect()
}

/// The advance of `s` in the Symbol font at `size` points.
pub fn symbol_width(s: &str, size: f64) -> f64 {
    symbol_bytes(s).iter().map(|&code| Std14::Symbol.width(code)).sum::<f64>() * size / 1000.0
}

/// The `bfchar` entries of a `ToUnicode` CMap for the Symbol font: one line per code, `<code>
/// <UTF-16BE>`, so copying and searching text drawn from it finds the Unicode characters. The
/// glyph names Symbol's built-in encoding uses map to Unicode only through the Adobe Glyph List,
/// which not every viewer applies to a font it has no program for.
pub fn symbol_to_unicode() -> Vec<String> {
    let mut seen = [false; 256];
    let mut lines = Vec::with_capacity(SYMBOL.len());
    for (c, code) in SYMBOL {
        let slot = usize::from(*code);
        if seen.get(slot).copied().unwrap_or(true) {
            continue;
        }
        if let Some(s) = seen.get_mut(slot) {
            *s = true;
        }
        let hex: String = c.to_string().encode_utf16().map(|u| format!("{u:04X}")).collect();
        lines.push(format!("<{code:02X}> <{hex}>\n"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greek_letters_take_symbols_codes() {
        // Spot checks against Annex D.5: `alpha` is 0x61, `Omega` 0x57. The two phis are swapped on purpose:
        // Symbol's `phi` (0x66) is drawn closed and its `phi1` (0x6A) looped, the other way round from
        // the Adobe Glyph List's names, so U+03C6 (the looped form in text fonts) takes 0x6A.
        assert_eq!(symbol_byte('α'), Some(0x61));
        assert_eq!(symbol_byte('φ'), Some(0x6A));
        assert_eq!(symbol_byte('ϕ'), Some(0x66));
        assert_eq!(symbol_byte('Ω'), Some(0x57));
        assert_eq!(symbol_byte('Σ'), Some(0x53));
        assert_eq!(symbol_byte('π'), Some(0x70));
        assert_eq!(symbol_byte('∞'), Some(0xA5));
        assert_eq!(symbol_byte('∑'), Some(0xE5));
        // The four double arrows: left, up, right, down.
        assert_eq!(['⇐', '⇑', '⇒', '⇓'].map(symbol_byte), [Some(0xDC), Some(0xDD), Some(0xDE), Some(0xDF)]);
        assert_eq!(['←', '↑', '→', '↓'].map(symbol_byte), [Some(0xAC), Some(0xAD), Some(0xAE), Some(0xAF)]);
    }

    #[test]
    fn every_code_is_a_glyph_the_font_has() {
        for (c, code) in SYMBOL {
            assert!(Std14::Symbol.width(*code) > 0.0, "{c} (code {code:#04x}) has no width in Symbol");
        }
    }

    #[test]
    fn the_text_fonts_keep_what_they_already_draw() {
        // Characters WinAnsiEncoding has stay in the item's font; the table adds only what it lacks.
        for (c, _) in SYMBOL {
            assert!(!crate::win_ansi_encodable(*c), "{c} is in WinAnsiEncoding already");
        }
        assert!(!symbol_encodable('°') && crate::win_ansi_encodable('°'));
    }

    #[test]
    fn a_character_is_listed_once_per_unicode_value() {
        for (i, (c, _)) in SYMBOL.iter().enumerate() {
            assert!(!SYMBOL.iter().skip(i + 1).any(|(d, _)| d == c), "{c} is listed twice");
        }
    }

    #[test]
    fn width_sums_symbols_own_metrics() {
        // alpha 631, phi1 (the looped U+03C6) 603 in the published Symbol metrics.
        assert_eq!(Std14::Symbol.width(0x61), 631.0);
        assert!((symbol_width("α", 10.0) - 6.31).abs() < 1e-9);
        assert!((symbol_width("αφ", 10.0) - 12.34).abs() < 1e-9);
        assert_eq!(symbol_width("", 12.0), 0.0);
    }

    #[test]
    fn copy_map_has_one_entry_per_code_and_names_the_first_character() {
        let lines = symbol_to_unicode();
        // OHM SIGN shares Omega's code, so Omega is the one reported.
        assert!(lines.contains(&"<57> <03A9>\n".to_string()));
        assert!(!lines.iter().any(|l| l.contains("2126")));
        let mut codes: Vec<&str> = lines.iter().map(|l| &l[..4]).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), lines.len());
    }
}
