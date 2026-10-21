//! Window chrome, kept out of the page's way: one tab strip across the top (the window's title bar
//! too, with the window buttons where the app draws them), and floating bars at the document's
//! right (search, document actions, side panels, page navigation and zoom). The workspaces float at
//! the left with the quick tools (`canvas::quick_bar`). The strip and the bars fade when idle.

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, vec2};

use crate::canvas::{DocView, Fit, PageLayout};
use crate::theme::{self, ThemePreference, Tokens};
use crate::{Dialog, Mode, PdfCraftApp, PropsTab, RightPanel, icons, widgets, window_controls};

/// Height of the tab strip when it doubles as the macOS title bar (#789). AppKit draws the traffic
/// lights itself, centred in the standard 28 pt title-bar band at the top of the window, and moving
/// them takes AppKit calls the workspace forbids (`unsafe`). So on macOS the strip is that band:
/// the lights sit vertically centred in it, and the controls beside them shrink to fit, as in
/// WordCraft (storytold/wordcraft#255).
pub const MAC_TITLE_BAR: f32 = 28.0;
/// The traffic lights as AppKit lays them out: button diameter, centre-to-centre pitch and the
/// first button's left edge (8–10 pt depending on the macOS version; the larger value keeps the
/// gap after them from shrinking).
const MAC_LIGHT: f32 = 12.0;
const MAC_LIGHT_PITCH: f32 = 20.0;
const MAC_LIGHTS_LEFT: f32 = 10.0;
/// Where the strip's content starts to the right of the traffic lights: the end of the three
/// lights plus the same gap the lights keep from the top of the window.
pub const MAC_CONTENT_LEFT: f32 = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT + (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;
/// The strip and its controls everywhere else.
const TAB_STRIP: f32 = 38.0;
const TAB_HEIGHT: f32 = 30.0;
const STRIP_BUTTON: f32 = 28.0;

pub fn tab_strip(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // macOS: the strip is the 28 pt title bar, so the traffic lights are centred in it; tabs and
    // buttons shrink to keep a margin above and below.
    let mac = cfg!(target_os = "macos") && app.integrated_titlebar;
    let left = if mac { MAC_CONTENT_LEFT as i8 } else { 8 };
    let (tab_h, button) = if mac { (MAC_TITLE_BAR - 2.0, 24.0) } else { (TAB_HEIGHT, STRIP_BUTTON) };
    // GNOME on Wayland and Windows: the strip is the window's title bar, with the window buttons
    // at its ends.
    let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
    let controls = app
        .window_controls
        .clone()
        .filter(|_| app.draws_window_frame() && !fullscreen)
        .unwrap_or_else(|| window_controls::Layout { left: Vec::new(), right: Vec::new() });
    // Windows' caption buttons sit in the window's corner, with no margin past them.
    let flush = window_controls::Style::native().flush();
    let right = if flush && !controls.right.is_empty() { 0 } else { 10 };
    let strip_id = egui::Id::new("tab-strip");
    // A tab being dragged keeps the strip awake wherever the pointer goes.
    let dragging = ui.ctx().dragged_id().is_some()
        && widgets::idle_rect(ui.ctx(), strip_id).is_some_and(|r| ui.ctx().input(|i| i.pointer.press_origin()).is_some_and(|p| r.contains(p)));
    let shown = widgets::idle_shown(ui.ctx(), strip_id, dragging);
    let opacity = widgets::IDLE_OPACITY + (1.0 - widgets::IDLE_OPACITY) * shown;
    let height = if mac { MAC_TITLE_BAR } else { TAB_STRIP };
    let margin = egui::Margin { left, right, top: 0, bottom: 0 };
    let body = |app: &mut PdfCraftApp, ui: &mut egui::Ui| {
        let full = ui.max_rect();
        widgets::note_idle_rect(ui.ctx(), strip_id, full);
        // Everything on the strip fades when idle; its background stays, so the window keeps its edge.
        ui.multiply_opacity(opacity);
        let drag = ui.interact(full, ui.id().with("titledrag"), Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if !controls.left.is_empty() {
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    window_controls::buttons(ui, &controls.left);
                });
                ui.add_space(window_controls::SIDE_GAP - 4.0);
            }
            app_icon(ui, button);
            if icons::button(ui, "house", button, app.active.is_none() && !app.combine_showing(), tl!("Home")).clicked() {
                app.active = None;
                app.combine_tab.focused = false;
            }
            main_menu(app, ui, button);
            // Keep the window buttons and a gap outside the scrolling tabs.
            let controls_width = window_controls::width(&controls.right) + 8.0;
            // Open stays in sight after the tabs: its icon, text and padding, and the gaps around it.
            let open_width = ui.fonts_mut(|f| f.layout_no_wrap(tl!("Open").into(), theme::medium(13.0), t.text).size().x) + 38.0 + 12.0;
            let mut tabs_width = (ui.available_width() - controls_width - open_width).max(0.0);
            // Tabs shrink to share the strip (names end in "…") and scroll only once they reach
            // their narrowest; then arrows either side say so and step through them.
            let mut natural: Vec<f32> = Vec::with_capacity(app.views.len() + 1);
            // This window's tabs; Combine has its tab in the main window only.
            let combine_here = app.combine_tab.open && app.current_window == crate::windows::MAIN;
            for i in app.window_tabs() {
                if let Some(name) = crate::canvas::tab_label(app, i) {
                    natural.push(tab_natural_width(ui, &t, &name));
                }
            }
            if combine_here {
                natural.push(tab_natural_width(ui, &t, tl!("Combine files")));
            }
            let gap = ui.spacing().item_spacing.x;
            let overflow = tabs_overflow(&natural, tabs_width, gap, TAB_MIN_WIDTH);
            if overflow {
                tabs_width = (tabs_width - 2.0 * (TAB_ARROW + gap)).max(0.0);
            }
            let cap = tab_cap(&natural, tabs_width, gap, TAB_MIN_WIDTH);
            // Where the tabs were scrolled to last frame, and how far they can go.
            let scroll_id = ui.id().with("tab_scroll");
            let (offset, max_offset) = ui.data(|d| d.get_temp::<(f32, f32)>(scroll_id)).unwrap_or((0.0, 0.0));
            let mut scroll_to = None;
            if overflow && arrow(ui, "chevron-left", offset > 0.5, tl!("Scroll tabs left")) {
                scroll_to = Some((offset - (TAB_MIN_WIDTH + gap)).max(0.0));
            }
            // Keyed by document, not index: a tab dragged past the active one moves it without
            // the strip jumping to it.
            let state = (app.active_ids().map(|(_, id)| id), app.views.len(), app.combine_showing());
            let changed = ui.data_mut(|data| {
                let id = ui.id().with("active_tab");
                let changed = data.get_temp::<(Option<pdfcraft_engine::DocId>, usize, bool)>(id) != Some(state);
                data.insert_temp(id, state);
                changed
            });
            let mut action = None;
            let mut close = None;
            // The pointer's x while a tab is dragged, for scrolling at the strip's edges.
            let mut drag_x = None;
            ui.scope(|ui| {
                ui.style_mut().always_scroll_the_only_direction = true;
                let mut area = egui::ScrollArea::horizontal().id_salt("document_tabs").max_width(tabs_width).auto_shrink([true, true]);
                if let Some(x) = ui.data_mut(|d| d.remove_temp::<f32>(scroll_id.with("to"))) {
                    area = area.horizontal_scroll_offset(x);
                }
                let out = area.show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        // Each document tab's index and rect, and the one being dragged
                        // (with whether it was just released).
                        let mut slots = Vec::with_capacity(app.views.len());
                        let mut dragged = None;
                        let in_main = app.current_window == crate::windows::MAIN;
                        for i in app.window_tabs() {
                            let Some(doc) = app.session.get(app.views[i].id) else { continue };
                            let Some(name) = crate::canvas::tab_label(app, i) else { continue };
                            let dirty = doc.dirty;
                            // A tab showing the document's title names its file on hover.
                            let file = (name != doc.name).then_some(doc.name.as_str());
                            let response = tab(
                                ui,
                                ui.id().with(("document-tab", app.views[i].id)),
                                &t,
                                "file-text",
                                &name,
                                file,
                                dirty,
                                app.active == Some(i),
                                &mut close,
                                i,
                                cap,
                                tab_h,
                                true,
                            );
                            slots.push((i, response.rect));
                            if response.dragged() || response.drag_stopped() {
                                dragged = Some((i, response.drag_stopped()));
                            }
                            if changed && app.active == Some(i) && !app.combine_showing() {
                                response.scroll_to_me(Some(Align::Center));
                            }
                            let id = app.views[i].id;
                            let reveal = cfg!(not(target_arch = "wasm32")) && doc.path.is_some();
                            response.context_menu(|ui| {
                                if ui.button(tl!("Close Tab")).clicked() {
                                    action = Some(crate::tab_context::Action::Close(id));
                                    ui.close();
                                }
                                if ui.add_enabled(!app.closed_tabs.is_empty(), egui::Button::new(tl!("Reopen Closed Tab"))).clicked() {
                                    action = Some(crate::tab_context::Action::Reopen);
                                    ui.close();
                                }
                                let label = if cfg!(target_os = "macos") { tl!("Reveal in Finder") } else { tl!("Reveal in File Explorer") };
                                if ui.add_enabled(reveal, egui::Button::new(label)).clicked() {
                                    action = Some(crate::tab_context::Action::Reveal(id));
                                    ui.close();
                                }
                                // Windows of their own: the desktop app's (a browser tab has one window).
                                if cfg!(not(target_arch = "wasm32")) {
                                    ui.separator();
                                    if ui.button(tl!("Move to New Window")).clicked() {
                                        action = Some(crate::tab_context::Action::NewWindow(id));
                                        ui.close();
                                    }
                                    if !in_main && ui.button(tl!("Move to Main Window")).clicked() {
                                        action = Some(crate::tab_context::Action::MainWindow(id));
                                        ui.close();
                                    }
                                }
                            });
                            if response.clicked() {
                                app.active = Some(i);
                            }
                            // A middle click closes the tab, as in Acrobat and browsers (#1853);
                            // unsaved changes ask first, as the close button does.
                            if response.middle_clicked() {
                                close = Some(i);
                            }
                        }
                        // Dragging a tab moves it (#1537): a line marks the gap it will land
                        // in, and releasing the button drops it there. Dragged well off the
                        // strip, it tears off: released over another window it moves there, and
                        // anywhere else it opens in a window of its own (desktop only).
                        if let Some((from, released)) = dragged {
                            let ctx = ui.ctx().clone();
                            // Outside the window the pointer can go unseen for a frame: keep the
                            // last place it was seen.
                            let seen_id = ui.id().with("tab_drag_seen");
                            let seen = ctx.pointer_latest_pos();
                            if let Some(p) = seen {
                                ctx.data_mut(|d| d.insert_temp(seen_id, p));
                            }
                            let at = seen.or_else(|| ctx.data(|d| d.get_temp::<egui::Pos2>(seen_id)));
                            let torn = cfg!(not(target_arch = "wasm32")) && at.is_none_or(|p| torn_off(full, ctx.content_rect(), p));
                            if released {
                                ctx.data_mut(|d| d.remove::<egui::Pos2>(seen_id));
                                if close.is_some() || action.is_some() {
                                    // The tab closed, or its menu chose something.
                                } else if torn {
                                    if let Some(id) = app.views.get(from).map(|v| v.id) {
                                        let inner = ctx.input(|i| i.viewport().inner_rect);
                                        let screen = at.zip(inner).map(|(p, r)| r.min + p.to_vec2());
                                        action = Some(crate::tab_context::Action::TearOff(id, screen, inner.map(|r| r.size())));
                                    }
                                } else if let Some(x) = at.map(|p| p.x)
                                    && let Some(to) = drop_target(&slots, from, drop_index(&slots, from, x))
                                {
                                    app.move_tab(from, to);
                                }
                            } else if torn {
                                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                                // The last tab only goes to another window, where this one can't draw.
                                if let Some(p) = at
                                    && app.window_tabs().len() > 1
                                    && let Some(name) = crate::canvas::tab_label(app, from)
                                {
                                    tear_off_preview(&ctx, &t, &name, p);
                                }
                            } else if let Some(x) = at.map(|p| p.x) {
                                let to = drop_index(&slots, from, x);
                                drag_x = Some(x);
                                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                                if let Some(marker) = drop_marker(&slots, from, to, ui.spacing().item_spacing.x) {
                                    let span = slots.first().map_or(ui.max_rect().y_range(), |(_, r)| r.y_range());
                                    // In front of the first tab the gap lies outside the scroll area's
                                    // clip rect (#2225): keep the whole line inside it.
                                    let clip = ui.clip_rect();
                                    let marker = marker.clamp(clip.left() + 1.0, (clip.right() - 1.0).max(clip.left() + 1.0));
                                    ui.painter().vline(marker, span.shrink(4.0), Stroke::new(2.0, t.accent));
                                }
                            }
                        }
                        if let Some(i) = close
                            && let Some(view) = app.views.get(i)
                        {
                            action.get_or_insert(crate::tab_context::Action::Close(view.id));
                        }
                        if let Some(action) = action {
                            app.tab_action(action);
                        }
                        if combine_here {
                            // After the document tabs; its index can't clash with theirs.
                            let mut close = None;
                            let response = tab(
                                ui,
                                ui.id().with("combine-tab"),
                                &t,
                                "files",
                                tl!("Combine files"),
                                None,
                                false,
                                app.combine_showing(),
                                &mut close,
                                usize::MAX,
                                cap,
                                tab_h,
                                false,
                            );
                            if changed && app.combine_showing() {
                                response.scroll_to_me(Some(Align::Center));
                            }
                            if response.clicked() {
                                app.open_combine_tab();
                            }
                            if close.is_some() || response.middle_clicked() {
                                app.close_combine_tab();
                            }
                        }
                    });
                });
                let max = (out.content_size.x - out.inner_rect.width()).max(0.0);
                ui.data_mut(|d| d.insert_temp(scroll_id, (out.state.offset.x, max)));
                // A tab dragged to either edge of a strip that scrolls brings the hidden tabs in.
                if let Some(x) = drag_x
                    && max > 0.0
                {
                    let step = if x < out.inner_rect.left() + TAB_DRAG_EDGE {
                        -TAB_DRAG_STEP
                    } else if x > out.inner_rect.right() - TAB_DRAG_EDGE {
                        TAB_DRAG_STEP
                    } else {
                        0.0
                    };
                    if step != 0.0 {
                        ui.data_mut(|d| d.insert_temp(scroll_id.with("to"), (out.state.offset.x + step).clamp(0.0, max)));
                        ui.ctx().request_repaint();
                    }
                }
            });
            if overflow && arrow(ui, "chevron-right", offset < max_offset - 0.5, tl!("Scroll tabs right")) {
                scroll_to = Some((offset + TAB_MIN_WIDTH + gap).min(max_offset));
            }
            if let Some(x) = scroll_to {
                ui.data_mut(|d| d.insert_temp(scroll_id.with("to"), x));
                ui.ctx().request_repaint();
            }
            ui.add_space(4.0);
            let tip = crate::commands::command_tip(ui.ctx(), tl!("Open a PDF ({key})"), "file.open");
            if widgets::ghost_button_sized(ui, "plus", tl!("Open"), tab_h).on_hover_text(tip).clicked() {
                app.open_dialog();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !controls.right.is_empty() {
                    // Laid out left to right inside the right-to-left run, so they read in
                    // the desktop's order.
                    let height = if flush { ui.max_rect().height() } else { window_controls::BUTTON };
                    let size = egui::vec2(window_controls::width(&controls.right) - window_controls::SIDE_GAP, height);
                    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        window_controls::buttons(ui, &controls.right);
                    });
                    ui.add_space(window_controls::SIDE_GAP - 4.0);
                }
            });
        });
    };
    let over = egui::Id::new(STRIP_OVER);
    if reads_under_strip(app) {
        // Reading a document, the strip floats over the page: it turns clear when idle, and the
        // pages scroll on under it.
        let window = ui.max_rect();
        let rect = Rect::from_min_size(window.min, vec2(window.width(), height));
        // Under it, where nothing else is drawn (the top of a side panel), the page's backdrop.
        ui.painter().rect_filled(rect, 0.0, t.pasteboard);
        egui::Area::new(strip_id.with("over-page")).order(egui::Order::Middle).fixed_pos(rect.min).show(ui.ctx(), |ui| {
            ui.set_width(rect.width());
            ui.set_height(rect.height());
            ui.painter().rect_filled(rect, 0.0, t.titlebar.gamma_multiply(shown));
            egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
                ui.set_height(rect.height());
                body(app, ui);
            });
        });
        ui.ctx().data_mut(|d| d.insert_temp(over, rect));
    } else {
        egui::Panel::top("tab_strip").exact_size(height).frame(egui::Frame::NONE.fill(t.titlebar).inner_margin(margin)).show(ui, |ui| body(app, ui));
        ui.ctx().data_mut(|d| d.remove::<Rect>(over));
    }
}

