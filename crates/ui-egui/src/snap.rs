//! Snapping of dragged vector points: to the points of other lines and shapes, in line with them
//! (the same x or y), and onto their edges. Everything is in screen points, so the reach feels the
//! same at any zoom; the caller converts the page's points to the screen and back.

use egui::{Pos2, Vec2};

/// How near (screen points) a dragged point must come to another point to take it.
pub(crate) const POINT_REACH: f32 = 9.0;
/// How near it must come to the x or y of another point to line up with it.
pub(crate) const ALIGN_REACH: f32 = 6.0;
/// How near it must come to another line's edge to land on it.
pub(crate) const EDGE_REACH: f32 = 6.0;
/// A drag with more candidate pairs than this is not snapped: a page of thousands of lines would
/// make every frame of the drag slow, and the drag itself must stay smooth.
pub(crate) const MAX_PAIRS: usize = 3_000_000;

/// What a point snapped to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    /// Another line's point.
    Point,
    /// In line with other points (the same x and/or the same y).
    Align,
    /// On another line's edge.
    Edge,
}

/// Where a dragged point goes, and what to draw to say why.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snap {
    pub pos: Pos2,
    pub kind: Kind,
    /// The points the dragged one took, lined up with, or landed on.
    pub targets: Vec<Pos2>,
    /// Guide lines from each target to `pos` (alignments).
    pub guides: Vec<[Pos2; 2]>,
}

impl Snap {
    fn at(pos: Pos2, kind: Kind, target: Pos2) -> Snap {
        Snap { pos, kind, targets: vec![target], guides: Vec::new() }
    }
}

/// The nearest point of segment `a`–`b` to `p`.
pub(crate) fn nearest_on_segment(p: Pos2, a: Pos2, b: Pos2) -> Pos2 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 > 0.0 && len2.is_finite() { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

/// Where the point `p` snaps to among `points` and `edges`, if anywhere: another point wins, then
/// lining up with other points, then another line's edge. `None` leaves `p` where it is.
pub(crate) fn snap_point(p: Pos2, points: &[Pos2], edges: &[(Pos2, Pos2)]) -> Option<Snap> {
    if !p.x.is_finite() || !p.y.is_finite() {
        return None;
    }
    let finite = |q: &&Pos2| q.x.is_finite() && q.y.is_finite();
    // Another point, the nearest within reach.
    let nearest = points.iter().filter(finite).map(|q| (q.distance(p), *q)).filter(|(d, _)| *d <= POINT_REACH).min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, q)) = nearest {
        return Some(Snap::at(q, Kind::Point, q));
    }
    // In line with another point's x and/or y.
    let along_x = points.iter().filter(finite).map(|q| ((q.x - p.x).abs(), *q)).filter(|(d, _)| *d <= ALIGN_REACH).min_by(|a, b| a.0.total_cmp(&b.0));
    let along_y = points.iter().filter(finite).map(|q| ((q.y - p.y).abs(), *q)).filter(|(d, _)| *d <= ALIGN_REACH).min_by(|a, b| a.0.total_cmp(&b.0));
    if along_x.is_some() || along_y.is_some() {
        let pos = Pos2::new(along_x.map_or(p.x, |(_, q)| q.x), along_y.map_or(p.y, |(_, q)| q.y));
        let mut targets = Vec::new();
        let mut guides = Vec::new();
        for (_, q) in along_x.into_iter().chain(along_y) {
            targets.push(q);
            guides.push([q, pos]);
        }
        return Some(Snap { pos, kind: Kind::Align, targets, guides });
    }
    // On another line's edge.
    edges
        .iter()
        .filter(|(a, b)| a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite())
        .map(|(a, b)| nearest_on_segment(p, *a, *b))
        .map(|q| (q.distance(p), q))
        .filter(|(d, _)| *d <= EDGE_REACH)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, q)| Snap::at(q, Kind::Edge, q))
}

