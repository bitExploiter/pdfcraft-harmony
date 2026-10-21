//! Small custom widgets built on the design tokens.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, vec2};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons};

/// Horizontal and vertical inset of a single-line field.
const FIELD_MARGIN: egui::Margin = egui::Margin::symmetric(10, 6);
/// Outer height of a single-line field, including its inset.
const FIELD_MIN_HEIGHT: f32 = 30.0;
/// Padding inside every modal.
const DIALOG_PAD_X: i8 = 20;
const DIALOG_PAD_Y: i8 = 16;

/// How long the floating bars and the tab strip stay fully opaque after they were last hovered or
/// used, in seconds, unless the preferences say otherwise ([`IdleChrome`]).
pub const IDLE_AFTER: f64 = 5.0;
/// The fade timers Preferences offers, in seconds.
pub const FADE_CHOICES: [u8; 8] = [1, 2, 3, 4, 5, 8, 10, 15];
/// How far idle chrome fades: faint enough to leave the page alone, still there to find.
pub const IDLE_OPACITY: f32 = 0.3;
/// How long after their last use the floating bars slide away, unless the preferences say
/// otherwise.
pub const HIDE_AFTER: f64 = 8.0;
/// The auto-hide timers Preferences offers, in seconds.
pub const HIDE_CHOICES: [u8; 9] = [2, 3, 4, 5, 8, 10, 15, 20, 30];
/// How far around where a hidden bar belongs the pointer brings it back: past the window's edge,
/// since the bars float 14 in from it.
const REVEAL_REACH: f32 = 28.0;

/// The preferences behind the chrome's idle behaviour, and whether something it serves is open,
/// set each frame before the chrome is drawn ([`set_idle_chrome`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IdleChrome {
    /// Whether the chrome fades when idle.
    pub fades: bool,
    /// Seconds the chrome stays opaque after its last use.
    pub fade_after: f64,
    /// Whether the floating bars slide out of the window when idle.
    pub auto_hide: bool,
    /// Seconds the bars stay in place after their last use.
    pub hide_after: f64,
    /// Something the chrome belongs with is open (a dialog, a panel of tools): it stays awake.
    pub held: bool,
}

impl Default for IdleChrome {
    fn default() -> Self {
        IdleChrome { fades: true, fade_after: IDLE_AFTER, auto_hide: true, hide_after: HIDE_AFTER, held: false }
    }
}

/// Use `chrome` for the idle chrome drawn from now on.
pub fn set_idle_chrome(ctx: &egui::Context, chrome: IdleChrome) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("idle-chrome"), chrome));
}

fn idle_chrome(ctx: &egui::Context) -> IdleChrome {
    ctx.data(|d| d.get_temp::<IdleChrome>(egui::Id::new("idle-chrome"))).unwrap_or_default()
}

/// How far the idle chrome `id` is shown, from 0 (faded) to 1 (in use), easing between them; the
/// fill of a bar that turns clear when idle follows it. See [`idle`].
pub fn idle_shown(ctx: &egui::Context, id: egui::Id, busy: bool) -> f32 {
    idle(ctx, id, busy, false).shown
}

/// Where idle chrome stands this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Idle {
    /// The opacity to draw it at: faded to [`IDLE_OPACITY`] when idle.
    pub opacity: f32,
    /// From 0 (faded) to 1 (in use).
    pub shown: f32,
    /// From 0 (in place) to 1 (slid out of the window); always 0 for chrome that doesn't hide.
    pub hidden: f32,
}

/// The idle state of the chrome `id`, which fades when idle. The rect it last recorded with
/// [`note_idle_rect`] is what counts as hovering it. `busy` keeps it awake while something of its
/// own is in use (a focused field, a dragged tab); an open menu or popup anywhere, or what
/// [`IdleChrome::held`] says is open, does too.
///
/// A bar that `hides` also slides out of the window once idle for [`IdleChrome::hide_after`],
/// while auto-hide is on, and the pointer coming near where it belongs brings it back.
pub fn idle(ctx: &egui::Context, id: egui::Id, busy: bool, hides: bool) -> Idle {
    // Each window's bars have their own clock.
    let id = id.with(ctx.viewport_id());
    let chrome = idle_chrome(ctx);
    // A timer that is not a sensible number of seconds falls back to the default.
    let seconds = |v: f64, default: f64| if v.is_finite() { v.clamp(0.5, 600.0) } else { default };
    let fade_after = chrome.fades.then(|| seconds(chrome.fade_after, IDLE_AFTER));
    let hide_after = (hides && chrome.auto_hide).then(|| seconds(chrome.hide_after, HIDE_AFTER));
    let now = ctx.input(|i| i.time);
    let rect = ctx.data(|d| d.get_temp::<Rect>(id.with("idle-rect")));
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let reach = if hide_after.is_some() { vec2(REVEAL_REACH, REVEAL_REACH / 2.0) } else { egui::Vec2::ZERO };
    let hovered = matches!((rect, pointer), (Some(r), Some(p)) if r.expand2(reach).contains(p));
    let key = id.with("idle-since");
    let since = if hovered || busy || chrome.held || egui::Popup::is_any_open(ctx) {
        ctx.data_mut(|d| d.insert_temp(key, now));
        now
    } else {
        ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(key, || now))
    };
    let elapsed = (now - since).max(0.0);
    let awake = fade_after.is_none_or(|at| elapsed < at);
    let stays = hide_after.is_none_or(|at| elapsed < at);
    // Wake once more when the bar is next due to fade or slide away, even if nothing else repaints.
    if let Some(at) = [fade_after, hide_after].into_iter().flatten().filter(|at| elapsed < *at).reduce(f64::min) {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((at - elapsed).clamp(0.0, at) + 0.05));
    }
    let shown = ctx.animate_bool_with_time(id.with("idle-fade"), awake, 0.4);
    let hidden = 1.0 - ctx.animate_bool_with_time(id.with("idle-hide"), stays, 0.25);
    Idle { opacity: IDLE_OPACITY + (1.0 - IDLE_OPACITY) * shown, shown, hidden }
}

/// Where the bar `id` belongs this frame (in place, even while slid away), for [`idle`] to tell
/// when it is hovered.
pub fn note_idle_rect(ctx: &egui::Context, id: egui::Id, rect: Rect) {
    // The window's id first: `viewport_id` must not be asked inside `data_mut`'s lock.
    let key = id.with(ctx.viewport_id()).with("idle-rect");
    ctx.data_mut(|d| d.insert_temp(key, rect));
}

/// Where the bar `id` belonged when this window last drew it ([`note_idle_rect`]).
pub fn idle_rect(ctx: &egui::Context, id: egui::Id) -> Option<Rect> {
    let key = id.with(ctx.viewport_id()).with("idle-rect");
    ctx.data(|d| d.get_temp::<Rect>(key))
}

