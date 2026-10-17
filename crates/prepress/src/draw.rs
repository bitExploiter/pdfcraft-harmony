//! The appearance streams of the individual marks, drawn in unrotated page user space.
//!
//! Every mark's form `/BBox` equals its annotation `/Rect` under the identity matrix, as the
//! generated appearances in `pdfcraft-annot` do, so a mark is drawn in the page's own
//! coordinates and needs no placement matrix.

use pdfcraft_fonts::{helvetica_width, literal, win_ansi};

use crate::{Info, Rect, Style};

/// Bezier handle length for a quarter circle of radius 1 (4/3 · (√2 − 1)).
const KAPPA: f64 = 0.552_284_749_830_793_4;
/// Gap between the crop/registration band and the colour bar or page-information line, in points.
pub(crate) const BAND_GAP: f64 = 3.0;
/// Patches of the colour bar: `(cyan, magenta, yellow, black)`, in drawing order.
const PATCHES: [[f64; 4]; 8] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
    [1.0, 1.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 0.75],
    [0.0, 0.0, 0.0, 0.5],
    [0.0, 0.0, 0.0, 0.25],
];

/// One mark, ready to become an annotation: the spec's `/MN` name, the rectangle it occupies and
/// the operators that draw it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Mark {
    pub name: &'static str,
    pub rect: Rect,
    pub content: Vec<u8>,
    /// Whether the appearance needs the Helvetica resource.
    pub font: bool,
}

/// A number as PDF syntax: at most three decimals, no trailing zeros, no negative zero.
/// (`pdfcraft-annot` keeps the same formatter for the same reason: stable, short appearances.)
pub(crate) fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// `r` grown by `by` on every side.
fn grow(r: Rect, by: f64) -> Rect {
    [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
}

/// Stroke setup shared by the marks drawn in the `All` separation. Butt caps and miter joins, so
/// a cut mark ends exactly where its geometry says it does.
fn stroke_prologue(style: &Style) -> String {
    format!("{} w 0 J 0 j /CS0 CS 1 SCN\n", n(style.weight))
}

/// The two arms of one corner mark. `sx`/`sy` point away from the box, so the arms never cross
/// into the area the mark measures.
pub(crate) fn corner(name: &'static str, at: [f64; 2], sx: f64, sy: f64, style: &Style) -> Mark {
    let [cx, cy] = at;
    let (near, far) = (style.offset, style.offset + style.length);
    let mut content = stroke_prologue(style);
    content.push_str(&format!("{} {} m {} {} l S\n", n(cx + sx * far), n(cy), n(cx + sx * near), n(cy)));
    content.push_str(&format!("{} {} m {} {} l S\n", n(cx), n(cy + sy * far), n(cx), n(cy + sy * near)));
    let (xa, xb) = (cx, cx + sx * far);
    let (ya, yb) = (cy, cy + sy * far);
    let rect = [xa.min(xb), ya.min(yb), xa.max(xb), ya.max(yb)];
    Mark { name, rect: grow(rect, style.weight / 2.0), content: content.into_bytes(), font: false }
}

/// A registration target: a stroked circle with a crosshair reaching twice its radius, the mark
/// press operators align plates by.
pub(crate) fn registration(centre: [f64; 2], style: &Style) -> Mark {
    let [x, y] = centre;
    let r = style.radius;
    let k = KAPPA * r;
    let mut content = stroke_prologue(style);
    content.push_str(&format!("{} {} m\n", n(x + r), n(y)));
    content.push_str(&format!("{} {} {} {} {} {} c\n", n(x + r), n(y + k), n(x + k), n(y + r), n(x), n(y + r)));
    content.push_str(&format!("{} {} {} {} {} {} c\n", n(x - k), n(y + r), n(x - r), n(y + k), n(x - r), n(y)));
    content.push_str(&format!("{} {} {} {} {} {} c\n", n(x - r), n(y - k), n(x - k), n(y - r), n(x), n(y - r)));
    content.push_str(&format!("{} {} {} {} {} {} c\n", n(x + k), n(y - r), n(x + r), n(y - k), n(x + r), n(y)));
    content.push_str("S\n");
    let reach = r * 2.0;
    content.push_str(&format!("{} {} m {} {} l S\n", n(x - reach), n(y), n(x + reach), n(y)));
    content.push_str(&format!("{} {} m {} {} l S\n", n(x), n(y - reach), n(x), n(y + reach)));
    let rect = [x - reach, y - reach, x + reach, y + reach];
    Mark { name: "RegistrationTarget", rect: grow(rect, style.weight / 2.0), content: content.into_bytes(), font: false }
}

/// How far a registration target of this style reaches from its centre.
pub(crate) fn registration_reach(style: &Style) -> f64 {
    style.radius * 2.0 + style.weight / 2.0
}

/// The width a colour bar of this style takes.
pub(crate) fn color_bar_width(style: &Style) -> f64 {
    style.patch * PATCHES.len() as f64
}

/// A row of solid `DeviceCMYK` patches: the four process inks, their three-colour overprint and
/// three tints of black, for checking ink density and overprint on press.
pub(crate) fn color_bar(origin: [f64; 2], style: &Style) -> Mark {
    let [x, y] = origin;
    let p = style.patch;
    let mut content = String::new();
    for (i, ink) in PATCHES.iter().enumerate() {
        let px = x + p * i as f64;
        let cmyk = ink.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" ");
        content.push_str(&format!("{cmyk} k {} {} {} {} re f\n", n(px), n(y), n(p), n(p)));
    }
    let rect = [x, y, x + color_bar_width(style), y + p];
    Mark { name: "ColorBar", rect, content: content.into_bytes(), font: false }
}

/// The height a page-information line of this style takes.
pub(crate) fn info_height(style: &Style) -> f64 {
    style.font_size * 1.2
}

/// The page-information line: the document's name at the left, the page's position in the middle
/// and the caller's stamp at the right, on one baseline across `span`.
///
/// `page` is 0-based and reported 1-based, as a reader counts pages.
pub(crate) fn page_info(span: (f64, f64), y: f64, page: usize, total: usize, info: &Info, style: &Style) -> Mark {
    let size = style.font_size;
    let (x0, x1) = span;
    let middle = format!("Page {} of {}", page.saturating_add(1), total);
    let baseline = y + size * 0.2;
    let mut content = format!("BT /Helv {} Tf /CS0 cs 1 scn\n", n(size)).into_bytes();
    for (text, x) in [
        (info.name.as_str(), x0),
        (middle.as_str(), (x0 + x1) / 2.0 - helvetica_width(&middle, size) / 2.0),
        (info.stamp.as_str(), x1 - helvetica_width(&info.stamp, size)),
    ] {
        if text.is_empty() {
            continue;
        }
        content.extend_from_slice(format!("1 0 0 1 {} {} Tm ", n(x), n(baseline)).as_bytes());
        content.extend_from_slice(&literal(&win_ansi(text)));
        content.extend_from_slice(b" Tj\n");
    }
    content.extend_from_slice(b"ET\n");
    Mark { name: "PageInformation", rect: [x0, y, x1, y + info_height(style)], content, font: true }
}