/// How far to shift a group being moved by `delta` so that one of its `moving` points lands on one
/// of the `targets`, with the point and the target it lands on. `None`: nothing within reach.
pub(crate) fn snap_move(moving: &[Pos2], delta: Vec2, targets: &[Pos2]) -> Option<(Vec2, Pos2)> {
    if moving.len().saturating_mul(targets.len()) > MAX_PAIRS {
        return None;
    }
    let mut best: Option<(f32, Vec2, Pos2)> = None;
    for m in moving.iter().filter(|m| m.x.is_finite() && m.y.is_finite()) {
        let at = *m + delta;
        for t in targets.iter().filter(|t| t.x.is_finite() && t.y.is_finite()) {
            let d = at.distance(*t);
            if d <= POINT_REACH && best.is_none_or(|(b, _, _)| d < b) {
                best = Some((d, *t - at, *t));
            }
        }
    }
    best.map(|(_, adjust, target)| (adjust, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> Pos2 {
        Pos2::new(x, y)
    }

    #[test]
    fn a_point_takes_the_nearest_point_within_reach() {
        let points = [p(100.0, 100.0), p(104.0, 100.0), p(300.0, 300.0)];
        let snap = snap_point(p(103.0, 101.0), &points, &[]).unwrap();
        assert_eq!((snap.kind, snap.pos), (Kind::Point, p(104.0, 100.0)), "the nearer of the two");
        assert!(snap_point(p(120.0, 120.0), &points, &[]).is_none(), "out of reach");
    }

    #[test]
    fn a_point_lines_up_with_the_x_or_y_of_others_and_says_so() {
        let points = [p(100.0, 40.0), p(10.0, 200.0)];
        // Near x = 100 only: the x is taken, the y is kept.
        let snap = snap_point(p(103.0, 150.0), &points, &[]).unwrap();
        assert_eq!((snap.kind, snap.pos), (Kind::Align, p(100.0, 150.0)));
        assert_eq!(snap.guides, vec![[p(100.0, 40.0), p(100.0, 150.0)]]);
        // Near one point's x and another's y: both are taken (a right angle between them).
        let snap = snap_point(p(103.0, 197.0), &points, &[]).unwrap();
        assert_eq!(snap.pos, p(100.0, 200.0));
        assert_eq!(snap.guides.len(), 2);
    }

    #[test]
    fn a_point_lands_on_an_edge_last() {
        let edges = [(p(0.0, 100.0), p(200.0, 100.0))];
        let snap = snap_point(p(80.0, 104.0), &[], &edges).unwrap();
        assert_eq!((snap.kind, snap.pos), (Kind::Edge, p(80.0, 100.0)));
        assert!(snap_point(p(80.0, 120.0), &[], &edges).is_none());
        // Beyond the segment's end it lands on the end, not on the line it lies along.
        let snap = snap_point(p(204.0, 103.0), &[], &edges).unwrap();
        assert_eq!(snap.pos, p(200.0, 100.0));
        // A point beats an alignment, an alignment beats an edge.
        let points = [p(80.0, 50.0)];
        assert_eq!(snap_point(p(82.0, 103.0), &points, &edges).unwrap().kind, Kind::Align);
        assert_eq!(snap_point(p(82.0, 52.0), &points, &edges).unwrap().kind, Kind::Point);
    }

    #[test]
    fn broken_numbers_snap_to_nothing_and_never_panic() {
        let points = [p(f32::NAN, 1.0), p(f32::INFINITY, 0.0), p(10.0, 10.0)];
        assert_eq!(snap_point(p(11.0, 11.0), &points, &[]).unwrap().pos, p(10.0, 10.0));
        assert!(snap_point(p(f32::NAN, 0.0), &points, &[]).is_none());
        assert!(snap_point(p(0.0, 0.0), &[], &[(p(f32::NAN, 0.0), p(1.0, 1.0))]).is_none());
        assert_eq!(nearest_on_segment(p(5.0, 5.0), p(1.0, 1.0), p(1.0, 1.0)), p(1.0, 1.0), "a segment that is a point");
    }

    #[test]
    fn a_moved_group_snaps_one_of_its_points_onto_a_target() {
        let moving = [p(0.0, 0.0), p(100.0, 0.0)];
        let targets = [p(250.0, 40.0), p(10.0, 90.0)];
        // Moved by (150, 38): the second point is at (250, 38), two points from (250, 40).
        let (adjust, target) = snap_move(&moving, Vec2::new(150.0, 38.0), &targets).unwrap();
        assert_eq!((adjust, target), (Vec2::new(0.0, 2.0), p(250.0, 40.0)));
        assert!(snap_move(&moving, Vec2::new(150.0, 10.0), &targets).is_none());
        assert!(snap_move(&[], Vec2::ZERO, &targets).is_none());
        // Too many pairs: no snapping rather than a slow frame.
        let many = vec![p(0.0, 0.0); 2_000];
        assert!(snap_move(&many, Vec2::ZERO, &many).is_none());
    }
}
