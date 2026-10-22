//! Real navigation controls in separate native viewports sharing one egui context.

use crate::{PdfCraftApp, chrome, docking, motion};
use egui::{Event, Key, Modifiers, ViewportId};

fn app() -> PdfCraftApp {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    let bytes = app.session.create_blank(200.0, 300.0, 12).unwrap();
    app.open_bytes("navigation.pdf", None, bytes.as_ref().clone()).unwrap();
    app
}

struct Windows {
    ctx: egui::Context,
    time: f64,
}

impl Windows {
    fn new() -> Self {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        egui_extras::install_image_loaders(&ctx);
        ctx.enable_accesskit();
        Self { ctx, time: 0.0 }
    }

    fn frame(&mut self, app: &mut PdfCraftApp, id: ViewportId, events: Vec<Event>, command: Option<&str>) -> egui::FullOutput {
        self.time += 0.1;
        let mut raw = egui::RawInput {
            viewport_id: id,
            time: Some(self.time),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))),
            events,
            ..Default::default()
        };
        raw.viewports.insert(id, egui::ViewportInfo::default());
        let mut out = self.ctx.run_ui(raw, |ui| {
            app.ctx = Some(ui.ctx().clone());
            app.motion.begin_pass(ui.ctx(), app.animation_effects);
            if let Some(command) = command {
                assert!(app.execute(command));
            }
            app.shortcuts(ui.ctx());
            chrome::right_rail(app, ui);
            egui::CentralPanel::default().show(ui, |ui| docking::show(app, ui));
        });
        out.textures_delta.clear();
        out
    }

    fn click(&mut self, app: &mut PdfCraftApp, id: ViewportId, at: egui::Pos2) {
        self.frame(app, id, vec![Event::PointerMoved(at)], None);
        for pressed in [true, false] {
            self.frame(
                app,
                id,
                vec![Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE }],
                None,
            );
        }
    }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

fn control(out: &egui::FullOutput, label: &str) -> egui::Pos2 {
    let update = out.platform_output.accesskit_update.as_ref().unwrap();
    let rect = update
        .nodes
        .iter()
        .find_map(|(_, node)| {
            (node.label().is_some_and(|text| text == label || text.starts_with(&format!("{label} (")))).then(|| node.bounds()).flatten()
        })
        .unwrap_or_else(|| panic!("missing control {label}"));
    egui::pos2(((rect.x0 + rect.x1) / 2.0) as f32, ((rect.y0 + rect.y1) / 2.0) as f32)
}

#[test]
fn undo_in_an_unedited_page_box_cannot_restore_another_windows_page_number() {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let child = ViewportId::from_hash_of("second-document");
    second.views[0].go_to_page(2);
    windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
    windows.frame(&mut second, child, Vec::new(), Some("view.focus_page_input"));
    windows.frame(&mut second, child, vec![key(Key::Z, Modifiers::COMMAND)], None);
    assert_eq!(second.views[0].page_input, "3", "undo in an unedited field must not import the other PDF's page number");
    assert_eq!(first.views[0].current, 0);
}

#[test]
fn undo_in_find_cannot_import_another_windows_query() {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let child = ViewportId::from_hash_of("second-document");
    for (app, query) in [(&mut first, "alpha"), (&mut second, "beta")] {
        app.views[0].open_find();
        app.views[0].find.as_mut().unwrap().query = query.into();
    }
    windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
    windows.frame(&mut second, child, Vec::new(), None);
    windows.frame(&mut second, child, vec![key(Key::Z, Modifiers::COMMAND)], None);
    assert_eq!(second.views[0].find.as_ref().unwrap().query, "beta", "find undo belongs to its own PDF window");
    assert_eq!(first.views[0].find.as_ref().unwrap().query, "alpha");
}

#[test]
fn bookmark_filter_does_not_hide_another_windows_bookmarks() {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let child = ViewportId::from_hash_of("second-document");
    for app in [&mut first, &mut second] {
        app.apply_edit(pdfcraft_engine::Edit::AddBookmark { parent: Vec::new(), index: 0, title: "Chapter Alpha".into(), page: 0 });
        app.set_option("panel", "bookmarks").unwrap();
    }
    let out = windows.frame(&mut first, ViewportId::ROOT, Vec::new(), None);
    windows.click(&mut first, ViewportId::ROOT, control(&out, "Search"));
    windows.frame(&mut first, ViewportId::ROOT, vec![Event::Text("missing".into())], None);
    let out = windows.frame(&mut second, child, Vec::new(), None);
    let painted: Vec<_> = out
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    assert!(painted.contains(&"Chapter Alpha"), "another window's query hid this PDF's bookmark: {painted:?}");
}

#[test]
fn reduced_motion_zoom_cache_cannot_zoom_another_viewport() {
    let ctx = egui::Context::default();
    let child = ViewportId::from_hash_of("second-document");
    let mut first = motion::Motion::default();
    let mut second = motion::Motion::default();
    ctx.run_ui(egui::RawInput { events: vec![Event::Zoom(1.25)], ..Default::default() }, |ui| {
        first.begin_pass(ui.ctx(), motion::AnimationEffects::Off);
        assert_eq!(motion::zoom_delta(ui.ctx()), 1.25);
        let mut raw = egui::RawInput { viewport_id: child, ..Default::default() };
        raw.viewports.insert(child, egui::ViewportInfo::default());
        ctx.run_ui(raw, |child_ui| {
            second.begin_pass(child_ui.ctx(), motion::AnimationEffects::On);
            assert_eq!(motion::zoom_delta(child_ui.ctx()), 1.0, "a viewport without a gesture must not inherit another window's zoom");
        })
        .textures_delta
        .clear();
    })
    .textures_delta
    .clear();
}