/// Where [`tab_strip`] leaves its rect while it floats over the page, for [`strip_cover`].
const STRIP_OVER: &str = "tab-strip-over-page";

/// Whether the strip floats over the page: reading one document (not a comparison), outside
/// full screen. Elsewhere it keeps a band of its own, since what is below it has controls at its
/// top (the tools panels, Home, Combine).
fn reads_under_strip(app: &PdfCraftApp) -> bool {
    app.mode == Mode::Read && !app.full_screen && app.active.is_some_and(|i| crate::compare_ui::side_by_side(app, i).is_none())
}

/// How much of the top of `area` the tab strip covers while it floats over the page: what is
/// laid out there starts below it, and only the pages run on under it.
pub fn strip_cover(ctx: &egui::Context, area: Rect) -> f32 {
    ctx.data(|d| d.get_temp::<Rect>(egui::Id::new(STRIP_OVER)))
        .filter(|strip| strip.x_range().intersects(area.x_range()) && strip.bottom() > area.top())
        .map_or(0.0, |strip| (strip.bottom() - area.top()).min(area.height()).max(0.0))
}

/// Forget the floating strip for a frame that doesn't draw it (full screen).
pub fn no_strip(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Rect>(egui::Id::new(STRIP_OVER)));
}

/// The app's icon at the start of the strip, where a window's title bar shows it.
fn app_icon(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let icon = Rect::from_center_size(rect.center(), vec2(18.0, 18.0));
    widgets::paint_brand_mark(ui, icon);
}

