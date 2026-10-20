//! Preferences: a header, a sidebar of sections beside one scrolling page of them (the sidebar
//! takes you to a section and marks the one you are reading), and OK at the foot. Each section is
//! a heading with a line saying what it holds, then cards of settings. Every setting applies as
//! it changes, as it always has; OK closes.

use egui::{Align, Align2, Color32, CornerRadius, Rect, RichText, Sense, Stroke, vec2};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, widgets};

/// The dialog's width (the modal's, see `dialogs`), and its sidebar's.
pub(crate) const DIALOG_W: f32 = 960.0;
const SIDEBAR_W: f32 = 220.0;
/// The tallest the page of settings gets.
const PAGE_H: f32 = 640.0;

/// A section of Preferences: where the sidebar takes you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    General,
    Appearance,
    Documents,
    Reading,
    Identity,
    FillSign,
    Scripting,
}

impl Section {
    const ALL: [Self; 7] = [Self::General, Self::Appearance, Self::Documents, Self::Reading, Self::Identity, Self::FillSign, Self::Scripting];

    fn icon(self) -> &'static str {
        match self {
            Self::General => "sliders-horizontal",
            Self::Appearance => "palette",
            Self::Documents => "file-text",
            Self::Reading => "eye",
            Self::Identity => "message-square-text",
            Self::FillSign => "signature",
            Self::Scripting => "shield-check",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::General => tl!("General & Interface"),
            Self::Appearance => tl!("Appearance & Theme"),
            Self::Documents => tl!("Documents & Viewing"),
            Self::Reading => tl!("Reading & Colours"),
            Self::Identity => tl!("Identity & Comments"),
            Self::FillSign => tl!("Fill & Sign"),
            Self::Scripting => tl!("Scripting & Security"),
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::General => tl!("The interface's language and size, and the floating bars around the page."),
            Self::Appearance => tl!("The colours PdfCraft's windows are drawn in."),
            Self::Documents => tl!("How documents open: the workspace, the page display and the zoom."),
            Self::Reading => tl!("Colours to read documents in, without changing the documents."),
            Self::Identity => tl!("Your name on comments, and how comments show."),
            Self::FillSign => tl!("Flattening, and the dates Fill & Sign writes."),
            Self::Scripting => tl!("What scripts in PDF files may do."),
        }
    }

    fn id(self) -> egui::Id {
        egui::Id::new(("preferences-section", self as u8))
    }
}

/// Where the sidebar asked to go, for the page to scroll there.
const GOTO: &str = "preferences-goto";
/// The section being read, as the page last found it, for the sidebar to mark.
const READING: &str = "preferences-reading";

/// Draw Preferences. Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let mut close = header(ui, t);
    ui.add_space(12.0);
    divider(ui, t);
    let height = (ui.ctx().content_rect().height() - 220.0).clamp(300.0, PAGE_H);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        sidebar(ui, t, height);
        let (line, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
        ui.painter().vline(line.center().x, line.y_range(), Stroke::new(1.0, t.divider));
        ui.add_space(20.0);
        ui.vertical(|ui| {
            egui::ScrollArea::vertical().id_salt("preferences-page").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width((ui.available_width() - 14.0).max(300.0));
                page(ui, app, t);
            });
        });
    });
    divider(ui, t);
    ui.add_space(10.0);
    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            close = true;
        }
    });
    close
}

/// The title, what Preferences holds, and Close. Returns `true` when Close was clicked.
fn header(ui: &mut egui::Ui, t: &Tokens) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        let (tile, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
        ui.painter().rect_filled(tile, CornerRadius::same(10), t.accent_soft);
        icons::paint(ui, Rect::from_center_size(tile.center(), vec2(20.0, 20.0)), "sliders-horizontal", 19.0, t.accent_text);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(tl!("Preferences")).font(theme::semibold(18.0)).color(t.text));
            ui.label(
                RichText::new(tl!("Settings for the interface, documents, reading and scripts.")).font(theme::regular(12.5)).color(t.text_muted),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            close = icons::button(ui, "x", 28.0, false, tl!("Close")).clicked();
        });
    });
    close
}