/// The card a floating bar sits on: the left tool bar and the right bar.
pub fn floating_frame(t: &Tokens) -> egui::Frame {
    egui::Frame::NONE
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(12))
        .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 100 } else { 28 }) })
        .inner_margin(egui::Margin::same(6))
}

fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    let t = Tokens::get(ctx);
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(DIALOG_PAD_X, DIALOG_PAD_Y))
        .shadow(ctx.global_style().visuals.window_shadow)
}

/// A modal with the card fill, a 10-point corner and room around its contents.
pub fn modal(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id)).frame(dialog_frame(ctx))
}

/// Show [`modal`], outline its controls, and remember the card rect for layout checks.
pub fn show_modal<R>(ctx: &egui::Context, id: &str, content: impl FnOnce(&mut egui::Ui) -> R) -> egui::ModalResponse<R> {
    let stored = egui::Id::new(id);
    modal(ctx, id).show(ctx, |ui| {
        begin_dialog(ui);
        let inner = content(ui);
        // The frame's stroke sits outside this content rect; the stored card is the fill.
        let card = ui.min_rect().expand2(egui::vec2(f32::from(DIALOG_PAD_X), f32::from(DIALOG_PAD_Y)));
        ui.ctx().data_mut(|d| d.insert_temp(stored.with("card"), card));
        inner
    })
}

/// Outlined controls at a shared 30-point height, with a focus stroke and 4-point corners.
///
/// Dialog fills match the theme's field colour, so radio buttons, check boxes, combo boxes
/// and number fields need the border or they disappear into the card.
pub fn begin_dialog(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    w.inactive.weak_bg_fill = t.field;
    // Slider rails and check-box interiors use the plain fill.
    w.inactive.bg_fill = t.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, t.text_muted);
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = CornerRadius::same(4);
    }
    ui.visuals_mut().selection.stroke = Stroke::new(1.5, t.accent);
    ui.spacing_mut().interact_size.y = FIELD_MIN_HEIGHT;
}

/// A single-line field tall enough to read and to click.
pub fn line(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(FIELD_MARGIN).min_size(vec2(0.0, FIELD_MIN_HEIGHT))
}

/// A multiline field with the same horizontal inset as [`line`].
pub fn block(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::multiline(text).margin(egui::Margin::symmetric(10, 6))
}

/// One line that stays within `max_width`, cut on a character boundary with an ellipsis.
/// Only the first 160 characters are measured, so a huge name cannot lay out thousands of candidates.
pub(crate) fn fit_line(text: &str, max_width: f32, mut width_of: impl FnMut(&str) -> f32) -> String {
    if !max_width.is_finite() || max_width <= 0.0 {
        return String::new();
    }
    let mut s: String = text.chars().take(160).collect();
    if s.len() == text.len() && width_of(text) <= max_width {
        return text.to_owned();
    }
    loop {
        s.pop();
        if s.is_empty() {
            return if width_of("\u{2026}") <= max_width { "\u{2026}".into() } else { String::new() };
        }
        let candidate = format!("{s}\u{2026}");
        if width_of(&candidate) <= max_width {
            return candidate;
        }
    }
}

pub(crate) fn text_width(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x)
}

/// Draw `text` from `pos` (left, vertically centered) so it never runs past `max_width`.
pub(crate) fn paint_left(ui: &egui::Ui, pos: egui::Pos2, text: &str, font: egui::FontId, color: Color32, max_width: f32) {
    let shown = fit_line(text, max_width, |s| text_width(ui, s, &font));
    let clip = Rect::from_min_size(egui::pos2(pos.x, pos.y - 14.0), vec2(max_width.max(0.0), 28.0));
    ui.painter().with_clip_rect(clip).text(pos, Align2::LEFT_CENTER, shown, font, color);
}

/// A mode-bar tab: text with an underline when active.
pub fn mode_tab(ui: &mut egui::Ui, label: &str, active: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = if active { theme::semibold(13.5) } else { theme::medium(13.5) };
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 22.0, 48.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label));
    let press = theme::Press::track(ui, &resp);
    let well = rect.shrink2(vec2(2.0, 9.0));
    if active {
        if press.down > 0.01 {
            ui.painter().rect_filled(well, CornerRadius::same(6), t.pressed.gamma_multiply(press.down));
        }
    } else {
        press.wash(ui, well, 6, false);
    }
    let body = rect.translate(press.offset());
    ui.painter().text(body.center(), Align2::CENTER_CENTER, label, font, if active { t.text } else { t.text_muted });
    if active {
        let r = Rect::from_min_max(rect.left_bottom() + vec2(10.0, -2.0), rect.right_bottom() - vec2(10.0, 0.0));
        ui.painter().rect_filled(r, CornerRadius { nw: 1, ne: 1, sw: 0, se: 0 }, t.accent);
    }
    theme::hand(resp)
}

/// Rounded pill button; `primary` fills with the accent.
pub fn pill_button(ui: &mut egui::Ui, label: &str, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 26.0, 28.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let press = theme::Press::track(ui, &resp);
    let (fill, stroke, text) = if primary {
        (press.fill(t.accent, t.accent_text, t.accent.lerp_to_gamma(Color32::BLACK, 0.22)), Stroke::NONE, Color32::WHITE)
    } else {
        (press.fill(t.card, t.hover, t.pressed), Stroke::new(1.0, t.border), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(14), fill, stroke, egui::StrokeKind::Inside);
    ui.painter().text(rect.translate(press.offset()).center(), Align2::CENTER_CENTER, label, font, text);
    theme::hand(resp)
}

/// Icon + label, transparent until hovered.
pub fn ghost_button(ui: &mut egui::Ui, icon: &str, label: &str) -> Response {
    ghost_button_sized(ui, icon, label, 30.0)
}

/// [`ghost_button`] at a given height (the macOS title bar is shorter than the tab strip elsewhere).
pub fn ghost_button_sized(ui: &mut egui::Ui, icon: &str, label: &str, height: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(13.0);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 38.0, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 6, false);
    let body = rect.translate(press.offset());
    icons::paint(ui, Rect::from_min_size(body.min + vec2(6.0, (height - 18.0) / 2.0), vec2(18.0, 18.0)), icon, 17.0, t.icon);
    ui.painter().text(body.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    theme::hand(resp)
}

pub fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> Response {
    ui.add(egui::Button::new(label).shortcut_text(shortcut))
}

/// Corner radius of a checkbox's box. With the widgets' radius (`Tokens::radius`, 6) egui's 14 px
/// box is a circle, the shape of a radio button, and View ▸ Page display has both.
const CHECKBOX_RADIUS: u8 = 3;

/// A checkbox whose box is an outlined rounded square, never mistaken for a radio button. Every
/// checkbox goes through here: clippy.toml disallows `ui.checkbox` and `egui::Checkbox`.
pub fn checkbox<'a>(checked: &'a mut bool, atoms: impl egui::IntoAtoms<'a>) -> impl egui::Widget + 'a {
    #[expect(clippy::disallowed_methods, reason = "the one place that styles the box")]
    let widget = egui::Checkbox::new(checked, atoms);
    toggle(widget, Some(CHECKBOX_RADIUS))
}

/// [`checkbox`] without a label: the selection box on Organize Pages thumbnails.
pub fn checkbox_without_text(checked: &mut bool) -> impl egui::Widget + '_ {
    #[expect(clippy::disallowed_methods, reason = "the one place that styles the box")]
    let widget = egui::Checkbox::without_text(checked);
    toggle(widget, Some(CHECKBOX_RADIUS))
}