/// Both theme entry points use the same choices and command path.
fn theme_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    for (preference, command, label) in [
        (ThemePreference::System, "view.theme.system", "Use system setting"),
        (ThemePreference::Light, "view.theme.light", "Light gray"),
        (ThemePreference::Dark, "view.theme.dark", "Dark gray"),
    ] {
        if ui.add(widgets::radio(app.theme_preference == preference, tl!(label))).clicked() {
            app.execute(command);
            ui.close();
        }
    }
    ui.separator();
    if ui.button(tl!("Appearance settings…")).clicked() {
        app.dialog = Some(Dialog::Preferences);
        ui.close();
    }
}

/// View ▸ Fade side panels (and its twin in Preferences): whether the floating chrome turns faint
/// when idle.
pub(crate) fn fade_toggle(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    ui.add(widgets::checkbox(&mut app.chrome_fades, tl!("Fade side panels")));
}

/// View ▸ Auto hide side panels (and its twin in Preferences): whether the floating bars at the
/// document's sides slide away when idle.
pub(crate) fn auto_hide_toggle(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    ui.add(widgets::checkbox(&mut app.auto_hide_bars, tl!("Auto hide side panels"))).on_hover_text(tl!(
        "Slide the floating bars beside the page out of the way when they are not in use. Move the pointer to the window's edge to bring them back."
    ));
}

/// The right-click menu on the title bar's theme button.
fn appearance_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if ui.button(tl!("Appearance settings…")).clicked() {
        app.dialog = Some(Dialog::Preferences);
        ui.close();
    }
}

/// A tab's chrome around its name: the icon on the left, the close button on the right.
const TAB_CHROME: f32 = 64.0;
/// The narrowest a tab shrinks to before the strip scrolls: room for most of a name.
const TAB_MIN_WIDTH: f32 = 180.0;
/// The arrows either side of tabs that don't fit.
const TAB_ARROW: f32 = 24.0;

/// Whether tabs of `natural` widths, `gap` apart, are wider than `budget` even at `min` each.
fn tabs_overflow(natural: &[f32], budget: f32, gap: f32, min: f32) -> bool {
    let narrowest: f32 = natural.iter().map(|w| w.min(min)).sum();
    narrowest + gap * natural.len().saturating_sub(1) as f32 > budget
}

/// How close to either end of a scrolling strip a dragged tab scrolls it, and how far per frame.
const TAB_DRAG_EDGE: f32 = 24.0;
const TAB_DRAG_STEP: f32 = 8.0;

/// How far above or below its strip a dragged tab goes before it tears off.
const TEAR_OFF: f32 = 40.0;

/// Whether a tab dragged to `p` has torn off its strip (`strip`): well above or below it, or out
/// of the window (`window`, its content in the window's own points).
fn torn_off(strip: Rect, window: Rect, p: egui::Pos2) -> bool {
    !window.contains(p) || p.y > strip.bottom() + TEAR_OFF || p.y < strip.top() - TEAR_OFF
}

/// What follows the pointer while a torn-off tab is dragged in its window: the tab's name and
/// what releasing it does.
fn tear_off_preview(ctx: &egui::Context, t: &Tokens, name: &str, p: egui::Pos2) {
    egui::Area::new(egui::Id::new("tab-tear-off")).order(egui::Order::Tooltip).fixed_pos(p + vec2(14.0, 14.0)).interactable(false).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(icons::image("file-text", 14.0, t.text_muted));
                ui.label(egui::RichText::new(name).font(theme::medium(13.0)).color(t.text));
            });
            ui.label(egui::RichText::new(tl!("Release to open in a new window")).font(theme::regular(12.0)).color(t.text_muted));
        });
    });
}

/// Where the tab at `from` lands when dropped with the pointer at `x`: past every other tab
/// whose centre is left of the pointer. `slots` are the tabs' indices and rects, in order.
fn drop_index(slots: &[(usize, Rect)], from: usize, x: f32) -> usize {
    slots.iter().filter(|(i, r)| *i != from && r.center().x < x).count()
}

/// Where in the app's list of views the tab at `from` lands when dropped at `to` (counted among
/// the other tabs of the strip, as [`drop_index`] counts): before the tab it lands in front of,
/// or just after the last. The strip shows one window's tabs, which needn't sit together in the
/// list. `None` when nothing moves.
fn drop_target(slots: &[(usize, Rect)], from: usize, to: usize) -> Option<usize> {
    // Dropped where it is in the strip: nothing moves.
    if slots.iter().position(|(i, _)| *i == from) == Some(to) {
        return None;
    }
    let others: Vec<usize> = slots.iter().map(|(i, _)| *i).filter(|i| *i != from).collect();
    // Where an index ends up once the dragged tab is out of the list.
    let without = |i: usize| if i > from { i - 1 } else { i };
    let target = match others.get(to) {
        Some(&before) => without(before),
        None => without(*others.last()?) + 1,
    };
    (target != from).then_some(target)
}

