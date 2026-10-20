//! Pure geometry and tick selection for document rulers.
//!
//! The canvas remains the authority for the final displayed page origin, length and scale. This
//! module only converts that one-dimensional geometry into visible ruler ticks; it does not lay
//! out pages, read scroll state or paint widgets.

use crate::units::Unit;

const MIN_MINOR_SPACING: f64 = 4.0;
const MIN_MEDIUM_SPACING: f64 = 16.0;
const MIN_MAJOR_SPACING: f64 = 48.0;
const MIN_LABEL_SPACING: f64 = 48.0;
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
    if unit == Unit::Inches {
        return inch_tick_spec(screen_points_per_unit);
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

/// Inch rulers use familiar binary fractions. The fractions shown by the short and medium ticks
/// thin out as the page is zoomed away, while labelled whole-inch marks remain widely spaced.
fn inch_tick_spec(screen_points_per_inch: f64) -> Option<TickSpec> {
    let (minor_interval_units, medium_every, major_every, label_every) = if screen_points_per_inch >= 128.0 {
        (1.0 / 16.0, 4, 16, 16)
    } else if screen_points_per_inch >= 64.0 {
        (1.0 / 8.0, 4, 8, 8)
    } else if screen_points_per_inch >= 32.0 {
        (1.0 / 4.0, 2, 4, 4)
    } else if screen_points_per_inch >= 16.0 {
        (1.0 / 2.0, 2, 4, 4)
    } else if screen_points_per_inch >= 8.0 {
        (1.0, 2, 4, 8)
    } else {
        let intervals = intervals(Unit::Inches);
        let minor_index = intervals.iter().position(|interval| interval * screen_points_per_inch >= MIN_MINOR_SPACING)?;
        let minor = intervals[minor_index];
        return Some(TickSpec {
            minor_interval_units: minor,
            medium_every: cadence(intervals, minor_index, minor, screen_points_per_inch, MIN_MEDIUM_SPACING),
            major_every: cadence(intervals, minor_index, minor, screen_points_per_inch, MIN_MAJOR_SPACING),
            label_every: cadence(intervals, minor_index, minor, screen_points_per_inch, MIN_LABEL_SPACING),
        });
    };
    let minimum_label_every = (MIN_LABEL_SPACING / (minor_interval_units * screen_points_per_inch)).ceil().max(1.0) as u32;
    Some(TickSpec { minor_interval_units, medium_every, major_every, label_every: label_every.max(minimum_label_every) })
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

/// Generate ticks across the complete ruler. The page supplies a stable origin and scale but does
/// not clip the ticks; measurements continue through the pasteboard on both sides.
pub fn visible_ticks(geometry: RulerGeometry) -> Vec<RulerTick> {
    let Some(screen_points_per_unit) = geometry.screen_points_per_unit() else {
        return Vec::new();
    };
    let Some(spec) = choose_tick_spec(geometry.unit, screen_points_per_unit) else {
        return Vec::new();
    };
    let visible_start = geometry.ruler_start;
    let visible_end = geometry.ruler_end;
    let per_point = geometry.unit.per_point();
    let start_units = (visible_start - geometry.page_origin) / geometry.screen_points_per_pdf_point * per_point;
    let end_units = (visible_end - geometry.page_origin) / geometry.screen_points_per_pdf_point * per_point;
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
    let value = if unit == Unit::Inches { value.abs() } else { value };
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

/// Compact, screen-fixed ruler bands modelled on the reference application's canvas chrome.
/// Zoom changes tick cadence, never the amount of document viewport reserved by the rulers.
pub const HORIZONTAL_RULER_HEIGHT: f32 = 18.0;
pub const VERTICAL_RULER_WIDTH: f32 = 20.0;
const HORIZONTAL_LABEL_OUTER_PADDING: f32 = 1.0;
const HORIZONTAL_LABEL_TICK_GAP: f32 = 1.0;
const HORIZONTAL_LABEL_AFTER_TICK: f32 = 2.0;
const VERTICAL_LABEL_ABOVE_TICK: f32 = 1.0;
const RULER_LABEL_SIZE: f32 = 9.0;

/// Read-only paint input for one ruler band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerPaintInput {
    pub orientation: RulerOrientation,
    pub rect: egui::Rect,
    pub page_origin: f64,
    pub page_length_pdf_points: f64,
    pub screen_points_per_pdf_point: f64,
    pub unit: Unit,
    /// Pointer position on this ruler's screen axis. The caller supplies it only while the
    /// pointer is over the document canvas.
    pub pointer_screen_position: Option<f64>,
}

impl RulerPaintInput {
    fn calculation_geometry(self) -> RulerGeometry {
        let (ruler_start, ruler_end) = match self.orientation {
            RulerOrientation::Horizontal => (f64::from(self.rect.left()), f64::from(self.rect.right())),
            RulerOrientation::Vertical => (f64::from(self.rect.top()), f64::from(self.rect.bottom())),
        };
        RulerGeometry {
            orientation: self.orientation,
            ruler_start,
            ruler_end,
            page_origin: self.page_origin,
            page_length_pdf_points: self.page_length_pdf_points,
            screen_points_per_pdf_point: self.screen_points_per_pdf_point,
            unit: self.unit,
        }
    }
}

/// Paint one clipped read-only ruler band. The caller owns layout and pointer policy.
pub fn paint_ruler(ui: &egui::Ui, input: RulerPaintInput) {
    let style = RulerPaintStyle::from_ui(ui);
    let painter = ui.painter().with_clip_rect(input.rect);
    painter.rect_filled(input.rect, egui::CornerRadius::ZERO, style.outside_background);

    let calculation = input.calculation_geometry();

    let ticks = visible_ticks(calculation);
    for kind in [TickKind::Minor, TickKind::Medium, TickKind::Major] {
        for tick in ticks.iter().filter(|tick| tick.kind == kind) {
            paint_tick(&painter, input, tick, style);
        }
    }
    if let Some(pointer) = input.pointer_screen_position.and_then(screen_coordinate) {
        paint_pointer_marker(&painter, input.orientation, input.rect, pointer, style.pointer);
    }
    paint_canvas_edge(&painter, input.orientation, input.rect, style.edge);
}

/// Paint the inert square where horizontal and vertical rulers meet.
pub fn paint_corner(ui: &egui::Ui, rect: egui::Rect, unit: Unit) {
    let style = RulerPaintStyle::from_ui(ui);
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, egui::CornerRadius::ZERO, style.outside_background);
    painter.line_segment([rect.right_top(), rect.right_bottom()], style.edge);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], style.edge);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, unit.label(), crate::theme::regular(RULER_LABEL_SIZE), style.label);
}

