//! End-to-end extraction and normal range selection. Fixtures are original PDF operators,
//! generated in memory; private reports can be exercised without distributing their assets.

use pdfcraft_render::{PageRenderer, PageText, RenderConfig, text::TextFilter};
use std::sync::Arc;

/// Extract the public text layer from an original, in-memory one-page Courier PDF.
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

/// Position a ten-point Courier text run independently of the preceding paint order.
fn run(text: &str, x: f32, y: f32) -> String {
    format!("BT /F1 10 Tf 1 0 0 1 {x} {y} Tm ({text}) Tj ET\n")
}

const LINES: [&str; 3] = ["Alfa text fills row here", "Beta text fills row here", "Last text fills row here"];

/// Original visual glyph geometry for script tests, without requiring an embedded font.
fn visual_glyphs(text: &str, x: f32, y: f32, advance: f32) -> Vec<pdfcraft_render::text::TextGlyph> {
    text.chars()
        .enumerate()
        .map(|(at, c)| {
            let x = x + at as f32 * advance;
            pdfcraft_render::text::TextGlyph {
                text: c.to_string(),
                rect: [x, y, x + advance, y + 10.0],
                quad: [[x, y + 10.0], [x + advance, y + 10.0], [x + advance, y], [x, y]],
                direction: [1.0, 0.0],
            }
        })
        .collect()
}

/// A normal logical RTL row determines the order of its genuine rectangular cell pieces;
/// a numeric cell is neutral and must not turn that row into a left-to-right table row.
#[test]
fn rtl_rows_with_numeric_cells_keep_their_logical_cell_order() {
    let mut glyphs = Vec::new();
    for row in 0..2 {
        let y = 10.0 + row as f32 * 12.0;
        for (x, text) in [(10.0, "םילקש"), (100.0, "120"), (200.0, "ריחמ")] {
            glyphs.extend(visual_glyphs(text, x, y, 6.0));
        }
    }
    let t = pdfcraft_render::text::layout(glyphs);
    assert_eq!(t.plain_text(), "מחיר 120 שקלים\nמחיר 120 שקלים");
    assert_eq!(t.column_text(&(0..t.glyphs.len()).collect::<Vec<_>>()), "מחיר\t120\tשקלים\nמחיר\t120\tשקלים");
}

/// Full-width CJK glyphs do not use inter-word spaces. A repeated gutter is not word spacing.
#[test]
fn cjk_columns_without_word_spaces_keep_complete_column_order() {
    use pdfcraft_render::text::layout;
    let body = "正文".repeat(10);
    let line = |col: char, row: char| format!("{col}{row}{body}");
    let mut glyphs = Vec::new();
    for (row, label) in ['一', '二', '三'].into_iter().enumerate() {
        for (col, name) in ['甲', '乙', '丙'].into_iter().enumerate() {
            glyphs.extend(visual_glyphs(&line(name, label), 10.0 + col as f32 * 228.0, 10.0 + row as f32 * 12.0, 10.0));
        }
    }
    let mut expected = Vec::new();
    for col in ['甲', '乙', '丙'] {
        for row in ['一', '二', '三'] {
            expected.push(line(col, row));
        }
    }
    assert_eq!(layout(glyphs).plain_text(), expected.join("\n"));
}

/// A text-show operation is spacing evidence, not proof that everything in it shares a line.
#[test]
fn one_tj_operation_can_paint_both_columns() {
    let mut ops = String::new();
    for row in 0..3 {
        let y = 740 - row * 12;
        ops.push_str(&format!("BT /F1 10 Tf 30 {y} Td [(L row {row} fills column) -800 (R row {row} fills column)] TJ ET\n"));
    }
    let expected =
        ['L', 'R'].into_iter().flat_map(|col| (0..3).map(move |row| format!("{col} row {row} fills column"))).collect::<Vec<_>>().join("\n");
    assert_eq!(extract(&ops).plain_text(), expected);
}

/// A trailing space must not borrow the unrelated next text show's column placement as Tw.
#[test]
fn trailing_spaces_do_not_measure_the_next_columns_gutter_as_word_spacing() {
    let line = "ABCDEFGHIJKLMNOPQRSTUVWXYZ ";
    for shared_show in [false, true] {
        let mut ops = String::new();
        for row in 0..3 {
            let y = 740.0 - row as f32 * 12.0;
            if shared_show {
                ops.push_str(&format!("BT /F1 10 Tf 30 {y} Td [({line}) -800 ({line})] TJ ET\n"));
            } else {
                for x in [30.0, 200.0] {
                    ops.push_str(&run(line, x, y));
                }
            }
        }
        let t = extract(&ops);
        let first = t.nearest(31.0, 59.0).unwrap();
        let last = t.nearest(185.0, 83.0).unwrap();
        assert!(
            t.line_rects(first..last + 1).iter().all(|r| r[2] <= 192.1),
            "shared TJ {shared_show}: a trailing space erased the gutter: {:?}",
            t.plain_text()
        );
    }
}