/// The x of the line marking where the tab at `from` drops at `to`: in the gap before the other
/// tab it lands in front of, or after the last. `None` when it would stay where it is.
fn drop_marker(slots: &[(usize, Rect)], from: usize, to: usize, gap: f32) -> Option<f32> {
    if slots.iter().position(|(i, _)| *i == from) == Some(to) {
        return None;
    }
    let mut others = slots.iter().filter(|(i, _)| *i != from).map(|(_, r)| r);
    match to.checked_sub(1) {
        None => others.next().map(|r| r.left() - gap / 2.0),
        Some(before) => others.nth(before).map(|r| r.right() + gap / 2.0),
    }
}

/// A scroll arrow beside the tabs; dimmed (and inert) at its end. True when clicked.
fn arrow(ui: &mut egui::Ui, icon: &str, enabled: bool, tip: &str) -> bool {
    ui.add_enabled_ui(enabled, |ui| icons::button(ui, icon, TAB_ARROW, false, tip)).inner.clicked()
}

/// A tab name, at most 28 characters (longer names end in "…").
fn tab_name(name: &str) -> String {
    if name.chars().count() > 28 { format!("{}…", name.chars().take(27).collect::<String>()) } else { name.to_string() }
}

/// The width a tab takes when nothing has to shrink.
fn tab_natural_width(ui: &egui::Ui, t: &Tokens, name: &str) -> f32 {
    let label = crate::bidi::visual(&tab_name(name)).into_owned();
    ui.fonts_mut(|f| f.layout_no_wrap(label, theme::regular(13.0), t.text).size().x) + TAB_CHROME
}

/// The widest any tab may be so that tabs of `natural` widths, `gap` apart, fit in `budget`:
/// narrow tabs keep their width and the rest share what is left equally (as browsers do).
/// `None` when every tab fits as it is; never below `min` (then the strip scrolls).
fn tab_cap(natural: &[f32], budget: f32, gap: f32, min: f32) -> Option<f32> {
    let n = natural.len();
    if n == 0 {
        return None;
    }
    let mut left = budget - gap * (n - 1) as f32;
    if natural.iter().sum::<f32>() <= left {
        return None;
    }
    let mut sorted = natural.to_vec();
    sorted.sort_by(f32::total_cmp);
    for (k, w) in sorted.iter().enumerate() {
        let share = left / (n - k) as f32;
        if *w > share {
            return Some(share.max(min));
        }
        left -= w;
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn tab(
    ui: &mut egui::Ui,
    id: egui::Id,
    t: &Tokens,
    icon: &str,
    name: &str,
    file: Option<&str>,
    dirty: bool,
    active: bool,
    close: &mut Option<usize>,
    index: usize,
    cap: Option<f32>,
    height: f32,
    draggable: bool,
) -> egui::Response {
    let font = theme::regular(13.0);
    let mut label = tab_name(name);
    // A shrunk tab shows as much of its name as fits (the tooltip has all of it).
    if let Some(cap) = cap {
        let room = (cap - TAB_CHROME).max(0.0);
        label = ui.fonts_mut(|f| {
            crate::panels::ellipsized_prefix(&label, |s| f.layout_no_wrap(crate::bidi::visual(s).into_owned(), font.clone(), t.text).size().x <= room)
        });
    }
    // Painted text only: the accessibility name below keeps the logical order.
    let label = crate::bidi::visual(&label).into_owned();
    let text_w = ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), font.clone(), t.text).size().x);
    let width = cap.map_or(text_w + TAB_CHROME, |cap| (text_w + TAB_CHROME).min(cap));
    // Full strip height so the active tab sits on the page area below instead of floating in the strip.
    let height = ui.available_height().max(height);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let a11y = if dirty { format!("{name} (edited)") } else { name.to_string() };
    let sense = if draggable { Sense::click_and_drag() } else { Sense::click() };
    let resp = craft_ui::tabs::Tab::new(id, &a11y, active).sense(sense).focus_stroke(Stroke::new(1.0, t.accent)).show_at(ui, rect, |ui, resp| {
        let press = theme::Press::track(ui, resp);
        let bg = if active {
            press.fill(t.chrome, t.chrome.lerp_to_gamma(t.hover, 0.65), t.hover)
        } else {
            press.fill(Color32::TRANSPARENT, t.hover, t.pressed)
        };
        if bg.a() > 0 {
            ui.painter().rect_filled(rect, CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, bg);
        }
        icons::paint(
            ui,
            Rect::from_min_size(rect.min + vec2(6.0, (height - 16.0) / 2.0), vec2(16.0, 16.0)),
            icon,
            15.0,
            if active { t.accent } else { t.text_muted },
        );
        ui.painter().text(rect.min + vec2(28.0, rect.height() / 2.0), Align2::LEFT_CENTER, label, font, if active { t.text } else { t.text_muted });
    });
    let x_rect = Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(20.0, 20.0));
    let x = ui.interact(x_rect, id.with("close"), Sense::click());
    let x_press = theme::Press::track(ui, &x);
    x_press.wash(ui, x_rect, 4, false);
    // Unsaved changes: a dot where the close button sits, until the tab is hovered.
    if dirty && !resp.hovered() && !x.hovered() {
        ui.painter().circle_filled(x_rect.center(), 4.0, if active { t.text } else { t.text_muted });
    } else if active || resp.hovered() || x.hovered() {
        icons::paint(ui, x_rect.translate(x_press.offset()), "x", 13.0, t.text_muted);
    }
    if theme::hand(x).clicked() {
        *close = Some(index);
    }
    let shown = match file {
        Some(file) => std::borrow::Cow::Owned(format!("{}\n{}", crate::bidi::visual(name), crate::bidi::visual(file))),
        None => crate::bidi::visual(name),
    };
    theme::hand(resp).on_hover_text(if dirty {
        crate::i18n::fmt(tl!("{name} — unsaved changes"), &[("name", shown.as_ref())])
    } else {
        shown.into_owned()
    })
}

