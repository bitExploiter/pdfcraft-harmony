//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A single closed `m`/`l` subpath
//! whose four corners form an axis-aligned rectangle in user space is the same shape and counts
//! too (#1510). Graphics state tracked here is deliberately small: the CTM stack (`q`/`Q`/`cm`),
//! the non-stroking color (`g`/`rg`/`k`, `cs`, `sc`, `scn`, defaulting unknown color spaces to
//! black fail-closed), the clipping state (`re W n`, form `/BBox`, and text clipping modes `Tr 4..=7`),
//! optional content layer visibility (`/OC BDC`/`EMC`), and the non-stroking alpha from ExtGState
//! (`gs` reading `/ca`). Form XObjects are followed read-only with a depth cap. Anything fancier
//! (other filled paths, non-axis-aligned rotations, shadings, patterns) is out of scope and
//! documented as such.
//!
//! Paint order: a page's content streams are one stream in pieces (ISO 32000-2 §7.8.2), so they
//! are parsed joined and walked once with a single graphics state — state set in an earlier
//! stream (a `cm`, a fill color) is still in force in the later ones. Each rectangle records the
//! position of the fill operator that painted it, as `(content-stream index, operator index in
//! the joined stream)`; rectangles inside a form XObject carry the `Do` operator's position.
//! Positions compare lexicographically in paint order.

use std::collections::HashSet;

use pdfcraft_content::{Matrix, Op, Pieces, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::AuditError;

/// Maximum form-XObject nesting followed. Deeper nesting (including a cycle) fails the audit: a
/// cover drawn below the cut could otherwise go unseen and the page be reported clean.
const MAX_DEPTH: usize = 32;
/// Bound on total operators walked per page, so a hostile stream cannot spin forever.
/// Hitting it fails the audit: a truncated walk must never report "clean".
const MAX_OPS: usize = 2_000_000;

/// One opaque filled rectangle, in page user space.
#[derive(Clone, Debug, PartialEq)]
pub struct FilledRect {
    /// `[x0, y0, x1, y1]`, normalized so `x0 <= x1` and `y0 <= y1`.
    pub rect: [f64; 4],
    /// The fill color as sRGB-ish `[r, g, b]` (CMYK is converted approximately).
    pub rgb: [f64; 3],
    /// Paint position: `(content-stream index, operator index in the joined stream)`.
    /// Rectangles inside a form XObject carry the `Do` operator's position.
    pub pos: (usize, usize),
}

/// Active clipping state in page user space.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Clip {
    /// No clipping has been applied yet.
    None,
    /// An axis-aligned clipping rectangle in user space.
    Rect([f64; 4]),
    /// Text glyph clipping active (`Tr 4..=7`). Fills through this clip paint the text glyphs, not a rectangle cover.
    Glyphs,
    /// Non-rectangular or complex clipping path (fail-closed: do not prune candidate covers).
    Unknown,
}

impl Clip {
    fn intersect_rect(self, r: [f64; 4]) -> Clip {
        match self {
            Clip::None => Clip::Rect(r),
            Clip::Rect(c) => match intersect(c, r) {
                Some(cr) => Clip::Rect(cr),
                None => Clip::Rect([0.0, 0.0, 0.0, 0.0]),
            },
            Clip::Glyphs => Clip::Glyphs,
            Clip::Unknown => Clip::Unknown,
        }
    }
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    fill: [f64; 3],
    alpha: f64,
    clip: Clip,
    tr: u32,
    space: usize,
}

/// The path under construction, as far as the audit cares about it.
enum Path {
    Empty,
    /// A `re` rectangle, in user space.
    Rect([f64; 4]),
    /// One `m`/`l` subpath, corners already mapped to user space; `true` once closed by `h`.
    Lines(Vec<[f64; 2]>, bool),
    /// Anything else: curves, a second subpath, a CTM change mid-path.
    Other,
}

