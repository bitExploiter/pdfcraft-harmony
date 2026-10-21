use std::sync::Arc;

use pdfcraft_cos::Document;

use super::*;

/// A 600×800 page drawing `content`, with form /Fm1 (drawing `form`) in its resources.
fn page(content: &str, form: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /XObject << /Fm1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [1 0 0 1 300 300] /Length {} >>\nstream\n{form}\nendstream",
            form.len()
        ),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn shapes(content: &str) -> Vec<PageShape> {
    reading_shapes(&page(content, ""), 0).unwrap().shapes
}

#[test]
fn a_filled_rectangle() {
    let s = shapes("0.2 0.4 0.6 rg 10 20 100 50 re f");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].paths, vec![vec![[10.0, 20.0], [110.0, 20.0], [110.0, 70.0], [10.0, 70.0]]]);
    assert!(s[0].closed);
    assert_eq!(s[0].fill, Some([0.2, 0.4, 0.6]));
    assert_eq!(s[0].stroke, None);
}

#[test]
fn colour_spaces() {
    let s = shapes("0.5 g 0 0 1 1 re f 0 0 0 1 k 0 0 1 1 re f /DeviceRGB cs 1 0 0 sc 0 0 1 1 re f");
    let fills: Vec<_> = s.iter().map(|s| s.fill).collect();
    assert_eq!(fills, vec![Some([0.5, 0.5, 0.5]), Some([0.0, 0.0, 0.0]), Some([1.0, 0.0, 0.0])]);
}

#[test]
fn stroked_line_with_width_and_colour() {
    let s = shapes("2 w 0 0 1 RG 50 50 m 250 50 l S");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].paths, vec![vec![[50.0, 50.0], [250.0, 50.0]]]);
    assert!(!s[0].closed);
    assert_eq!(s[0].fill, None);
    assert_eq!(s[0].stroke, Some([0.0, 0.0, 1.0]));
    assert_eq!(s[0].line_width, 2.0);
}

#[test]
fn fill_and_stroke_together() {
    let s = shapes("1 0 0 rg 0 1 0 RG 0 0 10 10 re B");
    assert_eq!((s[0].fill, s[0].stroke), (Some([1.0, 0.0, 0.0]), Some([0.0, 1.0, 0.0])));
}

#[test]
fn transforms_and_saved_state() {
    // `cm` moves and scales the path; `Q` restores the matrix and colour.
    let s = shapes("1 0 0 rg q 2 0 0 2 100 100 cm 0 g 0 0 10 10 re f Q 0 0 10 10 re f");
    assert_eq!(s[0].paths, vec![vec![[100.0, 100.0], [120.0, 100.0], [120.0, 120.0], [100.0, 120.0]]]);
    assert_eq!(s[0].fill, Some([0.0, 0.0, 0.0]), "the gray set inside q");
    assert_eq!(s[1].paths, vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]]);
    assert_eq!(s[1].fill, Some([1.0, 0.0, 0.0]), "restored");
}

#[test]
fn curves_are_flattened() {
    let s = shapes("0 0 m 0 100 100 100 100 0 c f");
    let pts = &s[0].paths[0];
    assert!(pts.len() > 4, "a curve becomes several segments");
    assert_eq!(pts.first(), Some(&[0.0, 0.0]));
    assert_eq!(pts.last(), Some(&[100.0, 0.0]));
    assert!(pts.iter().any(|p| p[1] > 70.0), "the curve bulges up");
}

#[test]
fn paths_that_are_not_painted_draw_nothing() {
    assert!(shapes("0 0 10 10 re n 0 0 10 10 re W n").is_empty());
}

#[test]
fn forms_are_drawn_with_their_matrix() {
    let doc = page("/Fm1 Do", "0 0 1 rg 0 0 10 10 re f");
    let s = reading_shapes(&doc, 0).unwrap().shapes;
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].paths, vec![vec![[300.0, 300.0], [310.0, 300.0], [310.0, 310.0], [300.0, 310.0]]]);
}

#[test]
fn several_subpaths_in_one_fill() {
    let s = shapes("0 0 10 10 re 20 0 10 10 re f");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].paths.len(), 2);
}

#[test]
fn hostile_content_is_bounded() {
    // Huge numbers, unbalanced Q, endless subpaths: no panic, and caps hold.
    let mut c = String::from("Q Q Q 1e300 0 0 1e300 0 0 cm 0 0 1 1 re f ");
    for i in 0..(MAX_PAGE_SHAPES + 50) {
        c.push_str(&format!("{i} 0 1 1 re f "));
    }
    let r = reading_shapes(&page(&c, ""), 0).unwrap();
    assert!(r.shapes.len() <= MAX_PAGE_SHAPES);
    assert!(r.truncated, "shapes past the cap are reported");
    let mut long = String::from("0 0 m ");
    for i in 0..(MAX_SHAPE_POINTS + 500) {
        long.push_str(&format!("{i} 1 l "));
    }
    long.push('S');
    let r = reading_shapes(&page(&long, ""), 0).unwrap();
    assert!(r.shapes.iter().all(|s| s.paths.iter().map(Vec::len).sum::<usize>() <= MAX_SHAPE_POINTS));
}

