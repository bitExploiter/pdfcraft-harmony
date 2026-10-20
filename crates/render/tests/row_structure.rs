//! Normal reading ranges must preserve rows as well as prose columns. These original,
//! generated documents deliberately have no tags, so geometry cannot rely on a structure tree.

use pdfcraft_render::{PageRenderer, PageText, RenderConfig, text::TextFilter};
use std::sync::Arc;

/// Build an original one-page document with standard Helvetica and WinAnsi bullet encoding.
fn extract(content: &str) -> PageText {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.7");
    let font = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding"});
    let contents = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages,
    "MediaBox" => vec![0.into(), 0.into(), 600.into(), Object::Integer(800)],
    "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => contents});
    doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
    let root = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
    doc.trailer.set("Root", root);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    PageRenderer::new(Arc::new(bytes), RenderConfig::default()).filtered_text(0, TextFilter::default()).unwrap()
}

/// Ignore line-ending choices while still checking every word boundary and word's position.
fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Place an independent twelve-point Helvetica fragment.
fn run(text: &str, x: f32, y: f32) -> String {
    format!("BT /F1 12 Tf 1 0 0 1 {x} {y} Tm ({text}) Tj ET\n")
}

/// TJ positioning and Tw spaces can create coincident ordinary word gaps on adjacent lines.
#[test]
fn justified_tj_and_tw_word_gaps_are_not_column_gutters() {
    let text = extract(
        "BT /F1 11 Tf 50 740 Td [(Justified) -600 (text) -600 (without) -600 (any) -600 (space) -600 (characters)] TJ ET\n\
         BT /F1 11 Tf 8 Tw 50 710 Td (Word spacing set with Tw on real spaces) Tj ET\n",
    );
    let first = "Justified text without any space characters";
    let second = "Word spacing set with Tw on real spaces";
    assert_eq!(words(&text.plain_text()), format!("{first} {second}"));
    let start = text.find("Justified")[0].start;
    let end = text.find("characters")[0].end;
    assert_eq!(words(&text.text_of(start..end)), first);
    assert_eq!(text.line_rects(start..end).len(), 1);
    // Tw alone must also work: the unrelated TJ line must not be what protects its gaps.
    for word_space in [8, 16] {
        let mut content = String::new();
        let line = "Many words live on this single physical row";
        for y in [740, 725] {
            content.push_str(&format!("BT /F1 11 Tf {word_space} Tw 50 {y} Td ({line}) Tj ET\n"));
        }
        assert_eq!(words(&extract(&content).plain_text()), format!("{line} {line}"));
    }
}

/// Repeated TJ advances can encode all word spaces without any Unicode space glyphs.
#[test]
fn justified_tj_without_space_glyphs_keeps_each_line_together() {
    let mut content = String::new();
    for y in [740, 725] {
        content.push_str(&format!("BT /F1 11 Tf 50 {y} Td [(Alpha) -600 (Bravo) -600 (Charlie) -600 (Delta) -600 (Echo)] TJ ET\n"));
    }
    let text = extract(&content);
    assert_eq!(words(&text.plain_text()), "Alpha Bravo Charlie Delta Echo Alpha Bravo Charlie Delta Echo");
}

/// The quote operators move lines and inherit/update spacing; all four show forms coexist.
#[test]
fn quote_text_shows_keep_word_spacing_and_row_order() {
    let text = extract(
        "BT /F1 11 Tf 20 TL 50 740 Td (First row has words) Tj\n\
        (Second row has words) '\n\
        8 0 (Third row has spaced words) \"\n\
        0 Tw 0 -20 Td [(Last) -600 (row) -600 (has) -600 (words)] TJ ET\n",
    );
    assert_eq!(words(&text.plain_text()), "First row has words Second row has words Third row has spaced words Last row has words");
}

