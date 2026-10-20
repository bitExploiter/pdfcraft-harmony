//! End-to-end extraction and normal range selection. Fixtures are original PDF operators,
//! generated in memory; private reports can be exercised without distributing their assets.

use pdfcraft_render::{PageRenderer, PageText, RenderConfig, text::TextFilter};
use std::sync::Arc;

fn extract(ops: &str) -> PageText {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.7");
    let font = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier"});
    let content = doc.add_object(Stream::new(dictionary! {}, ops.as_bytes().to_vec()));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages,
    "MediaBox" => vec![0.into(), 0.into(), 600.into(), Object::Integer(800)],
    "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content});
    doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
    let root = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
    doc.trailer.set("Root", root);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    PageRenderer::new(Arc::new(bytes), RenderConfig::default()).filtered_text(0, TextFilter::default()).unwrap()
}

fn run(text: &str, x: f32, y: f32) -> String {
    format!("BT /F1 10 Tf 1 0 0 1 {x} {y} Tm ({text}) Tj ET\n")
}

const LINES: [&str; 3] = ["Alfa text fills row here", "Beta text fills row here", "Last text fills row here"];

#[test]
fn normal_ranges_and_highlights_stay_inside_narrow_columns() {
    // Courier: 24 glyphs * 6 pt, 8 pt gutter. Row-major paint crosses it in one text object.
    let ops: String = (0..3)
        .flat_map(|row| (0..3).map(move |col| (row, col)))
        .map(|(row, col)| run(LINES[row], 30.0 + col as f32 * 152.0, 700.0 - row as f32 * 12.0))
        .collect();
    let t = extract(&ops);
    assert_eq!(t.plain_text(), vec![LINES.join("\n"); 3].join("\n"));
    let from = t.nearest(31.0, 99.0).unwrap();
    let to = t.nearest(172.0, 123.0).unwrap();
    assert_eq!(t.text_of(from..to + 1), LINES.join("\n"));
    let rects = t.line_rects(from..to + 1);
    assert_eq!(rects.len(), 3);
    assert!(rects.iter().all(|r| r[2] <= 174.1), "{rects:?}");
    assert_eq!(t.line_at(from), Some((from, from + 23)), "triple click stops at the gutter");
    // Intentional movement into another column must still traverse complete logical lines.
    let other = t.nearest(183.0, 99.0).unwrap();
    assert!(t.text_of(from..other + 1).starts_with(&LINES.join("\n")));
}

#[test]
fn spanning_headings_and_footer_surround_complete_columns() {
    let mut ops = run("A heading that spans both body columns", 30.0, 740.0);
    for x in [30.0, 182.0] {
        for (row, line) in LINES.iter().enumerate() {
            ops.push_str(&run(line, x, 700.0 - row as f32 * 12.0));
        }
    }
    ops.push_str(&run("A footer that spans both body columns", 30.0, 640.0));
    assert_eq!(
        extract(&ops).plain_text(),
        format!("A heading that spans both body columns\n{}\n{}\nA footer that spans both body columns", LINES.join("\n"), LINES.join("\n"))
    );
}

#[test]
fn section_breaks_can_change_the_number_of_columns() {
    let mut ops = String::new();
    for x in [30.0, 182.0, 334.0] {
        for (row, line) in LINES.iter().enumerate() {
            ops.push_str(&run(line, x, 740.0 - row as f32 * 12.0));
        }
    }
    ops.push_str(&run("A section heading spanning all three columns across the page", 30.0, 680.0));
    for x in [30.0, 182.0] {
        for (row, line) in LINES.iter().enumerate() {
            ops.push_str(&run(line, x, 640.0 - row as f32 * 12.0));
        }
    }
    let expected = format!(
        "{}\nA section heading spanning all three columns across the page\n{}",
        vec![LINES.join("\n"); 3].join("\n"),
        vec![LINES.join("\n"); 2].join("\n")
    );
    assert_eq!(extract(&ops).plain_text(), expected);
}

#[test]
fn short_table_cells_keep_normal_column_ranges_and_rectangular_rows() {
    let mut ops = String::new();
    for (row, (left, right)) in [("Name", "Qty"), ("Apple", "12"), ("Pear", "7")].iter().enumerate() {
        ops.push_str(&run(left, 30.0, 700.0 - row as f32 * 12.0));
        ops.push_str(&run(right, 80.0, 700.0 - row as f32 * 12.0));
    }
    let t = extract(&ops);
    assert_eq!(t.plain_text(), "Name\nApple\nPear\nQty\n12\n7");
    assert_eq!(t.column_text(&t.glyphs_in([0.0, 0.0, 200.0, 200.0])), "Name\tQty\nApple\t12\nPear\t7");
}

#[test]
fn text_wrapping_around_an_illustration_keeps_its_column() {
    let mut ops = String::new();
    // A blank illustration occupies the upper right. Right prose starts below it, alongside
    // the last three left lines, rather than sharing the left column's first baseline.
    for row in 0..7 {
        ops.push_str(&run(LINES[row % 3], 30.0, 740.0 - row as f32 * 12.0));
    }
    ops.push_str("q 0.8 g 182 710 144 40 re f Q\n");
    for (row, line) in LINES.iter().enumerate() {
        ops.push_str(&run(line, 182.0, 692.0 - row as f32 * 12.0));
    }
    let mut expected: Vec<_> = (0..7).map(|row| LINES[row % 3]).collect();
    expected.extend(LINES);
    assert_eq!(extract(&ops).plain_text(), expected.join("\n"));
}

