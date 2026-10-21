//! PowerPoint presentation with each page's content where it is on the page: one slide per page,
//! the slide the size of the page, vector shapes behind pictures behind text. The way a PDF opens
//! as slides that look like it ([`pptx()`](crate::pptx) instead reflows a page into a title and
//! body text, for a deck to rework).

use crate::pptx::{MAX_ITEM_EMU, emu, package, run_drawingml, slide_rels, slide_xml};
use crate::{Block, Image, Page, Shape, esc};

/// Points along an axis to EMU, clamped to `±MAX_ITEM_EMU` (NaN reads as 0).
fn pos(pt: f64) -> i64 {
    let v = pt * 12700.0;
    if v.is_finite() {
        v.round().clamp(-(MAX_ITEM_EMU as f64), MAX_ITEM_EMU as f64) as i64
    } else if v > 0.0 {
        MAX_ITEM_EMU
    } else {
        0
    }
}

/// A size in EMU, at least 1 so no shape is empty.
fn len(pt: f64) -> i64 {
    pos(pt).max(1)
}

/// Most shapes drawn on one slide, and most points in one of them: a chart with tens of
/// thousands of marks stays openable rather than exact.
const MAX_SHAPES: usize = 4000;
const MAX_POINTS: usize = 2000;

/// A counterclockwise angle on the page as DrawingML's clockwise 60 000ths of a degree; `None`
/// for upright.
fn rot(ccw: f64) -> Option<i64> {
    let cw = (-ccw).rem_euclid(360.0);
    (cw.is_finite() && cw > 0.01 && cw < 359.99).then(|| (cw * 60000.0).round() as i64)
}

fn xfrm(x: i64, y: i64, cx: i64, cy: i64, rotation: Option<i64>) -> String {
    let r = rotation.map(|r| format!(" rot=\"{r}\"")).unwrap_or_default();
    format!("<a:xfrm{r}><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>")
}

/// A shape colour as a fill, black included: a shape without one takes the theme's.
fn solid(color: [f64; 3]) -> String {
    let [r, g, b] = crate::rgb8(color.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 }));
    format!("<a:solidFill><a:srgbClr val=\"{r:02X}{g:02X}{b:02X}\"/></a:solidFill>")
}

/// One vector shape: an axis-aligned rectangle as a rectangle, anything else as a free-form path.
fn shape(id: usize, s: &Shape, page_h: f64) -> Option<String> {
    let points: Vec<[f64; 2]> = s.paths.iter().flatten().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
    let (x0, x1) = points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    let (y0, y1) = points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
    if points.is_empty() || !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return None;
    }
    let (w, h) = (len(x1 - x0), len(y1 - y0));
    let rect = s.paths.len() == 1
        && s.paths.first().is_some_and(|p| (4..=5).contains(&p.len()) && p.iter().all(|q| (q[0] == x0 || q[0] == x1) && (q[1] == y0 || q[1] == y1)));
    let geometry = if rect {
        "<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>".to_string()
    } else {
        let mut d = String::new();
        for sub in &s.paths {
            let mut pts = sub.iter().filter(|p| p[0].is_finite() && p[1].is_finite()).take(MAX_POINTS);
            let Some(first) = pts.next() else { continue };
            let at = |p: &[f64; 2]| format!("<a:pt x=\"{}\" y=\"{}\"/>", pos(p[0] - x0).clamp(0, w), pos(y1 - p[1]).clamp(0, h));
            d.push_str(&format!("<a:moveTo>{}</a:moveTo>", at(first)));
            for p in pts {
                d.push_str(&format!("<a:lnTo>{}</a:lnTo>", at(p)));
            }
            if s.closed {
                d.push_str("<a:close/>");
            }
        }
        format!(
            "<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"0\" t=\"0\" r=\"r\" b=\"b\"/><a:pathLst><a:path w=\"{w}\" h=\"{h}\">{d}</a:path></a:pathLst></a:custGeom>"
        )
    };
    let fill = s.fill.map(solid).unwrap_or_else(|| "<a:noFill/>".into());
    let line = match s.stroke {
        Some(c) => {
            let lw = if s.line_width.is_finite() && s.line_width > 0.0 { s.line_width.min(1000.0) } else { 1.0 };
            format!("<a:ln w=\"{}\">{}</a:ln>", pos(lw), solid(c))
        }
        None => "<a:ln><a:noFill/></a:ln>".into(),
    };
    Some(format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Shape {id}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}{geometry}{fill}{line}</p:spPr></p:sp>",
        xfrm(pos(x0), pos(page_h - y1), w, h, None)
    ))
}

