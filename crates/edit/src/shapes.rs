//! Vector artwork a page draws (Export a PDF ▸ PowerPoint as pages, and the apps that open a
//! PDF as slides): filled and stroked paths with their colours, in drawing order and in page user
//! space, including what form XObjects draw.
//!
//! What a slide can hold is kept and the rest is approximated: device colour spaces (gray, RGB,
//! CMYK, and their `cs`/`sc` forms) give the colour, other spaces (patterns, ICC, separations)
//! read as the alternate a reader would most likely show (gray for one component, RGB for three,
//! CMYK for four) or as black; curves are flattened to straight segments; clipping, shadings,
//! transparency and dashes are left out. Reading is bounded: [`MAX_PAGE_SHAPES`] shapes a page,
//! [`MAX_SHAPE_POINTS`] points a shape, and forms as [`crate::reading_images`] reads them.

use pdfcraft_content::{Matrix, Op, parse};
use pdfcraft_cos::{Dict, Document, ObjRef};

use crate::EditError;

/// Most shapes read from one page; [`ReadingShapes::truncated`] says when a page had more.
pub const MAX_PAGE_SHAPES: usize = 4000;
/// Most points in one shape (all its subpaths); longer paths keep their first points.
pub const MAX_SHAPE_POINTS: usize = 2000;
/// Segments a Bézier curve is flattened into.
const CURVE_STEPS: usize = 8;
/// Graphics-state nesting kept (`q`); deeper saves are ignored with their restores.
const MAX_STACK: usize = 256;

/// A painted path.
#[derive(Clone, Debug, PartialEq)]
pub struct PageShape {
    /// Subpaths of points in page user space (after the CTM).
    pub paths: Vec<Vec<[f64; 2]>>,
    /// Filled shapes are closed; a stroke is closed when its path said so (`h`, `s`, `re`).
    pub closed: bool,
    /// Fill colour as sRGB in 0..=1, `None` when not filled.
    pub fill: Option<[f64; 3]>,
    /// Stroke colour, `None` when not stroked.
    pub stroke: Option<[f64; 3]>,
    /// Stroke width in user space units (the CTM's average scale applied).
    pub line_width: f64,
}

/// A page's shapes, and whether reading left some out ([`MAX_PAGE_SHAPES`], or forms past the
/// limits).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadingShapes {
    pub shapes: Vec<PageShape>,
    pub truncated: bool,
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    fill: [f64; 3],
    stroke: [f64; 3],
    /// The colour space set by `cs`/`CS` is a pattern: `sc`/`scn` don't give a colour.
    fill_pattern: bool,
    stroke_pattern: bool,
    line_width: f64,
}

struct Walk<'a> {
    doc: &'a Document,
    gs: Gs,
    stack: Vec<Gs>,
    /// The path being built, in user space.
    subpaths: Vec<Vec<[f64; 2]>>,
    points: usize,
    closed: bool,
    out: ReadingShapes,
    forms: Vec<ObjRef>,
    budget: crate::text::FormBudget,
}

/// A colour from operands: one is gray, three RGB, four CMYK (naive conversion).
fn colour(op: &Op) -> Option<[f64; 3]> {
    let n: Vec<f64> = op.operands.iter().filter_map(|o| o.as_f64()).filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)).collect();
    match n.as_slice() {
        [g] => Some([*g; 3]),
        [r, g, b] => Some([*r, *g, *b]),
        [c, m, y, k] => Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]),
        _ => None,
    }
}