/// The app menu, an icon in the tab strip after Home: File, Edit, Pages, View and Help.
fn main_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui, size: f32) {
    let t = Tokens::get(ui.ctx());
    let resp = icons::button(ui, "panel-left", size, false, tl!("Menu"));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(230.0);
        ui.menu_button(tl!("File"), |ui| crate::commands::registry_menu(app, ui, "File"));
        ui.menu_button(tl!("Edit"), |ui| crate::commands::registry_menu(app, ui, "Edit"));
        ui.menu_button(tl!("Pages"), |ui| crate::commands::registry_menu(app, ui, "Pages"));
        ui.menu_button(tl!("View"), |ui| {
            if let Some(i) = app.active {
                let v = &mut app.views[i];
                let ctx = ui.ctx().clone();
                let key = |s| crate::commands::shortcut_label(&ctx, s);
                use crate::commands::{ACTUAL_SIZE, FIT_WIDTH, NEXT_VIEW, PAGE_LEVEL, PREV_VIEW, ROTATE_CCW, ROTATE_CW, ZOOM_IN, ZOOM_OUT};
                ui.label(egui::RichText::new(tl!("Zoom")).color(t.text_faint).small());
                if widgets::menu_item(ui, tl!("Actual size"), &key(ACTUAL_SIZE)).clicked() {
                    v.set_zoom(1.0);
                }
                if widgets::menu_item(ui, tl!("Zoom to page level"), &key(PAGE_LEVEL)).clicked() {
                    v.fit = Fit::Page;
                }
                if widgets::menu_item(ui, tl!("Fit to width"), &key(FIT_WIDTH)).clicked() {
                    v.fit = Fit::Width;
                }
                if widgets::menu_item(ui, tl!("Fit to height"), "").clicked() {
                    v.fit = Fit::Height;
                    v.goto = Some((v.current, 0.0));
                }
                if widgets::menu_item(ui, tl!("Fit visible"), &crate::commands::command_shortcut_label(&ctx, "view.fit_visible")).clicked() {
                    ui.close();
                    app.execute("view.fit_visible");
                    return;
                }
                if widgets::menu_item(ui, tl!("Zoom in"), &key(ZOOM_IN)).clicked() {
                    v.zoom_step(true);
                }
                if widgets::menu_item(ui, tl!("Zoom out"), &key(ZOOM_OUT)).clicked() {
                    v.zoom_step(false);
                }
                if widgets::menu_item(ui, tl!("Rotate view clockwise"), &key(ROTATE_CW)).clicked() {
                    v.rotate_view(true);
                }
                if widgets::menu_item(ui, tl!("Rotate view counterclockwise"), &key(ROTATE_CCW)).clicked() {
                    v.rotate_view(false);
                }
                ui.separator();
                ui.label(egui::RichText::new(tl!("Page navigation")).color(t.text_faint).small());
                if ui.add_enabled(!v.back.is_empty(), egui::Button::new(tl!("Previous view")).shortcut_text(key(PREV_VIEW))).clicked() {
                    v.view_history(false);
                }
                if ui.add_enabled(!v.forward.is_empty(), egui::Button::new(tl!("Next view")).shortcut_text(key(NEXT_VIEW))).clicked() {
                    v.view_history(true);
                }
                ui.separator();
                if let Some(id) = page_display_menu(ui, &app.views[i]) {
                    app.execute(id);
                }
                ui.separator();
            }
            crate::commands::registry_menu(app, ui, "View");
            ui.menu_button(tl!("Display theme"), |ui| theme_menu(app, ui));
            fade_toggle(app, ui);
            auto_hide_toggle(app, ui);
            ui.menu_button(tl!("Side panels"), |ui| {
                for (p, label) in [
                    (RightPanel::Comments, tl!("Comments")),
                    (RightPanel::Bookmarks, tl!("Bookmarks")),
                    (RightPanel::Pages, tl!("Pages")),
                    (RightPanel::Fields, tl!("Fields")),
                    (RightPanel::Layers, tl!("Layers")),
                    (RightPanel::Attachments, tl!("Attachments")),
                    (RightPanel::Signatures, tl!("Signatures")),
                    (RightPanel::Accessibility, tl!("Accessibility Checker")),
                    (RightPanel::Tags, tl!("Tags")),
                    (RightPanel::Search, tl!("Search")),
                    (RightPanel::Compare, tl!("Compare")),
                ] {
                    if ui.add(widgets::radio(app.right == Some(p), tl!(label))).clicked() {
                        app.choose_right_panel(Some(p));
                    }
                }
            });
        });
        ui.menu_button(tl!("Help"), |ui| crate::commands::registry_menu(app, ui, "Help"));
    });
}

/// Between a floating bar and the top or bottom of the area it floats over.
const BAR_MARGIN: f32 = 14.0;
/// The strip at the right edge of the document area (or Home, Combine, a comparison) whose
/// centre line the right bars float on: they sit over the page, as the left bar does, and their
/// cards (a button and the card's margins, 42 wide) end the same 14 from the edge as the left
/// bar starts from its own.
const RIGHT_BAR_COLUMN: f32 = 70.0;
/// The size of the right bar's buttons, and of the page navigation's.
const BAR_BUTTON: f32 = 30.0;

/// How the right bars share the area with what [`with_right_bar`] lays out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RightBar {
    /// Over a document: the pages run under the bars to the edge, as they do under the left bar.
    Float,
    /// Beside Home, Combine and a comparison, whose own toolbars end at the right edge: the bars
    /// get a column of their own, so they never cover those controls.
    Beside,
}

/// Lay `content` out, with the right bars floating over its right edge or in a column beside it.
pub fn with_right_bar(app: &mut PdfCraftApp, ui: &mut egui::Ui, how: RightBar, content: impl FnOnce(&mut PdfCraftApp, &mut egui::Ui)) {
    let full = ui.available_rect_before_wrap();
    let area = match how {
        RightBar::Float => full,
        RightBar::Beside => full.with_max_x((full.right() - RIGHT_BAR_COLUMN).max(full.left())),
    };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    child.set_clip_rect(area.intersect(ui.clip_rect()));
    let inset = egui::Id::new(RIGHT_BAR_INSET);
    let top = egui::Id::new(PAGE_TOP);
    ui.ctx().data_mut(|d| {
        d.insert_temp(inset, if how == RightBar::Float { RIGHT_BAR_COLUMN } else { 0.0 });
        d.remove::<f32>(top);
    });
    content(app, &mut child);
    let page_top = ui.ctx().data_mut(|d| {
        d.remove::<f32>(inset);
        d.remove_temp::<f32>(top)
    });
    // Below the notices over a document, whose buttons end at the right edge too.
    let top = page_top.map_or(full.top(), |y| y.clamp(full.top(), full.bottom()));
    let column = Rect::from_min_max(egui::pos2((full.right() - RIGHT_BAR_COLUMN).max(full.left()), top), full.max);
    // After the content, so this frame's bars are the ones on top of it.
    right_bar(app, column, ui.ctx());
    ui.advance_cursor_after_rect(full);
}

/// Where [`with_right_bar`] says how much of its content's right edge the bars cover.
const RIGHT_BAR_INSET: &str = "right-bar-inset";

/// How much of the right edge of the content being laid out the right bars float over: what a
/// popup bar pinned to that edge (find, camera) stays left of. Zero where they are not shown (full
/// screen, a comparison's halves).
pub fn right_bar_inset(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::new(RIGHT_BAR_INSET))).unwrap_or(0.0)
}

/// Where [`note_page_top`] leaves the top of the page view for [`with_right_bar`].
const PAGE_TOP: &str = "right-bar-page-top";

/// Say where the page view starts, below any notices, for the right bars to start there too.
pub fn note_page_top(ctx: &egui::Context, y: f32) {
    if y.is_finite() {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(PAGE_TOP), y));
    }
}