/// A radio button with an outline and a fill, like [`checkbox`]. Every radio button goes through
/// here or [`radio_value`]: clippy.toml disallows `ui.radio`, `ui.radio_value` and
/// `egui::RadioButton`.
pub fn radio<'a>(selected: bool, atoms: impl egui::IntoAtoms<'a>) -> impl egui::Widget + 'a {
    #[expect(clippy::disallowed_methods, reason = "the one place that styles the circle")]
    let widget = egui::RadioButton::new(selected, atoms);
    toggle(widget, None)
}

/// [`radio`] selected while `*current == alternative`; a click selects it (`Ui::radio_value`).
pub fn radio_value<'a, V: PartialEq>(ui: &mut egui::Ui, current: &mut V, alternative: V, atoms: impl egui::IntoAtoms<'a>) -> Response {
    let mut response = ui.add(radio(*current == alternative, atoms));
    if response.clicked() && *current != alternative {
        *current = alternative;
        response.mark_changed();
    }
    response
}

/// The box of a checkbox or the circle of a radio button. egui draws it with the widgets' visuals
/// and has no style of its own for it, so it is set in a scope around the widget, keeping what
/// the caller set (dialogs and the search panel already outline theirs):
/// - an outline, darker on hover;
/// - on the light theme, the plain fill (`Tokens::hover`), as dialogs have: the field colour is
///   the panels' white, so the box would vanish;
/// - for a checkbox, a corner radius small enough to keep the box square.
fn toggle<'a>(widget: impl egui::Widget + 'a, radius: Option<u8>) -> impl egui::Widget + 'a {
    move |ui: &mut egui::Ui| {
        ui.scope(|ui| {
            let t = Tokens::get(ui.ctx());
            let w = &mut ui.visuals_mut().widgets;
            if let Some(radius) = radius {
                for v in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                    v.corner_radius = CornerRadius::same(radius);
                }
            }
            if !t.dark() && w.inactive.bg_fill == t.field {
                w.inactive.bg_fill = t.hover;
            }
            if w.inactive.bg_stroke.is_empty() {
                w.inactive.bg_stroke = Stroke::new(1.0, t.border);
            }
            if w.hovered.bg_stroke.is_empty() {
                w.hovered.bg_stroke = Stroke::new(1.0, t.text_muted);
            }
            ui.add(widget)
        })
        .inner
    }
}

/// A tool panel's section heading, in small capitals like [`section_title`], with `trailing` (a
/// link to an action) at the right of the same line.
pub fn section_header(ui: &mut egui::Ui, text: &str, trailing: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(12.0);
    heading_row(ui, &crate::i18n::upper_label(text), trailing);
    ui.add_space(4.0);
}

/// The line of a [`section_header`] alone, for a heading already in the interface language and
/// in capitals ([`crate::i18n::upper_label`]).
pub fn heading_row(ui: &mut egui::Ui, upper: &str, trailing: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(upper).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), trailing);
    });
}

/// Choices side by side as one row of equal buttons, `width` wide in all, the chosen one outlined
/// in the accent colour (Pages to Print, Page Sizing). Each is its label and whether it is
/// chosen; returns the index clicked.
pub fn choice_row(ui: &mut egui::Ui, choices: &[(&str, bool)], width: f32) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let gap = 6.0;
    let n = choices.len().max(1) as f32;
    let w = ((width - gap * (n - 1.0)) / n).max(40.0);
    let font = theme::regular(12.5);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, (label, on)) in choices.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
            let press = theme::Press::track(ui, &resp);
            let (fill, stroke) = if *on { (t.accent_soft, t.accent) } else { (press.fill(t.field, t.hover, t.pressed), t.border) };
            ui.painter().rect(rect, CornerRadius::same(7), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
            let shown = fit_line(label, (w - 12.0).max(0.0), |s| text_width(ui, s, &font));
            let cut = shown != *label;
            ui.painter().text(rect.center() + press.offset(), Align2::CENTER_CENTER, shown, font.clone(), if *on { t.accent_text } else { t.text });
            let name = (*label).to_owned();
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), *on, name.clone()));
            // The whole label on hover, when it had to be cut to fit.
            let resp = if cut { resp.on_hover_text(name.as_str()) } else { resp };
            if theme::hand(resp).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// A link-styled action at the end of a [`section_header`] line.
pub fn header_link(ui: &mut egui::Ui, text: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let resp =
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::medium(12.0)).color(t.accent_text)).selectable(false).sense(Sense::click()));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    theme::hand(resp)
}

/// A notice at the top of a tool panel: `icon` on a tinted tile, a `title` line and `body`.
pub fn callout(ui: &mut egui::Ui, icon: &str, title: &str, body: impl Into<egui::WidgetText>) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE
        .fill(t.accent_soft.gamma_multiply(if t.dark() { 0.7 } else { 0.8 }))
        .stroke(Stroke::new(1.0, t.accent.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (tile, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                ui.painter().rect_filled(tile, CornerRadius::same(7), t.accent.gamma_multiply(0.22));
                icons::paint(ui, Rect::from_center_size(tile.center(), vec2(16.0, 16.0)), icon, 15.0, t.accent_text);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(egui::RichText::new(title).font(theme::semibold(13.0)).color(t.text));
                    ui.add(egui::Label::new(body).wrap());
                });
            });
        });
}

/// A quiet how-to line in a bordered box with an info icon.
pub fn hint_box(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(10, 8)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(ui, r, "info", 14.0, t.text_faint);
                ui.add(egui::Label::new(egui::RichText::new(text).font(theme::regular(12.0)).color(t.text_muted)).wrap());
            });
        },
    );
}

