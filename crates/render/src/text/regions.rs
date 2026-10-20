//! Whitespace subdivision before line reconstruction. A column boundary must survive both
//! content-order run construction and geometric run merging. Neither PDF paint order nor a
//! single pair of neighbouring letters can establish that boundary.
//!
//! Recursive XY subdivision uses projections of nonblank glyph boxes. Vertical cuts need
//! multiline text on both sides and a gap wider than ordinary word spacing; horizontal cuts
//! expose sections separated by spanning headings, footers and changes of column layout.
//! Supported vertical gutters take precedence over paragraph whitespace. Spanning content
//! obstructs those gutters, exposing horizontal cuts before its local column layouts.
//! Retaining that tree allows adjacent paragraph bands to share compatible columns. This
//! is a geometric fallback, not semantic table detection or tagged-PDF structure-tree order.

use super::TextGlyph;

mod flow;
use flow::Layout;

const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy)]
struct Gap {
    at: f32,
    size: f32,
    before: usize,
    /// Rounding allowance for transformed coordinates and their subtraction. In particular,
    /// an ordinary space must not become a gutter just above the measured space width.
    error: f32,
}

enum Task {
    Visit(Region, usize, bool),
    Join(Cut, bool),
}

#[derive(Clone, Copy)]
struct Cut {
    axis: usize,
    at: f32,
    span: [f32; 2],
    rows: bool,
}

/// A reading region; a proven single-row leaf can join distant labels and values.
pub(super) struct Leaf {
    pub(super) ids: Vec<usize>,
    pub(super) join_row: bool,
}

struct Region {
    ids: Vec<usize>,
    // Ink indices sorted once at the root and filtered at cuts. Repeated projection sorts
    // would turn a balanced tree into O(n log² n) work on ordinary multi-line pages.
    x: Vec<usize>,
    y: Vec<usize>,
}

struct Regions {
    layout: Layout,
    structured: bool,
}

/// Read a projection edge without allowing an invalid index to panic during sorting.
fn edge(glyphs: &[TextGlyph], id: usize, axis: usize) -> f32 {
    glyphs.get(id).and_then(|g| g.rect.get(axis)).copied().unwrap_or(0.0)
}