/// Paragraph whitespace shared by columns must not turn their continuous reading order
/// into horizontal page bands, even when the paragraph gap dwarfs a narrow gutter.
#[test]
fn aligned_paragraph_breaks_keep_complete_columns_in_normal_ranges() {
    for columns in [2, 3] {
        for paragraph_gap in [12.0, 24.0, 84.0, 180.0] {
            let line = |col, paragraph, row| format!("{col} para {paragraph} row {row} fill");
            let width = line('L', 0, 0).len() as f32 * 6.0;
            let mut ops = String::new();
            // Row-major PDF paint order, with distinct text in every paragraph and column.
            for paragraph in 0..2 {
                for row in 0..3 {
                    for (col, label) in ['L', 'M', 'R'].into_iter().take(columns).enumerate() {
                        let y = 740.0 - paragraph as f32 * (24.0 + paragraph_gap) - row as f32 * 12.0;
                        ops.push_str(&run(&line(label, paragraph, row), 30.0 + col as f32 * (width + 8.0), y));
                    }
                }
            }
            let column = |label| (0..2).flat_map(|paragraph| (0..3).map(move |row| line(label, paragraph, row))).collect::<Vec<_>>().join("\n");
            let t = extract(&ops);
            let expected = ['L', 'M', 'R'].into_iter().take(columns).map(column).collect::<Vec<_>>().join("\n");
            assert_eq!(t.plain_text(), expected, "{columns} columns, paragraph gap {paragraph_gap}");
            let first = t.find(&line('L', 0, 0))[0].start;
            let last = t.find(&line('L', 1, 2))[0].end;
            assert_eq!(t.text_of(first..last), column('L'));
            let across_paragraphs = format!("{} {}", line('L', 0, 2), line('L', 1, 0));
            let hits = t.find(&across_paragraphs);
            assert_eq!(hits.len(), 1, "search follows paragraph continuity in the same column");
            assert!(t.line_rects(hits[0].clone()).iter().all(|r| r[2] <= 30.1 + width));
            let rects = t.line_rects(first..last);
            assert_eq!(rects.len(), 6);
            assert!(rects.iter().all(|r| r[0] >= 29.9 && r[2] <= 30.1 + width), "{rects:?}");
        }
    }
}

/// Narrow gutters constrain normal drag, triple-click and highlight ranges, while an
/// intentional cross-column range still traverses the complete preceding column.
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

/// Full-width headings and footers enclose complete body columns in reading order.
#[test]
fn spanning_headings_and_footer_surround_complete_columns() {
    let mut ops = run("A heading that spans both body columns", 30.0, 740.0);
    for x in [30.0, 182.0] {
        for paragraph in 0..2 {
            for (row, line) in LINES.iter().enumerate() {
                ops.push_str(&run(line, x, 700.0 - paragraph as f32 * 60.0 - row as f32 * 12.0));
            }
        }
    }
    ops.push_str(&run("A footer that spans both body columns", 30.0, 580.0));
    let column = [LINES, LINES].concat().join("\n");
    assert_eq!(
        extract(&ops).plain_text(),
        format!("A heading that spans both body columns\n{column}\n{column}\nA footer that spans both body columns")
    );
}

/// A larger heading's single word space can fully cover the narrower body gutter. That
/// local word relationship must survive even when body spaces dominate the page estimate.
#[test]
fn larger_spanning_headings_keep_their_words_together_above_columns() {
    for size in [14.0, 20.0, 30.0] {
        let cell = size * 0.6;
        let x = 178.0 - cell * 0.5 - 5.0 * cell;
        let mut ops = format!("BT /F1 {size} Tf {x} 740 Td (ALPHA BETA) Tj ET\n");
        for x in [30.0, 182.0] {
            for (row, text) in LINES.iter().enumerate() {
                ops.push_str(&run(text, x, 700.0 - row as f32 * 12.0));
            }
        }
        let t = extract(&ops);
        let body = [LINES, LINES].concat().join("\n");
        assert_eq!(t.plain_text(), format!("ALPHA BETA\n{body}"), "heading size {size}");
        let first = t.find("ALPHA")[0].start;
        let last = t.find("BETA")[0].end;
        assert_eq!(t.text_of(first..last), "ALPHA BETA");
        assert_eq!(t.line_rects(first..last).len(), 1);
    }
}