/// Icon toggles joined in one rounded group (Bold, Italic, Underline; the alignments). Each item
/// is its icon, its name and whether it is on; returns the index clicked. An `accent` group fills
/// the buttons that are on with the accent colour; otherwise they get a quiet wash.
pub fn toggle_group(ui: &mut egui::Ui, items: &[(&str, &str, bool)], accent: bool) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let mut clicked = None;
    egui::Frame::NONE.fill(t.field).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(3)).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, (icon, name, on)) in items.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 26.0), Sense::click());
                    let press = theme::Press::track(ui, &resp);
                    let fill = match (*on, accent) {
                        (true, true) => {
                            press.fill(t.accent, t.accent.lerp_to_gamma(Color32::WHITE, 0.1), t.accent.lerp_to_gamma(Color32::BLACK, 0.1))
                        }
                        (true, false) => press.fill(t.pressed, t.pressed, t.selected),
                        (false, _) => press.fill(Color32::TRANSPARENT, t.hover, t.pressed),
                    };
                    if fill.a() > 0 {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
                    }
                    let ink = if *on && accent {
                        Color32::WHITE
                    } else if *on {
                        t.text
                    } else {
                        t.icon
                    };
                    icons::paint(ui, Rect::from_center_size(rect.center() + press.offset(), vec2(16.0, 16.0)), icon, 15.0, ink);
                    let name = (*name).to_owned();
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, *on, name.clone()));
                    if theme::hand(resp).on_hover_text(name.as_str()).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        },
    );
    clicked
}

/// A labelled value on a card the colour of a field: `icon` and `label` at the left, the control
/// `value` adds at the right (it is handed the label's id, for `labelled_by`).
pub fn value_card(ui: &mut egui::Ui, icon: Option<&str>, label: &str, value: impl FnOnce(&mut egui::Ui, egui::Id)) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_min_height(24.0);
                if let Some(icon) = icon {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    icons::paint(ui, r, icon, 14.0, t.text_faint);
                }
                let l = ui.add(egui::Label::new(egui::RichText::new(label).font(theme::regular(12.5)).color(t.text_muted)).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| value(ui, l.id));
            });
        });
}

/// A small status pill (a tool's Ready): `text` in `ink` on `fill`.
pub fn pill(ui: &egui::Ui, right_center: egui::Pos2, text: &str, ink: Color32, fill: Color32) -> Rect {
    let font = theme::semibold(10.0);
    let w = text_width(ui, text, &font);
    let r = Rect::from_center_size(right_center - vec2(w / 2.0 + 8.0, 0.0), vec2(w + 14.0, 18.0));
    ui.painter().rect(r, CornerRadius::same(5), fill, Stroke::new(1.0, ink.gamma_multiply(0.35)), egui::StrokeKind::Inside);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, font, ink);
    r
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(10.0);
    // Section titles across every panel go through here, so one translation point covers them.
    ui.label(egui::RichText::new(crate::i18n::upper_label(text)).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
    ui.add_space(2.0);
}

/// Paint one line of `text` left-aligned and vertically centred on `pos`, cut with "…" so it is
/// at most `max_width` wide: a long file name or path stays inside its row (#425).
pub fn row_text(ui: &egui::Ui, pos: egui::Pos2, text: impl Into<String>, font: egui::FontId, color: Color32, max_width: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    let galley = ui.painter().layout_job(job);
    let rect = Align2::LEFT_CENTER.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
}

/// Text wrapped to the available width, for text people write (comments, replies). A paragraph
/// in a right-to-left script (Persian, Arabic, Hebrew) reads right to left and sits against the
/// right edge ([`crate::bidi::wrap`]); text without one is an ordinary wrapping label.
/// `truncate` keeps the first line only, with an ellipsis where the text goes on.
pub fn wrapped_text(ui: &mut egui::Ui, text: &str, font: egui::FontId, color: Color32, truncate: bool) -> Response {
    if !crate::bidi::has_rtl(text) {
        let mut label = egui::Label::new(egui::RichText::new(text).font(font).color(color)).wrap();
        if truncate {
            label = label.truncate();
        }
        return ui.add(label);
    }
    let width = ui.available_width().max(1.0);
    let lines = wrapped_lines(ui, text, &font, width, truncate);
    let galleys: Vec<_> = lines.iter().map(|line| (line.rtl, ui.painter().layout_no_wrap(line.text.clone(), font.clone(), color))).collect();
    let height = galleys.iter().map(|(_, g)| g.size().y).sum();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    // Painted text only: the accessibility name keeps the logical order.
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    let mut y = rect.top();
    for (rtl, galley) in galleys {
        let x = if rtl { rect.right() - galley.size().x } else { rect.left() };
        let h = galley.size().y;
        ui.painter().galley(egui::pos2(x, y), galley, color);
        y += h;
    }
    resp
}

const ELLIPSIS: &str = "\u{2026}";

/// Wrapped right-to-left text kept between frames. Wrapping measures many candidate lines, so
/// re-wrapping every comment on every frame would make a panel of long comments slow; the
/// result depends only on the text, font, width, scale and collapsed state, so it is looked up.
#[derive(Clone, Default)]
struct WrapCache {
    lines: std::collections::HashMap<u64, std::sync::Arc<[crate::bidi::Line]>>,
}

/// How many wrapped texts the cache holds before it starts over (each costs its text's size).
const WRAP_CACHE_MAX: usize = 256;

/// A collapsed comment shows one line, so no more than this many characters are wrapped for it:
/// a long comment doesn't cost a full wrap just to show its start.
const COLLAPSED_WRAP_CHARS: usize = 1024;

/// The lines `wrapped_text` paints for a right-to-left `text`, from the cache when the same text
/// was wrapped the same way before.
fn wrapped_lines(ui: &egui::Ui, text: &str, font: &egui::FontId, width: f32, truncate: bool) -> std::sync::Arc<[crate::bidi::Line]> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (text, font, width.to_bits(), ui.ctx().pixels_per_point().to_bits(), truncate).hash(&mut hasher);
    let key = hasher.finish();
    let cache_id = egui::Id::new("pdfcraft-rtl-wrap-cache");
    if let Some(hit) = ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<WrapCache>(cache_id).lines.get(&key).cloned()) {
        return hit;
    }
    // Collapsed: only the start is wrapped (cut at a char boundary, so the slice can't fail).
    let (measured, cut) = match text.char_indices().nth(COLLAPSED_WRAP_CHARS) {
        Some((at, _)) if truncate => (text.get(..at).unwrap_or(text), true),
        _ => (text, false),
    };
    let measure = |f: &mut egui::epaint::text::FontsView<'_>, s: &str| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    let mut lines = ui.fonts_mut(|f| {
        let room = if truncate { width - measure(f, ELLIPSIS) } else { width };
        crate::bidi::wrap(measured, |s| measure(f, s) <= room)
    });
    if truncate && (lines.len() > 1 || cut) {
        lines.truncate(1);
        if let Some(line) = lines.first_mut() {
            // The ellipsis goes where the line's reading ends: the left of a right-to-left line.
            if line.rtl {
                line.text.insert_str(0, ELLIPSIS);
            } else {
                line.text.push_str(ELLIPSIS);
            }
        }
    }
    let lines: std::sync::Arc<[crate::bidi::Line]> = lines.into();
    ui.ctx().data_mut(|d| {
        let cache = &mut d.get_temp_mut_or_default::<WrapCache>(cache_id).lines;
        if cache.len() >= WRAP_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, lines.clone());
    });
    lines
}