/// Original indices, partitioned in reading order. Within each leaf, retain paint order for
/// redraw detection, combining marks and overlapping runs in the existing line reconstructor.
pub(super) fn partition(glyphs: &[TextGlyph], spaces_from_shows: &[f32]) -> Vec<Leaf> {
    let all: Vec<_> = (0..glyphs.len()).collect();
    if glyphs.iter().any(|g| !g.rect.iter().all(|v| v.is_finite()) || !(g.rect[2] - g.rect[0]).is_finite() || !(g.rect[3] - g.rect[1]).is_finite()) {
        return vec![Leaf { ids: all, join_row: false }];
    }
    let mut x: Vec<_> = all.iter().copied().filter(|&i| glyphs.get(i).is_some_and(|g| !g.text.trim().is_empty())).collect();
    x.sort_by(|&a, &b| edge(glyphs, a, 0).total_cmp(&edge(glyphs, b, 0)));
    let mut y = x.clone();
    y.sort_by(|&a, &b| edge(glyphs, a, 1).total_cmp(&edge(glyphs, b, 1)));
    let mut pending = vec![Task::Visit(Region { ids: all, x, y }, 0, false)];
    let mut done: Vec<Regions> = Vec::new();
    while let Some(task) = pending.pop() {
        let (region, depth, row_scope) = match task {
            Task::Visit(region, depth, row_scope) => (region, depth, row_scope),
            Task::Join(cut, row_scope) => {
                let (Some(right), Some(mut left)) = (done.pop(), done.pop()) else {
                    // Internal stack invariant; preserve all glyphs if it is ever broken.
                    return vec![Leaf { ids: (0..glyphs.len()).collect(), join_row: false }];
                };
                left.structured |= right.structured || cut.axis == 0 || cut.rows;
                if !left.structured {
                    // Horizontal whitespace is only a way to expose columns, not evidence
                    // that slightly separated glyphs should become separate logical lines.
                    // Without a supported column cut, leave line grouping to the existing
                    // reconstructor (important for map labels and decorative typography).
                    let mut a = left.layout.into_leaves().into_iter().flat_map(|r| r.ids).peekable();
                    let mut b = right.layout.into_leaves().into_iter().flat_map(|r| r.ids).peekable();
                    let mut ids = Vec::new();
                    while let (Some(x), Some(y)) = (a.peek(), b.peek()) {
                        ids.extend(if x <= y { a.next() } else { b.next() });
                    }
                    ids.extend(a);
                    ids.extend(b);
                    let join_row = row_scope && one_line(glyphs, &ids);
                    left.layout = Layout::Leaf(Leaf { ids, join_row });
                } else {
                    left.layout = if cut.axis == 0 {
                        Layout::columns(left.layout, right.layout, cut.span)
                    } else if cut.rows {
                        Layout::Rows(vec![left.layout, right.layout])
                    } else {
                        Layout::sections(left.layout, right.layout, 0)
                    };
                }
                left.structured |= row_scope;
                done.push(left);
                continue;
            }
        };
        let cut = (depth < MAX_DEPTH).then(|| subdivision(glyphs, &region, spaces_from_shows)).flatten();
        let Some(cut) = cut else {
            let join_row = row_scope && one_line(glyphs, &region.y);
            done.push(Regions { layout: Layout::Leaf(Leaf { ids: region.ids, join_row }), structured: row_scope });
            continue;
        };
        let Cut { axis, at, .. } = cut;
        let before = |&i: &usize| {
            glyphs.get(i).is_some_and(|g| {
                let r = g.rect;
                // Half-sums cannot overflow for finite coordinates.
                r[axis] * 0.5 + r[axis + 2] * 0.5 < at
            })
        };
        let (first, second): (Vec<_>, Vec<_>) = region.ids.into_iter().partition(before);
        if first.is_empty() || second.is_empty() {
            done.push(Regions { layout: Layout::Leaf(Leaf { ids: first.into_iter().chain(second).collect(), join_row: false }), structured: false });
            continue;
        }
        let (first_x, second_x) = region.x.into_iter().partition(before);
        let (first_y, second_y) = region.y.into_iter().partition(before);
        // Stack order: top before bottom, left before right. Axis filtering is linear per
        // level; visual-band sorts for prose/tracking are bounded by MAX_DEPTH * n log n.
        let child_rows = axis == 1 && cut.rows;
        pending.push(Task::Join(cut, row_scope));
        pending.push(Task::Visit(Region { ids: second, x: second_x, y: second_y }, depth + 1, child_rows));
        pending.push(Task::Visit(Region { ids: first, x: first_x, y: first_y }, depth + 1, child_rows));
    }
    done.pop().map_or_else(Vec::new, |r| r.layout.into_leaves())
}

/// Select the upper median in place, or return no estimate for an empty sample.
fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mid = values.len() / 2;
    Some(*values.select_nth_unstable_by(mid, f32::total_cmp).1)
}

/// A row scope must not widen merging for a leaf that still contains several baselines.
fn one_line(glyphs: &[TextGlyph], ids: &[usize]) -> bool {
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    let mut height = f32::INFINITY;
    for &id in ids {
        let Some(g) = glyphs.get(id) else { continue };
        top = top.min(g.rect[1]);
        bottom = bottom.max(g.rect[3]);
        height = height.min((g.rect[3] - g.rect[1]).max(0.0));
    }
    height.is_finite() && height > 0.0 && bottom - top <= height * 1.5
}