/// A hairline across the dialog.
fn divider(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, t.divider));
}

/// The sections, the one being read marked; a click takes the page there.
fn sidebar(ui: &mut egui::Ui, t: &Tokens, height: f32) {
    let reading = ui.data(|d| d.get_temp::<u8>(egui::Id::new(READING))).unwrap_or(0);
    ui.allocate_ui_with_layout(vec2(SIDEBAR_W, height), egui::Layout::top_down(Align::Min), |ui| {
        ui.set_width(SIDEBAR_W);
        ui.add_space(12.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        for section in Section::ALL {
            let on = section as u8 == reading;
            let (rect, resp) = ui.allocate_exact_size(vec2(SIDEBAR_W - 16.0, 40.0), Sense::click());
            let press = theme::Press::track(ui, &resp);
            let fill = if on { t.accent_soft } else { press.fill(Color32::TRANSPARENT, t.hover, t.pressed) };
            if fill.a() > 0 {
                ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
            }
            let body = rect.translate(press.offset());
            let ink = if on { t.accent_text } else { t.text };
            icons::paint(
                ui,
                Rect::from_center_size(body.left_center() + vec2(22.0, 0.0), vec2(18.0, 18.0)),
                section.icon(),
                17.0,
                if on { t.accent_text } else { t.icon },
            );
            widgets::paint_left(ui, body.left_center() + vec2(42.0, 0.0), section.title(), theme::medium(13.5), ink, (body.width() - 50.0).max(0.0));
            let title = section.title().to_owned();
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, title.clone()));
            if theme::hand(resp).clicked() {
                ui.data_mut(|d| d.insert_temp(egui::Id::new(GOTO), section as u8));
            }
        }
    });
}

/// Every section, one after another.
fn page(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let top = ui.clip_rect().top();
    let goto = ui.data_mut(|d| d.remove_temp::<u8>(egui::Id::new(GOTO)));
    let mut reading = 0u8;
    for section in Section::ALL {
        let start = ui.cursor().top();
        // Read from where its heading reaches the top of the page (a little below it).
        if start <= top + 48.0 {
            reading = section as u8;
        }
        heading(ui, t, section, goto == Some(section as u8));
        match section {
            Section::General => general(ui, app, t),
            Section::Appearance => appearance(ui, app, t),
            Section::Documents => documents(ui, app, t),
            Section::Reading => reading_colours(ui, app, t),
            Section::Identity => identity(ui, app, t),
            Section::FillSign => fill_sign(ui, app, t),
            Section::Scripting => scripting(ui, app, t),
        }
        ui.add_space(28.0);
    }
    // The last section can't scroll to the top: at the end of the page it is the one read.
    if ui.clip_rect().bottom() >= ui.cursor().top() - 4.0 {
        reading = Section::Scripting as u8;
    }
    ui.data_mut(|d| d.insert_temp(egui::Id::new(READING), reading));
}

/// A section's title and what it holds; scrolled to the top of the page when the sidebar asks.
fn heading(ui: &mut egui::Ui, t: &Tokens, section: Section, scroll_here: bool) {
    let resp = ui
        .push_id(section.id(), |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(section.title()).font(theme::semibold(22.0)).color(t.text));
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(section.description()).font(theme::regular(13.0)).color(t.text_muted)).wrap());
            ui.add_space(12.0);
        })
        .response;
    if scroll_here {
        ui.scroll_to_rect(resp.rect, Some(Align::TOP));
    }
}