fn picture(id: usize, r_id: &str, im: &Image, page_h: f64) -> String {
    let [x0, y0, x1, y1] = im.rect;
    let turned = rot(im.rotation);
    // A turned picture's rect is its bounding box; DrawingML turns the unturned size about its
    // centre.
    let (w, h) = if turned.is_some() { (im.size[0], im.size[1]) } else { (x1 - x0, y1 - y0) };
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    format!(
        "<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"Picture {id}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>\
<p:blipFill><a:blip r:embed=\"{r_id}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>\
<p:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
        xfrm(pos(cx - w / 2.0), pos(page_h - (cy + h / 2.0)), len(w), len(h), turned)
    )
}

fn text_box(id: usize, b: &Block, page_h: f64, ink: Ink) -> String {
    let [x0, y0, x1, y1] = b.rect;
    let turned = rot(b.rotation);
    // Text set in a substitute font may run a little wider: give it room rather than a new line.
    let (w, h) = if turned.is_some() { ((y1 - y0) * 1.04, x1 - x0) } else { ((x1 - x0) * 1.04, y1 - y0) };
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (left, top) = if turned.is_some() { (cx - w / 2.0, cy + h / 2.0) } else { (x0, y1) };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"TextBox {id}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>\
<p:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>\
<p:txBody><a:bodyPr wrap=\"{}\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" rtlCol=\"0\" anchor=\"t\"><a:noAutofit/></a:bodyPr><a:lstStyle/>{}</p:txBody></p:sp>",
        xfrm(pos(left), pos(page_h - top), len(w), len(h), turned),
        // Lines the page drew stay as drawn: a font that sets wider than the PDF's must not wrap
        // them onto the box below. Only a paragraph given without its lines may rewrap.
        if b.lines <= 1 || b.rows.len() >= 2 { "none" } else { "square" },
        paragraphs(b, ink)
    )
}

/// How an invisible text block is shown: in the colour of the outlines that drew it, or not at
/// all (text over a scan).
#[derive(Clone, Copy)]
enum Ink {
    Own,
    Outlines([f64; 3]),
    Hidden,
}

/// A text box's paragraphs: one per line the page drew (so a list without bullet characters
/// keeps its items apart, and the box looks as the page did), or the block as one paragraph.
fn paragraphs(b: &Block, ink: Ink) -> String {
    let indent =
        if b.lines >= 2 && b.indent.is_finite() && b.indent.abs() > 2.0 { format!("<a:pPr indent=\"{}\"/>", pos(b.indent)) } else { String::new() };
    let rows: Vec<&str> = b.rows.iter().map(String::as_str).filter(|r| !r.is_empty()).collect();
    if rows.len() < 2 {
        return format!("<a:p>{indent}{}</a:p>", run(b, &b.text, ink));
    }
    // The first line's indent (or, hanging, the others') as that paragraph's left margin.
    let shift = if b.indent.is_finite() && b.indent.abs() > 2.0 { b.indent } else { 0.0 };
    rows.iter()
        .enumerate()
        .map(|(k, r)| {
            let margin = if (k == 0) == (shift > 0.0) && shift != 0.0 { format!("<a:pPr marL=\"{}\"/>", pos(shift.abs())) } else { String::new() };
            format!("<a:p>{margin}{}</a:p>", run(b, r, ink))
        })
        .collect()
}

fn run(b: &Block, text: &str, ink: Ink) -> String {
    with_font(&styled_run(b, text, ink), &b.font)
}