/// The floating bars in `column`: search, document properties, save and print, keyboard shortcuts
/// and the theme, then the side panels at the top; page navigation and zoom at the bottom while
/// a document is open. Both fade when idle.
fn right_bar(app: &mut PdfCraftApp, column: Rect, ctx: &egui::Context) {
    let t = Tokens::get(ctx);
    // The bottom bar first: the top one scrolls in whatever height it leaves.
    let nav_height = nav_bar(app, column, ctx);
    let reserved = nav_height.map_or(0.0, |h| h + BAR_MARGIN);
    // The card's own margins (6 each side) come on top of the scrolled height.
    let max_height = (column.height() - 2.0 * BAR_MARGIN - reserved - 12.0).max(BAR_BUTTON);

    let active = app.active_ids();
    let doc = active.and_then(|(_, id)| app.session.get(id));
    let has_doc = doc.is_some();
    let inspector = inspector_shown(app, ctx);
    // What the side panels hold, for the dot on their buttons.
    let has = doc.map(|doc| {
        (
            !doc.info.annotations.is_empty(),
            !doc.info.outline.is_empty(),
            !doc.info.fields.is_empty(),
            !doc.info.layers.is_empty(),
            !doc.info.attachments.is_empty(),
            doc.is_signed(),
            app.a11y.report.as_ref().is_some_and(|(d, _)| Some(*d) == active.map(|(_, id)| id)),
        )
    });

    let id = egui::Id::new("right-bar");
    let (slide, opacity) = idle_slide(ctx, id, column, false);
    let shown = egui::Area::new(id)
        .order(egui::Order::Middle)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(column.center_top() + vec2(0.0, BAR_MARGIN) + slide)
        .constrain(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(opacity);
            widgets::floating_frame(&t).show(ui, |ui| {
                // The side panels come last: when the window is short, they are what scrolls. An area
                // lays its content out in last frame's size, so the scroll area asks for its height
                // outright; otherwise a bar sized in a small window would stay that small.
                egui::ScrollArea::vertical().id_salt("right-bar-scroll").max_height(max_height).min_scrolled_height(max_height).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.vertical(|ui| {
                        // Search opens Find tools and commands, a floating search with its own close button.
                        let key = crate::commands::command_shortcut_label(ui.ctx(), "view.palette");
                        let tip = if key.is_empty() {
                            tl!("Find tools and commands").to_string()
                        } else {
                            format!("{} ({key})", tl!("Find tools and commands"))
                        };
                        let search = icons::button(ui, "search", BAR_BUTTON, app.palette_open, &tip);
                        // A press here is not a click outside the palette: the button toggles it.
                        crate::palette::note_toggle_rect(ui.ctx(), search.rect);
                        if search.clicked() {
                            app.palette_open = !app.palette_open;
                            if !app.palette_open {
                                app.palette_query.clear();
                            }
                        }
                        ui.add_enabled_ui(has_doc, |ui| {
                            let tip = crate::commands::command_tip(ui.ctx(), tl!("Document properties ({key})"), "file.properties");
                            if icons::button(ui, "info", BAR_BUTTON, false, &tip).clicked() {
                                app.dialog = Some(Dialog::Properties(PropsTab::Description));
                            }
                            let tip = crate::commands::command_tip(ui.ctx(), tl!("Save ({key})"), "file.save");
                            if icons::button(ui, "save", BAR_BUTTON, false, &tip).clicked() {
                                app.run_command("file.save");
                            }
                            let tip = crate::commands::command_tip(ui.ctx(), tl!("Print ({key})"), "print.dialog");
                            if icons::button(ui, "printer", BAR_BUTTON, false, &tip).clicked() {
                                app.run_command("print.dialog");
                            }
                        });
                        divider_h(ui, &t);
                        if icons::button(ui, "circle-help", BAR_BUTTON, false, tl!("Keyboard shortcuts")).clicked() {
                            app.dialog = Some(Dialog::Shortcuts);
                        }
                        theme_button(app, ui, BAR_BUTTON);
                        if let Some((comments, outline, fields, layers, files, signed, checked)) = has {
                            divider_h(ui, &t);
                            let panel_button = |ui: &mut egui::Ui, panel: RightPanel, icon: &str, tip: &str, has: bool| {
                                let selected = app.right == Some(panel) && (app.mode == Mode::Read || inspector);
                                let r = icons::button(ui, icon, BAR_BUTTON, selected, tl!(tip));
                                if has && !selected {
                                    let c = r.rect.right_top() + vec2(-7.0, 7.0);
                                    ui.painter().circle_filled(c, 3.0, t.accent);
                                }
                                if r.clicked() {
                                    defer_panel_choice(ui.ctx(), if selected { None } else { Some(panel) });
                                }
                            };
                            panel_button(ui, RightPanel::Comments, "message-square-text", "Comments", comments);
                            panel_button(ui, RightPanel::Bookmarks, "bookmark", "Bookmarks", outline);
                            panel_button(ui, RightPanel::Pages, "files", "Page thumbnails", false);
                            panel_button(ui, RightPanel::Fields, "text-cursor-input", "Form fields", fields);
                            panel_button(ui, RightPanel::Layers, "layers", "Layers", layers);
                            panel_button(ui, RightPanel::Attachments, "paperclip", "Attachments", files);
                            panel_button(ui, RightPanel::Signatures, "signature", "Signatures", signed);
                            if checked {
                                panel_button(ui, RightPanel::Accessibility, "accessibility", "Accessibility Checker", false);
                            }
                            panel_button(ui, RightPanel::Tags, "tag", "Tags", false);
                        }
                    });
                });
            });
        });
    widgets::note_idle_rect(ctx, id, shown.response.rect.translate(-slide));
}

/// Where a side panel picked on the right bar waits for [`apply_bar_choices`].
const PANEL_CHOICE: &str = "right-bar-panel-choice";
/// Whether the inspector was showing when the frame began, for [`inspector_shown`].
const INSPECTOR_SHOWN: &str = "right-bar-inspector-shown";

/// Note, before the docking layout is drawn, whether it shows the inspector: while it is being
/// drawn (and the right bar with it) the layout is out of `app.docking`.
pub fn note_inspector(app: &PdfCraftApp, ctx: &egui::Context) {
    let shown = app.docking.exposed(crate::docking::Panel::Inspector);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(INSPECTOR_SHOWN), shown));
}

/// Whether the inspector is showing, as noted at the start of the frame.
fn inspector_shown(app: &PdfCraftApp, ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::new(INSPECTOR_SHOWN))).unwrap_or_else(|| app.docking.exposed(crate::docking::Panel::Inspector))
}

/// Remember a side panel picked on the right bar. The bar is drawn inside the docking layout,
/// which writes its own state back when it finishes, so a panel revealed from inside it would be
/// undone: the choice is applied once the layout is drawn.
fn defer_panel_choice(ctx: &egui::Context, choice: Option<RightPanel>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PANEL_CHOICE), choice));
}

/// Where a workspace picked on the left bar waits for [`apply_bar_choices`].
const MODE_CHOICE: &str = "left-bar-mode-choice";

/// Remember a workspace picked on the left bar, which is drawn inside the docking layout too
/// (see [`defer_panel_choice`]).
pub fn defer_mode(ctx: &egui::Context, mode: Mode) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(MODE_CHOICE), mode));
}

/// Apply what was picked on the floating bars this frame, now that the layout is drawn: the
/// workspace first, then the side panel.
pub fn apply_bar_choices(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let mode = ctx.data_mut(|d| {
        let id = egui::Id::new(MODE_CHOICE);
        let mode = d.get_temp::<Mode>(id);
        d.remove::<Mode>(id);
        mode
    });
    if let Some(mode) = mode {
        app.switch_mode(mode, true);
        ctx.request_repaint();
    }
    if let Some(choice) = ctx.data_mut(|d| d.remove_temp::<Option<RightPanel>>(egui::Id::new(PANEL_CHOICE))) {
        app.choose_right_panel(choice);
        ctx.request_repaint();
    }
}

/// The theme button: a click cycles System, Light and Dark; a right-click opens the menu.
fn theme_button(app: &mut PdfCraftApp, ui: &mut egui::Ui, size: f32) {
    let (icon, label) = match app.theme_preference {
        ThemePreference::System => ("monitor", tl!("Use system setting")),
        ThemePreference::Light => ("sun", tl!("Light gray")),
        ThemePreference::Dark => ("moon", tl!("Dark gray")),
    };
    let tip = format!("{}: {label}", tl!("Display theme"));
    let response = icons::button(ui, icon, size, false, &tip);
    if response.clicked() {
        app.execute(match app.theme_preference {
            ThemePreference::System => "view.theme.light",
            ThemePreference::Light => "view.theme.dark",
            ThemePreference::Dark => "view.theme.system",
        });
    }
    egui::Popup::context_menu(&response).id(response.id.with("theme_menu")).show(|ui| appearance_menu(app, ui));
}