impl Path {
    /// The rectangle a fill of this path paints, if the path is one.
    fn rect(&self) -> Option<[f64; 4]> {
        match self {
            Path::Rect(r) => Some(*r),
            Path::Lines(pts, _) => lines_rect(pts),
            Path::Empty | Path::Other => None,
        }
    }
}

/// The bounding box of a closed polygon that is an axis-aligned rectangle, or `None`. A repeated
/// start point is dropped first; the remaining four corners must alternate horizontal and
/// vertical edges.
fn lines_rect(pts: &[[f64; 2]]) -> Option<[f64; 4]> {
    // CTM products leave rounding noise, so corners compare with a tolerance (in points).
    const EPS: f64 = 1e-6;
    let same = |a: f64, b: f64| (a - b).abs() <= EPS;
    let slice = if pts.len() == 5 && same(pts[0][0], pts[4][0]) && same(pts[0][1], pts[4][1]) { pts.get(..4)? } else { pts };
    let [a, b, c, d] = <[[f64; 2]; 4]>::try_from(slice).ok()?;
    let fits = |first_horizontal: bool| {
        [(a, b), (b, c), (c, d), (d, a)]
            .iter()
            .enumerate()
            .all(|(i, (p, q))| if (i % 2 == 0) == first_horizontal { same(p[1], q[1]) } else { same(p[0], q[0]) })
    };
    if !(fits(true) || fits(false)) {
        return None;
    }
    let (xs, ys) = ([a[0], b[0], c[0], d[0]], [a[1], b[1], c[1], d[1]]);
    let r = [
        xs.into_iter().fold(f64::INFINITY, f64::min),
        ys.into_iter().fold(f64::INFINITY, f64::min),
        xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
        ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
    ];
    (r[2] - r[0] > EPS && r[3] - r[1] > EPS).then_some(r)
}

fn axis_aligned(m: &Matrix) -> bool {
    let [a, b, c, d, ..] = m.0;
    (b.abs() < 1e-9 && c.abs() < 1e-9) || (a.abs() < 1e-9 && d.abs() < 1e-9)
}

fn intersect(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]).then_some(r)
}

impl Default for Gs {
    fn default() -> Self {
        Gs { ctm: Matrix::IDENTITY, fill: [0.0, 0.0, 0.0], alpha: 1.0, clip: Clip::None, tr: 0, space: 1 }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    pieces: &'a Pieces,
    out: Vec<FilledRect>,
    ops_seen: usize,
    off_layers: HashSet<ObjRef>,
    oc_hidden_depth: usize,
}

impl<'a> Walker<'a> {
    fn bump(&mut self) -> Result<(), AuditError> {
        self.ops_seen += 1;
        if self.ops_seen > MAX_OPS {
            return Err(AuditError::TooManyOps);
        }
        Ok(())
    }