/// Transient message at the bottom centre.
pub fn toast(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((msg, start)) = app.toast.clone() else { return };
    let now = ctx.input(|i| i.time);
    let start = if start == 0.0 { now } else { start };
    app.toast = Some((msg.clone(), start));
    if now - start > 3.5 {
        app.toast = None;
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    // Above the progress notice while a job runs.
    let card = if app.progress_notice.is_some() { ctx.data(|d| d.get_temp::<f32>(egui::Id::new(PROGRESS_HEIGHT))).unwrap_or(0.0) } else { 0.0 };
    let lift = if card > 0.0 { 28.0 + card + 10.0 } else { 28.0 };
    egui::Area::new(egui::Id::new("toast"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(screen.center_bottom() - vec2(0.0, lift))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(if t.dark() { Color32::from_rgb(0xEC, 0xEC, 0xEF) } else { Color32::from_rgb(0x2A, 0x2A, 0x2F) })
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    // An Area remembers its previous content width. A short notice
                    // must not force the next validation message into a narrow column.
                    // Allow natural short labels; wrap longer ones within the viewport,
                    // including the frame's 32 points and 16-point outside margins.
                    ui.set_max_width((screen.width() - 64.0).clamp(1.0, 560.0));
                    ui.add(
                        egui::Label::new(egui::RichText::new(msg).color(if t.dark() { Color32::from_rgb(0x22, 0x22, 0x26) } else { Color32::WHITE }))
                            .wrap(),
                    );
                });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// A background job's progress, shown at the bottom centre until the job is done.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressNotice {
    /// What is happening ("Optimizing… image 3 of 40").
    pub label: String,
    /// From 0 to 1.
    pub fraction: f32,
    /// Show a Cancel button.
    pub cancellable: bool,
}

/// Where the progress notice keeps its height this frame, for placing the toast above it.
const PROGRESS_HEIGHT: &str = "progress-notice-height";

/// Draw [`PdfCraftApp::progress_notice`]: the label, a bar that eases towards the fraction with
/// a light sweeping across it, the percentage and Cancel. Returns true when Cancel was clicked.
/// With animation effects off ([`crate::motion`]) the bar steps to the fraction and has no sweep.
pub fn progress_notice(app: &PdfCraftApp, ctx: &egui::Context) -> bool {
    let Some(p) = &app.progress_notice else { return false };
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    // The same inverted card as the toast.
    let (fill, text) = if t.dark() {
        (Color32::from_rgb(0xEC, 0xEC, 0xEF), Color32::from_rgb(0x22, 0x22, 0x26))
    } else {
        (Color32::from_rgb(0x2A, 0x2A, 0x2F), Color32::WHITE)
    };
    let fraction = if p.fraction.is_finite() { p.fraction.clamp(0.0, 1.0) } else { 0.0 };
    let animated = crate::motion::animated(ctx);
    let shown = ctx.animate_value_with_time(egui::Id::new("progress-notice-bar"), fraction, if animated { 0.3 } else { 0.0 });
    let now = ctx.input(|i| i.time);
    let mut cancel = false;
    let area = egui::Area::new(egui::Id::new("progress-notice"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(screen.center_bottom() - vec2(0.0, 28.0))
        .show(ctx, |ui| {
            egui::Frame::NONE.fill(fill).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                let width = 340.0_f32.min(screen.width() - 64.0).max(160.0);
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&p.label).color(text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(format!("{:.0}%", shown * 100.0)).font(theme::medium(12.0)).color(text.gamma_multiply(0.7)));
                    });
                });
                ui.add_space(6.0);
                let (track, resp) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
                // The label above names the job; the bar carries the percentage.
                resp.widget_info(|| {
                    let mut info = egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator);
                    info.value = Some(f64::from(fraction) * 100.0);
                    info
                });
                let painter = ui.painter();
                painter.rect_filled(track, CornerRadius::same(3), text.gamma_multiply(0.18));
                let done = Rect::from_min_size(track.min, vec2(track.width() * shown, track.height()));
                if done.width() > 0.5 {
                    painter.rect_filled(done, CornerRadius::same(3), t.accent);
                    // A soft light sweeping along the filled part says the job is alive between
                    // updates.
                    if animated {
                        let band = 48.0;
                        let x = done.left() - band + ((now * 160.0) % f64::from(done.width() + band)) as f32;
                        let clip = painter.with_clip_rect(done);
                        for (i, a) in [0.10_f32, 0.22, 0.10].into_iter().enumerate() {
                            let r = Rect::from_min_size(egui::pos2(x + i as f32 * band / 3.0, done.top()), vec2(band / 3.0, done.height()));
                            clip.rect_filled(r, CornerRadius::ZERO, Color32::WHITE.gamma_multiply(a));
                        }
                    }
                }
                if p.cancellable {
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b = egui::Button::new(egui::RichText::new(tl!("Cancel")).font(theme::medium(12.5)).color(text)).frame(false);
                        cancel = ui.add(b).clicked();
                    });
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PROGRESS_HEIGHT), area.response.rect.height()));
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
    cancel
}

/// A brand image compiled into the app (assets/brand/, Design Kreativ's marks; not open source), as
/// a texture decoded once. `None` unless it is an 8-bit RGBA PNG, as the brand images are.
fn brand_texture(ctx: &egui::Context, name: &'static str, bytes: &'static [u8]) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("brand-texture", name));
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let pixels = buf.get(..info.buffer_size())?;
    let image = egui::ColorImage::from_rgba_unmultiplied([info.width as usize, info.height as usize], pixels);
    let texture = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

/// The Design Kreativ wordmark, `height` points tall: the light one on a dark theme.
pub fn brand_logo(ui: &mut egui::Ui, height: f32) -> Response {
    let (name, bytes): (&'static str, &'static [u8]) = if ui.visuals().dark_mode {
        ("design-kreativ-logo-light", include_bytes!("../../../assets/brand/design-kreativ-logo-light.png"))
    } else {
        ("design-kreativ-logo-dark", include_bytes!("../../../assets/brand/design-kreativ-logo-dark.png"))
    };
    brand_image(ui, name, bytes, height, crate::brand::ORGANISATION)
}

/// The Design Kreativ mark (the KreativPdf app icon), `size` points square.
pub fn brand_mark(ui: &mut egui::Ui, size: f32) -> Response {
    brand_image(ui, "design-kreativ-mark", include_bytes!("../../../assets/brand/design-kreativ-mark.png"), size, crate::brand::ORGANISATION)
}

