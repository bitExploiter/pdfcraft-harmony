//! Whitespace subdivision before line reconstruction. A column boundary must survive both
//! content-order run construction and geometric run merging. Neither PDF paint order nor a
//! single pair of neighbouring letters can establish that boundary.
//!
//! Recursive XY subdivision uses projections of nonblank glyph boxes. Vertical cuts need
//! multiline text on both sides and a gap wider than ordinary word spacing; horizontal cuts
//! expose sections separated by spanning headings, footers and changes of column layout.
//! Supported vertical gutters take precedence over paragraph whitespace. Spanning content
//! obstructs those gutters, exposing horizontal cuts before its local column layouts. This
//! is a geometric fallback, not semantic table detection or tagged-PDF structure-tree order.

use super::TextGlyph;

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
    Visit(Region, usize),
    Join(usize),
}

struct Region {
    ids: Vec<usize>,
    // Ink indices sorted once at the root and filtered at cuts. Repeated projection sorts
    // would turn a balanced tree into O(n log² n) work on ordinary multi-line pages.
    x: Vec<usize>,
    y: Vec<usize>,
}

struct Regions {
    leaves: Vec<Vec<usize>>,
    columns: bool,
}

/// Read a projection edge without allowing an invalid index to panic during sorting.
fn edge(glyphs: &[TextGlyph], id: usize, axis: usize) -> f32 {
    glyphs.get(id).and_then(|g| g.rect.get(axis)).copied().unwrap_or(0.0)
}

/// Original indices, partitioned in reading order. Within each leaf, retain paint order for
/// redraw detection, combining marks and overlapping runs in the existing line reconstructor.
pub(super) fn partition(glyphs: &[TextGlyph]) -> Vec<Vec<usize>> {
    let all: Vec<_> = (0..glyphs.len()).collect();
    if glyphs.iter().any(|g| !g.rect.iter().all(|v| v.is_finite())) {
        return vec![all];
    }
    let mut x: Vec<_> = all.iter().copied().filter(|&i| glyphs.get(i).is_some_and(|g| !g.text.trim().is_empty())).collect();
    x.sort_by(|&a, &b| edge(glyphs, a, 0).total_cmp(&edge(glyphs, b, 0)));
    let mut y = x.clone();
    y.sort_by(|&a, &b| edge(glyphs, a, 1).total_cmp(&edge(glyphs, b, 1)));
    let mut pending = vec![Task::Visit(Region { ids: all, x, y }, 0)];
    let mut done: Vec<Regions> = Vec::new();
    while let Some(task) = pending.pop() {
        let (region, depth) = match task {
            Task::Visit(region, depth) => (region, depth),
            Task::Join(axis) => {
                let (Some(right), Some(mut left)) = (done.pop(), done.pop()) else {
                    // Internal stack invariant; preserve all glyphs if it is ever broken.
                    return vec![(0..glyphs.len()).collect()];
                };
                left.columns |= right.columns || axis == 0;
                if !left.columns {
                    // Horizontal whitespace is only a way to expose columns, not evidence
                    // that slightly separated glyphs should become separate logical lines.
                    // Without a supported column cut, leave line grouping to the existing
                    // reconstructor (important for map labels and decorative typography).
                    let mut a = left.leaves.into_iter().flatten().peekable();
                    let mut b = right.leaves.into_iter().flatten().peekable();
                    let mut ids = Vec::new();
                    while let (Some(x), Some(y)) = (a.peek(), b.peek()) {
                        ids.extend(if x <= y { a.next() } else { b.next() });
                    }
                    ids.extend(a);
                    ids.extend(b);
                    left.leaves = vec![ids];
                } else {
                    left.leaves.extend(right.leaves);
                }
                done.push(left);
                continue;
            }
        };
        let cut = (depth < MAX_DEPTH).then(|| subdivision(glyphs, &region)).flatten();
        let Some((axis, at)) = cut else {
            done.push(Regions { leaves: vec![region.ids], columns: false });
            continue;
        };
        let before = |&i: &usize| {
            glyphs.get(i).is_some_and(|g| {
                let r = g.rect;
                // Half-sums cannot overflow for finite coordinates.
                r[axis] * 0.5 + r[axis + 2] * 0.5 < at
            })
        };
        let (first, second): (Vec<_>, Vec<_>) = region.ids.into_iter().partition(before);
        if first.is_empty() || second.is_empty() {
            done.push(Regions { leaves: vec![first.into_iter().chain(second).collect()], columns: false });
            continue;
        }
        let (first_x, second_x) = region.x.into_iter().partition(before);
        let (first_y, second_y) = region.y.into_iter().partition(before);
        // Stack order: top before bottom, left before right. Depth bounds total sorting work
        // to O(n log n + MAX_DEPTH * n), even for successively unbalanced cuts.
        pending.push(Task::Join(axis));
        pending.push(Task::Visit(Region { ids: second, x: second_x, y: second_y }, depth + 1));
        pending.push(Task::Visit(Region { ids: first, x: first_x, y: first_y }, depth + 1));
    }
    done.pop().map_or_else(Vec::new, |r| r.leaves)
}