/// A card of settings under `title` (with `icon`), what it is for in `subtitle`.
fn card(ui: &mut egui::Ui, t: &Tokens, icon: &str, title: &str, subtitle: Option<&str>, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(t.hover)
        .stroke(Stroke::new(1.0, t.divider))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (tile, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                ui.painter().rect_filled(tile, CornerRadius::same(8), t.card);
                icons::paint(ui, Rect::from_center_size(tile.center(), vec2(16.0, 16.0)), icon, 15.0, t.accent_text);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(title).font(theme::semibold(14.5)).color(t.text));
                    if let Some(s) = subtitle {
                        ui.add(egui::Label::new(RichText::new(s).font(theme::regular(12.0)).color(t.text_muted)).wrap());
                    }
                });
            });
            ui.add_space(10.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            content(ui);
        });
    ui.add_space(14.0);
}

/// A setting on a row of its own: its name and what it does at the left, `control` at the right.
fn row(ui: &mut egui::Ui, t: &Tokens, name: &str, about: Option<&str>, control: impl FnOnce(&mut egui::Ui)) {
    named_row(ui, t, name, about, false, control);
}

/// [`row`], its name only painted when `painted`: for a control that carries the name itself (a
/// switch), so assistive technology hears it once.
fn named_row(ui: &mut egui::Ui, t: &Tokens, name: &str, about: Option<&str>, painted: bool, control: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.fill(t.card).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let right = (ui.available_width() * 0.45).clamp(140.0, 280.0);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - right - 12.0).max(120.0));
                ui.spacing_mut().item_spacing.y = 2.0;
                if painted {
                    let galley = ui.painter().layout(name.to_owned(), theme::medium(13.0), t.text, ui.available_width());
                    let (r, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                    ui.painter().galley(r.min, galley, t.text);
                } else {
                    ui.label(RichText::new(name).font(theme::medium(13.0)).color(t.text));
                }
                if let Some(a) = about {
                    ui.add(egui::Label::new(RichText::new(a).font(theme::regular(12.0)).color(t.text_muted)).wrap());
                }
            });
            ui.with_layout(egui::Layout::right_to_left(Align::Center), control);
        });
    });
}

/// An on/off switch with `label` as its name (a checkbox to assistive technology). Returns its
/// response, `changed` when flipped.
fn switch(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
    let track = t.pressed.lerp_to_gamma(t.accent, how);
    ui.painter().rect_filled(rect, CornerRadius::same(11), track);
    let knob = egui::pos2(rect.left() + 11.0 + how * (rect.width() - 22.0), rect.center().y);
    ui.painter().circle_filled(knob, 8.0, Color32::WHITE);
    if resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(2.0), CornerRadius::same(13), Stroke::new(1.5, t.accent), egui::StrokeKind::Outside);
    }
    let (state, name) = (*on, label.to_owned());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, state, name.clone()));
    theme::hand(resp).on_hover_text(label)
}

/// A setting that is on or off, on a row of its own.
fn switch_row(ui: &mut egui::Ui, t: &Tokens, on: &mut bool, name: &str, about: Option<&str>) -> bool {
    let mut changed = false;
    named_row(ui, t, name, about, true, |ui| changed = switch(ui, on, name).changed());
    changed
}

/// Radio buttons joined in one bar, each with its icon (optional) and name. Returns the index
/// clicked.
fn segments(ui: &mut egui::Ui, items: &[(Option<&str>, &str, bool)]) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let mut clicked = None;
    egui::Frame::NONE.fill(t.card).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let n = items.len().max(1) as f32;
        let w = ((ui.available_width() - 4.0 * (n - 1.0)) / n).max(48.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, (icon, name, on)) in items.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
                let press = theme::Press::track(ui, &resp);
                let fill = if *on { t.accent_soft } else { press.fill(Color32::TRANSPARENT, t.hover, t.pressed) };
                if fill.a() > 0 {
                    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
                }
                let ink = if *on { t.accent_text } else { t.text };
                let font = theme::medium(13.0);
                let text_w = widgets::text_width(ui, name, &font);
                let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
                let room = (rect.width() - 12.0 - icon_w).max(0.0);
                let shown = widgets::fit_line(name, room, |s| widgets::text_width(ui, s, &font));
                let start = rect.center().x - (text_w.min(room) + icon_w) / 2.0 + press.offset().x;
                let y = rect.center().y + press.offset().y;
                if let Some(icon) = icon {
                    icons::paint(ui, Rect::from_center_size(egui::pos2(start + 8.0, y), vec2(16.0, 16.0)), icon, 15.0, ink);
                }
                ui.painter().text(egui::pos2(start + icon_w, y), Align2::LEFT_CENTER, shown, font, ink);
                let label = (*name).to_owned();
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, *on, label.clone()));
                if theme::hand(resp).clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

