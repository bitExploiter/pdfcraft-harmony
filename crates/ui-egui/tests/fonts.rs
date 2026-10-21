//! The interface fonts with and without the optional craft-fonts build input (`CRAFT_FONTS_DIR`).

use egui::epaint::text::{Fonts, TextOptions};
use egui::{Color32, FontFamily, FontId};
use pdfcraft_ui_egui::theme;

const JAPANESE: &str = "日本語の文字";
const CHINESE: &str = "简体中文欢迎";
const ARABIC: &str = "واحد اثنين";
const TELUGU: &str = "తెలుగు ఫైల్";

fn families() -> Vec<FontId> {
    vec![FontId::proportional(13.0), FontId::monospace(13.0), theme::medium(13.0), theme::semibold(17.0)]
}

/// Lays `text` out in every interface family and returns each galley's width.
fn layout_widths(fonts: &mut Fonts, text: &str) -> Vec<f32> {
    let mut view = fonts.with_pixels_per_point(2.0);
    families().into_iter().map(|id| view.layout_no_wrap(text.to_owned(), id, Color32::BLACK).size().x).collect()
}

/// Built with craft-fonts, Japanese text renders with real glyphs (no tofu) in every family,
/// from a craft-fonts face placed after the app's own fonts.
#[test]
fn japanese_ui_text_uses_craft_fonts() {
    if pdfcraft_fonts::ui_japanese_fonts().is_empty() {
        eprintln!("skipping japanese_ui_text_uses_craft_fonts: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
        return;
    }
    let defs = theme::font_definitions();
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first_jp = stack.iter().position(|n| n.starts_with("BIZ UDPGothic")).expect("BIZ UDPGothic is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first_jp, "{family:?}: {stack:?}");
        assert!(stack[first_jp..].iter().all(|n| pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.name() == *n)), "{stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, JAPANESE), "{id:?} lacks {JAPANESE}");
    }
    // Real glyphs are about a full em wide each; tofu boxes and missing glyphs are not.
    for w in layout_widths(&mut fonts, JAPANESE) {
        assert!(w > 13.0 * 0.8 * JAPANESE.chars().count() as f32, "{w}");
    }
}

/// Chinese mode orders the CJK fallback with the `Hans` group first, so one line never
/// mixes faces with different vertical metrics. Without an allowed `Hans` face in the
/// build input the order part still holds; glyph coverage follows the craft-fonts checkout.
#[test]
fn chinese_ui_text_prefers_the_chinese_face() {
    if pdfcraft_fonts::ui_chinese_fonts().is_empty() {
        eprintln!(
            "skipping chinese_ui_text_prefers_the_chinese_face: no Hans face bundled (set CRAFT_FONTS_DIR with an allowed Chinese face to run it)"
        );
        return;
    }
    let defs = theme::font_definitions_for(true);
    let hans: Vec<String> = pdfcraft_fonts::ui_chinese_fonts().iter().map(|f| f.name()).collect();
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first_zh = stack.iter().position(|n| hans.iter().any(|h| h == n)).expect("a Hans face is a fallback");
        let first_ja = stack.iter().position(|n| n.starts_with("BIZ UDPGothic")).expect("BIZ UDPGothic is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first_zh && first_zh < first_ja, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, CHINESE), "{id:?} lacks {CHINESE}");
    }
    for w in layout_widths(&mut fonts, CHINESE) {
        assert!(w > 13.0 * 0.8 * CHINESE.chars().count() as f32, "{w}");
    }
}

/// Built with a craft-fonts Arabic face, Arabic text has real glyphs in every family, from a
/// face placed after the app's own fonts.
#[test]
fn arabic_ui_text_uses_craft_fonts() {
    let arabic: Vec<String> = pdfcraft_fonts::ui_arabic_fonts().iter().map(|f| f.name()).collect();
    if arabic.is_empty() {
        eprintln!("skipping arabic_ui_text_uses_craft_fonts: no Arab face bundled (set CRAFT_FONTS_DIR with an Arabic face to run it)");
        return;
    }
    let defs = theme::font_definitions();
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first_ar = stack.iter().position(|n| arabic.contains(n)).expect("an Arab face is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first_ar, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, ARABIC), "{id:?} lacks {ARABIC}");
    }
}

/// Built with a craft-fonts Telugu face, Telugu text has real glyphs in every family, from a
/// face placed after the app's own fonts, and is shaped: the conjunct క్ష is narrower than క్
/// and ష held apart by a zero-width non-joiner.
#[test]
fn telugu_ui_text_uses_craft_fonts() {
    let telugu: Vec<String> = pdfcraft_fonts::CRAFT_FONTS.iter().filter(|f| f.covers("Telu")).map(|f| f.name()).collect();
    if telugu.is_empty() {
        eprintln!("skipping telugu_ui_text_uses_craft_fonts: no Telu face bundled (set CRAFT_FONTS_DIR with a Telugu face to run it)");
        return;
    }
    let defs = theme::font_definitions();
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first_te = stack.iter().position(|n| telugu.contains(n)).expect("a Telu face is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first_te, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, TELUGU), "{id:?} lacks {TELUGU}");
    }
    let joined = layout_widths(&mut fonts, "క్ష");
    let apart = layout_widths(&mut fonts, "క్\u{200c}ష");
    for (j, a) in joined.iter().zip(&apart) {
        assert!(j < a, "క్ష is one conjunct, narrower than క్‌ష: {j} vs {a}");
    }
}

/// On a machine with a suitable installed font, the installed definitions end every family
/// with it and Arabic text has glyphs; the embedded-only definitions never name it.
#[test]
fn system_fallback_fills_missing_scripts() {
    assert!(!theme::font_definitions().font_data.contains_key(theme::SYSTEM_FALLBACK));
    let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
    if !defs.font_data.contains_key(theme::SYSTEM_FALLBACK) {
        eprintln!("skipping system_fallback_fills_missing_scripts: no installed fallback font (or PDFCRAFT_SYSTEM_FONTS=0)");
        assert!(defs.families.values().all(|stack| !stack.iter().any(|n| n == theme::SYSTEM_FALLBACK)));
        return;
    }
    for (family, stack) in &defs.families {
        assert_eq!(stack.last().map(String::as_str), Some(theme::SYSTEM_FALLBACK), "{family:?}: {stack:?}");
        assert_eq!(stack.iter().filter(|n| *n == theme::SYSTEM_FALLBACK).count(), 1, "{family:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, ARABIC), "{id:?} lacks {ARABIC}");
        assert!(fonts.has_glyphs(&id, "PdfCraft"), "{id:?}");
    }
    assert!(layout_widths(&mut fonts, ARABIC).iter().all(|w| w.is_finite() && *w > 0.0));
}

/// On a machine with a suitable installed Devanagari font, the installed definitions include
/// it and Hindi text has glyphs; the embedded-only definitions never name it.
#[test]
fn system_fallback_covers_devanagari() {
    assert!(!theme::font_definitions().font_data.contains_key(theme::SYSTEM_DEVANAGARI_FALLBACK));
    let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
    if !defs.font_data.contains_key(theme::SYSTEM_DEVANAGARI_FALLBACK) {
        eprintln!("skipping system_fallback_covers_devanagari: no installed Devanagari fallback font (or PDFCRAFT_SYSTEM_FONTS=0)");
        assert!(defs.families.values().all(|stack| !stack.iter().any(|n| n == theme::SYSTEM_DEVANAGARI_FALLBACK)));
        return;
    }
    for (family, stack) in &defs.families {
        assert!(stack.iter().any(|n| n == theme::SYSTEM_DEVANAGARI_FALLBACK), "{family:?}: {stack:?}");
        assert_eq!(stack.iter().filter(|n| *n == theme::SYSTEM_DEVANAGARI_FALLBACK).count(), 1, "{family:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    const DEVANAGARI: &str = "हिंदी पाठ सम्पादन";
    for id in families() {
        assert!(fonts.has_glyphs(&id, DEVANAGARI), "{id:?} lacks {DEVANAGARI}");
    }
    assert!(layout_widths(&mut fonts, DEVANAGARI).iter().all(|w| w.is_finite() && *w > 0.0));
}

/// On a machine with an installed Telugu font, the installed definitions include it and Telugu
/// text has glyphs, so the language's own name in Preferences is not boxes (#1531). macOS always
/// ships Kohinoor Telugu, so there the test never skips.
#[test]
fn system_fallback_covers_telugu() {
    assert!(!theme::font_definitions().font_data.contains_key(theme::SYSTEM_TELUGU_FALLBACK));
    let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
    let off = std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0");
    if !defs.font_data.contains_key(theme::SYSTEM_TELUGU_FALLBACK) && (off || !cfg!(target_os = "macos")) {
        eprintln!("skipping system_fallback_covers_telugu: no installed Telugu fallback font (or PDFCRAFT_SYSTEM_FONTS=0)");
        assert!(defs.families.values().all(|stack| !stack.iter().any(|n| n == theme::SYSTEM_TELUGU_FALLBACK)));
        return;
    }
    for (family, stack) in &defs.families {
        assert_eq!(stack.iter().filter(|n| *n == theme::SYSTEM_TELUGU_FALLBACK).count(), 1, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, TELUGU), "{id:?} lacks {TELUGU}");
    }
    assert!(layout_widths(&mut fonts, TELUGU).iter().all(|w| w.is_finite() && *w > 0.0));
}

/// On a machine with installed Sinhala and Tamil fonts, the installed definitions include them
/// and Sinhala and Tamil text, such as a file name, has glyphs instead of boxes (#1688). macOS
/// always ships Sinhala Sangam MN and Tamil Sangam MN, so there the test never skips.
#[test]
fn system_fallback_covers_sinhala_and_tamil() {
    for (name, text) in [(theme::SYSTEM_SINHALA_FALLBACK, "ශ්‍රී ලංකා ගොනුව"), (theme::SYSTEM_TAMIL_FALLBACK, "தமிழ் கோப்பு")]
    {
        assert!(!theme::font_definitions().font_data.contains_key(name));
        let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
        let off = std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0");
        if !defs.font_data.contains_key(name) && (off || !cfg!(target_os = "macos")) {
            eprintln!("skipping {name}: no installed fallback font (or PDFCRAFT_SYSTEM_FONTS=0)");
            assert!(defs.families.values().all(|stack| !stack.iter().any(|n| n == name)));
            continue;
        }
        for (family, stack) in &defs.families {
            assert_eq!(stack.iter().filter(|n| *n == name).count(), 1, "{family:?}: {stack:?}");
        }
        let mut fonts = Fonts::new(TextOptions::default(), defs);
        for id in families() {
            assert!(fonts.has_glyphs(&id, text), "{id:?} lacks {text}");
        }
        assert!(layout_widths(&mut fonts, text).iter().all(|w| w.is_finite() && *w > 0.0));
    }
}

/// The Han characters of a catalog's translations.
fn catalog_han(catalog: &str) -> String {
    let mut han: Vec<char> = catalog
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').nth(2))
        .flat_map(str::chars)
        .filter(|c| matches!(*c, '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'))
        .collect();
    han.sort_unstable();
    han.dedup();
    han.into_iter().collect()
}

/// In a Chinese interface built without a `Hans` face (every release so far, #826), a Chinese
/// face installed on this machine goes before the Japanese faces, which lack hundreds of the
/// catalog's characters, and draws every Han character of both Chinese catalogs.
#[test]
fn chinese_ui_uses_an_installed_chinese_face() {
    if !pdfcraft_fonts::ui_chinese_fonts().is_empty() {
        eprintln!("skipping chinese_ui_uses_an_installed_chinese_face: a Hans face is bundled");
        return;
    }
    for (cjk, catalog) in [
        (theme::CjkPreference::SimplifiedChinese, include_str!("../src/i18n/zh-hans.tsv")),
        (theme::CjkPreference::TraditionalChinese, include_str!("../src/i18n/zh-hant.tsv")),
    ] {
        let defs = theme::installed_font_definitions(cjk);
        if !defs.font_data.contains_key(theme::SYSTEM_FALLBACK) {
            eprintln!("skipping chinese_ui_uses_an_installed_chinese_face: no installed Chinese face (or PDFCRAFT_SYSTEM_FONTS=0)");
            return;
        }
        for (family, stack) in &defs.families {
            let system = stack.iter().position(|n| n == theme::SYSTEM_FALLBACK).expect("in every family");
            assert_eq!(stack.iter().filter(|n| *n == theme::SYSTEM_FALLBACK).count(), 1, "{family:?}");
            let own = stack.iter().position(|n| n.starts_with("Inter") || n == "JetBrainsMono").expect("the app's own font");
            assert!(own < system, "{family:?}: {stack:?}");
            if let Some(first_ja) = stack.iter().position(|n| n.starts_with("BIZ UDPGothic")) {
                assert!(system < first_ja, "{family:?}: {stack:?}");
            }
        }
        let han = catalog_han(catalog);
        assert!(han.contains('欢') || han.contains('說'), "{cjk:?}");
        let mut fonts = Fonts::new(TextOptions::default(), defs);
        for id in families() {
            let missing: String = han.chars().filter(|c| !fonts.has_glyphs(&id, &c.to_string())).collect();
            assert!(missing.is_empty(), "{cjk:?} {id:?} lacks {missing}");
        }
    }
    // Other languages keep the installed face last, as before.
    let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
    for stack in defs.families.values() {
        assert!(stack.iter().position(|n| n == theme::SYSTEM_FALLBACK).is_none_or(|i| i + 1 == stack.len()), "{stack:?}");
    }
}

/// Without craft-fonts the interface fonts still install and lay out any text (Japanese falls
/// back to egui's replacement glyph) without panicking; Latin text is unaffected.
#[test]
fn ui_fonts_work_without_craft_fonts() {
    let mut fonts = Fonts::new(TextOptions::default(), theme::font_definitions());
    for id in families() {
        assert!(fonts.has_glyphs(&id, "PdfCraft"), "{id:?}");
        assert_eq!(fonts.has_glyphs(&id, JAPANESE), !pdfcraft_fonts::ui_japanese_fonts().is_empty(), "{id:?}");
    }
    assert!(layout_widths(&mut fonts, JAPANESE).iter().all(|w| w.is_finite() && *w > 0.0));
    assert!(layout_widths(&mut fonts, "PdfCraft").iter().all(|w| *w > 20.0));
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
}

/// Latin- and Cyrillic-script catalogs (Czech, Brazilian Portuguese, German, Spanish, French, Russian, Bulgarian, Hungarian, Ukrainian, Italian, Slovene) are drawn entirely by the app's own faces
/// (Inter, JetBrains Mono with Inter fallback): no letter falls through to egui's defaults or a CJK fallback. (egui's
/// `has_glyphs` can't answer this: with only the primary face it is also the replacement face.)
#[test]
fn primary_ui_fonts_cover_latin_and_cyrillic_catalogs() {
    use skrifa::MetadataProvider as _;
    let defs = theme::font_definitions();
    assert_eq!(&defs.families[&FontFamily::Monospace][..2], ["JetBrainsMono", "Inter"]);
    let fallback = &defs.font_data["Inter"];
    let fallback_font = skrifa::FontRef::from_index(&fallback.font, fallback.index).unwrap();
    let fallback_charmap = fallback_font.charmap();
    for (code, catalog) in [
        ("cs", include_str!("../src/i18n/cs.tsv")),
        ("el", include_str!("../src/i18n/el.tsv")),
        ("pt-br", include_str!("../src/i18n/pt-br.tsv")),
        ("de", include_str!("../src/i18n/de.tsv")),
        ("es", include_str!("../src/i18n/es.tsv")),
        ("fr", include_str!("../src/i18n/fr.tsv")),
        ("ru", include_str!("../src/i18n/ru.tsv")),
        ("kk", include_str!("../src/i18n/kk.tsv")),
        ("bg", include_str!("../src/i18n/bg.tsv")),
        ("hu", include_str!("../src/i18n/hu.tsv")),
        ("uk", include_str!("../src/i18n/uk.tsv")),
        ("it", include_str!("../src/i18n/it.tsv")),
        ("tr", include_str!("../src/i18n/tr.tsv")),
        ("sl", include_str!("../src/i18n/sl.tsv")),
        ("lt", include_str!("../src/i18n/lt.tsv")),
        ("vi", include_str!("../src/i18n/vi.tsv")),
    ] {
        let mut text = String::from("áčďéěíňóřšťúůýžÁČĎÉĚÍŇÓŘŠŤÚŮÝŽãõçâêôàÃÕÇÂÊÔÀñÑüÜäÄöÖőűß„“¿¡«»…ґҐєЄіІїЇәғқңөұүһӘҒҚҢӨҰҮҺ");
        for line in catalog.lines().filter(|l| !l.starts_with('#')) {
            if let Some(translation) = line.split('\t').nth(2) {
                text.extend(translation.chars().filter(|c| !c.is_whitespace()));
            }
        }
        for name in ["Inter", "Inter-Medium", "Inter-SemiBold", "JetBrainsMono"] {
            let data = &defs.font_data[name];
            let font = skrifa::FontRef::from_index(&data.font, data.index).unwrap();
            let charmap = font.charmap();
            let mut missing: Vec<char> =
                text.chars().filter(|c| charmap.map(*c).is_none() && (name != "JetBrainsMono" || fallback_charmap.map(*c).is_none())).collect();
            missing.sort_unstable();
            missing.dedup();
            assert!(missing.is_empty(), "{code}: {name} lacks {missing:?}");
        }
    }
}

/// PdfCraft patch (vendored epaint bidi, UAX #9): right-to-left text lays out in visual order
/// while the galley keeps one glyph per character in *logical* order, which is what egui's cursor
/// and selection code relies on. This is the layout half of editing Arabic/Hebrew in place.
#[test]
fn arabic_text_lays_out_right_to_left_in_logical_order() {
    if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
        eprintln!("skipping arabic_text_lays_out_right_to_left_in_logical_order: no Arabic face bundled (set CRAFT_FONTS_DIR)");
        return;
    }
    let mut fonts = Fonts::new(TextOptions::default(), theme::font_definitions());
    let mut view = fonts.with_pixels_per_point(2.0);
    let galley = view.layout_no_wrap(ARABIC.to_owned(), FontId::proportional(13.0), Color32::BLACK);
    let row = &galley.rows[0].row;

    // Noto Sans Arabic decomposes dotted letters (ث ن ي) into a base plus separate i'jam dot glyphs
    // via `ccmp`, so a row has more glyphs than characters. Confirm this test actually exercises
    // that case — otherwise the invariant below is trivially true.
    let n = ARABIC.chars().count();
    assert!(row.glyphs.len() > n, "expected decomposition (more glyphs than chars): {} glyphs, {n} chars", row.glyphs.len());

    // The invariant egui's caret and selection rely on: one cursor column per character. The drawn
    // i'jam dots are render-only glyphs, excluded from the cursor/text model, so the row reports
    // exactly `n` columns and its text is the input in logical order with no duplicated mark chars.
    // (Before this fix, the surplus dot glyphs each became a column, drifting the caret.)
    assert_eq!(row.char_count_excluding_newline().0, n, "one caret column per character, not per glyph");
    assert_eq!(row.text(), ARABIC, "cursor text is the input in logical order");

    // Right-to-left: the advancing base glyphs step strictly leftwards in logical order.
    // (`render_only` is crate-private to epaint, so bases are identified here by their advance.)
    let bases: Vec<f32> = row.glyphs.iter().filter(|g| g.advance_width > 0.0).map(|g| g.pos.x).collect();
    assert!(bases.first().unwrap() > bases.last().unwrap(), "RTL text must run right-to-left");
    assert!(bases.windows(2).all(|w| w[1] < w[0]), "RTL base glyphs must descend in x: {bases:?}");

    // Combining marks carry their base character and sit on it (no horizontal advance of their own).
    for (i, g) in row.glyphs.iter().enumerate().filter(|(_, g)| g.advance_width == 0.0) {
        let base = &row.glyphs[i - 1];
        assert_eq!(g.chr, base.chr, "a mark repeats its base character");
        assert!((g.pos.x - base.pos.x).abs() < 0.5, "a mark sits on its base");
    }
}

/// PdfCraft patch: left-to-right text is untouched by the bidi path (it only engages when a row
/// contains right-to-left script), so Latin glyphs still ascend in x exactly as upstream.
#[test]
fn latin_text_layout_is_unchanged_by_bidi() {
    let mut fonts = Fonts::new(TextOptions::default(), theme::font_definitions());
    let mut view = fonts.with_pixels_per_point(2.0);
    let galley = view.layout_no_wrap("Hello world".to_owned(), FontId::proportional(13.0), Color32::BLACK);
    let row = &galley.rows[0].row;
    let xs: Vec<f32> = row.glyphs.iter().map(|g| g.pos.x).collect();
    assert!(xs.windows(2).all(|w| w[0] <= w[1]), "LTR glyphs must ascend in x: {xs:?}");
}
