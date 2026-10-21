//! Keep the subdivision tree until adjacent paragraph bands can share their columns.
//! Spanning leaves and proven table rows are barriers; whitespace alone is not a section.

use super::{Leaf, MAX_DEPTH};

type Span = [f32; 2];

pub(super) enum Layout {
    Leaf(Leaf),
    Columns(Band),
    Sections(Vec<Self>),
    Rows(Vec<Self>),
}

/// Ordered column children with one empty projection corridor between each pair.
pub(super) struct Band {
    parts: Vec<Layout>,
    gutters: Vec<Span>,
}

impl Layout {
    /// Flatten connected vertical cuts, independently of which gutter was widest first.
    pub(super) fn columns(left: Self, right: Self, gutter: Span) -> Self {
        let mut band = match left {
            Self::Columns(band) => band,
            other => Band { parts: vec![other], gutters: Vec::new() },
        };
        band.gutters.push(gutter);
        match right {
            Self::Columns(right) => {
                band.parts.extend(right.parts);
                band.gutters.extend(right.gutters);
            }
            other => band.parts.push(other),
        }
        Self::Columns(band)
    }

    /// Coalesce adjacent column bands across paragraph gaps, preserving heading barriers.
    pub(super) fn sections(top: Self, bottom: Self, depth: usize) -> Self {
        let mut sections = match top {
            Self::Sections(parts) => parts,
            other => vec![other],
        };
        let following = match bottom {
            Self::Sections(parts) => parts,
            other => vec![other],
        };
        for next in following {
            let Some(last) = sections.pop() else {
                sections.push(next);
                continue;
            };
            match (last, next) {
                (Self::Columns(a), Self::Columns(b)) if depth < MAX_DEPTH => {
                    if let Some(seams) = common_gutters(&a.gutters, &b.gutters) {
                        let (a_seams, b_seams): (Vec<_>, Vec<_>) = seams.iter().map(|&(a, b, _)| (a, b)).unzip();
                        let parts = a.groups(&a_seams).into_iter().zip(b.groups(&b_seams)).map(|(a, b)| Self::sections(a, b, depth + 1)).collect();
                        sections.push(Self::Columns(Band { parts, gutters: seams.into_iter().map(|(_, _, span)| span).collect() }));
                    } else {
                        sections.extend([Self::Columns(a), Self::Columns(b)]);
                    }
                }
                (last, next) => sections.extend([last, next]),
            }
        }
        Self::Sections(sections)
    }

    /// Iterative traversal retains each original leaf exactly once, including blank glyphs.
    pub(super) fn into_leaves(self) -> Vec<Leaf> {
        let mut pending = vec![self];
        let mut leaves = Vec::new();
        while let Some(node) = pending.pop() {
            match node {
                Self::Leaf(leaf) => leaves.push(leaf),
                Self::Columns(Band { parts, .. }) | Self::Sections(parts) | Self::Rows(parts) => pending.extend(parts.into_iter().rev()),
            }
        }
        leaves
    }
}

impl Band {
    /// Group columns between common gutters. Unmatched local gutters stay inside a group,
    /// allowing a heading over only some columns without interrupting the other columns.
    fn groups(self, shared: &[usize]) -> Vec<Layout> {
        let mut cuts = shared.iter().copied().peekable();
        let mut parts = self.parts.into_iter();
        let mut group = Band { parts: Vec::new(), gutters: Vec::new() };
        let mut groups = Vec::new();
        for (at, gutter) in self.gutters.into_iter().enumerate() {
            group.parts.extend(parts.next());
            if cuts.peek() == Some(&at) {
                cuts.next();
                groups.push(group.pack());
                group = Band { parts: Vec::new(), gutters: Vec::new() };
            } else {
                group.gutters.push(gutter);
            }
        }
        group.parts.extend(parts);
        groups.push(group.pack());
        groups
    }

    /// A group without an internal gutter is its only child, avoiding artificial barriers.
    fn pack(mut self) -> Layout {
        if self.parts.len() == 1
            && let Some(part) = self.parts.pop()
        {
            part
        } else {
            Layout::Columns(self)
        }
    }
}

/// Match overlapping empty corridors in linear time. A wide corridor overlapping several
/// narrower ones is ambiguous and must not connect otherwise unrelated column layouts.
fn common_gutters(a: &[Span], b: &[Span]) -> Option<Vec<(usize, usize, Span)>> {
    let (mut i, mut j) = (0usize, 0usize);
    let mut shared = Vec::new();
    while let Some((a_span, b_span)) = a.get(i).zip(b.get(j)) {
        if a_span[1] <= b_span[0] {
            i = i.saturating_add(1);
        } else if b_span[1] <= a_span[0] {
            j = j.saturating_add(1);
        } else {
            if a.get(i.saturating_add(1)).is_some_and(|s| s[0] < b_span[1]) || b.get(j.saturating_add(1)).is_some_and(|s| s[0] < a_span[1]) {
                return None;
            }
            shared.push((i, j, [a_span[0].max(b_span[0]), a_span[1].min(b_span[1])]));
            i = i.saturating_add(1);
            j = j.saturating_add(1);
        }
    }
    (!shared.is_empty()).then_some(shared)
}