/// Explicit spaces carry nominal widths, but Tw also moves the next glyph. Measure that
/// advance within the same source line rather than treating the extra spacing as a gutter.
fn space_advances(glyphs: &[TextGlyph], ids: &[usize], height: f32, spaces_from_shows: &[f32]) -> Vec<f32> {
    let mut widths = Vec::new();
    for &id in ids {
        let Some(g) = glyphs.get(id) else { continue };
        if g.text.is_empty() || !g.text.chars().all(char::is_whitespace) {
            continue;
        }
        let nominal = g.rect[2] - g.rect[0];
        if nominal <= 0.0 || nominal > height {
            continue;
        }
        // Only a measured advance within one source string is trustworthy. The next
        // paint-order glyph can instead start an adjacent column after a trailing space.
        let advance = spaces_from_shows.get(id).copied().filter(|s| s.is_finite() && *s > 0.0).map_or(nominal, |s| s * (g.rect[3] - g.rect[1]));
        widths.push(nominal.max(advance));
    }
    widths
}

/// A single local word space can cover a body gutter even when its font is a minority on
/// the page. Repeated alignment spaces do not provide this evidence: one advance must span
/// the whole corridor. Sweep sorted spaces and candidates rather than rescanning the page.
fn word_bridges(glyphs: &[TextGlyph], ids: &[usize], spaces_from_shows: &[f32], candidates: &[Gap]) -> Vec<bool> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut spans: Vec<_> = ids
        .iter()
        .filter_map(|&id| {
            let g = glyphs.get(id)?;
            let mut chars = g.text.chars();
            let c = chars.next()?;
            if !c.is_whitespace() || c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') || chars.next().is_some() {
                return None;
            }
            let nominal = g.rect[2] - g.rect[0];
            let advance = spaces_from_shows.get(id).copied().filter(|s| s.is_finite() && *s > 0.0).map_or(nominal, |s| s * (g.rect[3] - g.rect[1]));
            let end = g.rect[0] + nominal.max(advance);
            end.is_finite().then_some([g.rect[0], end])
        })
        .collect();
    spans.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut spans = spans.into_iter().peekable();
    let mut end = f32::NEG_INFINITY;
    candidates
        .iter()
        .map(|gap| {
            let left = gap.at - gap.size * 0.5;
            while spans.peek().is_some_and(|span| span[0] <= left + gap.error) {
                if let Some(span) = spans.next() {
                    end = end.max(span[1]);
                }
            }
            end >= gap.at + gap.size * 0.5 - gap.error
        })
        .collect()
}

/// Recognize conventional list labels and colon-terminated field labels without limiting
/// their width or digit count. Such labels do not constitute full prose lines.
#[derive(Default)]
struct Label {
    letters: bool,
    number: bool,
    separated: bool,
    right: Option<f32>,
    punctuation: bool,
    other: bool,
    last: Option<char>,
}

impl Label {
    /// Inspect text without allocating or setting a document-specific label-length cutoff.
    fn add(&mut self, g: &TextGlyph) {
        let h = g.rect[3] - g.rect[1];
        self.separated |= self.right.is_some_and(|right| g.rect[0] - right > h * super::WORD_GAP);
        self.right = Some(g.rect[2]);
        for c in g.text.chars() {
            if c.is_whitespace() {
                self.separated |= self.last.is_some();
                continue;
            }
            self.last = Some(c);
            if c.is_numeric() {
                self.number = true;
            } else if c.is_alphabetic() {
                self.letters = true;
            } else if matches!(c, '.' | ')' | '(' | '[' | ']' | '{' | '}' | '-' | '–' | '—' | '•' | '‣' | '◦' | '▪' | '▫' | '●' | '○' | '■' | '□')
            {
                self.punctuation = true;
            } else {
                self.other = true;
            }
        }
    }

    /// Field-label syntax and numeric/alphabetic/Roman list markers are row relationships.
    fn is_label(&self) -> bool {
        self.last == Some(':')
            || (!self.other
                && !self.separated
                && ((!self.letters && (self.number || self.punctuation)) || (self.punctuation && matches!(self.last, Some('.' | ')' | ']' | '}')))))
    }
}

struct Support {
    prose: Vec<bool>,
    labels: bool,
}