/// The floating page navigation and zoom bar at the bottom of `column`, while a document is open.
/// Returns its height, for the bar above to stay clear of it.
fn nav_bar(app: &mut PdfCraftApp, column: Rect, ctx: &egui::Context) -> Option<f32> {
    let t = Tokens::get(ctx);
    let (index, id) = app.active_ids()?;
    let doc = app.session.get(id)?;
    let page_count = doc.info.pages.len();
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let view = app.views.get_mut(index)?;
    let bar_id = egui::Id::new("nav-bar");
    let editing = ctx.memory(|m| m.has_focus(view.page_input_id()) || m.has_focus(egui::Id::new("zoom-input")));
    let (slide, opacity) = idle_slide(ctx, bar_id, column, editing);
    let mut page_display = None;
    let shown = egui::Area::new(bar_id)
        .order(egui::Order::Middle)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(column.center_bottom() - vec2(0.0, BAR_MARGIN) + slide)
        .constrain(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(opacity);
            widgets::floating_frame(&t).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    // History is distinct from stepping to the adjacent page, and belongs to this tab.
                    for (forward, icon, label, shortcut, enabled) in [
                        (false, "chevron-left", tl!("Previous view"), crate::commands::PREV_VIEW, !view.back.is_empty()),
                        (true, "chevron-right", tl!("Next view"), crate::commands::NEXT_VIEW, !view.forward.is_empty()),
                    ] {
                        let tip = format!("{label} ({})", crate::commands::shortcut_label(ui.ctx(), shortcut));
                        if ui.add_enabled_ui(enabled, |ui| icons::button(ui, icon, BAR_BUTTON, false, &tip)).inner.clicked() {
                            view.view_history(forward);
                        }
                    }
                    divider_h(ui, &t);
                    if icons::button(ui, "chevron-up", BAR_BUTTON, false, tl!("Previous page")).clicked() {
                        view.step_page(false);
                    }
                    let page_input_id = view.page_input_id();
                    // Consume replacement selection once, including when the editor has lost focus.
                    let reselect = std::mem::take(&mut view.page_input_reselect);
                    // Navigation updates the idle display, never the text being edited.
                    if !ui.memory(|m| m.has_focus(page_input_id)) {
                        view.page_input = view.current_page_label(labels.get(view.current).map(String::as_str));
                    } else if reselect {
                        // The replacement changed the label while egui retained this editor's cursor.
                        let mut state = egui::TextEdit::load_state(ui.ctx(), page_input_id).unwrap_or_default();
                        let end = egui::text::CCursor::new(view.page_input.chars().count());
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
                        state.store(ui.ctx(), page_input_id);
                    }
                    let edit = egui::TextEdit::singleline(&mut view.page_input)
                        .id(page_input_id)
                        .desired_width(BAR_BUTTON - 6.0)
                        .horizontal_align(Align::Center)
                        .font(theme::medium(12.0))
                        .margin(vec2(2.0, 4.0));
                    let r = egui::Frame::NONE
                        .fill(t.field)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(CornerRadius::same(5))
                        .show(ui, |ui| ui.add(edit))
                        .inner
                        .on_hover_text(tl!("Current page — type a page number or label (such as iv) and press Enter"));
                    if r.lost_focus() {
                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            // A page label first (logical page numbers, as Acrobat), then a number.
                            let typed = view.page_input.clone();
                            view.go_to_typed(&typed, &labels);
                        }
                        view.page_input = view.current_page_label(labels.get(view.current).map(String::as_str));
                    }
                    ui.label(egui::RichText::new(page_count.to_string()).font(theme::regular(11.0)).color(t.text_muted));
                    if icons::button(ui, "chevron-down", BAR_BUTTON, false, tl!("Next page")).clicked() {
                        view.step_page(true);
                    }
                    divider_h(ui, &t);
                    let tip = crate::commands::key_tip(ui.ctx(), tl!("Zoom in ({key})"), crate::commands::ZOOM_IN);
                    if icons::button(ui, "zoom-in", BAR_BUTTON, false, &tip).clicked() {
                        view.zoom_step(true);
                    }
                    zoom_field(ui, &t, view);
                    let tip = crate::commands::key_tip(ui.ctx(), tl!("Zoom out ({key})"), crate::commands::ZOOM_OUT);
                    if icons::button(ui, "zoom-out", BAR_BUTTON, false, &tip).clicked() {
                        view.zoom_step(false);
                    }
                    divider_h(ui, &t);
                    // Not `columns-2` while fitting the page: that is the two-page view's icon.
                    let fit_icon = if view.fit == Fit::Width { "maximize" } else { "maximize-2" };
                    if icons::button(ui, fit_icon, BAR_BUTTON, false, tl!("Toggle fit page / fit width")).clicked() {
                        view.fit = if view.fit == Fit::Width { Fit::Page } else { Fit::Width };
                        view.goto = Some((view.current, 0.0));
                    }
                    let tip = crate::commands::key_tip(ui.ctx(), tl!("Rotate view clockwise ({key})"), crate::commands::ROTATE_CW);
                    if icons::button(ui, "rotate-cw", BAR_BUTTON, false, &tip).clicked() {
                        view.rotate_view(true);
                    }
                    // Its command runs once `view` is free.
                    let tip = crate::i18n::fmt(tl!("Page display: {layout}"), &[("layout", tl!(view.layout.label()))]);
                    let resp = icons::button(ui, view.layout.icon(), BAR_BUTTON, false, &tip);
                    page_display = egui::Popup::menu(&resp)
                        .align(egui::RectAlign::LEFT_END)
                        .show(|ui| {
                            ui.set_min_width(220.0);
                            rail_view_menu(ui, view)
                        })
                        .and_then(|r| r.inner);
                });
            });
        });
    widgets::note_idle_rect(ctx, bar_id, shown.response.rect.translate(-slide));
    if let Some(id) = page_display {
        app.execute(id);
    }
    Some(shown.response.rect.height())
}

/// How far the right bar `id`, which belongs in `column`, has slid out of the window to the right
/// while idle (see [`widgets::idle`]), and the opacity to draw it at.
fn idle_slide(ctx: &egui::Context, id: egui::Id, column: Rect, busy: bool) -> (egui::Vec2, f32) {
    let idle = widgets::idle(ctx, id, busy, true);
    // Far enough to take the card and its shadow out of the window.
    let reach = widgets::idle_rect(ctx, id).map_or(RIGHT_BAR_COLUMN, |r| column.right() - r.left()) + 24.0;
    (vec2(reach.max(0.0) * idle.hidden, 0.0), idle.opacity)
}

/// The zoom as a field (#1755): it shows the zoom, and Enter applies a percentage typed into it
/// (150, 150%, 62.5 %); Escape, or what doesn't read as one, keeps the zoom as it is.
fn zoom_field(ui: &mut egui::Ui, t: &Tokens, view: &mut DocView) {
    let id = egui::Id::new("zoom-input");
    let draft = id.with("draft");
    let mut typed = ui.data(|d| d.get_temp::<String>(draft)).unwrap_or_else(|| format!("{:.0}%", view.zoom * 100.0));
    let edit = egui::TextEdit::singleline(&mut typed)
        .id(id)
        .desired_width(BAR_BUTTON + 4.0)
        .horizontal_align(Align::Center)
        .font(theme::medium(10.5))
        .text_color(t.text_muted)
        .frame(egui::Frame::NONE)
        .margin(vec2(2.0, 2.0));
    let r = ui.add(edit).on_hover_text(tl!("Zoom — type a percentage and press Enter"));
    ui.ctx().accesskit_node_builder(id, |node| node.set_label(tl!("Zoom")));
    // Focused, the whole value is selected, to type over.
    if r.gained_focus() {
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(typed.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), id);
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(draft, typed.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(draft));
        if r.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(zoom) = DocView::parse_zoom_percent(&typed)
        {
            view.set_zoom(zoom);
        }
    }
}

/// A short line between groups in a vertical floating bar.
fn divider_h(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(BAR_BUTTON, 7.0), Sense::hover());
    ui.painter().hline(r.x_range().shrink(5.0), r.center().y, Stroke::new(1.0, t.divider));
}

/// View ▸ Page display and the rail's Page display menu: the layouts and the cover page.
/// Returns the command a click asks for, for the caller to run.
fn page_display_menu(ui: &mut egui::Ui, view: &DocView) -> Option<&'static str> {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("Page display")).color(t.text_faint).small());
    let mut picked = None;
    for l in PageLayout::ORDER {
        if ui.add(widgets::radio(view.layout == l, tl!(l.label()))).clicked() {
            picked = Some(l.command());
        }
    }
    ui.separator();
    // Continuous scrolling is an independent checkbox over the base layout, as in Acrobat (#752).
    let mut continuous = view.continuous();
    if ui.add(widgets::checkbox(&mut continuous, tl!("Continuous scrolling"))).changed() {
        picked = Some("view.continuous");
    }
    let mut cover = view.cover;
    if ui.add_enabled(view.cover_applies(), widgets::checkbox(&mut cover, tl!("Show cover page in two-page view"))).changed() {
        picked = Some("view.layout.cover");
    }
    if picked.is_some() {
        ui.close();
    }
    picked
}