/// A page whose text is drawn with fill opacity `ca` through ExtGState /G0 (`q /G0 gs … Q`),
/// then a line drawn after the `Q`.
fn faded_text(ca: &str) -> Document {
    let content = "q /G0 gs BT /F1 12 Tf 72 700 Td (Hidden layer) Tj ET Q BT /F1 12 Tf 72 600 Td (Shown text) Tj ET";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /ExtGState << /G0 6 0 R >> >> >>"
            .into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        format!("<< /Type /ExtGState /ca {ca} >>"),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

#[test]
fn text_with_zero_fill_opacity_is_transparent() {
    let blocks = crate::reading_blocks_checked(&faded_text("0"), 0).unwrap().blocks;
    let hidden = blocks.iter().find(|b| b.text.contains("Hidden layer")).expect("hidden layer read");
    let shown = blocks.iter().find(|b| b.text.contains("Shown text")).expect("shown text read");
    assert!(hidden.transparent && !hidden.invisible, "a text layer made invisible by opacity, not by Tr 3");
    assert!(!shown.transparent, "Q restores the opacity");
    let blocks = crate::reading_blocks_checked(&faded_text("0.5"), 0).unwrap().blocks;
    assert!(blocks.iter().all(|b| !b.transparent), "half-transparent text still shows");
}

/// A 600×800 page drawing `content` with Helvetica as /F1.
fn text_page(content: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// The reading blocks' texts of a page of 11 pt lines, each `(y, text)` at x = 72.
fn paragraphs(lines: &[(u32, &str)]) -> Vec<String> {
    let content: String = lines.iter().map(|(y, t)| format!("BT /F1 11 Tf 72 {y} Td ({t}) Tj ET ")).collect();
    crate::reading_blocks_checked(&text_page(&content), 0).unwrap().blocks.into_iter().map(|b| b.text).collect()
}

#[test]
fn a_first_line_indent_is_measured() {
    let content = "BT /F1 11 Tf 108 700 Td (Indented first line of a paragraph that is long) Tj -36 -14 Td (second line back at the margin of the text) Tj 0 -14 Td (third line also at the margin of the text) Tj ET";
    let blocks = crate::reading_blocks_checked(&text_page(content), 0).unwrap().blocks;
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert!((blocks[0].indent - 36.0).abs() < 0.5, "{}", blocks[0].indent);
}

#[test]
fn paragraphs_part_at_the_space_between_them() {
    // A one-line paragraph, 22 pt above a three-line one set at a 14 pt pitch.
    let p = paragraphs(&[
        (700, "A short opening line."),
        (678, "The next paragraph starts here and"),
        (664, "runs on over a second line and"),
        (650, "a third one."),
    ]);
    assert_eq!(p, ["A short opening line.", "The next paragraph starts here and runs on over a second line and a third one."]);
}

#[test]
fn lines_at_one_pitch_stay_one_paragraph() {
    let p = paragraphs(&[(700, "One paragraph over"), (686, "three lines at the"), (672, "same pitch, 12 apples.")]);
    assert_eq!(p.len(), 1, "{p:?}");
    // Two lines give no pitch to compare with: they stay together.
    assert_eq!(paragraphs(&[(700, "Two lines that"), (680, "belong together.")]).len(), 1);
}

#[test]
fn list_items_are_paragraphs() {
    let p = paragraphs(&[
        (700, "Before the trip:"),
        (686, "\\267 A warm jacket"),
        (672, "\\267 Walking shoes"),
        (658, "2. Fill the bottles"),
        (644, "3\\) Leave the route"),
    ]);
    assert_eq!(p.len(), 5, "{p:?}");
}

#[test]
fn a_table_row_does_not_run_into_the_paragraph_above() {
    // A paragraph line, then a header row of three cells on one baseline.
    let content = "BT /F1 11 Tf 72 700 Td (The table below lists the prices for the season.) Tj ET \
        BT /F1 11 Tf 72 686 Td (Item) Tj ET BT /F1 11 Tf 200 686 Td (Size) Tj ET BT /F1 11 Tf 300 686 Td (Price) Tj ET";
    let blocks = crate::reading_blocks_checked(&text_page(content), 0).unwrap().blocks;
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert!(texts.contains(&"The table below lists the prices for the season."), "{texts:?}");
}

#[test]
fn two_columns_of_text_keep_their_paragraphs() {
    // Lines of two columns share baselines (drawn a column at a time): neither column's rows are
    // table cells.
    let content = "BT /F1 11 Tf 72 700 Td (Left column first line) Tj ET BT /F1 11 Tf 72 686 Td (left column second line) Tj ET \
        BT /F1 11 Tf 320 700 Td (Right column first line) Tj ET BT /F1 11 Tf 320 686 Td (right column second line) Tj ET";
    let blocks = crate::reading_blocks_checked(&text_page(content), 0).unwrap().blocks;
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert!(texts.contains(&"Left column first line left column second line"), "{texts:?}");
}