/// Require adjacent, substantially filled lines on each side of a candidate. Labels and
/// isolated table cells do not establish prose columns merely by sharing whitespace.
/// Each glyph contributes to one fragment, and fragments touch only neighbouring cuts.
fn prose_support(glyphs: &[TextGlyph], region: &Region, candidates: &[Gap], height: f32, word_space: f32) -> Support {
    if candidates.is_empty() {
        return Support { prose: Vec::new(), labels: false };
    }
    let mut supported = vec![[false; 2]; candidates.len()];
    let mut label_rows = vec![0usize; candidates.len()];
    let mut previous: Vec<[Option<[f32; 2]>; 2]> = vec![[None; 2]; candidates.len()];
    let mut row = Vec::new();
    let mut top = f32::NEG_INFINITY;
    let mut visit = |row: &mut Vec<usize>| {
        row.sort_by(|&a, &b| edge(glyphs, a, 0).total_cmp(&edge(glyphs, b, 0)));
        let mut fragments: Vec<(usize, [f32; 4], Label)> = Vec::new();
        for &id in row.iter() {
            let Some(g) = glyphs.get(id) else { continue };
            let part = candidates.partition_point(|gap| gap.at < g.rect[0]);
            if let Some((_, bounds, label)) = fragments.last_mut().filter(|(last, _, _)| *last == part) {
                super::union(bounds, &g.rect);
                label.add(g);
            } else {
                let mut label = Label::default();
                label.add(g);
                fragments.push((part, g.rect, label));
            }
        }
        for pair in fragments.windows(2) {
            let (Some(left), Some(right)) = (pair.first(), pair.get(1)) else { continue };
            if left.0.checked_add(1) != Some(right.0) || !left.2.is_label() {
                continue;
            }
            let Some(gap) = candidates.get(left.0) else { continue };
            if gap.at - gap.size * 0.5 - left.1[2] <= height
                && right.1[0] - gap.at - gap.size * 0.5 <= word_space + gap.error
                && let Some(count) = label_rows.get_mut(left.0)
            {
                *count = count.saturating_add(1);
            }
        }
        for (part, rect, label) in fragments {
            for (candidate, side) in [(Some(part), 0), (part.checked_sub(1), 1)] {
                let Some(candidate) = candidate else { continue };
                let Some(gap) = candidates.get(candidate) else { continue };
                let reaches = if side == 0 { gap.at - gap.size * 0.5 - rect[2] } else { rect[0] - gap.at - gap.size * 0.5 };
                if label.is_label() || rect[2] - rect[0] <= gap.size || reaches > word_space + gap.error {
                    continue;
                }
                let Some(last) = previous.get_mut(candidate).and_then(|s| s.get_mut(side)) else { continue };
                if let Some(prev) = *last {
                    let leading = rect[1] - prev[1];
                    if leading >= -height * 0.5
                        && leading < height + gap.error
                        && let Some(found) = supported.get_mut(candidate).and_then(|s| s.get_mut(side))
                    {
                        *found = true;
                    }
                }
                *last = Some([rect[1], rect[3]]);
            }
        }
        row.clear();
    };
    for &id in &region.y {
        let y = edge(glyphs, id, 1);
        if y - top > height * 0.5 {
            visit(&mut row);
            top = y;
        }
        row.push(id);
    }
    visit(&mut row);
    Support { prose: supported.into_iter().map(|s| s.into_iter().all(|v| v)).collect(), labels: label_rows.into_iter().any(|count| count >= 2) }
}

/// Gaps in an already sorted projection. Overlapping glyphs
/// form one occupied interval, so cuts never slice a ligature, overprint or shifted script.
fn gaps(glyphs: &[TextGlyph], sorted: &[usize], axis: usize) -> Vec<Gap> {
    let mut end = f32::NEG_INFINITY;
    let mut gaps = Vec::new();
    for (pos, &id) in sorted.iter().enumerate() {
        let Some(glyph) = glyphs.get(id) else { continue };
        let r = glyph.rect;
        if r[axis] > end && end.is_finite() {
            gaps.push(Gap {
                at: end * 0.5 + r[axis] * 0.5,
                size: r[axis] - end,
                before: pos,
                error: end.abs().max(r[axis].abs()) * (8.0 * f32::EPSILON),
            });
        }
        end = end.max(r[axis + 2]);
    }
    gaps
}

