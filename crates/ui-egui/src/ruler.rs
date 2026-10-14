//! Pure geometry and tick selection for document rulers.
//!
//! The canvas remains the authority for the final displayed page origin, length and scale. This
//! module only converts that one-dimensional geometry into visible ruler ticks; it does not lay
//! out pages, read scroll state or paint widgets.

use crate::units::Unit;

const MIN_MINOR_SPACING: f64 = 4.0;
const MIN_MEDIUM_SPACING: f64 = 16.0;
const MIN_MAJOR_SPACING: f64 = 48.0;
const MIN_LABEL_SPACING: f64 = 64.0;
const MAX_TICKS: usize = 16_384;

const INCH_INTERVALS: &[f64] = &[
    1.0 / 64.0,
    1.0 / 32.0,
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0,
    2.0,
    4.0,
    8.0,
    16.0,
    32.0,
    64.0,
    128.0,
    256.0,
    512.0,
    1_024.0,
    2_048.0,
    4_096.0,
];
const METRIC_INTERVALS: &[f64] =
    &[0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0, 20_000.0, 50_000.0, 100_000.0];
const POINT_INTERVALS: &[f64] = &[
    1.0,
    2.0,
    5.0,
    10.0,
    20.0,
    50.0,
    100.0,
    200.0,
    500.0,
    1_000.0,
    2_000.0,
    5_000.0,
    10_000.0,
    20_000.0,
    50_000.0,
    100_000.0,
    200_000.0,
    500_000.0,
    1_000_000.0,
];

/// Screen axis measured by a ruler. Values grow rightward or downward in displayed-page space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulerOrientation {
    Horizontal,
    Vertical,
}

/// Importance of a generated ruler tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickKind {
    Minor,
    Medium,
    Major,
}

/// Tick cadence selected for a unit at the current displayed scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickSpec {
    pub minor_interval_units: f64,
    pub medium_every: u32,
    pub major_every: u32,
    pub label_every: u32,
}

/// One visible tick, in logical screen points and the selected display unit.
#[derive(Clone, Debug, PartialEq)]
pub struct RulerTick {
    pub screen_position: f64,
    pub value: f64,
    pub kind: TickKind,
    pub label: Option<String>,
}

/// Final one-dimensional page geometry supplied by the canvas.
///
/// `ruler_start` and `ruler_end` are the visible ruler bounds in logical screen points.
/// `page_origin` is the displayed page's left or top edge on that same axis. The page length is in
/// PDF points and `screen_points_per_pdf_point` is the final scale used to draw the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerGeometry {
    pub orientation: RulerOrientation,
    pub ruler_start: f64,
    pub ruler_end: f64,
    pub page_origin: f64,
    pub page_length_pdf_points: f64,
    pub screen_points_per_pdf_point: f64,
    pub unit: Unit,
}

impl RulerGeometry {
    /// Whether all geometry can be converted without division by zero or non-finite coordinates.
    pub fn is_valid(self) -> bool {
        self.ruler_start.is_finite()
            && self.ruler_end.is_finite()
            && self.ruler_end > self.ruler_start
            && self.page_origin.is_finite()
            && self.page_length_pdf_points.is_finite()
            && self.page_length_pdf_points >= 0.0
            && self.screen_points_per_pdf_point.is_finite()
            && self.screen_points_per_pdf_point > 0.0
            && (self.page_origin + self.page_length_pdf_points * self.screen_points_per_pdf_point).is_finite()
    }

    /// Logical screen points occupied by one selected display unit.
    pub fn screen_points_per_unit(self) -> Option<f64> {
        if !self.is_valid() {
            return None;
        }
        let per_point = self.unit.per_point();
        let scale = self.screen_points_per_pdf_point / per_point;
        scale.is_finite().then_some(scale)
    }
}