/// Select the upper median in place, or return no estimate for an empty sample.
fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mid = values.len() / 2;
    Some(*values.select_nth_unstable_by(mid, f32::total_cmp).1)
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

/// Prefer a supported continuous column gutter; use horizontal whitespace only when no
/// such gutter exists, so aligned paragraph breaks cannot interleave neighbouring columns.
fn subdivision(glyphs: &[TextGlyph], region: &Region) -> Option<(usize, f32)> {
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
    let mut spaces: Vec<_> = region
        .ids
        .iter()
        .filter_map(|&i| glyphs.get(i))
        .filter(|g| !g.text.is_empty() && g.text.chars().all(char::is_whitespace))
        .map(|g| g.rect[2] - g.rect[0])
        .filter(|&w| w > 0.0 && w <= h)
        .collect();
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
    let vertical = xgaps
        .iter()
        .filter(|g| {
            let Some((&left, &right)) = g.before.checked_sub(1).and_then(|i| prefix.get(i)).zip(suffix.get(g.before)) else { return false };
            g.size > gutter_floor + g.error
                && left[1] - left[0] > h * 1.5
                && right[1] - right[0] > h * 1.5
                && left[1].min(right[1]) - left[0].max(right[0]) > h * 0.5
        })
        .max_by(|a, b| a.size.total_cmp(&b.size).then_with(|| balance(a).cmp(&balance(b))));
    match (vertical, horizontal) {
        // A persistent gutter establishes a complete column even across large paragraph
        // gaps. A heading spanning it obstructs the projection and permits a horizontal
        // cut instead; the size of empty paragraph whitespace is not section evidence.
        (Some(x), _) => Some((0, x.at)),
        (_, Some(y)) => Some((1, y.at)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make a fixed-size glyph so projection tests have exact, independent geometry.
    fn glyph(text: &str, x: f32, y: f32) -> TextGlyph {
        TextGlyph { text: text.into(), rect: [x, y, x + 6.0, y + 10.0], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] }
    }

    /// Horizontal whitespace without columns must preserve the legacy line grouping input.
    #[test]
    fn horizontal_gaps_alone_do_not_replace_line_reconstruction() {
        let gs: Vec<_> = (0..20).flat_map(|row| (0..30).map(move |col| glyph("a", col as f32 * 6.0, row as f32 * 12.0))).collect();
        assert_eq!(partition(&gs), vec![(0..gs.len()).collect::<Vec<_>>()]);
    }

    /// Partitioning preserves every index exactly once, including blank and invalid geometry.
    #[test]
    fn every_original_index_survives_subdivision_including_blank_glyphs() {
        let mut gs: Vec<_> = (0..4).flat_map(|col| (0..80).map(move |row| glyph("a", col as f32 * 50.0, row as f32 * 12.0))).collect();
        gs.extend([glyph(" ", 25.0, 5.0), glyph("", 75.0, 15.0)]);
        let mut ids: Vec<_> = partition(&gs).into_iter().flatten().collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..gs.len()).collect::<Vec<_>>());
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            gs.push(glyph("a", value, value));
            assert_eq!(partition(&gs), vec![(0..gs.len()).collect::<Vec<_>>()]);
        }
        assert!(partition(&[]).into_iter().flatten().next().is_none());
    }
}