impl Walk<'_> {
    fn point(&self, x: f64, y: f64) -> [f64; 2] {
        let (x, y) = self.gs.ctm.apply(x, y);
        [x, y]
    }

    fn add(&mut self, p: [f64; 2], new_subpath: bool) {
        if !(p[0].is_finite() && p[1].is_finite()) || self.points >= MAX_SHAPE_POINTS {
            return;
        }
        if new_subpath || self.subpaths.is_empty() {
            self.subpaths.push(Vec::new());
        }
        if let Some(sp) = self.subpaths.last_mut() {
            sp.push(p);
            self.points += 1;
        }
    }

    fn current(&self) -> Option<[f64; 2]> {
        self.subpaths.last().and_then(|s| s.last()).copied()
    }

    fn curve(&mut self, c1: [f64; 2], c2: [f64; 2], end: [f64; 2]) {
        let Some(p0) = self.current() else { return self.add(end, true) };
        for i in 1..=CURVE_STEPS {
            let t = i as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            let at = |k: usize| u * u * u * p0[k] + 3.0 * u * u * t * c1[k] + 3.0 * u * t * t * c2[k] + t * t * t * end[k];
            self.add([at(0), at(1)], false);
        }
    }

    fn paint(&mut self, fill: bool, stroke: bool, close: bool) {
        let paths = std::mem::take(&mut self.subpaths);
        let closed = std::mem::take(&mut self.closed) || close || fill;
        self.points = 0;
        let paths: Vec<_> = paths.into_iter().filter(|p| p.len() >= 2).collect();
        if paths.is_empty() || !(fill || stroke) {
            return;
        }
        if self.out.shapes.len() >= MAX_PAGE_SHAPES {
            self.out.truncated = true;
            return;
        }
        let [a, b, c, d, ..] = self.gs.ctm.0;
        let scale = ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0;
        let width = self.gs.line_width * scale;
        self.out.shapes.push(PageShape {
            paths,
            closed,
            fill: fill.then_some(self.gs.fill),
            stroke: stroke.then_some(self.gs.stroke),
            line_width: if width.is_finite() && width > 0.0 { width } else { 1.0 },
        });
    }

    fn run(&mut self, ops: &[Op], resources: &Dict) {
        for op in ops {
            let nums = |n: usize| -> Option<Vec<f64>> {
                let v: Vec<f64> = op.operands.iter().filter_map(|o| o.as_f64()).collect();
                (v.len() >= n && v.iter().all(|x| x.is_finite())).then_some(v)
            };
            match op.op.as_slice() {
                b"q" => {
                    if self.stack.len() < MAX_STACK {
                        self.stack.push(self.gs.clone());
                    }
                }
                b"Q" => {
                    if let Some(g) = self.stack.pop() {
                        self.gs = g;
                    }
                }
                b"cm" => {
                    if let Some(m) = op.nums::<6>().filter(|m| m.iter().all(|v| v.is_finite())) {
                        self.gs.ctm = Matrix(m).then(&self.gs.ctm);
                    }
                }
                b"w" => {
                    if let Some(v) = nums(1).and_then(|v| v.first().copied()) {
                        self.gs.line_width = v.abs();
                    }
                }
                b"g" | b"rg" | b"k" => {
                    if let Some(c) = colour(op) {
                        self.gs.fill = c;
                        self.gs.fill_pattern = false;
                    }
                }
                b"G" | b"RG" | b"K" => {
                    if let Some(c) = colour(op) {
                        self.gs.stroke = c;
                        self.gs.stroke_pattern = false;
                    }
                }
                b"cs" => self.gs.fill_pattern = op.name(0) == Some(b"Pattern"),
                b"CS" => self.gs.stroke_pattern = op.name(0) == Some(b"Pattern"),
                b"sc" | b"scn" => {
                    if !self.gs.fill_pattern
                        && let Some(c) = colour(op)
                    {
                        self.gs.fill = c;
                    }
                }
                b"SC" | b"SCN" => {
                    if !self.gs.stroke_pattern
                        && let Some(c) = colour(op)
                    {
                        self.gs.stroke = c;
                    }
                }
                b"m" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        let p = self.point(x, y);
                        self.add(p, true);
                    }
                }
                b"l" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        let p = self.point(x, y);
                        self.add(p, false);
                    }
                }
                b"c" => {
                    if let Some([x1, y1, x2, y2, x3, y3]) = op.nums::<6>() {
                        let (c1, c2, e) = (self.point(x1, y1), self.point(x2, y2), self.point(x3, y3));
                        self.curve(c1, c2, e);
                    }
                }
                b"v" => {
                    if let Some([x2, y2, x3, y3]) = op.nums::<4>() {
                        let c1 = self.current().unwrap_or_else(|| self.point(x2, y2));
                        let (c2, e) = (self.point(x2, y2), self.point(x3, y3));
                        self.curve(c1, c2, e);
                    }
                }
                b"y" => {
                    if let Some([x1, y1, x3, y3]) = op.nums::<4>() {
                        let (c1, e) = (self.point(x1, y1), self.point(x3, y3));
                        self.curve(c1, e, e);
                    }
                }
                b"h" => self.closed = true,
                b"re" => {
                    if let Some([x, y, w, h]) = op.nums::<4>() {
                        let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
                        for (i, (px, py)) in corners.into_iter().enumerate() {
                            let p = self.point(px, py);
                            self.add(p, i == 0);
                        }
                        self.closed = true;
                    }
                }
                b"f" | b"F" | b"f*" => self.paint(true, false, true),
                b"S" => self.paint(false, true, false),
                b"s" => self.paint(false, true, true),
                b"B" | b"B*" => self.paint(true, true, false),
                b"b" | b"b*" => self.paint(true, true, true),
                // End the path without painting; a clip (`W n`) shows nothing by itself.
                b"n" => self.paint(false, false, false),
                b"Do" => {
                    let Some(name) = op.name(0) else { continue };
                    if let Some(form) = crate::text::form_call(self.doc, resources, name, self.gs.ctm, &self.forms, &mut self.budget) {
                        if let Some(r) = form.obj {
                            self.forms.push(r);
                        }
                        let saved = (self.gs.clone(), std::mem::take(&mut self.stack), std::mem::take(&mut self.subpaths), self.points, self.closed);
                        self.gs.ctm = form.ctm;
                        self.points = 0;
                        self.closed = false;
                        self.run(&parse(&form.data).ops, &form.resources);
                        (self.gs, self.stack, self.subpaths, self.points, self.closed) = saved;
                        if form.obj.is_some() {
                            self.forms.pop();
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// The shapes page `page` (0-based) paints, in drawing order; see the module documentation.
pub fn reading_shapes(doc: &Document, page: usize) -> Result<ReadingShapes, EditError> {
    let p = crate::images::page_of(doc, page)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let pieces = crate::images::joined(&crate::images::streams(doc, &p.dict));
    let gs = Gs { ctm: Matrix::IDENTITY, fill: [0.0; 3], stroke: [0.0; 3], fill_pattern: false, stroke_pattern: false, line_width: 1.0 };
    let mut walk = Walk {
        doc,
        gs,
        stack: Vec::new(),
        subpaths: Vec::new(),
        points: 0,
        closed: false,
        out: ReadingShapes::default(),
        forms: Vec::new(),
        budget: crate::text::FormBudget::default(),
    };
    walk.run(&pieces.parse().ops, &res);
    walk.out.truncated |= walk.budget.truncated;
    Ok(walk.out)
}

#[cfg(test)]
#[path = "shapes_tests.rs"]
mod tests;