/// Choose the smallest supported minor interval that remains legible at this scale.
pub fn choose_tick_spec(unit: Unit, screen_points_per_unit: f64) -> Option<TickSpec> {
    if !screen_points_per_unit.is_finite() || screen_points_per_unit <= 0.0 {
        return None;
    }
    let intervals = intervals(unit);
    let minor_index = intervals.iter().position(|interval| interval * screen_points_per_unit >= MIN_MINOR_SPACING).unwrap_or(intervals.len() - 1);
    let minor = intervals[minor_index];
    let minor_spacing = minor * screen_points_per_unit;
    if minor_spacing < MIN_MINOR_SPACING {
        return None;
    }
    Some(TickSpec {
        minor_interval_units: minor,
        medium_every: cadence(intervals, minor_index, minor, screen_points_per_unit, MIN_MEDIUM_SPACING),
        major_every: cadence(intervals, minor_index, minor, screen_points_per_unit, MIN_MAJOR_SPACING),
        label_every: cadence(intervals, minor_index, minor, screen_points_per_unit, MIN_LABEL_SPACING.max(minor_spacing)),
    })
}

/// Convert PDF points from the displayed page origin to a logical screen position.
pub fn page_points_to_screen(geometry: RulerGeometry, page_points: f64) -> Option<f64> {
    if !geometry.is_valid() || !page_points.is_finite() {
        return None;
    }
    let screen = geometry.page_origin + page_points * geometry.screen_points_per_pdf_point;
    screen.is_finite().then_some(screen)
}

/// Convert a logical screen position to PDF points from the displayed page origin.
pub fn screen_to_page_points(geometry: RulerGeometry, screen_position: f64) -> Option<f64> {
    if !geometry.is_valid() || !screen_position.is_finite() {
        return None;
    }
    let points = (screen_position - geometry.page_origin) / geometry.screen_points_per_pdf_point;
    points.is_finite().then_some(points)
}

/// Generate ticks within both the displayed page and the visible ruler range.
///
/// The returned ticks are ordered by screen position. Invalid, empty or entirely off-screen
/// geometry returns an empty vector. Work is bounded even for hostile or accidental huge inputs.
pub fn visible_ticks(geometry: RulerGeometry) -> Vec<RulerTick> {
    let Some(screen_points_per_unit) = geometry.screen_points_per_unit() else {
        return Vec::new();
    };
    let Some(spec) = choose_tick_spec(geometry.unit, screen_points_per_unit) else {
        return Vec::new();
    };
    let page_end = geometry.page_origin + geometry.page_length_pdf_points * geometry.screen_points_per_pdf_point;
    let visible_start = geometry.ruler_start.max(geometry.page_origin);
    let visible_end = geometry.ruler_end.min(page_end);
    if visible_end < visible_start || geometry.page_length_pdf_points == 0.0 {
        return Vec::new();
    }

    let per_point = geometry.unit.per_point();
    let start_units = ((visible_start - geometry.page_origin) / geometry.screen_points_per_pdf_point * per_point).max(0.0);
    let page_units = geometry.page_length_pdf_points * per_point;
    let end_units = ((visible_end - geometry.page_origin) / geometry.screen_points_per_pdf_point * per_point).min(page_units);
    if !start_units.is_finite() || !end_units.is_finite() || end_units < start_units {
        return Vec::new();
    }

    let first_f = (start_units / spec.minor_interval_units).ceil();
    let last_f = (end_units / spec.minor_interval_units).floor();
    if first_f < i64::MIN as f64 || first_f > i64::MAX as f64 || last_f < i64::MIN as f64 || last_f > i64::MAX as f64 {
        return Vec::new();
    }
    let first = first_f as i64;
    let last = last_f as i64;
    if last < first {
        return Vec::new();
    }
    let available = last.saturating_sub(first).saturating_add(1);
    let count = usize::try_from(available).unwrap_or(MAX_TICKS).min(MAX_TICKS);
    let mut ticks = Vec::with_capacity(count);
    for offset in 0..count {
        let Some(index) = first.checked_add(offset as i64) else {
            break;
        };
        let value = index as f64 * spec.minor_interval_units;
        let screen_position = geometry.page_origin + value * screen_points_per_unit;
        if !screen_position.is_finite() {
            break;
        }
        let kind = if divisible(index, spec.major_every) {
            TickKind::Major
        } else if divisible(index, spec.medium_every) {
            TickKind::Medium
        } else {
            TickKind::Minor
        };
        let label = divisible(index, spec.label_every).then(|| format_label(geometry.unit, value));
        ticks.push(RulerTick { screen_position, value, kind, label });
    }
    ticks
}