    /// Walk one operator slice with the given graphics state. `at_do` is the `Do` position to
    /// stamp on rectangles found inside a form XObject, or `None` at page level (where each
    /// fill operator's own position is used).
    fn walk_ops(
        &mut self,
        ops: &[Op],
        gs: &mut Gs,
        stack: &mut Vec<Gs>,
        resources: &Dict,
        depth: usize,
        at_do: Option<(usize, usize)>,
    ) -> Result<(), AuditError> {
        let mut path = Path::Empty;
        let mut clip_next = false;
        for (i, op) in ops.iter().enumerate() {
            self.bump()?;
            // Paint position of this operator: its own `(piece, joined index)` at page level,
            // the enclosing `Do`'s position inside a form XObject.
            let pos = match at_do {
                Some(p) => p,
                None => (self.pieces.pieces_of(op).0, i),
            };
            match op.op.as_slice() {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        *gs = g;
                    }
                    path = Path::Empty;
                    clip_next = false;
                }
                b"cm" => {
                    let before = gs.ctm;
                    if let Some(m) = Matrix::from_operands(&op.operands)
                        && m.0.iter().all(|v| v.is_finite())
                    {
                        gs.ctm = m.then(&gs.ctm);
                    }
                    // A `cm` that leaves the CTM unchanged (an identity matrix) cannot move the
                    // pending path, so keep it for the fill (#1333).
                    if gs.ctm != before && !matches!(path, Path::Empty) {
                        path = Path::Other;
                    }
                }
                b"g" => {
                    gs.space = 1;
                    if let Some(v) = op.num(0).filter(|v| v.is_finite()) {
                        let v = v.clamp(0.0, 1.0);
                        gs.fill = [v, v, v];
                    }
                }
                b"rg" => {
                    gs.space = 3;
                    if let Some([r, g, b]) = op.nums::<3>()
                        && [r, g, b].iter().all(|v| v.is_finite())
                    {
                        gs.fill = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
                    }
                }
                b"k" => {
                    gs.space = 4;
                    if let Some([c, m, y, k]) = op.nums::<4>()
                        && [c, m, y, k].iter().all(|v| v.is_finite())
                    {
                        // Naive CMYK -> RGB; good enough to tell black from white.
                        let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
                        gs.fill = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
                    }
                }
                b"cs" => {
                    if let Some(name) = op.name(0) {
                        gs.space = match name {
                            b"DeviceGray" => 1,
                            b"DeviceRGB" => 3,
                            b"DeviceCMYK" => 4,
                            _ => 0,
                        };
                        gs.fill = [0.0, 0.0, 0.0];
                    }
                }
                b"sc" | b"scn" => match gs.space {
                    1 => {
                        if let Some(v) = op.num(0).filter(|v| v.is_finite()) {
                            let v = v.clamp(0.0, 1.0);
                            gs.fill = [v, v, v];
                        }
                    }
                    3 => {
                        if let Some([r, g, b]) = op.nums::<3>()
                            && [r, g, b].iter().all(|v| v.is_finite())
                        {
                            gs.fill = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
                        }
                    }
                    4 => {
                        if let Some([c, m, y, k]) = op.nums::<4>()
                            && [c, m, y, k].iter().all(|v| v.is_finite())
                        {
                            let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
                            gs.fill = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
                        }
                    }
                    _ => {
                        gs.fill = [0.0, 0.0, 0.0];
                    }
                },
                b"gs" => {
                    if let Some(name) = op.name(0) {
                        gs.alpha = ext_alpha(self.doc, resources, name);
                    }
                }
                b"re" => {
                    let rect = op.nums::<4>().and_then(|[x, y, w, h]| {
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w != 0.0 && h != 0.0 && axis_aligned(&gs.ctm) {
                            let r = gs.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                            (r[2] > r[0] && r[3] > r[1]).then_some(r)
                        } else {
                            None
                        }
                    });
                    path = rect.map_or(Path::Other, Path::Rect);
                }
                b"m" => {
                    path = match (path, user_point(gs, op)) {
                        (Path::Empty, Some(p)) => Path::Lines(vec![p], false),
                        _ => Path::Other,
                    };
                }
                b"l" => {
                    path = match (path, user_point(gs, op)) {
                        // Past a repeated start point it can no longer be a rectangle.
                        (Path::Lines(mut pts, false), Some(p)) if pts.len() < 5 => {
                            pts.push(p);
                            Path::Lines(pts, false)
                        }
                        _ => Path::Other,
                    };
                }
                b"h" => {
                    path = match path {
                        Path::Lines(pts, _) => Path::Lines(pts, true),
                        // `re` already closed its subpath, so a further `h` changes nothing.
                        Path::Rect(r) => Path::Rect(r),
                        _ => Path::Other,
                    };
                }
                b"W" | b"W*" => clip_next = true,
                b"n" => {
                    if clip_next {
                        if let Some(r) = path.rect() {
                            gs.clip = gs.clip.intersect_rect(r);
                        } else {
                            gs.clip = Clip::Unknown;
                        }
                        clip_next = false;
                    }
                    path = Path::Empty;
                }
                // `b`/`b*` close the path before filling it, which the fill does anyway.
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                    let p = std::mem::replace(&mut path, Path::Empty);
                    if clip_next {
                        if let Some(r) = p.rect() {
                            gs.clip = gs.clip.intersect_rect(r);
                        } else {
                            gs.clip = Clip::Unknown;
                        }
                        clip_next = false;
                    }
                    if self.oc_hidden_depth == 0
                        && gs.alpha >= 1.0
                        && let Some(r) = p.rect()
                    {
                        let visible_r = match gs.clip {
                            Clip::None | Clip::Unknown => Some(r),
                            Clip::Rect(c) => intersect(r, c),
                            Clip::Glyphs => None,
                        };
                        if let Some(r) = visible_r {
                            self.out.push(FilledRect { rect: r, rgb: gs.fill, pos });
                        }
                    }
                }
                b"S" | b"s" => {
                    if clip_next {
                        if let Some(r) = path.rect() {
                            gs.clip = gs.clip.intersect_rect(r);
                        } else {
                            gs.clip = Clip::Unknown;
                        }
                        clip_next = false;
                    }
                    path = Path::Empty;
                }
                b"c" | b"v" | b"y" => path = Path::Other,
                b"Tr" => {
                    if let Some(mode) = op.num(0).and_then(|v| (v.is_finite() && v >= 0.0).then_some(v as u32)) {
                        gs.tr = mode;
                    }
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if (4..=7).contains(&gs.tr) {
                        gs.clip = Clip::Glyphs;
                    }
                }
                b"BDC" => {
                    let is_off =
                        if op.name(0) == Some(b"OC") { is_layer_off(self.doc, resources, &self.off_layers, op.operands.get(1)) } else { false };
                    if is_off || self.oc_hidden_depth > 0 {
                        self.oc_hidden_depth += 1;
                    }
                }
                b"BMC" => {
                    if self.oc_hidden_depth > 0 {
                        self.oc_hidden_depth += 1;
                    }
                }
                b"EMC" => {
                    if self.oc_hidden_depth > 0 {
                        self.oc_hidden_depth -= 1;
                    }
                }
                b"Do" => {
                    if let Some(name) = op.name(0) {
                        if depth >= MAX_DEPTH {
                            return Err(AuditError::TooDeep);
                        }
                        self.walk_xobject(name, gs, resources, depth, pos)?;
                    }
                    path = Path::Empty;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Follow a form XObject read-only, composing its matrix onto the current CTM. State
    /// changes inside never leak out; every rectangle found is stamped with the `Do`
    /// position `pos`.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, resources: &Dict, depth: usize, pos: (usize, usize)) -> Result<(), AuditError> {
        if self.oc_hidden_depth > 0 {
            return Ok(());
        }
        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        let Some(entry) = xobjects.get(name) else { return Ok(()) };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return Ok(()) };
        let dict = &stream.dict;
        if dict.name(b"Subtype") != Some(b"Form") {
            return Ok(());
        }
        if let Some(Object::Ref(r)) = dict.get(b"OC")
            && self.off_layers.contains(r)
        {
            return Ok(());
        }
        let matrix = dict
            .get(b"Matrix")
            .map(|o| self.doc.resolve(o))
            .and_then(|o| o.as_array().map(|a| a.to_vec()))
            .and_then(|a| {
                let v: Vec<f64> = a.iter().filter_map(|o| self.doc.resolve(o).as_f64()).collect();
                (v.len() == 6 && v.iter().all(|x| x.is_finite())).then(|| Matrix([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .unwrap_or(Matrix::IDENTITY);
        let bbox = dict.get(b"BBox").map(|o| self.doc.resolve(o)).and_then(|o| o.as_array().map(|a| a.to_vec())).and_then(|a| {
            let v: Vec<f64> = a.iter().filter_map(|o| self.doc.resolve(o).as_f64()).collect();
            (v.len() == 4 && v.iter().all(|x| x.is_finite())).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
        });
        let sub_resources = dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        // A form stream that cannot be decoded fails the audit: it might hide a cover.
        let data = stream.decoded_strict()?;
        let ops = parse(&data).ops;
        // The XObject matrix maps form space into the user space active at `Do`.
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        if let Some(bbox) = bbox {
            if axis_aligned(&sub_gs.ctm) {
                let form_box = sub_gs.ctm.bbox(bbox);
                sub_gs.clip = sub_gs.clip.intersect_rect(form_box);
            } else {
                sub_gs.clip = Clip::Unknown;
            }
        }
        let saved_oc_depth = self.oc_hidden_depth;
        let mut sub_stack = Vec::new();
        let res = self.walk_ops(&ops, &mut sub_gs, &mut sub_stack, &sub_resources, depth + 1, Some(pos));
        self.oc_hidden_depth = saved_oc_depth;
        res
    }
}

/// The `x y` operands of `m`/`l`, mapped to user space.
fn user_point(gs: &Gs, op: &Op) -> Option<[f64; 2]> {
    let [x, y] = op.nums::<2>().filter(|v| v.iter().all(|n| n.is_finite()))?;
    let (x, y) = gs.ctm.apply(x, y);
    Some([x, y])
}

/// The non-stroking alpha (`/ca`) of a named ExtGState, defaulting to opaque.
fn ext_alpha(doc: &Document, resources: &Dict, name: &[u8]) -> f64 {
    let alpha = resources
        .get(b"ExtGState")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(name).map(|o| doc.resolve(o)))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(b"ca").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()));
    alpha.filter(|a| a.is_finite()).map(|a| a.clamp(0.0, 1.0)).unwrap_or(1.0)
}

fn off_layers(doc: &Document) -> HashSet<ObjRef> {
    let mut off_set = HashSet::new();
    let Some(root) = doc.trailer().get(b"Root").map(|r| doc.resolve(r)) else { return off_set };
    let Some(catalog) = root.as_dict() else { return off_set };
    let Some(oc_props) = catalog.get(b"OCProperties").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()) else {
        return off_set;
    };
    let Some(d) = oc_props.get(b"D").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()) else {
        return off_set;
    };
    if let Some(off_arr) = d.get(b"OFF").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()) {
        for item in off_arr {
            if let Object::Ref(r) = item {
                off_set.insert(r);
            }
        }
    }
    off_set
}