#[derive(Clone, Copy)]
struct RulerPaintStyle {
    outside_background: egui::Color32,
    tick: egui::Stroke,
    major_tick: egui::Stroke,
    edge: egui::Stroke,
    label: egui::Color32,
    pointer: egui::Color32,
}

impl RulerPaintStyle {
    fn from_ui(ui: &egui::Ui) -> Self {
        let tokens = crate::theme::Tokens::get(ui.ctx());
        Self {
            outside_background: tokens.chrome,
            tick: egui::Stroke::new(1.0, tokens.text_muted),
            major_tick: egui::Stroke::new(1.0, tokens.text),
            edge: egui::Stroke::new(1.0, tokens.border),
            label: tokens.text,
            pointer: tokens.accent,
        }
    }
}

fn paint_tick(painter: &egui::Painter, input: RulerPaintInput, tick: &RulerTick, style: RulerPaintStyle) {
    let Some(at) = screen_coordinate(tick.screen_position) else { return };
    let length = tick_length(input.rect, input.orientation, tick.kind);
    let stroke = if tick.kind == TickKind::Major { style.major_tick } else { style.tick };
    match input.orientation {
        RulerOrientation::Horizontal => {
            painter.line_segment([egui::pos2(at, input.rect.bottom()), egui::pos2(at, input.rect.bottom() - length)], stroke);
            if let Some(label) = &tick.label
                && horizontal_label_fits(input.rect, at, label)
            {
                painter.text(
                    egui::pos2(at + HORIZONTAL_LABEL_AFTER_TICK, input.rect.top() + HORIZONTAL_LABEL_OUTER_PADDING),
                    egui::Align2::LEFT_TOP,
                    label,
                    crate::theme::regular(RULER_LABEL_SIZE),
                    style.label,
                );
            }
        }
        RulerOrientation::Vertical => {
            painter.line_segment([egui::pos2(input.rect.right(), at), egui::pos2(input.rect.right() - length, at)], stroke);
            if let Some(label) = &tick.label
                && vertical_label_fits(input.rect, at, label, length)
            {
                painter.text(
                    egui::pos2(input.rect.center().x, at - VERTICAL_LABEL_ABOVE_TICK),
                    egui::Align2::CENTER_BOTTOM,
                    label,
                    crate::theme::regular(RULER_LABEL_SIZE),
                    style.label,
                );
            }
        }
    }
}