/// A quiet note under a group of settings, with an info icon.
fn note(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(ui, r, "info", 14.0, t.text_faint);
        ui.add(egui::Label::new(RichText::new(text).font(theme::regular(12.0)).color(t.text_muted)).wrap());
    });
}

/// A timer in seconds from `choices`, as a list.
fn seconds(ui: &mut egui::Ui, id: &str, secs: &mut u8, choices: &[u8]) {
    egui::ComboBox::from_id_salt(id).selected_text(format!("{secs} s")).width(90.0).show_ui(ui, |ui| {
        for &choice in choices {
            if ui.selectable_value(secs, choice, format!("{choice} s")).clicked() {
                ui.close();
            }
        }
    });
}

// ───────────────────────────────────────────────────────────────────────────────── the sections

fn general(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, "languages", tl!("Interface"), None, |ui| {
        row(ui, t, tl!("Interface language"), None, |ui| {
            let selected = crate::i18n::Lang::from_code(&app.language).map_or(tl!("Auto"), crate::i18n::Lang::name);
            let before = app.language.clone();
            egui::ComboBox::from_id_salt("interface-language").selected_text(selected).width(200.0).show_ui(ui, |ui| {
                if ui.selectable_value(&mut app.language, crate::i18n::AUTO.to_string(), tl!("Auto")).clicked() {
                    ui.close();
                }
                for language in crate::i18n::Lang::all() {
                    if ui.selectable_value(&mut app.language, language.code().to_string(), language.name()).clicked() {
                        ui.close();
                    }
                }
            });
            // Relabel the rest of this dialog in the new language right away, not next frame.
            if app.language != before {
                crate::i18n::set_current(crate::i18n::Lang::from_pref(&app.language));
            }
        });
        row(
            ui,
            t,
            tl!("Interface size"),
            Some(tl!("Auto follows the system's text size. Pages always render at the display's full resolution.")),
            |ui| {
                use crate::ui_scale;
                // Auto shows the size it draws at: the display's scale times the system's text scale.
                let native = ui.ctx().native_pixels_per_point().unwrap_or(1.0);
                let auto_percent = ui_scale::percent(ui_scale::drawn(None, app.system_text_scale, native));
                let auto = crate::i18n::fmt(tl!("Auto ({percent}%)"), &[("percent", &auto_percent.to_string())]);
                let selected = app.ui_scale.map_or_else(|| auto.clone(), |f| format!("{}%", ui_scale::percent(f)));
                egui::ComboBox::from_id_salt("interface-size").selected_text(selected).width(200.0).show_ui(ui, |ui| {
                    if ui.selectable_value(&mut app.ui_scale, None, auto).clicked() {
                        ui.close();
                    }
                    for p in ui_scale::PRESETS {
                        let factor = f32::from(p) / 100.0;
                        if ui.selectable_label(app.ui_scale.is_some_and(|f| ui_scale::percent(f) == u32::from(p)), format!("{p}%")).clicked() {
                            app.ui_scale = Some(factor);
                            ui.close();
                        }
                    }
                });
            },
        );
        row(ui, t, tl!("Tools panel side"), None, |ui| {
            let right = app.docking.tools_on_right();
            // Right to left: Right, then Left.
            if ui.selectable_label(right, tl!("Right")).clicked()
                && let Err(error) = app.docking.set_tools_on_right(true)
            {
                app.notify(error);
            }
            if ui.selectable_label(!right, tl!("Left")).clicked()
                && let Err(error) = app.docking.set_tools_on_right(false)
            {
                app.notify(error);
            }
        });
        // Only where the app draws the title bar (GNOME on Wayland, Windows); elsewhere the desktop draws it.
        if app.window_controls.is_some() {
            // The tip is about GNOME; on Windows the name says it all (Windows' own title bar returns).
            let about = (!cfg!(target_os = "windows")).then(|| {
                tl!("Show a basic title bar for the window buttons instead of the tab strip. GNOME doesn't draw title bars for apps, so this one doesn't follow its theme.")
            });
            switch_row(ui, t, &mut app.system_title_bar, tl!("Use system title bar"), about);
        }
    });
    card(ui, t, "panels-top-left", tl!("Floating bars"), Some(tl!("The bars beside the page and the tab strip above it.")), |ui| {
        named_row(ui, t, tl!("Fade side panels"), None, true, |ui| {
            ui.add_enabled_ui(app.chrome_fades, |ui| seconds(ui, "chrome-fade-after", &mut app.chrome_fade_after, &widgets::FADE_CHOICES));
            ui.add_space(8.0);
            switch(ui, &mut app.chrome_fades, tl!("Fade side panels"));
        });
        named_row(
            ui,
            t,
            tl!("Auto hide side panels"),
            Some(tl!(
                "Slide the floating bars beside the page out of the way when they are not in use. Move the pointer to the window's edge to bring them back."
            )),
            true,
            |ui| {
                ui.add_enabled_ui(app.auto_hide_bars, |ui| seconds(ui, "chrome-hide-after", &mut app.chrome_hide_after, &widgets::HIDE_CHOICES));
                ui.add_space(8.0);
                switch(ui, &mut app.auto_hide_bars, tl!("Auto hide side panels"));
            },
        );
    });
    card(ui, t, "gauge", tl!("Motion"), None, |ui| {
        ui.label(RichText::new(tl!("Scroll speed (mouse wheel and arrow keys)")).font(theme::medium(13.0)).color(t.text));
        let speeds = crate::scroll_speed::ScrollSpeed::ALL;
        let items: Vec<(Option<&str>, &str, bool)> =
            speeds.iter().map(|s| (None, tl_ctx!("scroll speed", s.label()), app.scroll_speed == *s)).collect();
        if let Some(s) = segments(ui, &items).and_then(|i| speeds.get(i)) {
            app.scroll_speed = *s;
        }
        // Whether anything animates, the wheel's scrolling and zooming included; left alone it
        // follows the system's animation setting.
        ui.label(RichText::new(tl!("Animation effects")).font(theme::medium(13.0)).color(t.text));
        let effects = crate::motion::AnimationEffects::ALL;
        let items: Vec<(Option<&str>, &str, bool)> =
            effects.iter().map(|e| (None, tl_ctx!("animation effects", e.label()), app.animation_effects == *e)).collect();
        if let Some(e) = segments(ui, &items).and_then(|i| effects.get(i)) {
            app.animation_effects = *e;
        }
    });
}