#[test]
fn navigation_buttons_and_shortcuts_change_only_their_own_window() {
    let child = ViewportId::from_hash_of("second-document");
    for (target, other) in [(ViewportId::ROOT, child), (child, ViewportId::ROOT), (child, ViewportId::from_hash_of("third-document"))] {
        check_navigation(target, other);
    }
}

fn check_navigation(target: ViewportId, other: ViewportId) {
    let mut windows = Windows::new();
    let (mut first, mut second) = (app(), app());
    let out = windows.frame(&mut first, target, Vec::new(), None);
    windows.frame(&mut second, other, Vec::new(), None);
    let untouched = (second.views[0].current, second.views[0].zoom, second.views[0].fit, second.views[0].rotation);
    let next = control(&out, "Next page");
    let zoom = control(&out, "Zoom in");
    windows.click(&mut first, target, next);
    assert_eq!(first.views[0].current, 1);
    let before = first.views[0].zoom;
    windows.click(&mut first, target, zoom);
    assert!(first.views[0].zoom > before);
    // Keyboard navigation is addressed to the document rather than a focused rail button.
    let document = first.views[0].viewport_rect().center();
    windows.click(&mut first, target, document);
    for (key_code, modifiers) in
        [(Key::ArrowRight, Modifiers::NONE), (Key::Plus, Modifiers::COMMAND), (Key::Plus, Modifiers::COMMAND | Modifiers::SHIFT)]
    {
        let changed_before = (first.views[0].current, first.views[0].zoom, first.views[0].rotation);
        windows.frame(&mut first, target, vec![key(key_code, modifiers)], None);
        windows.frame(&mut second, other, Vec::new(), None);
        if key_code == Key::ArrowRight {
            assert_eq!(first.views[0].current, changed_before.0 + 1, "the target PDF must receive the page key");
        } else if modifiers.shift {
            assert_ne!(first.views[0].rotation, changed_before.2, "the target PDF must rotate");
        } else {
            assert!(first.views[0].zoom > changed_before.1, "the target PDF must receive the zoom shortcut");
        }
        assert_eq!((second.views[0].current, second.views[0].zoom, second.views[0].fit, second.views[0].rotation), untouched);
    }
    windows.frame(&mut first, target, Vec::new(), Some("view.focus_page_input"));
    windows.frame(&mut first, target, vec![Event::Text("5".into())], None);
    windows.frame(&mut first, target, vec![key(Key::Enter, Modifiers::NONE)], None);
    windows.frame(&mut second, other, Vec::new(), None);
    assert_eq!(first.views[0].current, 4, "the page-number shortcut and Enter must address the target window's field");
    assert_eq!((second.views[0].current, second.views[0].zoom, second.views[0].fit, second.views[0].rotation), untouched);
}

#[test]
fn pinch_and_command_wheel_zoom_only_the_hovered_window() {
    let child = ViewportId::from_hash_of("second-document");
    for (target, other) in [(ViewportId::ROOT, child), (child, ViewportId::ROOT)] {
        for effects in ["on", "off"] {
            for gesture in [
                Event::Zoom(1.2),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 40.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::COMMAND,
                },
            ] {
                let mut windows = Windows::new();
                let (mut first, mut second) = (app(), app());
                first.set_option("animation_effects", effects).unwrap();
                second.set_option("animation_effects", effects).unwrap();
                first.set_option("zoom", "150").unwrap();
                second.set_option("zoom", "80").unwrap();
                windows.frame(&mut first, target, Vec::new(), None);
                windows.frame(&mut second, other, Vec::new(), None);
                let pointer = first.views[0].viewport_rect().center();
                let untouched = (second.views[0].current, second.views[0].zoom, second.views[0].page_screen_rect(0));
                windows.frame(&mut first, target, vec![Event::PointerMoved(pointer), gesture], None);
                for _ in 0..5 {
                    windows.frame(&mut first, target, Vec::new(), None);
                    windows.frame(&mut second, other, Vec::new(), None);
                }
                assert!(first.views[0].zoom > 1.5, "the target window must receive the gesture");
                assert_eq!((second.views[0].current, second.views[0].zoom, second.views[0].page_screen_rect(0)), untouched);
            }
        }
    }
}

#[test]
fn single_page_wheel_turns_only_the_hovered_windows_page() {
    let child = ViewportId::from_hash_of("second-document");
    for (target, other) in [(ViewportId::ROOT, child), (child, ViewportId::ROOT)] {
        let mut windows = Windows::new();
        let (mut first, mut second) = (app(), app());
        for app in [&mut first, &mut second] {
            app.set_option("layout", "single").unwrap();
            app.set_option("zoom", "50").unwrap();
        }
        windows.frame(&mut first, target, Vec::new(), None);
        windows.frame(&mut second, other, Vec::new(), None);
        let pointer = first.views[0].viewport_rect().center();
        windows.frame(
            &mut first,
            target,
            vec![
                Event::PointerMoved(pointer),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -1.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                },
            ],
            None,
        );
        windows.frame(&mut second, other, Vec::new(), None);
        assert_eq!(first.views[0].current, 1);
        assert_eq!(second.views[0].current, 0);
    }
}