fn estimated_label_width(label: &str) -> f32 {
    label.chars().count() as f32 * RULER_LABEL_SIZE * 0.62
}

fn horizontal_label_fits(rect: egui::Rect, at: f32, label: &str) -> bool {
    let left = at + HORIZONTAL_LABEL_AFTER_TICK;
    left >= rect.left() + 1.0 && left + estimated_label_width(label) <= rect.right() - 1.0
}

fn vertical_label_fits(rect: egui::Rect, at: f32, label: &str, _tick_length: f32) -> bool {
    let half_width = estimated_label_width(label) * 0.5;
    let centre = rect.center().x;
    centre - half_width >= rect.left() + 1.0
        && centre + half_width <= rect.right() - 1.0
        && at - VERTICAL_LABEL_ABOVE_TICK - RULER_LABEL_SIZE >= rect.top() + 1.0
        && at - VERTICAL_LABEL_ABOVE_TICK <= rect.bottom() - 1.0
}

fn tick_length(rect: egui::Rect, orientation: RulerOrientation, kind: TickKind) -> f32 {
    let available = match orientation {
        RulerOrientation::Horizontal => (rect.height() - HORIZONTAL_LABEL_OUTER_PADDING - RULER_LABEL_SIZE - HORIZONTAL_LABEL_TICK_GAP).max(0.0),
        RulerOrientation::Vertical => rect.width().max(0.0),
    };
    let desired: f32 = match (orientation, kind) {
        (RulerOrientation::Horizontal, TickKind::Minor) => 2.0,
        (RulerOrientation::Horizontal, TickKind::Medium) => 4.0,
        (RulerOrientation::Horizontal, TickKind::Major) => 6.0,
        (RulerOrientation::Vertical, TickKind::Minor) => 2.0,
        (RulerOrientation::Vertical, TickKind::Medium) => 4.0,
        (RulerOrientation::Vertical, TickKind::Major) => 6.0,
    };
    desired.min(available)
}

fn paint_canvas_edge(painter: &egui::Painter, orientation: RulerOrientation, rect: egui::Rect, stroke: egui::Stroke) {
    let points = match orientation {
        RulerOrientation::Horizontal => [rect.left_bottom(), rect.right_bottom()],
        RulerOrientation::Vertical => [rect.right_top(), rect.right_bottom()],
    };
    painter.line_segment(points, stroke);
}

fn paint_pointer_marker(painter: &egui::Painter, orientation: RulerOrientation, rect: egui::Rect, axis_position: f32, color: egui::Color32) {
    let points = pointer_marker_points(orientation, rect, axis_position);
    painter.add(egui::Shape::convex_polygon(points.to_vec(), color, egui::Stroke::NONE));
}

fn pointer_marker_points(orientation: RulerOrientation, rect: egui::Rect, axis_position: f32) -> [egui::Pos2; 3] {
    match orientation {
        RulerOrientation::Horizontal => [
            egui::pos2(axis_position - 4.0, rect.bottom() - 1.0),
            egui::pos2(axis_position + 4.0, rect.bottom() - 1.0),
            egui::pos2(axis_position, rect.bottom() - 7.0),
        ],
        RulerOrientation::Vertical => [
            egui::pos2(rect.right() - 1.0, axis_position - 4.0),
            egui::pos2(rect.right() - 1.0, axis_position + 4.0),
            egui::pos2(rect.right() - 7.0, axis_position),
        ],
    }
}