fn appearance(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, "palette", tl!("Appearance mode"), Some(tl!("Follow the system, or always light or dark.")), |ui| {
        crate::appearance::preferences_section(ui, app, t);
    });
}

fn documents(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(
        ui,
        t,
        "layout-grid",
        tl!("Default workspace mode"),
        Some(tl!("Used when opening PDFs. An explicit launch or control mode takes precedence for the session.")),
        |ui| {
            let modes = [
                (crate::Mode::AllTools, "layout-grid", tl!("All tools")),
                (crate::Mode::Read, "book-open", tl!("Read")),
                (crate::Mode::Edit, "pencil", tl!("Edit")),
                (crate::Mode::Convert, "file-output", tl!("Convert")),
                (crate::Mode::Sign, "signature", tl!("E-Sign")),
            ];
            let items: Vec<(Option<&str>, &str, bool)> = modes.iter().map(|(m, icon, label)| (Some(*icon), *label, app.default_mode == *m)).collect();
            if let Some((m, _, _)) = segments(ui, &items).and_then(|i| modes.get(i)) {
                app.default_mode = *m;
            }
        },
    );
    let defaults = &mut app.view_defaults;
    card(
        ui,
        t,
        "book-open",
        tl!("Default page display"),
        Some(tl!("Used when a PDF doesn't ask for a layout or zoom. Continuous scrolling never snaps between pages.")),
        |ui| {
            let layouts = crate::canvas::PageLayout::ORDER;
            let items: Vec<(Option<&str>, &str, bool)> = layouts.iter().map(|l| (Some(l.icon()), tl!(l.label()), defaults.layout == *l)).collect();
            if let Some(l) = segments(ui, &items).and_then(|i| layouts.get(i)) {
                defaults.layout = *l;
            }
            switch_row(ui, t, &mut defaults.continuous, tl!("Continuous scrolling"), None);
        },
    );
    card(ui, t, "zoom-in", tl!("Default zoom"), None, |ui| {
        use crate::canvas::Fit;
        let fits = [(Fit::Width, tl!("Fit to width")), (Fit::Page, tl!("Zoom to page level")), (Fit::None, tl!("Custom"))];
        let items: Vec<(Option<&str>, &str, bool)> = fits.iter().map(|(f, label)| (None, *label, defaults.fit == *f)).collect();
        if let Some((f, _)) = segments(ui, &items).and_then(|i| fits.get(i)) {
            defaults.fit = *f;
        }
        row(ui, t, tl!("Custom"), None, |ui| {
            // Editing the percentage selects Custom.
            let mut percent = defaults.zoom * 100.0;
            if ui.add(egui::DragValue::new(&mut percent).range(8.0..=6400.0).max_decimals(0).suffix("%")).changed() {
                (defaults.fit, defaults.zoom) = (Fit::None, percent / 100.0);
            }
        });
    });
    card(ui, t, "folder-open", tl!("Files and units"), None, |ui| {
        ui.label(RichText::new(tl!("Units")).font(theme::medium(13.0)).color(t.text));
        let units = crate::units::Unit::ALL;
        let items: Vec<(Option<&str>, &str, bool)> = units.iter().map(|u| (None, u.label(), app.units == *u)).collect();
        if let Some(u) = segments(ui, &items).and_then(|i| units.get(i)) {
            app.units = *u;
        }
        note(ui, t, tl!("For page sizes and margins."));
        // The web build has no file paths to reopen, nor files on disk to watch.
        #[cfg(not(target_arch = "wasm32"))]
        {
            switch_row(ui, t, &mut app.reopen_last_session, tl!("Reopen the files that were open when PdfCraft last closed"), None);
            switch_row(ui, t, &mut app.reload_changed_files, tl!("Reload documents when their files change on disk"), None);
        }
    });
}

