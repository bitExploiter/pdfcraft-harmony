//! Filled rectangular panel boundaries used by editable text grouping.

use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::{Dict, Document, ObjRef};

struct Container {
    inverse: Matrix,
    rect: [f64; 4],
}

#[derive(Default)]
pub(super) struct Containers {
    filled: Vec<Container>,
    pending: Option<Container>,
    complex: bool,
}

impl Containers {
    pub(super) fn collect(doc: &Document, ops: &[Op], resources: &Dict) -> Self {
        let mut collector = Collector { doc, containers: Self::default(), path: Vec::new(), budget: super::FormBudget::default() };
        collector.walk(ops, resources, Matrix::IDENTITY);
        collector.containers
    }

    fn observe(&mut self, op: &Op, ctm: Matrix) {
        match op.op.as_slice() {
            b"re" => {
                if self.pending.is_some() {
                    self.complex = true;
                }
                self.pending = op.nums::<4>().zip(ctm.invert()).and_then(|([x, y, width, height], inverse)| {
                    let rect = [x.min(x + width), y.min(y + height), x.max(x + width), y.max(y + height)];
                    (rect.iter().all(|v| v.is_finite()) && rect[2] > rect[0] && rect[3] > rect[1]).then_some(Container { inverse, rect })
                });
            }
            b"m" | b"l" | b"c" | b"v" | b"y" => self.complex = true,
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if !self.complex
                    && self.filled.len() < 4096
                    && let Some(rect) = self.pending.take()
                {
                    self.filled.push(rect);
                }
                self.pending = None;
                self.complex = false;
            }
            b"n" | b"S" | b"s" => {
                self.pending = None;
                self.complex = false;
            }
            _ => {}
        }
    }

    /// The panels that hold `rect`'s centre and are taller than it. A highlight or inline shading
    /// drawn to the height of its glyphs is not a panel.
    pub(super) fn membership(&self, rect: [f64; 4]) -> Vec<usize> {
        let (x, y) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
        self.filled
            .iter()
            .enumerate()
            .filter_map(|(index, container)| {
                let ys =
                    [(rect[0], rect[1]), (rect[2], rect[1]), (rect[0], rect[3]), (rect[2], rect[3])].map(|(x, y)| container.inverse.apply(x, y).1);
                let height = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max) - ys.iter().copied().fold(f64::INFINITY, f64::min);
                let (x, y) = container.inverse.apply(x, y);
                let [left, bottom, right, top] = container.rect;
                (x >= left && x <= right && y >= bottom && y <= top && top - bottom >= height * 1.15).then_some(index)
            })
            .collect()
    }
}

struct Collector<'a> {
    doc: &'a Document,
    containers: Containers,
    path: Vec<ObjRef>,
    budget: super::FormBudget,
}

impl Collector<'_> {
    fn walk(&mut self, ops: &[Op], resources: &Dict, initial: Matrix) {
        let mut local = Containers::default();
        let mut ctm = initial;
        let mut stack = Vec::new();
        for op in ops {
            local.observe(op, ctm);
            match op.op.as_slice() {
                b"q" => stack.push(ctm),
                b"Q" => {
                    if let Some(saved) = stack.pop() {
                        ctm = saved;
                    }
                }
                b"cm" => {
                    if let Some(matrix) = op.nums::<6>() {
                        ctm = Matrix(matrix).then(&ctm);
                    }
                }
                b"Do" => {
                    if let Some(name) = op.name(0)
                        && let Some(form) = super::form_call(self.doc, resources, name, ctm, &self.path, &mut self.budget)
                    {
                        if let Some(obj) = form.obj {
                            self.path.push(obj);
                        }
                        self.walk(&pdfcraft_content::parse(&form.data).ops, &form.resources, form.ctm);
                        if form.obj.is_some() {
                            self.path.pop();
                        }
                    }
                }
                _ => {}
            }
        }
        let remaining = 4096_usize.saturating_sub(self.containers.filled.len());
        self.containers.filled.extend(local.filled.into_iter().take(remaining));
    }
}