/// Estimate ordinary letter spacing from visual bands, independently of paint order. A
/// repeated gap in widely tracked lettering is typography, not a column gutter. Use the
/// same lower-third statistic as word reconstruction, so occasional word/column gaps do
/// not set the estimate. The caller only needs this when a possible gutter exists.
fn tracking(glyphs: &[TextGlyph], sorted_y: &[usize], h: f32) -> f32 {
    let mut row = Vec::new();
    let mut samples = Vec::new();
    let mut top = f32::NEG_INFINITY;
    let mut sample = |row: &mut Vec<usize>| {
        row.sort_by(|&a, &b| edge(glyphs, a, 0).total_cmp(&edge(glyphs, b, 0)));
        samples.extend(row.windows(2).filter_map(|pair| {
            let a = glyphs.get(*pair.first()?)?;
            let b = glyphs.get(*pair.get(1)?)?;
            Some((b.rect[0] - a.rect[2]).max(0.0))
        }));
        row.clear();
    };
    for &id in sorted_y {
        let Some(glyph) = glyphs.get(id) else { continue };
        let r = glyph.rect;
        if r[1] - top > h * 0.5 {
            sample(&mut row);
            top = r[1];
        }
        row.push(id);
    }
    sample(&mut row);
    if samples.is_empty() {
        return 0.0;
    }
    let k = samples.len() / 3;
    *samples.select_nth_unstable_by(k, f32::total_cmp).1
}

/// Without Unicode spaces or usable show provenance, repeated non-letter gaps in visual
/// rows provide independent word-spacing evidence. One gap alone may be the column gutter;
/// only rows containing several such gaps contribute. Uniform advances bound the ambiguous
/// estimate by a character cell; proportional advances use an em. Repeating gaps between
/// complete monospaced lines are not word spaces. Keep weak candidates for row detection.
fn fallback_word_space(glyphs: &[TextGlyph], sorted_y: &[usize], h: f32, letter_gap: f32) -> Option<f32> {
    let mut row = Vec::new();
    let mut samples = Vec::new();
    let mut top = f32::NEG_INFINITY;
    let mut sample = |row: &mut Vec<usize>| {
        row.sort_by(|&a, &b| edge(glyphs, a, 0).total_cmp(&edge(glyphs, b, 0)));
        let gaps: Vec<_> = row
            .windows(2)
            .filter_map(|pair| {
                let a = glyphs.get(*pair.first()?)?;
                let b = glyphs.get(*pair.get(1)?)?;
                // These scripts place characters continuously rather than using word
                // spaces; their repeated column gutters cannot estimate inter-word gaps.
                if a.text.chars().any(super::is_cjk) || b.text.chars().any(super::is_cjk) {
                    return None;
                }
                let gap = b.rect[0] - a.rect[2];
                (gap.is_finite() && gap > letter_gap + h * super::WORD_GAP).then_some(gap)
            })
            .collect();
        if gaps.len() >= 2 {
            samples.extend(gaps);
        }
        row.clear();
    };
    for &id in sorted_y {
        let y = edge(glyphs, id, 1);
        if y - top > h * 0.5 {
            sample(&mut row);
            top = y;
        }
        row.push(id);
    }
    sample(&mut row);
    let (mut smallest, mut widest, mut error) = (f32::INFINITY, 0.0_f32, 0.0_f32);
    for &id in sorted_y {
        let Some(g) = glyphs.get(id) else { continue };
        if g.text.chars().take(2).count() != 1 || g.text.chars().any(super::is_cjk) {
            continue;
        }
        let height = g.rect[3] - g.rect[1];
        if height <= 0.0 {
            continue;
        }
        let advance = (g.rect[2] - g.rect[0]) / height;
        if !advance.is_finite() || advance <= 0.0 || advance > 1.0 {
            continue;
        }
        smallest = smallest.min(advance);
        widest = widest.max(advance);
        // Coordinates and both subtractions contribute rounding error. Normalize each
        // advance to its own em so changes of point size do not look proportional.
        let coordinate = g.rect.iter().map(|v| v.abs()).fold(0.0_f32, f32::max);
        error = error.max(coordinate * (16.0 * f32::EPSILON) / height);
    }
    let cap = letter_gap + if widest - smallest > error { h } else { widest * h };
    median(&mut samples).map(|space| space.min(cap)).filter(|space| *space > 0.0)
}