fn reading_colours(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    // Reading colours (Acrobat: Replace Document Colors): solid text and line-art paints are
    // remapped while rendering, and the page clears to the background colour. The preference is
    // app-wide: every open document follows it, and documents open with it.
    card(
        ui,
        t,
        "droplet",
        tl!("Replace document colours"),
        Some(tl!("Colours are remapped while rendering; the document itself never changes.")),
        |ui| {
            row(ui, t, tl!("Reading colours"), None, |ui| crate::js_ui::reading_colours_combo(ui, app));
            preview(ui, t, app.reading_colors);
            if app.reading_colors.params.is_some() {
                let mut changed = false;
                if let Some(params) = app.reading_colors.params.as_mut() {
                    row(ui, t, tl!("Text"), None, |ui| changed |= ui.color_edit_button_srgb(&mut params.text).changed());
                    row(ui, t, tl!("Page background"), None, |ui| changed |= ui.color_edit_button_srgb(&mut params.background).changed());
                    changed |= switch_row(ui, t, &mut params.only_black, tl!("Only change the colour of black text or line art"), None);
                    changed |= switch_row(ui, t, &mut params.line_art, tl!("Change the colour of line art as well as text"), None);
                }
                if changed {
                    app.set_reading_colors(app.reading_colors);
                }
            }
        },
    );
}