fn intervals(unit: Unit) -> &'static [f64] {
    match unit {
        Unit::Inches => INCH_INTERVALS,
        Unit::Millimetres | Unit::Centimetres => METRIC_INTERVALS,
        Unit::Points => POINT_INTERVALS,
    }
}

fn cadence(intervals: &[f64], minor_index: usize, minor: f64, screen_points_per_unit: f64, minimum_spacing: f64) -> u32 {
    let selected = intervals[minor_index..]
        .iter()
        .copied()
        .find(|interval| interval * screen_points_per_unit >= minimum_spacing)
        .unwrap_or(*intervals.last().unwrap_or(&minor));
    let ratio = (selected / minor).round();
    if !ratio.is_finite() || ratio < 1.0 {
        1
    } else if ratio > u32::MAX as f64 {
        u32::MAX
    } else {
        ratio as u32
    }
}

fn divisible(index: i64, every: u32) -> bool {
    every != 0 && index.rem_euclid(i64::from(every)) == 0
}

fn format_label(unit: Unit, value: f64) -> String {
    let decimals = unit.decimals();
    let rounded = value.round();
    if rounded == 0.0 {
        "0".to_owned()
    } else if (value - rounded).abs() <= f64::EPSILON * value.abs().max(1.0) {
        format!("{rounded:.0}")
    } else {
        format!("{value:.decimals$}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(unit: Unit, scale: f64) -> RulerGeometry {
        RulerGeometry {
            orientation: RulerOrientation::Horizontal,
            ruler_start: 0.0,
            ruler_end: 1_000.0,
            page_origin: 200.0,
            page_length_pdf_points: 612.0,
            screen_points_per_pdf_point: scale,
            unit,
        }
    }

    #[test]
    fn page_and_screen_coordinates_round_trip() {
        let g = geometry(Unit::Points, 2.0);
        assert_eq!(page_points_to_screen(g, 0.0), Some(200.0));
        assert_eq!(page_points_to_screen(g, 72.0), Some(344.0));
        assert_eq!(screen_to_page_points(g, 344.0), Some(72.0));
    }

    #[test]
    fn vertical_geometry_uses_the_same_displayed_axis_policy() {
        let mut g = geometry(Unit::Millimetres, 1.25);
        g.orientation = RulerOrientation::Vertical;
        g.page_origin = 40.0;
        assert_eq!(page_points_to_screen(g, 80.0), Some(140.0));
        assert_eq!(screen_to_page_points(g, 140.0), Some(80.0));
    }

    #[test]
    fn invalid_geometry_and_positions_are_rejected() {
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let g = geometry(Unit::Inches, scale);
            assert!(!g.is_valid());
            assert!(visible_ticks(g).is_empty());
            assert_eq!(page_points_to_screen(g, 10.0), None);
        }
        let g = geometry(Unit::Inches, 1.0);
        assert_eq!(page_points_to_screen(g, f64::NAN), None);
        assert_eq!(screen_to_page_points(g, f64::INFINITY), None);
    }

    #[test]
    fn tick_specs_meet_spacing_and_coarsen_when_zooming_out() {
        for unit in Unit::ALL {
            let zoomed_in = choose_tick_spec(unit, 100.0).unwrap();
            let zoomed_out = choose_tick_spec(unit, 10.0).unwrap();
            assert!(zoomed_out.minor_interval_units >= zoomed_in.minor_interval_units);
            assert!(zoomed_in.minor_interval_units * 100.0 >= MIN_MINOR_SPACING);
            assert!(zoomed_out.minor_interval_units * 10.0 >= MIN_MINOR_SPACING);
            assert!(zoomed_in.medium_every <= zoomed_in.major_every);
            assert!(zoomed_in.major_every <= zoomed_in.label_every);
        }
    }

    #[test]
    fn common_scales_have_practical_unit_intervals() {
        let inches = choose_tick_spec(Unit::Inches, 96.0).unwrap();
        assert_eq!(inches.minor_interval_units, 1.0 / 16.0);
        assert_eq!(inches.medium_every, 4);
        assert_eq!(inches.major_every, 8);
        assert_eq!(inches.label_every, 16);

        let millimetres = choose_tick_spec(Unit::Millimetres, 96.0 / 25.4).unwrap();
        assert_eq!(millimetres.minor_interval_units, 2.0);
        assert_eq!(millimetres.medium_every, 3);
        assert_eq!(millimetres.major_every, 10);
        assert_eq!(millimetres.label_every, 10);
    }

    #[test]
    fn visible_ticks_are_clipped_ordered_and_labelled() {
        let mut g = geometry(Unit::Inches, 96.0 / 72.0);
        g.ruler_start = 240.0;
        g.ruler_end = 500.0;
        let ticks = visible_ticks(g);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|tick| tick.screen_position >= g.ruler_start && tick.screen_position <= g.ruler_end));
        assert!(ticks.windows(2).all(|pair| pair[0].screen_position < pair[1].screen_position && pair[0].value < pair[1].value));
        assert!(ticks.iter().any(|tick| tick.kind == TickKind::Major));
        assert!(ticks.iter().any(|tick| tick.label.is_some()));
    }

    #[test]
    fn a_page_partly_before_the_ruler_starts_at_the_first_visible_tick() {
        let mut g = geometry(Unit::Points, 1.0);
        g.ruler_start = 0.0;
        g.ruler_end = 200.0;
        g.page_origin = -100.0;
        let ticks = visible_ticks(g);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|tick| tick.screen_position >= g.ruler_start && tick.screen_position <= g.ruler_end));
        assert!(ticks.first().is_some_and(|tick| tick.value >= 100.0));
    }

    #[test]
    fn negative_page_coordinates_convert_without_changing_the_axis_policy() {
        let g = geometry(Unit::Points, 2.0);
        assert_eq!(page_points_to_screen(g, -10.0), Some(180.0));
        assert_eq!(screen_to_page_points(g, 180.0), Some(-10.0));
    }

    #[test]
    fn extreme_scales_do_not_generate_dense_or_non_finite_ticks() {
        for scale in [1.0e-12, 1.0e12] {
            for unit in Unit::ALL {
                let ticks = visible_ticks(geometry(unit, scale));
                assert!(ticks.len() <= MAX_TICKS);
                assert!(ticks.iter().all(|tick| tick.screen_position.is_finite() && tick.value.is_finite()));
            }
        }
        assert!(choose_tick_spec(Unit::Points, 0.0).is_none());
        assert!(choose_tick_spec(Unit::Points, f64::NAN).is_none());
    }

    #[test]
    fn a_page_entirely_outside_the_ruler_has_no_ticks() {
        let mut g = geometry(Unit::Points, 1.0);
        g.ruler_start = 0.0;
        g.ruler_end = 100.0;
        g.page_origin = 200.0;
        assert!(visible_ticks(g).is_empty());
    }

    #[test]
    fn zero_length_page_has_no_ticks() {
        let mut g = geometry(Unit::Centimetres, 1.0);
        g.page_length_pdf_points = 0.0;
        assert!(g.is_valid());
        assert!(visible_ticks(g).is_empty());
    }

    #[test]
    fn every_unit_generates_bounded_ticks_at_fractional_scales() {
        for unit in Unit::ALL {
            for scale in [0.75, 1.0, 1.25, 1.333_333, 1.5, 2.0, 4.0] {
                let ticks = visible_ticks(geometry(unit, scale));
                assert!(!ticks.is_empty(), "{unit:?} at {scale}");
                assert!(ticks.len() <= MAX_TICKS);
                assert!(ticks.iter().all(|tick| tick.screen_position.is_finite() && tick.value.is_finite()));
            }
        }
    }

    #[test]
    fn hostile_huge_inputs_are_bounded_or_rejected() {
        let mut g = geometry(Unit::Points, f64::MIN_POSITIVE);
        g.ruler_end = f64::MAX / 4.0;
        g.page_length_pdf_points = f64::MAX / 4.0;
        assert!(visible_ticks(g).len() <= MAX_TICKS);

        g.page_origin = 0.0;
        g.page_length_pdf_points = f64::MAX;
        g.screen_points_per_pdf_point = 2.0;
        assert!(!g.is_valid());
        assert!(visible_ticks(g).is_empty());
    }

    #[test]
    fn labels_do_not_show_negative_zero() {
        assert_eq!(format_label(Unit::Inches, -0.0), "0");
        assert_eq!(format_label(Unit::Points, 72.0), "72");
    }
}
