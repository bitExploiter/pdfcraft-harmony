//! The interface fonts with and without the optional craft-fonts build input (`CRAFT_FONTS_DIR`).

use egui::epaint::text::{Fonts, TextOptions};
use egui::{Color32, FontFamily, FontId};
use egui_kittest::kittest::Queryable;
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

/// A Korean bookmark title and file name, as a Korean user's documents carry them.
const KOREAN_BOOKMARK: &str = "한글 북마크 테스트";
const KOREAN_FILE_NAME: &str = "안티바이러스제품 보안요구사항.pdf";

/// Whether `face` (font data in `defs`) maps every Hangul syllable.
fn covers_hangul(defs: &egui::FontDefinitions, face: &str) -> bool {
    use skrifa::MetadataProvider as _;
    let data = &defs.font_data[face];
    skrifa::FontRef::from_index(&data.font, data.index).is_ok_and(|f| ('\u{AC00}'..='\u{D7A3}').all(|c| f.charmap().map(c).is_some()))
}

/// The atlas cell each Hangul syllable of `text` is drawn from in `id`, and the cell of egui's
/// replacement glyph. A missing glyph is drawn as the replacement box (□), so tofu shows as every
/// syllable sharing that one cell.
fn hangul_cells(fonts: &mut Fonts, id: &FontId, text: &str) -> (Vec<(char, [u16; 4])>, [u16; 4]) {
    let mut view = fonts.with_pixels_per_point(2.0);
    let cell = |g: &egui::epaint::text::Glyph| [g.uv_rect.min[0], g.uv_rect.min[1], g.uv_rect.max[0], g.uv_rect.max[1]];
    let galley = view.layout_no_wrap(text.to_owned(), id.clone(), Color32::BLACK);
    let cells = galley.rows.iter().flat_map(|r| &r.glyphs).filter(|g| ('\u{AC00}'..='\u{D7A3}').contains(&g.chr)).map(|g| (g.chr, cell(g))).collect();
    // U+FFFF is a noncharacter no face maps, so it is always drawn as the replacement glyph.
    let replacement = view.layout_no_wrap("\u{FFFF}".to_owned(), id.clone(), Color32::BLACK);
    let replacement = replacement.rows.iter().flat_map(|r| &r.glyphs).map(cell).next().expect("one glyph");
    (cells, replacement)
}

/// Asserts that every Hangul syllable of `text` is a real glyph in every family: distinct
/// syllables come from distinct atlas cells, none of them the replacement box.
fn assert_hangul_drawn(fonts: &mut Fonts, text: &str) {
    for id in families() {
        assert!(fonts.has_glyphs(&id, text), "{id:?} lacks a glyph of {text}");
        let (cells, replacement) = hangul_cells(fonts, &id, text);
        assert!(!cells.is_empty(), "{text}");
        for (c, cell) in &cells {
            assert_ne!(*cell, replacement, "{id:?}: {c} is drawn as tofu in {text}");
            for (d, other) in &cells {
                assert!(c == d || cell != other, "{id:?}: {c} and {d} share one glyph in {text}");
            }
        }
    }
}