/// Prefer a supported continuous column gutter; use horizontal whitespace only when no
/// such gutter exists, so aligned paragraph breaks cannot interleave neighbouring columns.
fn subdivision(glyphs: &[TextGlyph], region: &Region, spaces_from_shows: &[f32]) -> Option<Cut> {
    let ink = &region.x;
    if ink.len() < 2 {
        return None;
    }
    let mut heights: Vec<_> = ink.iter().filter_map(|&i| glyphs.get(i).map(|g| g.rect[3] - g.rect[1])).filter(|&h| h > 0.0).collect();
    let min_height = heights.iter().copied().fold(f32::INFINITY, f32::min);
    let top = ink.iter().filter_map(|&i| glyphs.get(i)).map(|g| g.rect[1]).fold(f32::INFINITY, f32::min);
    let bottom = ink.iter().filter_map(|&i| glyphs.get(i)).map(|g| g.rect[3]).fold(f32::NEG_INFINITY, f32::max);
    if bottom - top <= min_height * 1.5 {
        // No descendant can meet the multiline support requirement either. Most leaves are
        // single lines; avoid sorting their letters onto both axes just to rediscover that.
        return None;
    }
    let h = median(&mut heights)?;
    let mut spaces = space_advances(glyphs, &region.ids, h, spaces_from_shows);
    spaces.extend(ink.iter().filter_map(|&id| spaces_from_shows.get(id)).copied().filter(|s| s.is_finite() && *s > 0.0).map(|s| s * h));
    // Where a producer positions words without space glyphs, an ordinary word space is
    // estimated as a quarter em. A half em also protects ordinary letter tracking. All
    // other evidence (persistence, multiline support, section gaps) is measured on the page.
    let word_space = median(&mut spaces).unwrap_or(h * 0.25).max(h * super::WORD_GAP);
    let ygaps = gaps(glyphs, &region.y, 1);
    let balance = |g: &Gap| g.before.min(ink.len().saturating_sub(g.before));
    let horizontal = ygaps.iter().max_by(|a, b| a.size.total_cmp(&b.size).then_with(|| balance(a).cmp(&balance(b))));

    let xgaps = gaps(glyphs, ink, 0);
    let letter_gap = if xgaps.iter().any(|g| g.size > word_space.max(h * 0.5)) { tracking(glyphs, &region.y, h) } else { 0.0 };
    let gutter_floor = (letter_gap + word_space).max(h * 0.5);
    let sorted = ink;
    // Prefix/suffix bounds make testing every candidate linear, rather than rescanning the
    // page for every interletter gap (important for long, widely tracked lines).
    let mut prefix = Vec::with_capacity(sorted.len());
    let mut bounds = [f32::INFINITY, f32::NEG_INFINITY];
    for &id in sorted {
        let Some(glyph) = glyphs.get(id) else { continue };
        let r = glyph.rect;
        bounds = [bounds[0].min(r[1]), bounds[1].max(r[3])];
        prefix.push(bounds);
    }
    let mut suffix = vec![[f32::INFINITY, f32::NEG_INFINITY]; sorted.len()];
    bounds = [f32::INFINITY, f32::NEG_INFINITY];
    for (pos, &id) in sorted.iter().enumerate().rev() {
        let Some(glyph) = glyphs.get(id) else { continue };
        let r = glyph.rect;
        bounds = [bounds[0].min(r[1]), bounds[1].max(r[3])];
        if let Some(slot) = suffix.get_mut(pos) {
            *slot = bounds;
        }
    }
    let candidates: Vec<_> = xgaps
        .iter()
        .filter(|g| {
            let Some((&left, &right)) = g.before.checked_sub(1).and_then(|i| prefix.get(i)).zip(suffix.get(g.before)) else { return false };
            g.size > gutter_floor + g.error
                && left[1] - left[0] > h * 1.5
                && right[1] - right[0] > h * 1.5
                && left[1].min(right[1]) - left[0].max(right[0]) > h * 0.5
        })
        .copied()
        .collect();
    let independent_space = (spaces.is_empty() && !candidates.is_empty()).then(|| fallback_word_space(glyphs, &region.y, h, letter_gap)).flatten();
    let supported = prose_support(glyphs, region, &candidates, h, word_space.max(independent_space.unwrap_or(0.0)));
    let has_prose = supported.prose.iter().any(|&prose| prose);
    let bridges = word_bridges(glyphs, &region.ids, spaces_from_shows, &candidates);
    let vertical = candidates
        .iter()
        .zip(supported.prose)
        .zip(bridges)
        .filter(|((gap, supported), bridge)| *supported && !bridge && independent_space.is_none_or(|space| gap.size > space + gap.error))
        .map(|((gap, _), _)| gap)
        .max_by(|a, b| a.size.total_cmp(&b.size).then_with(|| balance(a).cmp(&balance(b))));
    match (vertical, horizontal) {
        // A persistent gutter establishes a complete column even across large paragraph
        // gaps. A heading spanning it obstructs the projection and permits a horizontal
        // cut instead; the size of empty paragraph whitespace is not section evidence.
        (Some(x), _) => Some(Cut { axis: 0, at: x.at, span: [x.at - x.size * 0.5, x.at + x.size * 0.5], rows: false }),
        (_, Some(y)) => Some(Cut {
            axis: 1,
            at: y.at,
            span: [y.at - y.size * 0.5, y.at + y.size * 0.5],
            rows: !has_prose && !candidates.is_empty() && (supported.labels || y.size >= h - y.error),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compare original ownership independently of the leaf's row reconstruction policy.
    fn parts(glyphs: &[TextGlyph]) -> Vec<Vec<usize>> {
        partition(glyphs, &[]).into_iter().map(|leaf| leaf.ids).collect()
    }

    /// Make a fixed-size glyph so projection tests have exact, independent geometry.
    fn glyph(text: &str, x: f32, y: f32) -> TextGlyph {
        TextGlyph { text: text.into(), rect: [x, y, x + 6.0, y + 10.0], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] }
    }

    /// Horizontal whitespace without columns must preserve the legacy line grouping input.
    #[test]
    fn horizontal_gaps_alone_do_not_replace_line_reconstruction() {
        let gs: Vec<_> = (0..20).flat_map(|row| (0..30).map(move |col| glyph("a", col as f32 * 6.0, row as f32 * 12.0))).collect();
        assert_eq!(parts(&gs), vec![(0..gs.len()).collect::<Vec<_>>()]);
    }

    /// Partitioning preserves every index exactly once, including blank and invalid geometry.
    #[test]
    fn every_original_index_survives_subdivision_including_blank_glyphs() {
        let mut gs: Vec<_> = (0..4)
            .flat_map(|col| {
                (0..80).flat_map(move |row| (0..6).map(move |letter| glyph("a", col as f32 * 44.0 + letter as f32 * 6.0, row as f32 * 12.0)))
            })
            .collect();
        assert_eq!(parts(&gs).len(), 4, "ownership check must exercise real subdivision");
        gs.extend([glyph(" ", 25.0, 5.0), glyph("", 75.0, 15.0)]);
        let mut ids: Vec<_> = parts(&gs).into_iter().flatten().collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..gs.len()).collect::<Vec<_>>());
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            gs.push(glyph("a", value, value));
            assert_eq!(parts(&gs), vec![(0..gs.len()).collect::<Vec<_>>()]);
        }
        assert!(parts(&[]).into_iter().flatten().next().is_none());
    }
}