/// Hanging list labels belong with their item bodies, including wrapped and single-line items.
#[test]
fn hanging_bullets_stay_with_their_own_list_items() {
    for labels_first in [false, true] {
        let items = [
            (700.0, "First item with several words", Some("and its continuation")),
            (650.0, "Second item has one line", None),
            (610.0, "Third item with several words", Some("and a final continuation")),
        ];
        let mut content = String::new();
        if labels_first {
            for (y, _, _) in items {
                content.push_str(&run("\\225", 50.0, y));
            }
        }
        let mut expected = Vec::new();
        for (y, first, rest) in items {
            if !labels_first {
                content.push_str(&run("\\225", 50.0, y));
            }
            content.push_str(&run(first, 64.0, y));
            expected.push(format!("• {first}"));
            if let Some(rest) = rest {
                content.push_str(&run(rest, 64.0, y - 14.0));
                expected.push(rest.to_string());
            }
        }
        let text = extract(&content);
        assert_eq!(words(&text.plain_text()), expected.join(" "), "labels first: {labels_first}");
        let start = text.find("continuation")[0].start;
        let end = text.find("Second item")[0].end;
        assert_eq!(words(&text.text_of(start..end)), "continuation • Second item");
    }
}

/// A label and a separately positioned value share one row, even across a large horizontal gap.
#[test]
fn form_label_value_pairs_remain_in_row_order() {
    let fields = [("Name:", "Jane Example"), ("Role:", "Project reader"), ("City:", "Lake Town")];
    for row_pitch in [15, 24] {
        let mut content = String::new();
        for (row, (label, value)) in fields.iter().enumerate() {
            content.push_str(&format!("BT /F1 12 Tf 50 {} Td ({label}) Tj 60 0 Td ({value}) Tj ET\n", 700 - row * row_pitch));
        }
        let text = extract(&content);
        assert_eq!(words(&text.plain_text()), fields.iter().map(|(label, value)| format!("{label} {value}")).collect::<Vec<_>>().join(" "));
        let start = text.find("Name:")[0].start;
        let end = text.find("Jane Example")[0].end;
        assert_eq!(words(&text.text_of(start..end)), "Name: Jane Example");
    }
}

/// Wider multi-digit numbering must not turn a hanging indent into a prose column.
#[test]
fn numbered_list_labels_stay_with_items_past_nine() {
    let mut content = String::new();
    let mut expected = Vec::new();
    for item in 1..=23 {
        let y = 760.0 - (item - 1) as f32 * 15.0;
        content.push_str(&run(&format!("{item}."), 50.0, y));
        let body = format!("Item {item} has its own body text");
        content.push_str(&run(&body, 74.0, y));
        expected.push(format!("{item}. {body}"));
    }
    let text = extract(&content);
    assert_eq!(words(&text.plain_text()), expected.join(" "));
}

/// Alphabetic numbering continues after Z and bracketed labels still belong to their item.
#[test]
fn alphabetic_and_bracketed_list_markers_stay_with_items() {
    for labels in [["AA.", "AB.", "AC."], ["[aa]", "[ab]", "[ac]"], ["(xiv)", "(xv)", "(xvi)"]] {
        let mut content = String::new();
        let mut expected = Vec::new();
        for (row, label) in labels.into_iter().enumerate() {
            let body = format!("Item {row} has its own body text");
            let y = 740.0 - row as f32 * 15.0;
            content.push_str(&run(label, 50.0, y));
            content.push_str(&run(&body, 90.0, y));
            expected.push(format!("{label} {body}"));
        }
        assert_eq!(words(&extract(&content).plain_text()), expected.join(" "));
    }
}

/// Sparse aligned table cells are rows, not six independent vertical prose paragraphs.
#[test]
fn short_table_rows_remain_in_row_order() {
    for wrapped in [false, true] {
        let mut content = String::new();
        let mut expected = Vec::new();
        for row in 0..2 {
            for col in 0..3 {
                let first = format!("R{row}C{col} cell text");
                let y = 700.0 - row as f32 * 60.0;
                content.push_str(&run(&first, 50.0 + col as f32 * 150.0, y));
                expected.push(first);
                if wrapped {
                    content.push_str(&run("wrap detail", 50.0 + col as f32 * 150.0, y - 14.0));
                    expected.push("wrap detail".to_string());
                }
            }
        }
        let text = extract(&content);
        assert_eq!(words(&text.plain_text()), expected.join(" "), "wrapped: {wrapped}");
        let from = text.find("R0C1")[0].start;
        let to = text.find("R0C2")[0].end;
        let between = if wrapped { "R0C1 cell text wrap detail R0C2" } else { "R0C1 cell text R0C2" };
        assert_eq!(words(&text.text_of(from..to)), between);
    }
}