/// The page as documents look today, and as they read with `colors`.
fn preview(ui: &mut egui::Ui, t: &Tokens, colors: pdfcraft_render::ReadingColors) {
    let page = |ui: &mut egui::Ui, bg: Color32, ink: Color32, label: &str| {
        ui.vertical(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(120.0, 76.0), Sense::hover());
            ui.painter().rect(r, CornerRadius::same(8), bg, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            ui.painter().rect_filled(Rect::from_min_size(r.min + vec2(10.0, 12.0), vec2(56.0, 6.0)), 3.0, ink);
            for (i, w) in [96.0, 90.0, 60.0].iter().enumerate() {
                ui.painter().rect_filled(
                    Rect::from_min_size(r.min + vec2(10.0, 30.0 + i as f32 * 12.0), vec2(*w, 4.0)),
                    2.0,
                    ink.gamma_multiply(0.7),
                );
            }
            ui.label(RichText::new(label).font(theme::medium(11.5)).color(t.text_muted));
        });
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        page(ui, Color32::WHITE, Color32::from_gray(0x30), tl!("Document"));
        match colors.params {
            Some(p) => {
                let c = |v: [u8; 3]| Color32::from_rgb(v[0], v[1], v[2]);
                page(ui, c(p.background), c(p.text), tl!("Reading colours"));
            }
            None => page(ui, Color32::WHITE, Color32::from_gray(0x30), tl!("Off")),
        }
    });
}

fn identity(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    // Identity: the author of new comments (Acrobat: Preferences ▸ Identity).
    card(ui, t, "user-round", tl!("Identity"), None, |ui| {
        ui.horizontal(|ui| {
            let label = ui.label(RichText::new(tl!("Name on new comments")).font(theme::medium(13.0)).color(t.text));
            ui.add(widgets::line(&mut app.comment_prefs.author).desired_width(ui.available_width()).char_limit(crate::MAX_AUTHOR_CHARS))
                .labelled_by(label.id);
        });
    });
    card(ui, t, "message-square-text", tl!("Comments"), None, |ui| {
        switch_row(ui, t, &mut app.comment_prefs.hide_empty_highlight_hover, tl!("Hide hover previews for highlights without comment text"), None);
    });
}

fn fill_sign(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, "file-lock-2", tl!("Fill & Sign"), None, |ui| {
        switch_row(
            ui,
            t,
            &mut app.flatten_fill_sign_on_save,
            tl!("Flatten Fill & Sign when saving"),
            Some(tl!("Text, marks and signatures become part of the page. Other comments stay editable.")),
        );
    });
    card(ui, t, "clock", tl!("Date format"), None, |ui| crate::js_ui::date_format(ui, app, t));
}

fn scripting(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, "square-terminal", "JavaScript", None, |ui| {
        let mut on = app.session.javascript();
        if switch_row(ui, t, &mut on, tl!("Enable Acrobat JavaScript"), None) {
            app.session.set_javascript(on);
        }
        egui::Frame::NONE.fill(t.card).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                icons::paint(ui, r, "shield-check", 16.0, t.accent_text);
                ui.add(
                    egui::Label::new(
                        RichText::new(tl!("Scripts run in a sandbox without file or network access. A script that asks to open a web page needs your permission first. With JavaScript off, Acrobat's standard format, validate and calculate functions still work."))
                            .font(theme::regular(12.5))
                            .color(t.text_muted),
                    )
                    .wrap(),
                );
            });
        });
    });
}