/// `run` set in `font` (when it is known), after its fill as DrawingML orders run properties.
fn with_font(run: &str, font: &str) -> String {
    let font = font.trim();
    match run.find("</a:rPr>") {
        Some(at) if !font.is_empty() => {
            let f = esc(font);
            format!("{}<a:latin typeface=\"{f}\"/><a:cs typeface=\"{f}\"/>{}", &run[..at], &run[at..])
        }
        _ => run.to_string(),
    }
}

fn styled_run(b: &Block, text: &str, ink: Ink) -> String {
    match ink {
        Ink::Own => run_drawingml(text, b.size, b.bold, b.italic, b.color),
        Ink::Outlines(c) => run_drawingml(text, b.size, b.bold, b.italic, c),
        Ink::Hidden => {
            let shown = run_drawingml(text, b.size, b.bold, b.italic, [0.0; 3]);
            // Fully transparent text: found by search and copy, invisible over the scan's words.
            let fill = "<a:solidFill><a:srgbClr val=\"000000\"><a:alpha val=\"0\"/></a:srgbClr></a:solidFill>";
            match shown.find("</a:rPr>") {
                Some(at) => format!("{}{fill}{}", &shown[..at], &shown[at..]),
                None => shown,
            }
        }
    }
}

/// The bounding box `[x0, y0, x1, y1]` of a shape's finite points.
fn bounds(s: &Shape) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in s.paths.iter().flatten().filter(|p| p[0].is_finite() && p[1].is_finite()) {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    (b[0] <= b[2] && b[1] <= b[3]).then_some(b)
}

/// For each invisible block, the shapes that are its glyphs: filled, unstroked, inside the block
/// (with a little room) and no taller than a line of it. Each shape belongs to one block at most.
/// Returns, per shape, whether it is a glyph, and per block how to show it.
fn glyph_outlines(p: &Page) -> (Vec<bool>, Vec<Ink>) {
    let shapes = p.shapes.len().min(MAX_SHAPES);
    let boxes: Vec<Option<[f64; 4]>> = p.shapes.iter().take(shapes).map(bounds).collect();
    let mut claimed = vec![false; shapes];
    let inks = p
        .blocks
        .iter()
        .map(|b| {
            if !b.invisible {
                return Ink::Own;
            }
            let size = if b.size.is_finite() && b.size > 0.0 { b.size } else { 12.0 };
            let m = size * 0.3;
            let [x0, y0, x1, y1] = b.rect;
            let mut colour = None;
            for (i, s) in p.shapes.iter().take(shapes).enumerate() {
                let (Some(Some(bb)), Some(false), Some(fill)) = (boxes.get(i), claimed.get(i).copied(), s.fill) else { continue };
                let inside = bb[0] >= x0 - m && bb[1] >= y0 - m && bb[2] <= x1 + m && bb[3] <= y1 + m;
                if inside && s.stroke.is_none() && bb[3] - bb[1] <= size * 1.6 {
                    if let Some(c) = claimed.get_mut(i) {
                        *c = true;
                    }
                    colour.get_or_insert(fill);
                }
            }
            colour.map_or(Ink::Hidden, Ink::Outlines)
        })
        .collect();
    (claimed, inks)
}