/// Paint the Design Kreativ mark in `rect` (the tab strip's app icon).
pub fn paint_brand_mark(ui: &egui::Ui, rect: Rect) {
    if let Some(texture) = brand_texture(ui.ctx(), "design-kreativ-mark", include_bytes!("../../../assets/brand/design-kreativ-mark.png")) {
        ui.painter().image(texture.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// A brand image `height` points tall, keeping its proportions; an empty space of that height
/// when it can't be decoded.
fn brand_image(ui: &mut egui::Ui, name: &'static str, bytes: &'static [u8], height: f32, alt: &str) -> Response {
    match brand_texture(ui.ctx(), name, bytes) {
        Some(texture) => {
            let [w, h] = texture.size();
            let width = if h > 0 { height * w as f32 / h as f32 } else { height };
            ui.add(egui::Image::new((texture.id(), vec2(width, height))).alt_text(alt))
        }
        None => ui.allocate_response(vec2(height, height), Sense::hover()),
    }
}

/// [`icon_pill`] with a ▾ part at its end that opens a menu of related choices. Returns the
/// main button's response and the ▾'s (give it to `egui::Popup::menu`).
pub fn split_pill(ui: &mut egui::Ui, icon: &str, label: &str, more: &str) -> (Response, Response) {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, _) = ui.allocate_exact_size(vec2(w + 46.0 + 28.0, 30.0), Sense::hover());
    let (main_rect, more_rect) = rect.split_left_right_at_x(rect.right() - 28.0);
    let main = ui.interact(main_rect, ui.id().with(("split-pill", label)), Sense::click());
    let arrow = ui.interact(more_rect, ui.id().with(("split-pill-more", label)), Sense::click());
    main.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    arrow.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), more));
    let radius = CornerRadius::same(15);
    ui.painter().rect(rect, radius, t.card, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let left = CornerRadius { nw: 15, sw: 15, ne: 0, se: 0 };
    let right = CornerRadius { nw: 0, sw: 0, ne: 15, se: 15 };
    let main_press = theme::Press::track(ui, &main);
    let arrow_press = theme::Press::track(ui, &arrow);
    for (r, press, corners) in [(main_rect, main_press, left), (more_rect, arrow_press, right)] {
        let fill = press.fill(Color32::TRANSPARENT, t.hover, t.pressed);
        if fill.a() > 0 {
            ui.painter().rect_filled(r.shrink(1.2), corners, fill);
        }
    }
    let body = rect.translate(main_press.offset());
    let more_body = more_rect.translate(arrow_press.offset());
    ui.painter().vline(more_rect.left(), rect.y_range().shrink(7.0), Stroke::new(1.0, t.text_muted));
    crate::icons::paint(ui, Rect::from_min_size(body.min + vec2(12.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, t.text);
    ui.painter().text(body.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    crate::icons::paint(ui, Rect::from_center_size(more_body.center(), vec2(14.0, 14.0)), "chevron-down", 13.0, t.text);
    (theme::hand(main).on_hover_text(label), theme::hand(arrow).on_hover_text(more))
}

/// A pill button with an icon (primary = filled accent).
pub fn icon_pill(ui: &mut egui::Ui, icon: &str, label: &str, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 46.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let press = theme::Press::track(ui, &resp);
    let (fill, stroke, text) = if primary {
        (press.fill(t.accent, t.accent_text, t.accent.lerp_to_gamma(Color32::BLACK, 0.22)), Stroke::NONE, Color32::WHITE)
    } else {
        (press.fill(t.card, t.hover, t.pressed), Stroke::new(1.0, t.border), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(15), fill, stroke, egui::StrokeKind::Inside);
    let body = rect.translate(press.offset());
    crate::icons::paint(ui, Rect::from_min_size(body.min + vec2(12.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, text);
    ui.painter().text(body.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, font, text);
    theme::hand(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    /// Runs one frame at `time` seconds with the pointer at `pointer`, and returns the opacity the
    /// bar `id` gets in it.
    fn idle_frame(ctx: &egui::Context, time: f64, pointer: Option<Pos2>, busy: bool) -> f32 {
        let mut input = egui::RawInput { time: Some(time), ..Default::default() };
        input.events.push(match pointer {
            Some(p) => egui::Event::PointerMoved(p),
            None => egui::Event::PointerGone,
        });
        let mut opacity = 0.0;
        let id = egui::Id::new("idle-test-bar");
        let output = ctx.run_ui(input, |ui| {
            opacity = idle(ui.ctx(), id, busy, false).opacity;
            note_idle_rect(ui.ctx(), id, Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(40.0, 200.0)));
        });
        output.drop_without_applying_deltas();
        opacity
    }

    fn opaque(opacity: f32) -> bool {
        (opacity - 1.0).abs() < 1e-3
    }

    /// Floating chrome stays opaque for five seconds after its last use, then fades; hovering it
    /// brings it straight back, and so does anything of its own being in use.
    #[test]
    fn idle_chrome_fades_after_five_seconds_and_wakes_on_hover() {
        let ctx = egui::Context::default();
        let away = Some(Pos2::new(300.0, 300.0));
        let over = Some(Pos2::new(20.0, 100.0));
        assert!(opaque(idle_frame(&ctx, 0.0, away, false)), "opaque at first");
        assert!(opaque(idle_frame(&ctx, 4.9, away, false)), "still opaque before five seconds");
        // Past five seconds, and past the fade's own 0.4 s.
        idle_frame(&ctx, 5.1, away, false);
        let faded = idle_frame(&ctx, 6.0, away, false);
        assert!((faded - IDLE_OPACITY).abs() < 1e-3, "faded to {faded}");
        // Hovering wakes it (the fade back takes its 0.4 s too).
        idle_frame(&ctx, 6.1, over, false);
        let woken = idle_frame(&ctx, 7.0, over, false);
        assert!(opaque(woken), "woke to {woken}");
        // The clock starts again from the last hover.
        assert!(opaque(idle_frame(&ctx, 11.9, away, false)));
        idle_frame(&ctx, 12.1, away, false);
        assert!(idle_frame(&ctx, 13.0, away, false) < 0.5, "fades five seconds after the hover ended");
        // In use (a focused field, a dragged tab) counts as active even with the pointer away.
        idle_frame(&ctx, 13.1, None, true);
        assert!(opaque(idle_frame(&ctx, 14.0, None, true)));
    }

    /// One frame of a bar that hides, at `time` with the pointer at `pointer`, under `chrome`.
    fn hiding_frame(ctx: &egui::Context, time: f64, pointer: Option<Pos2>, chrome: IdleChrome) -> Idle {
        let mut input = egui::RawInput { time: Some(time), ..Default::default() };
        input.events.push(match pointer {
            Some(p) => egui::Event::PointerMoved(p),
            None => egui::Event::PointerGone,
        });
        let mut idle_now = Idle { opacity: 0.0, shown: 0.0, hidden: 0.0 };
        let id = egui::Id::new("hiding-test-bar");
        let output = ctx.run_ui(input, |ui| {
            set_idle_chrome(ui.ctx(), chrome);
            idle_now = idle(ui.ctx(), id, false, true);
            // The bar belongs 14 in from the window's left edge.
            note_idle_rect(ui.ctx(), id, Rect::from_min_size(Pos2::new(14.0, 14.0), vec2(44.0, 300.0)));
        });
        output.drop_without_applying_deltas();
        idle_now
    }

    /// The floating bars slide out of the window eight seconds after their last use; the pointer
    /// at the window's edge beside where they belong brings them back. With auto-hide off they
    /// only fade.
    #[test]
    fn idle_bars_slide_away_after_eight_seconds_and_come_back_near_their_place() {
        let ctx = egui::Context::default();
        let on = IdleChrome::default();
        let away = Some(Pos2::new(600.0, 300.0));
        assert_eq!(hiding_frame(&ctx, 0.0, away, on).hidden, 0.0);
        let faded = hiding_frame(&ctx, 7.9, away, on);
        assert!(faded.hidden < 1e-3 && faded.shown < 1e-3, "faded at five seconds, still in place: {faded:?}");
        hiding_frame(&ctx, 8.1, away, on);
        assert!(hiding_frame(&ctx, 9.0, away, on).hidden > 0.999, "slid away after eight seconds");
        // The window's very edge, level with the bar, is near enough.
        let edge = Some(Pos2::new(1.0, 100.0));
        hiding_frame(&ctx, 9.1, edge, on);
        let back = hiding_frame(&ctx, 10.0, edge, on);
        assert!(back.hidden < 1e-3 && (back.shown - 1.0).abs() < 1e-3, "back in place and opaque: {back:?}");
        // Off: it fades, and stays.
        let off = IdleChrome { auto_hide: false, ..on };
        hiding_frame(&ctx, 10.1, away, off);
        let stays = hiding_frame(&ctx, 30.0, away, off);
        assert!(stays.hidden < 1e-3 && stays.shown < 1e-3, "{stays:?}");
    }

    /// The fade and the slide each wait as long as their preference says, and either can be off.
    #[test]
    fn the_fade_and_the_slide_follow_their_own_timers() {
        let away = Some(Pos2::new(600.0, 300.0));
        let slow = IdleChrome { fade_after: 15.0, hide_after: 3.0, ..IdleChrome::default() };
        let ctx = egui::Context::default();
        hiding_frame(&ctx, 0.0, away, slow);
        hiding_frame(&ctx, 3.1, away, slow);
        let early = hiding_frame(&ctx, 4.0, away, slow);
        assert!((early.shown - 1.0).abs() < 1e-3 && early.hidden > 0.999, "slid away after three seconds, still opaque: {early:?}");
        hiding_frame(&ctx, 15.1, away, slow);
        assert!(hiding_frame(&ctx, 16.0, away, slow).shown < 1e-3, "faded after fifteen");
        let quick = IdleChrome { fade_after: 1.0, ..IdleChrome::default() };
        let ctx = egui::Context::default();
        hiding_frame(&ctx, 0.0, away, quick);
        hiding_frame(&ctx, 1.1, away, quick);
        let faded = hiding_frame(&ctx, 2.0, away, quick);
        assert!(faded.shown < 1e-3 && faded.hidden < 1e-3, "faded after one second, in place until eight: {faded:?}");
        // No fade: opaque until it slides away, and after.
        let clear = IdleChrome { fades: false, ..IdleChrome::default() };
        let ctx = egui::Context::default();
        hiding_frame(&ctx, 0.0, away, clear);
        hiding_frame(&ctx, 8.1, away, clear);
        let gone = hiding_frame(&ctx, 30.0, away, clear);
        assert!((gone.shown - 1.0).abs() < 1e-3 && gone.hidden > 0.999, "{gone:?}");
    }

    /// While something the chrome belongs with is open (a dialog, the tools panel), it stays awake
    /// and in place however long the pointer is away; once it closes, the clock starts.
    #[test]
    fn open_dialogs_and_panels_hold_the_chrome_awake() {
        let away = Some(Pos2::new(600.0, 300.0));
        let held = IdleChrome { held: true, ..IdleChrome::default() };
        let ctx = egui::Context::default();
        hiding_frame(&ctx, 0.0, away, held);
        let still = hiding_frame(&ctx, 60.0, away, held);
        assert!((still.shown - 1.0).abs() < 1e-3 && still.hidden < 1e-3, "{still:?}");
        let free = IdleChrome::default();
        assert!((hiding_frame(&ctx, 64.0, away, free).shown - 1.0).abs() < 1e-3, "four seconds after it closed");
        hiding_frame(&ctx, 68.1, away, free);
        let later = hiding_frame(&ctx, 69.0, away, free);
        assert!(later.shown < 1e-3 && later.hidden > 0.999, "{later:?}");
    }

    /// View ▸ Page display mixes radio buttons and checkboxes. With the theme's 6 px widget radius
    /// egui's 14 px checkbox box drew as a circle, and on the light theme an unticked box or an
    /// unselected radio button was white on white with no outline, so it couldn't be seen.
    #[test]
    fn checkboxes_and_radio_buttons_are_outlined_filled_and_distinct() {
        for kind in [theme::ThemeKind::Light, theme::ThemeKind::Dark] {
            let ctx = egui::Context::default();
            theme::apply(&ctx, kind);
            let t = Tokens::for_kind(kind);
            let side = ctx.global_style().spacing.icon_width;
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.add(checkbox(&mut false, "Show cover page in two-page view"));
                ui.add(radio(false, "Single page"));
            });
            let boxes: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if (r.rect.width() - side).abs() < 0.5 && (r.rect.height() - side).abs() < 0.5 => Some(r),
                    _ => None,
                })
                .collect();
            let circles: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Circle(c) if (c.radius * 2.0 - side).abs() < 1.0 => Some(c),
                    _ => None,
                })
                .collect();
            assert_eq!((boxes.len(), circles.len()), (1, 1), "{kind:?}: one checkbox box and one radio circle");
            let r = boxes[0].corner_radius;
            let radius = r.nw.max(r.ne).max(r.sw).max(r.se);
            assert!(f32::from(radius) <= side / 4.0, "{kind:?}: a {side} px box with corner radius {radius} looks like a radio button");
            for (what, fill, edge) in [("checkbox", boxes[0].fill, boxes[0].stroke), ("radio button", circles[0].fill, circles[0].stroke)] {
                assert!(fill != t.panel && fill != t.card, "{kind:?}: an unselected {what} is filled like the panel behind it");
                assert!(
                    edge.width >= 1.0 && edge.color != t.panel && edge.color != t.card,
                    "{kind:?}: an unselected {what} needs an outline, got {edge:?}"
                );
            }
            output.drop_without_applying_deltas();
        }
        // An outline the caller set (dialogs) is kept.
        let ctx = egui::Context::default();
        let own = Stroke::new(1.5, Color32::RED);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = own;
            ui.add(checkbox(&mut true, "Collate"));
            ui.add(radio(false, "All"));
        });
        let kept = output
            .shapes
            .iter()
            .filter(|s| match &s.shape {
                egui::Shape::Rect(r) => r.stroke == own,
                egui::Shape::Circle(c) => c.stroke == own,
                _ => false,
            })
            .count();
        assert_eq!(kept, 2, "the checkbox and the radio button keep the caller's outline");
        output.drop_without_applying_deltas();
    }

    fn toast_rect(app: &mut PdfCraftApp, ctx: &egui::Context, width: f32, now: &mut f64) -> Rect {
        // Let the Area settle after a notice/viewport change, without advancing to
        // expiry. No native rendering or renderer worker is needed for this layout.
        for _ in 0..3 {
            *now += 0.016;
            let output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 900.0))), time: Some(*now), ..Default::default() },
                |ui| toast(app, ui.ctx()),
            );
            output.drop_without_applying_deltas();
        }
        ctx.memory(|memory| memory.area_rect(egui::Id::new("toast")).unwrap())
    }

    #[test]
    fn toast_width_adapts_to_long_notices_and_narrow_viewports() {
        let ctx = egui::Context::default();
        let mut app = PdfCraftApp::new();
        let mut now = 1.0;
        app.notify("Saved");
        let short = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!(short.width() < 160.0, "short notices retain their natural width: {short:?}");
        app.notify("Fill in name failed: The value entered is not valid for the field [ name ]");
        let long = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!(long.width() > 300.0, "a previous short Area must not squeeze validation feedback: {long:?}");
        assert!(long.height() < 85.0, "ordinary feedback must not become a seven-line column: {long:?}");
        for width in [320.0, 180.0] {
            let rect = toast_rect(&mut app, &ctx, width, &mut now);
            assert!(rect.width() <= width - 32.0 + 1.0, "frame and viewport margins must fit: {rect:?}");
            assert!(rect.left() >= 15.0 && rect.right() <= width - 15.0, "notice remains horizontally on screen: {rect:?}");
        }
        app.notify("Saved");
        let again = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!((again.width() - short.width()).abs() < 1.0, "long notices must not impose a permanent minimum width");
    }

    /// The text shapes `wrapped_text` paints in a 120 pt wide column at x = 0.
    fn wrapped_shapes(text: &str, truncate: bool) -> Vec<(Pos2, Rect, String)> {
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::font_definitions());
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let column = Rect::from_min_size(Pos2::ZERO, vec2(120.0, 400.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                wrapped_text(ui, text, theme::regular(13.0), Color32::WHITE, truncate);
            });
        });
        let shapes = output
            .shapes
            .iter()
            .filter_map(|shape| if let egui::Shape::Text(t) = &shape.shape { Some((t.pos, t.galley.rect, t.galley.job.text.clone())) } else { None })
            .collect();
        output.drop_without_applying_deltas();
        shapes
    }

    /// Right-to-left text is wrapped once and looked up on later frames; the cache stays bounded
    /// however many comments go by, and a collapsed comment of any length still shows one line.
    #[test]
    fn rtl_wrapping_is_cached_and_bounded() {
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::font_definitions());
        let cache_len =
            |ctx: &egui::Context| ctx.data_mut(|d| d.get_temp_mut_or_default::<WrapCache>(egui::Id::new("pdfcraft-rtl-wrap-cache")).lines.len());
        let persian = "این یک نظر طولانی به زبان فارسی است که باید در چند خط شکسته شود";
        let long = persian.repeat(2_000);
        let mut first = None;
        for _ in 0..3 {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let lines = wrapped_lines(ui, persian, &theme::regular(13.0), 120.0, false);
                assert!(lines.len() > 1);
                if let Some(prev) = &first {
                    assert!(std::sync::Arc::ptr_eq(prev, &lines), "the second frame reuses the first frame's wrap");
                }
                first = Some(lines);
                let collapsed = wrapped_lines(ui, &long, &theme::regular(13.0), 120.0, true);
                assert_eq!(collapsed.len(), 1);
                assert!(collapsed[0].text.starts_with(ELLIPSIS), "{:?}", collapsed[0].text);
            })
            .drop_without_applying_deltas();
        }
        assert_eq!(cache_len(&ctx), 2, "one entry per text and layout");
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for i in 0..WRAP_CACHE_MAX + 50 {
                wrapped_lines(ui, &format!("{persian} {i}"), &theme::regular(13.0), 120.0, false);
            }
        })
        .drop_without_applying_deltas();
        assert!(cache_len(&ctx) <= WRAP_CACHE_MAX, "bounded: {}", cache_len(&ctx));
    }

    #[test]
    fn rtl_comment_wraps_against_the_right_edge() {
        let persian = "این یک نظر طولانی به زبان فارسی است که باید در چند خط شکسته شود";
        let lines = wrapped_shapes(persian, false);
        assert!(lines.len() > 1, "{lines:?}");
        let right = lines.iter().map(|(pos, rect, _)| pos.x + rect.width()).fold(0.0, f32::max);
        for (pos, rect, text) in &lines {
            assert!(pos.x >= -0.5 && (pos.x + rect.width() - right).abs() < 0.5, "{text:?} at {pos:?} must be right-aligned");
        }
        assert!(right <= 120.5, "lines stay inside the column: {right}");
        // The first words read on the first line, at the top.
        let top = lines.iter().min_by(|a, b| a.0.y.total_cmp(&b.0.y)).unwrap();
        assert!(top.2.contains("این"), "{top:?}");
        // Collapsed: one line, its ellipsis on the left where right-to-left reading ends.
        let collapsed = wrapped_shapes(persian, true);
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].2.starts_with('\u{2026}'), "{collapsed:?}");
        // Text without right-to-left script is a plain label, as before.
        let latin = wrapped_shapes("A short note", false);
        assert_eq!(latin.len(), 1);
        assert_eq!(latin[0].2, "A short note");
        assert!(latin[0].0.x.abs() < 0.5);
    }

    #[test]
    fn line_field_is_at_least_thirty_points_tall() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut height = 0.0;
        ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))), ..Default::default() }, |ui| {
            begin_dialog(ui);
            height = ui.add(line(&mut text)).rect.height();
        })
        .drop_without_applying_deltas();
        assert!(height >= FIELD_MIN_HEIGHT, "field height {height}");
    }

    #[test]
    fn fit_line_keeps_short_text_and_ellipsizes_the_rest() {
        let width = |s: &str| s.chars().count() as f32;
        assert_eq!(fit_line("localhost", 20.0, width), "localhost");
        let long = "Apple Worldwide Developer Relations Certification Authority";
        let fitted = fit_line(long, 24.0, width);
        assert!(fitted.ends_with('\u{2026}'), "{fitted}");
        assert!(fitted.chars().count() <= 24, "{fitted}");
        assert!(long.starts_with(fitted.trim_end_matches('\u{2026}')));
        assert_eq!(fit_line(long, 0.0, width), "");
        assert_eq!(fit_line("ééé", 2.0, width), "é\u{2026}");
    }
}