/// Spanning elements must not make a larger paragraph gap interleave complete columns.
#[test]
fn spanning_header_footer_keep_columns_continuous_across_large_paragraph_gaps() {
    let heading = "A spanning title across the complete multi column body on this page";
    let footer = "A footer across all three columns and the entire body width";
    for columns in [2, 3] {
        for gap in [12.0, 84.0, 180.0] {
            let labels = ['L', 'M', 'R'];
            let line = |col, p, row| format!("{col} p{p} row{row} fills column");
            let width = line('L', 0, 0).len() as f32 * 6.0;
            let mut ops = run(heading, 30.0, 760.0);
            for p in 0..2 {
                for row in 0..3 {
                    for (col, label) in labels.into_iter().take(columns).enumerate() {
                        ops.push_str(&run(
                            &line(label, p, row),
                            30.0 + col as f32 * (width + 8.0),
                            700.0 - p as f32 * (24.0 + gap) - row as f32 * 12.0,
                        ));
                    }
                }
            }
            ops.push_str(&run(footer, 30.0, 700.0 - (24.0 + gap) - 24.0 - 48.0));
            let column = |label| (0..2).flat_map(|p| (0..3).map(move |row| line(label, p, row))).collect::<Vec<_>>().join("\n");
            let expected = labels.into_iter().take(columns).map(column).collect::<Vec<_>>().join("\n");
            let t = extract(&ops);
            assert_eq!(t.plain_text(), format!("{heading}\n{expected}\n{footer}"), "{columns} columns, paragraph gap {gap}");
            let first = t.find(&line('L', 0, 0))[0].start;
            let last = t.find(&line('L', 1, 2))[0].end;
            assert_eq!(t.text_of(first..last), column('L'));
            assert!(t.line_rects(first..last).iter().all(|r| r[2] <= 30.1 + width));
        }
    }
}

/// A heading over some columns must not interrupt the neighbouring complete column.
#[test]
fn partial_spanning_heading_preserves_adjacent_column_through_paragraph_gaps() {
    let heading = "A heading across only the first two body columns";
    let footer = "A footer across all three columns and the entire body width";
    let mut ops = run(heading, 30.0, 740.0);
    for (row, text) in LINES.iter().enumerate() {
        ops.push_str(&run(text, 334.0, 740.0 - row as f32 * 12.0));
    }
    for p in 0..2 {
        for (row, text) in LINES.iter().enumerate() {
            for x in [30.0, 182.0, 334.0] {
                ops.push_str(&run(text, x, 680.0 - p as f32 * 180.0 - row as f32 * 12.0));
            }
        }
    }
    ops.push_str(&run(footer, 30.0, 430.0));
    let column = [LINES, LINES].concat().join("\n");
    let expected = format!("{heading}\n{column}\n{column}\n{}\n{column}\n{footer}", LINES.join("\n"));
    assert_eq!(extract(&ops).plain_text(), expected);
}

/// A spanning section heading separates independently reconstructed column layouts.
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

/// Short table cells preserve normal row order and rectangular row/tab extraction.
#[test]
fn short_table_cells_preserve_normal_rows_and_rectangular_tabs() {
    let mut ops = String::new();
    for (row, (left, right)) in [("Name", "Qty"), ("Apple", "12"), ("Pear", "7")].iter().enumerate() {
        ops.push_str(&run(left, 30.0, 700.0 - row as f32 * 12.0));
        ops.push_str(&run(right, 80.0, 700.0 - row as f32 * 12.0));
    }
    let t = extract(&ops);
    assert_eq!(t.plain_text(), "Name Qty\nApple 12\nPear 7");
    assert_eq!(t.column_text(&t.glyphs_in([0.0, 0.0, 200.0, 200.0])), "Name\tQty\nApple\t12\nPear\t7");
}

/// A right column starting below an illustration follows the complete left column.
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

/// Offset column starts do not require more than one overlapping baseline for support.
#[test]
fn staggered_short_columns_need_only_one_shared_baseline() {
    let ops = format!("{}{}{}{}", run(LINES[0], 30.0, 700.0), run(LINES[1], 30.0, 688.0), run(LINES[0], 182.0, 688.0), run(LINES[1], 182.0, 676.0));
    assert_eq!(extract(&ops).plain_text(), [LINES[0], LINES[1], LINES[0], LINES[1]].join("\n"));
}

/// A heading over two columns belongs to their region rather than the adjacent third one.
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

/// Straight turned text is subdivided in its baseline frame with original quads retained.
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

/// Relative typography and spatial evidence must survive transformations and reversed draws.
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

/// Repeated word spaces and isolated same-line fragments are insufficient column evidence.
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

/// The original magazine's left paragraph must retain local highlights and copied text.
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

/// Geometric subdivision must not fragment curved labels elsewhere in the private corpus.
#[test]
#[ignore = "private corpus: set PDFCRAFT_COLUMN_PDF to the supplied magazine, then run --ignored"]
fn magazine_decorative_map_labels_remain_searchable() {
    let bytes = std::fs::read(std::env::var_os("PDFCRAFT_COLUMN_PDF").expect("PDFCRAFT_COLUMN_PDF")).unwrap();
    let t = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).filtered_text(48, TextFilter::default()).unwrap();
    for label in ["Mare Nostrum", "Mediterranean Sea"] {
        assert!(!t.find(label).is_empty(), "decorative label split into false columns: {label}");
    }
}