/// A Korean bookmark title and file name are drawn with real Hangul glyphs (not □) in every
/// interface family whatever the interface language, from an installed Hangul face when no
/// embedded one has Hangul. On Windows with Malgun Gothic installed (every Windows 10/11), the
/// installed face is required.
#[test]
fn korean_file_names_and_bookmarks_are_not_tofu() {
    assert!(!theme::font_definitions().font_data.contains_key(theme::SYSTEM_HANGUL_FALLBACK));
    let embedded = theme::font_definitions();
    let bundled = embedded.font_data.keys().any(|name| covers_hangul(&embedded, name));
    // Without a Hangul face the check above does see tofu: it is not vacuous.
    if !bundled {
        let mut fonts = Fonts::new(TextOptions::default(), embedded);
        let (cells, replacement) = hangul_cells(&mut fonts, &FontId::proportional(13.0), KOREAN_BOOKMARK);
        assert!(cells.iter().any(|(_, cell)| *cell == replacement), "embedded faces draw every syllable: {cells:?}");
    }
    let windows_malgun = cfg!(windows)
        && std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_none_or(|v| v != "0")
        && std::path::Path::new(&std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into())).join("Fonts").join("malgun.ttf").is_file();
    // Every interface language, not only Korean: the names come from the user's documents.
    for cjk in [theme::CjkPreference::Default, theme::CjkPreference::SimplifiedChinese, theme::CjkPreference::TraditionalChinese] {
        let defs = theme::installed_font_definitions(cjk);
        let installed = defs.font_data.contains_key(theme::SYSTEM_HANGUL_FALLBACK);
        assert!(installed || bundled || !windows_malgun, "{cjk:?}: Malgun Gothic is installed but not used");
        if !installed && !bundled {
            eprintln!("skipping korean_file_names_and_bookmarks_are_not_tofu ({cjk:?}): no Hangul face (or PDFCRAFT_SYSTEM_FONTS=0)");
            continue;
        }
        if installed {
            for (family, stack) in &defs.families {
                assert_eq!(stack.iter().filter(|n| *n == theme::SYSTEM_HANGUL_FALLBACK).count(), 1, "{family:?}");
                let at = stack.iter().position(|n| n == theme::SYSTEM_HANGUL_FALLBACK).expect("counted above");
                // After the app's own faces and every craft-fonts face, so Latin, Japanese and
                // Chinese text keeps its faces.
                assert!(stack[..at].iter().any(|n| n.starts_with("Inter") || n == "JetBrainsMono"), "{family:?}: {stack:?}");
                assert!(!stack[at..].iter().any(|n| pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.name() == *n)), "{family:?}: {stack:?}");
            }
        }
        let mut fonts = Fonts::new(TextOptions::default(), defs);
        assert_hangul_drawn(&mut fonts, KOREAN_BOOKMARK);
        assert_hangul_drawn(&mut fonts, KOREAN_FILE_NAME);
        // Real glyphs are about a full em wide each; tofu boxes are not.
        let syllables = KOREAN_FILE_NAME.chars().filter(|c| ('\u{AC00}'..='\u{D7A3}').contains(c)).count() as f32;
        assert!(layout_widths(&mut fonts, KOREAN_FILE_NAME).iter().all(|w| w.is_finite() && *w > 13.0 * 0.8 * syllables));
    }
    // Latin text keeps the app's own faces and layout.
    let mut with = Fonts::new(TextOptions::default(), theme::installed_font_definitions(theme::CjkPreference::Default));
    let mut without = Fonts::new(TextOptions::default(), theme::font_definitions());
    assert_eq!(layout_widths(&mut with, "PdfCraft report-2026.pdf"), layout_widths(&mut without, "PdfCraft report-2026.pdf"));
}

/// The installed fonts show the Korean names in real widgets, as the bookmarks panel and the
/// tab strip do.
#[test]
fn korean_names_show_in_the_interface() {
    let mut installed = false;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(480.0, 160.0)).build_ui(move |ui| {
        // Fonts set with set_fonts take effect from the next frame.
        if !installed {
            theme::install_fonts_for(ui.ctx(), theme::CjkPreference::Default);
            installed = true;
            return;
        }
        ui.label(KOREAN_BOOKMARK);
        let _ = ui.selectable_label(true, KOREAN_FILE_NAME);
    });
    h.run();
    assert!(h.query_by_label(KOREAN_BOOKMARK).is_some());
    assert!(h.query_by_label(KOREAN_FILE_NAME).is_some());
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
/// (Inter, JetBrains Mono): no letter falls through to egui's defaults or a CJK fallback. (egui's
/// `has_glyphs` can't answer this: with only the primary face it is also the replacement face.)
#[test]
fn primary_ui_fonts_cover_latin_and_cyrillic_catalogs() {
    use skrifa::MetadataProvider as _;
    let defs = theme::font_definitions();
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
            let mut missing: Vec<char> = text.chars().filter(|c| charmap.map(*c).is_none()).collect();
            missing.sort_unstable();
            missing.dedup();
            assert!(missing.is_empty(), "{code}: {name} lacks {missing:?}");
        }
    }
}