fn screen_coordinate(value: f64) -> Option<f32> {
    (value.is_finite() && (f64::from(f32::MIN)..=f64::from(f32::MAX)).contains(&value)).then_some(value as f32)
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
        assert_eq!(inches.minor_interval_units, 1.0 / 8.0);
        assert_eq!(inches.medium_every, 4);
        assert_eq!(inches.major_every, 8);
        assert_eq!(inches.label_every, 8);

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
    fn ruler_ticks_continue_when_the_page_is_outside_the_viewport() {
        let mut g = geometry(Unit::Points, 1.0);
        g.ruler_start = 0.0;
        g.ruler_end = 100.0;
        g.page_origin = 200.0;
        let ticks = visible_ticks(g);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|tick| tick.value < 0.0));
    }

    #[test]
    fn zero_length_page_still_supplies_a_ruler_origin() {
        let mut g = geometry(Unit::Centimetres, 1.0);
        g.page_length_pdf_points = 0.0;
        assert!(g.is_valid());
        assert!(!visible_ticks(g).is_empty());
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

    fn paint_input(orientation: RulerOrientation, rect: egui::Rect) -> RulerPaintInput {
        RulerPaintInput {
            orientation,
            rect,
            page_origin: match orientation {
                RulerOrientation::Horizontal => f64::from(rect.left()) + 20.0,
                RulerOrientation::Vertical => f64::from(rect.top()) + 20.0,
            },
            page_length_pdf_points: 612.0,
            screen_points_per_pdf_point: 1.0,
            unit: Unit::Points,
            pointer_screen_position: Some(match orientation {
                RulerOrientation::Horizontal => f64::from(rect.center().x),
                RulerOrientation::Vertical => f64::from(rect.center().y),
            }),
        }
    }

    #[test]
    fn generated_labels_keep_the_minimum_screen_spacing() {
        for unit in Unit::ALL {
            for scale in [0.25, 0.75, 1.0, 1.333_333, 2.0, 4.0, 16.0] {
                let labels: Vec<f64> =
                    visible_ticks(geometry(unit, scale)).into_iter().filter(|tick| tick.label.is_some()).map(|tick| tick.screen_position).collect();
                assert!(labels.windows(2).all(|pair| pair[1] - pair[0] >= MIN_LABEL_SPACING - 0.01));
            }
        }
    }

    #[test]
    fn tick_hierarchy_has_strictly_increasing_lengths() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, HORIZONTAL_RULER_HEIGHT));
        for orientation in [RulerOrientation::Horizontal, RulerOrientation::Vertical] {
            let minor = tick_length(rect, orientation, TickKind::Minor);
            let medium = tick_length(rect, orientation, TickKind::Medium);
            let major = tick_length(rect, orientation, TickKind::Major);
            assert!(minor < medium && medium < major);
        }
    }

    #[test]
    fn horizontal_labels_have_outer_padding_and_a_gap_before_major_ticks() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, HORIZONTAL_RULER_HEIGHT));
        let label_top = rect.top() + HORIZONTAL_LABEL_OUTER_PADDING;
        let label_bottom = label_top + RULER_LABEL_SIZE;
        let major_tick_top = rect.bottom() - tick_length(rect, RulerOrientation::Horizontal, TickKind::Major);
        assert!(label_top > rect.top());
        assert!(major_tick_top - label_bottom >= HORIZONTAL_LABEL_TICK_GAP);
    }

    #[test]
    fn vertical_labels_are_centred_above_major_ticks_without_overlap() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VERTICAL_RULER_WIDTH, 300.0));
        let at = 100.0;
        assert!(vertical_label_fits(rect, at, "8", tick_length(rect, RulerOrientation::Vertical, TickKind::Major)));
        let label_bottom = at - VERTICAL_LABEL_ABOVE_TICK;
        assert!(label_bottom < at);
        assert_eq!(rect.center().x, VERTICAL_RULER_WIDTH * 0.5);
    }

    #[test]
    fn horizontal_labels_begin_after_their_major_tick() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, HORIZONTAL_RULER_HEIGHT));
        assert!(horizontal_label_fits(rect, 72.0, "1"));
        assert!(!horizontal_label_fits(rect, rect.right() - 2.0, "8"));
    }

    #[test]
    fn inch_ticks_use_binary_fractions_and_reference_label_cadence() {
        let at_100 = choose_tick_spec(Unit::Inches, 96.0).unwrap();
        assert_eq!(at_100.minor_interval_units, 1.0 / 8.0);
        assert_eq!((at_100.medium_every, at_100.major_every, at_100.label_every), (4, 8, 8));

        let zoomed_out = choose_tick_spec(Unit::Inches, 12.0).unwrap();
        assert_eq!(zoomed_out.minor_interval_units, 1.0);
        assert_eq!(zoomed_out.label_every, 8);
    }

    #[test]
    fn inch_labels_show_magnitudes_on_both_sides_of_zero() {
        assert_eq!(format_label(Unit::Inches, -8.0), "8");
        assert_eq!(format_label(Unit::Inches, 8.0), "8");
        assert_eq!(format_label(Unit::Inches, -0.0), "0");
    }

    #[test]
    fn edge_labels_are_suppressed_instead_of_clipped() {
        let horizontal = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, HORIZONTAL_RULER_HEIGHT));
        assert!(horizontal_label_fits(horizontal, horizontal.left(), "32"));
        assert!(horizontal_label_fits(horizontal, horizontal.center().x, "32"));
        assert!(!horizontal_label_fits(horizontal, horizontal.right(), "32"));

        let vertical = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VERTICAL_RULER_WIDTH, 100.0));
        assert!(!vertical_label_fits(vertical, vertical.top(), "8", 6.0));
        assert!(vertical_label_fits(vertical, vertical.center().y, "8", 6.0));
    }

    #[test]
    fn pointer_markers_point_toward_the_canvas_edge() {
        let horizontal = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, HORIZONTAL_RULER_HEIGHT));
        let h = pointer_marker_points(RulerOrientation::Horizontal, horizontal, 100.0);
        assert_eq!(h[2], egui::pos2(100.0, horizontal.bottom() - 7.0));
        assert_eq!(h[0].y, horizontal.bottom() - 1.0);

        let vertical = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VERTICAL_RULER_WIDTH, 300.0));
        let v = pointer_marker_points(RulerOrientation::Vertical, vertical, 100.0);
        assert_eq!(v[2], egui::pos2(vertical.right() - 7.0, 100.0));
        assert_eq!(v[0].x, vertical.right() - 1.0);
    }

    #[test]
    fn both_themes_paint_two_rulers_and_the_corner_in_an_isolated_harness() {
        for kind in [crate::theme::ThemeKind::Light, crate::theme::ThemeKind::Dark] {
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::theme::font_definitions());
            crate::theme::apply(&ctx, kind);
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let corner = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VERTICAL_RULER_WIDTH, HORIZONTAL_RULER_HEIGHT));
                let horizontal = egui::Rect::from_min_size(egui::pos2(VERTICAL_RULER_WIDTH, 0.0), egui::vec2(400.0, HORIZONTAL_RULER_HEIGHT));
                let vertical = egui::Rect::from_min_size(egui::pos2(0.0, HORIZONTAL_RULER_HEIGHT), egui::vec2(VERTICAL_RULER_WIDTH, 400.0));
                paint_corner(ui, corner, Unit::Points);
                paint_ruler(ui, paint_input(RulerOrientation::Horizontal, horizontal));
                paint_ruler(ui, paint_input(RulerOrientation::Vertical, vertical));
            });
            assert!(output.shapes.len() > 20, "{kind:?} ruler harness produced too few shapes");
            output.drop_without_applying_deltas();
        }
    }
}