/// A PowerPoint presentation of `pages`, each page a slide with its shapes, pictures and text
/// where they are on the page. The slides take the first page's size (a presentation has one).
/// No pages give one blank slide, so the package stays valid.
pub fn pptx_positioned(pages: &[Page], title: &str) -> Vec<u8> {
    let dummy = [Page::default()];
    let pages = if pages.is_empty() { &dummy[..] } else { pages };
    let (sw, sh) = pages.first().map_or((9144000, 6858000), |p| (emu(p.width, 720.0), emu(p.height, 540.0)));
    let mut media: Vec<(String, &Image)> = Vec::new();
    let mut slides = Vec::with_capacity(pages.len());
    for p in pages {
        let page_h = if p.height.is_finite() && p.height > 0.0 { p.height } else { sh as f64 / 12700.0 };
        let mut shapes = String::new();
        let mut rels = Vec::new();
        let mut id = 2usize;
        let (glyphs, inks) = glyph_outlines(p);
        for (i, s) in p.shapes.iter().take(MAX_SHAPES).enumerate() {
            if glyphs.get(i).copied().unwrap_or(false) {
                continue;
            }
            if let Some(xml) = shape(id, s, page_h) {
                shapes.push_str(&xml);
                id += 1;
            }
        }
        for im in &p.images {
            let name = format!("image{}.{}", media.len() + 1, im.ext);
            let r_id = format!("rIdImg{}", rels.len() + 1);
            shapes.push_str(&picture(id, &r_id, im, page_h));
            rels.push((r_id, name.clone()));
            media.push((name, im));
            id += 1;
        }
        for (b, ink) in p.blocks.iter().zip(inks).filter(|(b, _)| !b.text.trim().is_empty()) {
            shapes.push_str(&text_box(id, b, page_h, ink));
            id += 1;
        }
        slides.push((slide_xml(&shapes), slide_rels(&rels)));
    }
    package(&slides, &media, sw, sh, title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::part;
    use crate::{Block, Image, Page, Shape};

    const EMU: i64 = 12700;

    fn block(text: &str, rect: [f64; 4], size: f64) -> Block {
        Block {
            text: text.into(),
            rect,
            size,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            invisible: false,
            font: String::new(),
            lines: 1,
            indent: 0.0,
            rows: Vec::new(),
        }
    }

    fn page() -> Page {
        Page {
            width: 720.0,
            height: 540.0,
            blocks: vec![
                Block { bold: true, color: [1.0, 1.0, 1.0], ..block("Northern Lights", [72.0, 420.0, 400.0, 460.0], 40.0) },
                block("A short tour of the aurora", [72.0, 380.0, 400.0, 400.0], 16.0),
            ],
            images: vec![Image {
                ext: "png",
                bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03".to_vec(),
                rect: [400.0, 100.0, 650.0, 300.0],
                size: [250.0, 200.0],
                rotation: 0.0,
            }],
            shapes: vec![
                // A dark full-slide background and a triangle with an outline.
                Shape {
                    paths: vec![vec![[0.0, 0.0], [720.0, 0.0], [720.0, 540.0], [0.0, 540.0]]],
                    closed: true,
                    fill: Some([0.1, 0.15, 0.25]),
                    ..Shape::default()
                },
                Shape {
                    paths: vec![vec![[100.0, 100.0], [200.0, 100.0], [150.0, 200.0]]],
                    closed: true,
                    fill: Some([1.0, 0.0, 0.0]),
                    stroke: Some([0.0, 0.0, 1.0]),
                    line_width: 2.0,
                },
            ],
        }
    }

    #[test]
    fn black_artwork_stays_black() {
        let mut p = page();
        p.shapes = vec![Shape {
            paths: vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]],
            closed: true,
            fill: Some([0.0; 3]),
            stroke: Some([0.0; 3]),
            line_width: 1.0,
        }];
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        // Without a colour, the shape would take the theme's.
        assert_eq!(slide.matches("<a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill>").count(), 2, "{slide}");
    }

    #[test]
    fn slide_size_is_the_page_size() {
        let pres = part(&pptx_positioned(&[page()], "Deck"), "ppt/presentation.xml");
        assert!(pres.contains(&format!("<p:sldSz cx=\"{}\" cy=\"{}\"/>", 720 * EMU, 540 * EMU)), "{pres}");
    }

    #[test]
    fn text_keeps_its_place_on_the_page() {
        let slide = part(&pptx_positioned(&[page()], "Deck"), "ppt/slides/slide1.xml");
        // y is measured from the top: 540 − 460 = 80 pt.
        assert!(slide.contains(&format!("<a:off x=\"{}\" y=\"{}\"/>", 72 * EMU, 80 * EMU)), "{slide}");
        assert!(slide.contains(&format!("<a:off x=\"{}\" y=\"{}\"/>", 72 * EMU, 140 * EMU)), "subtitle at 540 − 400: {slide}");
        assert!(slide.contains("Northern Lights") && slide.contains("A short tour of the aurora"));
        assert!(slide.contains("<a:srgbClr val=\"FFFFFF\"/>"), "white title");
        assert!(slide.contains("lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\""), "no insets, so the text sits where it was");
    }

    #[test]
    fn pictures_keep_their_place_and_size() {
        let slide = part(&pptx_positioned(&[page()], "Deck"), "ppt/slides/slide1.xml");
        assert!(
            slide.contains(&format!("<a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/>", 400 * EMU, 240 * EMU, 250 * EMU, 200 * EMU)),
            "{slide}"
        );
    }

    #[test]
    fn rectangles_and_paths_are_drawn_behind_everything() {
        let slide = part(&pptx_positioned(&[page()], "Deck"), "ppt/slides/slide1.xml");
        let background = slide.find("<a:srgbClr val=\"1A2640\"/>").expect("background fill");
        let triangle = slide.find("<a:custGeom>").expect("triangle as a free-form path");
        let picture = slide.find("<p:pic>").expect("picture");
        let title = slide.find("Northern Lights").expect("title");
        assert!(background < triangle && triangle < picture && picture < title, "paint order: shapes, pictures, text");
        assert!(slide.contains("<a:prstGeom prst=\"rect\">"), "an axis-aligned rectangle stays a rectangle");
        assert!(slide.contains(&format!("<a:ln w=\"{}\">", 2 * EMU)), "outline width");
        assert!(slide.contains("<a:srgbClr val=\"0000FF\"/>"), "outline colour");
    }

    #[test]
    fn a_first_line_indent_is_kept() {
        let mut p = page();
        p.blocks = vec![Block { indent: 24.0, lines: 3, ..block("A paragraph of three lines", [72.0, 300.0, 400.0, 345.0], 12.0) }];
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        assert!(slide.contains(&format!("<a:pPr indent=\"{}\"/>", 24 * EMU)), "{slide}");
    }

    #[test]
    fn text_boxes_keep_their_lines_and_font() {
        // Drawn lines never rewrap in a font that sets wider than the PDF's, and the family is named.
        let mut p = page();
        p.blocks = vec![Block { font: "Times New Roman".into(), ..block("Kaan Doğan", [72.0, 700.0, 180.0, 724.0], 24.0) }];
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        assert!(slide.contains("wrap=\"none\""), "{slide}");
        assert!(slide.contains("<a:latin typeface=\"Times New Roman\"/>"), "{slide}");
        // A paragraph the page wrapped, given as one block without its lines, may wrap again.
        let mut p = page();
        p.blocks = vec![Block { lines: 3, ..block("A paragraph the page set over three lines", [72.0, 300.0, 200.0, 345.0], 12.0) }];
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        assert!(slide.contains("wrap=\"square\""), "{slide}");
    }

    #[test]
    fn each_line_of_a_text_box_is_a_paragraph() {
        // A list drawn without bullet characters: the lines keep their breaks.
        let mut p = page();
        let rows = ["Ship the app in October", "Reach ten thousand users", "Keep replies under a day"];
        p.blocks = vec![Block { lines: 3, rows: rows.map(String::from).to_vec(), ..block(&rows.join(" "), [72.0, 300.0, 400.0, 345.0], 12.0) }];
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        assert_eq!(slide.matches("<a:p>").count(), 3, "{slide}");
        assert!(rows.iter().all(|r| slide.contains(&format!(">{r}</a:t>"))), "{slide}");
        // A first-line indent stays on the first line.
        let mut p = page();
        p.blocks = vec![Block {
            lines: 3,
            indent: 24.0,
            rows: rows.map(String::from).to_vec(),
            ..block(&rows.join(" "), [72.0, 300.0, 400.0, 345.0], 12.0)
        }];
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        assert_eq!(slide.matches(&format!("<a:pPr marL=\"{}\"/>", 24 * EMU)).count(), 1, "{slide}");
    }

    #[test]
    fn rotated_text_is_turned() {
        let mut p = page();
        p.blocks = vec![Block { rotation: 90.0, ..block("Up the side", [20.0, 100.0, 40.0, 300.0], 12.0) }];
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        // Counterclockwise 90° on the page is 270° clockwise in DrawingML (60 000ths of a degree).
        assert!(slide.contains("rot=\"16200000\""), "{slide}");
    }

    #[test]
    fn every_page_is_a_slide_and_the_package_is_whole() {
        let bytes = pptx_positioned(&[page(), Page { width: 720.0, height: 540.0, ..Page::default() }], "Deck");
        assert!(part(&bytes, "ppt/slides/slide2.xml").contains("<p:spTree>"));
        assert!(part(&bytes, "ppt/slides/_rels/slide1.xml.rels").contains("../media/image1.png"));
        assert!(part(&bytes, "[Content_Types].xml").contains("/ppt/slides/slide2.xml"));
        assert!(part(&pptx_positioned(&[], "Empty"), "ppt/slides/slide1.xml").contains("<p:spTree>"), "no pages: one blank slide");
    }

    #[test]
    fn hostile_numbers_stay_in_range() {
        let p = Page {
            width: f64::NAN,
            height: 1e300,
            blocks: vec![block("x", [f64::NEG_INFINITY, f64::NAN, 1e308, -1e308], f64::INFINITY)],
            images: vec![],
            shapes: vec![Shape {
                paths: vec![vec![[f64::NAN, 1e300]; 3], (0..100_000).map(|i| [i as f64, 0.0]).collect()],
                closed: true,
                fill: Some([2.0, -1.0, f64::NAN]),
                stroke: None,
                line_width: f64::INFINITY,
            }],
        };
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        assert!(!slide.contains("NaN") && !slide.contains("inf"), "{}", &slide[..slide.len().min(400)]);
        assert!(slide.len() < 4_000_000, "points per path are capped");
    }

    /// Glyph outlines of "Hi" (two filled bars and a dot) inside an invisible text block, the way
    /// some exporters draw text as paths with a selectable layer on top.
    fn outlined_text() -> Page {
        let bar = |x: f64, top: f64| Shape {
            paths: vec![vec![[x, 400.0], [x + 3.0, 400.0], [x + 3.0, top], [x, top]]],
            closed: true,
            fill: Some([1.0, 0.0, 0.0]),
            ..Shape::default()
        };
        Page {
            width: 720.0,
            height: 540.0,
            blocks: vec![Block { invisible: true, ..block("Hi", [100.0, 398.0, 130.0, 414.0], 14.0) }],
            images: vec![],
            shapes: vec![
                Shape {
                    paths: vec![vec![[0.0, 0.0], [720.0, 0.0], [720.0, 540.0], [0.0, 540.0]]],
                    closed: true,
                    fill: Some([0.9, 0.9, 0.9]),
                    ..Shape::default()
                },
                bar(101.0, 412.0),
                bar(110.0, 412.0),
                bar(118.0, 409.0),
            ],
        }
    }

    #[test]
    fn outlined_text_becomes_editable_text_in_its_colour() {
        let slide = part(&pptx_positioned(&[outlined_text()], "Deck"), "ppt/slides/slide1.xml");
        assert_eq!(slide.matches("<p:sp>").count(), 2, "the page background and the text box; the glyph outlines are gone: {slide}");
        assert!(slide.contains("<a:srgbClr val=\"FF0000\"/></a:solidFill></a:rPr><a:t>Hi</a:t>"), "the text takes the outlines' colour: {slide}");
        assert!(slide.contains("<a:srgbClr val=\"E6E6E6\"/>"), "large artwork behind the text stays");
    }

    #[test]
    fn invisible_text_over_a_scan_stays_invisible() {
        let mut p = outlined_text();
        p.shapes.truncate(1);
        let slide = part(&pptx_positioned(&[p], "Deck"), "ppt/slides/slide1.xml");
        assert!(slide.contains("<a:alpha val=\"0\"/>"), "OCR text is kept for search and copy but not drawn over the scan: {slide}");
    }
}