fn is_layer_off(doc: &Document, resources: &Dict, off_layers: &HashSet<ObjRef>, prop: Option<&Object>) -> bool {
    let Some(prop) = prop else { return false };
    match prop {
        Object::Ref(r) => off_layers.contains(r),
        Object::Name(name) => {
            let Some(props) = resources.get(b"Properties").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()) else {
                return false;
            };
            let Some(entry) = props.get(name) else { return false };
            if let Object::Ref(r) = entry { off_layers.contains(r) } else { false }
        }
        _ => false,
    }
}

/// Every opaque filled rectangle drawn by a page's content (and its form XObjects), with paint
/// positions. A page's content streams are walked once, joined, with one graphics state, so
/// state set in an earlier stream is still in force in the later ones. Any content stream that
/// cannot be decoded is an audit failure, never an empty result.
pub fn filled_rects(doc: &Document, page: &Dict) -> Result<Vec<FilledRect>, AuditError> {
    let resources = page.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let contents: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut streams = Vec::with_capacity(contents.len());
    for c in &contents {
        if let Object::Stream(s) = &*doc.resolve(c) {
            streams.push(s.decoded_strict()?);
        }
    }
    let pieces = Pieces::join(&streams);
    let parsed = pieces.parse();
    let off = off_layers(doc);
    let mut w = Walker { doc, pieces: &pieces, out: Vec::new(), ops_seen: 0, off_layers: off, oc_hidden_depth: 0 };
    let mut gs = Gs::default();
    let mut stack = Vec::new();
    w.walk_ops(&parsed.ops, &mut gs, &mut stack, &resources, 0, None)?;
    Ok(w.out)
}