#[test]
fn staggered_short_columns_need_only_one_shared_baseline() {
    let ops = format!("{}{}{}{}", run(LINES[0], 30.0, 700.0), run(LINES[1], 30.0, 688.0), run(LINES[0], 182.0, 688.0), run(LINES[1], 182.0, 676.0));
    assert_eq!(extract(&ops).plain_text(), [LINES[0], LINES[1], LINES[0], LINES[1]].join("\n"));
}

#[test]
fn a_heading_can_span_two_columns_alongside_a_third_column() {
    let mut ops = run("A heading across two columns", 30.0, 740.0);
    for x in [30.0, 182.0] {
        for (row, line) in LINES.iter().enumerate() {
            ops.push_str(&run(line, x, 716.0 - row as f32 * 12.0));
        }
    }
    for (row, line) in LINES.iter().enumerate() {
        ops.push_str(&run(line, 334.0, 740.0 - row as f32 * 12.0));
    }
    assert_eq!(extract(&ops).plain_text(), format!("A heading across two columns\n{}", vec![LINES.join("\n"); 3].join("\n")));
}

#[test]
fn straight_columns_rotated_thirty_degrees_keep_their_reading_order() {
    let mut ops = "q 0.8660254 0.5 -0.5 0.8660254 350 50 cm\n".to_string();
    for x in [30.0, 182.0] {
        for (row, line) in LINES.iter().enumerate() {
            ops.push_str(&run(line, x, 600.0 - row as f32 * 12.0));
        }
    }
    ops.push_str("Q\n");
    assert_eq!(extract(&ops).plain_text(), format!("{}\n{}", LINES.join("\n"), LINES.join("\n")));
}

#[test]
fn layout_is_independent_of_scale_translation_and_glyph_paint_order() {
    let mut ops = String::new();
    for x in [30.0, 182.0] {
        for (row, line) in LINES.iter().enumerate() {
            ops.push_str(&run(line, x, 700.0 - row as f32 * 12.0));
        }
    }
    let source = extract(&ops);
    let expected = format!("{}\n{}", LINES.join("\n"), LINES.join("\n"));
    for scale in [0.125, 0.5, 1.0, 4.0, 16.0] {
        let mut glyphs = source.glyphs.clone();
        glyphs.reverse();
        for g in &mut glyphs {
            for (axis, v) in g.rect.iter_mut().enumerate() {
                *v = *v * scale + if axis % 2 == 0 { -125.0 } else { 250.0 };
            }
            for p in &mut g.quad {
                p[0] = p[0] * scale - 125.0;
                p[1] = p[1] * scale + 250.0;
            }
        }
        assert_eq!(pdfcraft_render::text::layout(glyphs).plain_text(), expected, "scale {scale}");
    }
}

#[test]
fn aligned_word_spaces_and_large_gaps_on_one_line_are_not_columns() {
    let mut ops = String::new();
    for row in 0..4 {
        ops.push_str(&run("Word word word word", 30.0, 700.0 - row as f32 * 12.0));
    }
    assert_eq!(extract(&ops).plain_text(), ["Word word word word"; 4].join("\n"));
    let separated = format!("{}{}", run("Left", 30.0, 700.0), run("Right", 130.0, 700.0));
    assert_eq!(extract(&separated).plain_text(), "Left\nRight");
}

#[test]
#[ignore = "private corpus: set PDFCRAFT_COLUMN_PDF to the supplied magazine, then run --ignored"]
fn magazine_page_twelve_normal_selection() {
    let bytes = std::fs::read(std::env::var_os("PDFCRAFT_COLUMN_PDF").expect("PDFCRAFT_COLUMN_PDF")).unwrap();
    let t = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).filtered_text(11, TextFilter::default()).unwrap();
    let start = t.find("At sunset")[0].start;
    let end = t.find("grip, he explains.")[0].end;
    assert!(start < end);
    let rects = t.line_rects(start..end);
    assert!(rects.len() >= 10, "multiline paragraph: {rects:?}");
    assert!(rects.iter().all(|r| r[0] >= 59.0 && r[2] < 225.0), "normal selection leaks across a gutter: {rects:?}");
    assert!(!t.text_of(start..end).contains("skyscraper"));
}

#[test]
#[ignore = "private corpus: set PDFCRAFT_COLUMN_PDF to the supplied magazine, then run --ignored"]
fn magazine_decorative_map_labels_remain_searchable() {
    let bytes = std::fs::read(std::env::var_os("PDFCRAFT_COLUMN_PDF").expect("PDFCRAFT_COLUMN_PDF")).unwrap();
    let t = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).filtered_text(48, TextFilter::default()).unwrap();
    for label in ["Mare Nostrum", "Mediterranean Sea"] {
        assert!(!t.find(label).is_empty(), "decorative label split into false columns: {label}");
    }
}