/// The rail's Page display menu: Acrobat's view choices around the page display. Zoom changes
/// apply at once; the command a click asks for is returned for the caller to run.
fn rail_view_menu(ui: &mut egui::Ui, view: &mut DocView) -> Option<&'static str> {
    let mut picked = None;
    for (id, label) in [("view.fit_width_scrolling", "Fit to width scrolling"), ("view.fit_one_page", "Fit one full page")] {
        if ui.button(tl!(label)).clicked() {
            picked = Some(id);
        }
    }
    ui.separator();
    if ui.button(tl!("Actual size")).clicked() {
        view.set_zoom(1.0);
        ui.close();
    }
    if ui.button(tl!("Zoom to page level")).clicked() {
        view.set_fit(Fit::Page);
        ui.close();
    }
    if ui.button(tl!("Fit visible")).clicked() {
        picked = Some("view.fit_visible");
    }
    ui.separator();
    picked = page_display_menu(ui, view).or(picked);
    ui.separator();
    for (id, label) in [("view.read_mode", "Read mode"), ("view.full_screen", "Full screen mode")] {
        if ui.button(tl!(label)).clicked() {
            picked = Some(id);
        }
    }
    if picked.is_some() {
        ui.close();
    }
    picked
}

#[cfg(test)]
mod tab_widths {
    use super::{MAC_CONTENT_LEFT, MAC_LIGHT, MAC_LIGHT_PITCH, MAC_LIGHTS_LEFT, MAC_TITLE_BAR, tab_cap};

    /// Tabs that fit keep their own widths.
    #[test]
    fn tabs_that_fit_do_not_shrink() {
        assert_eq!(tab_cap(&[200.0, 150.0], 400.0, 4.0, 110.0), None);
        assert_eq!(tab_cap(&[], 0.0, 4.0, 110.0), None);
    }

    /// Too wide: narrow tabs keep their width, the wide ones share the rest equally, and
    /// together they fill the strip exactly.
    #[test]
    fn wide_tabs_share_what_narrow_ones_leave() {
        let natural = [120.0, 300.0, 300.0];
        let cap = tab_cap(&natural, 500.0, 4.0, 110.0).expect("must shrink");
        let used: f32 = natural.iter().map(|w| w.min(cap)).sum::<f32>() + 2.0 * 4.0;
        assert!((used - 500.0).abs() < 0.01, "{cap} uses {used}");
        assert!((cap - 186.0).abs() < 0.01, "{cap}");
    }

    /// Arrows appear only when the tabs can't fit even at their narrowest.
    #[test]
    fn arrows_only_when_the_narrowest_tabs_overflow() {
        use super::tabs_overflow;
        assert!(!tabs_overflow(&[250.0; 3], 600.0, 4.0, 180.0), "three shrunk tabs fit");
        assert!(tabs_overflow(&[250.0; 4], 600.0, 4.0, 180.0), "four don't, even at 180");
        assert!(!tabs_overflow(&[100.0; 5], 600.0, 4.0, 180.0), "narrow tabs fit as they are");
        assert!(!tabs_overflow(&[], 0.0, 4.0, 180.0));
    }

    /// Tabs never get narrower than the minimum: past it, the strip scrolls instead.
    #[test]
    fn tabs_stop_at_their_minimum() {
        assert_eq!(tab_cap(&[250.0; 12], 600.0, 4.0, 110.0), Some(110.0));
        assert_eq!(tab_cap(&[250.0; 3], 0.0, 4.0, 110.0), Some(110.0));
    }

    /// Three 100-wide tabs, 4 apart, starting at x = 0.
    fn three_slots() -> Vec<(usize, egui::Rect)> {
        (0..3).map(|i| (i, egui::Rect::from_min_size(egui::pos2(i as f32 * 104.0, 0.0), egui::vec2(100.0, 30.0)))).collect()
    }

    /// A dragged tab tears off well above or below its strip, or out of the window; along the
    /// strip, and just off it, it is still being moved among the tabs.
    #[test]
    fn a_tab_tears_off_well_off_its_strip_or_out_of_the_window() {
        use super::torn_off;
        let window = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 700.0));
        let strip = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 40.0));
        assert!(!torn_off(strip, window, egui::pos2(500.0, 20.0)), "on the strip");
        assert!(!torn_off(strip, window, egui::pos2(500.0, 79.0)), "just below it");
        assert!(torn_off(strip, window, egui::pos2(500.0, 81.0)), "well below, over the page");
        assert!(torn_off(strip, window, egui::pos2(1200.0, 20.0)), "level with it, past the window's edge");
        assert!(torn_off(strip, window, egui::pos2(500.0, -60.0)), "above the window");
    }

    /// A dragged tab lands past every other tab whose centre the pointer has crossed.
    #[test]
    fn a_dragged_tab_drops_past_the_centres_it_crossed() {
        use super::drop_index;
        let slots = three_slots();
        // Centres at 50, 154 and 258.
        assert_eq!(drop_index(&slots, 0, 10.0), 0, "still over itself");
        assert_eq!(drop_index(&slots, 0, 160.0), 1, "past the middle tab's centre");
        assert_eq!(drop_index(&slots, 0, 900.0), 2, "past the end");
        assert_eq!(drop_index(&slots, 2, -50.0), 0, "before the start");
        assert_eq!(drop_index(&slots, 2, 100.0), 1, "between the first and middle centres");
        assert_eq!(drop_index(&[], 0, 100.0), 0);
    }

    /// The drop line sits in the gap the tab will land in, and not at all where it already is.
    #[test]
    fn the_drop_line_marks_the_gap_and_hides_when_nothing_moves() {
        use super::drop_marker;
        let slots = three_slots();
        assert_eq!(drop_marker(&slots, 0, 0, 4.0), None);
        assert_eq!(drop_marker(&slots, 1, 1, 4.0), None);
        // The first tab dropped at the end: after the last tab.
        assert_eq!(drop_marker(&slots, 0, 2, 4.0), Some(310.0));
        // The first tab dropped between the middle and the last: in the gap between them.
        assert_eq!(drop_marker(&slots, 0, 1, 4.0), Some(206.0));
        // The last tab dropped at the start: before the first tab.
        assert_eq!(drop_marker(&slots, 2, 0, 4.0), Some(-2.0));
        assert_eq!(drop_marker(&[], 0, 1, 4.0), None);
    }

    /// Dropping a tab places it among the strip's tabs even when other windows' tabs sit
    /// between them in the app's list.
    #[test]
    fn a_dropped_tab_lands_among_its_windows_tabs() {
        use super::drop_target;
        let at = |i: usize| (i, egui::Rect::from_min_size(egui::pos2(i as f32 * 100.0, 0.0), egui::vec2(90.0, 30.0)));
        // The strip shows tabs 0, 1 and 3 of the list (2 is in another window).
        let slots = [at(0), at(1), at(3)];
        assert_eq!(drop_target(&slots, 0, 2), Some(3), "after the last: past list item 3 (at 2 once 0 is out)");
        assert_eq!(drop_target(&slots, 3, 0), Some(0), "before the first");
        assert_eq!(drop_target(&slots, 3, 1), Some(1), "before tab 1");
        assert_eq!(drop_target(&slots, 0, 0), None, "where it is");
        assert_eq!(drop_target(&slots, 3, 2), None, "where it is, at the end");
        // All of the list in one strip: the index it is dropped at.
        let all = [at(0), at(1), at(2)];
        assert_eq!(drop_target(&all, 0, 2), Some(2));
        assert_eq!(drop_target(&all, 2, 0), Some(0));
        assert_eq!(drop_target(&[], 0, 0), None);
    }

    #[test]
    fn the_mac_title_bar_centres_and_clears_the_traffic_lights() {
        // The lights are centred in the bar, and content keeps the same gap after them (#789).
        let margin = (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;
        let lights_end = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT;
        assert_eq!(MAC_CONTENT_LEFT - lights_end, margin);
        // `egui::Margin` is in `i8`.
        assert!(MAC_CONTENT_LEFT <= f32::from(i8::MAX));
    }
}
