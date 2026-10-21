//! The macOS menu bar mirrors the command registry and the window's view menus.
//! The model and dispatch are tested without AppKit; only `macos` touches native objects.

use pdfcraft_engine::commands::{self, Shortcut};

use crate::{PdfCraftApp, RightPanel, canvas::Fit, theme::ThemePreference};

#[cfg(target_os = "macos")]
mod macos;

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    id: String,
    label: String,
    enabled: bool,
    shortcut: Option<Shortcut>,
    checked: Option<bool>,
    children: Vec<Entry>,
}

impl Entry {
    fn item(id: &str, label: &str, enabled: bool, shortcut: Option<Shortcut>) -> Self {
        Self { id: id.into(), label: label.into(), enabled, shortcut, checked: None, children: Vec::new() }
    }

    fn group(id: &str, label: &str, children: Vec<Self>) -> Self {
        Self { children, ..Self::item(id, tl!(label), true, None) }
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    fn find(&self, id: &str) -> Option<&Self> {
        if self.id == id { Some(self) } else { self.children.iter().find_map(|child| child.find(id)) }
    }
}

const PANELS: &[(RightPanel, &str, &str)] = &[
    (RightPanel::Comments, "panel.comments", "Comments"),
    (RightPanel::Bookmarks, "panel.bookmarks", "Bookmarks"),
    (RightPanel::Pages, "panel.pages", "Pages"),
    (RightPanel::Fields, "panel.fields", "Fields"),
    (RightPanel::Layers, "panel.layers", "Layers"),
    (RightPanel::Attachments, "panel.attachments", "Attachments"),
    (RightPanel::Signatures, "panel.signatures", "Signatures"),
    (RightPanel::Accessibility, "panel.accessibility", "Accessibility Checker"),
    (RightPanel::Tags, "panel.tags", "Tags"),
    (RightPanel::Search, "panel.search", "Search"),
    (RightPanel::Compare, "panel.compare", "Compare"),
];

fn entries(app: &PdfCraftApp, ctx: &egui::Context) -> Vec<Entry> {
    let free = !app.modal_open() && !app.palette_open;
    let typing = ctx.text_edit_focused();
    let active = app.active_ids().map(|(_, id)| id);
    let mut menus = Vec::new();
    for name in ["File", "Edit", "Pages", "View", "Help"] {
        let mut items = Vec::new();
        for spec in commands::menu(name) {
            let label = commands::current_label(spec, &app.session, active);
            let text_action = typing && matches!(spec.id, "edit.undo" | "edit.redo" | "edit.copy");
            let label = crate::i18n::menu_label(spec.id, if text_action { spec.label } else { &label });
            let enabled = text_action || (free && (app.command_enabled(spec) || (spec.id == "file.close" && active.is_none())));
            let mut item = Entry::item(spec.id, &label, enabled, spec.shortcut);
            if spec.id == "edit.copy" {
                item.shortcut = Some(crate::commands::COPY);
                item.enabled |= free && app.active.and_then(|i| app.views.get(i)).is_some_and(|v| v.organize);
            }
            if spec.id == "file.open_recent" {
                item.enabled = free && !app.recent.is_empty();
                // The normal MRU list holds twelve files. Bound the native menu too, even if a
                // hand-edited settings file restored a longer list.
                item.children = app.recent.iter().take(12).map(|r| Entry::item(&format!("recent:{}", r.path), &r.name, free, None)).collect();
                item.children.push(Entry::item("file.clear_recent", tl!("Clear Recent Files"), free && !app.recent.is_empty(), None));
            }
            items.push(item);
        }
        if name == "View" {
            let document = free && active.is_some();
            use crate::commands::{ACTUAL_SIZE, FIT_WIDTH, PAGE_LEVEL, ROTATE_CCW, ROTATE_CW, ZOOM_IN, ZOOM_OUT};
            let mut view = vec![
                Entry::item("zoom.actual", tl!("Actual size"), document, Some(ACTUAL_SIZE)),
                Entry::item("zoom.page", tl!("Zoom to page level"), document, Some(PAGE_LEVEL)),
                Entry::item("zoom.width", tl!("Fit to width"), document, Some(FIT_WIDTH)),
                Entry::item("zoom.height", tl!("Fit to height"), document, None),
                registry_item(app, "view.fit_visible", document),
                Entry::item("zoom.in", tl!("Zoom in"), document, Some(ZOOM_IN)),
                Entry::item("zoom.out", tl!("Zoom out"), document, Some(ZOOM_OUT)),
                Entry::item("rotate.cw", tl!("Rotate view clockwise"), document, Some(ROTATE_CW)),
                Entry::item("rotate.ccw", tl!("Rotate view counterclockwise"), document, Some(ROTATE_CCW)),
                registry_item(app, "view.previous_view", free),
                registry_item(app, "view.next_view", free),
            ];
            let current = app.active.and_then(|i| app.views.get(i));
            let display =
                ["view.layout.single", "view.layout.two_up", "view.continuous", "view.layout.cover", "view.fit_width_scrolling", "view.fit_one_page"]
                    .into_iter()
                    .map(|id| {
                        let item = registry_item(app, id, document);
                        match (id, current) {
                            ("view.layout.single", Some(v)) => item.checked(v.layout == crate::canvas::PageLayout::Single),
                            ("view.layout.two_up", Some(v)) => item.checked(v.layout == crate::canvas::PageLayout::TwoUp),
                            ("view.continuous", Some(v)) => item.checked(v.continuous()),
                            ("view.layout.cover", Some(v)) => item.checked(v.cover),
                            _ => item,
                        }
                    })
                    .collect();
            view.push(Entry::group("display", "Page display", display));
            view.append(&mut items);
            let themes = [
                ("view.theme.system", ThemePreference::System),
                ("view.theme.light", ThemePreference::Light),
                ("view.theme.dark", ThemePreference::Dark),
            ]
            .into_iter()
            .map(|(id, theme)| registry_item(app, id, free).checked(app.theme_preference == theme))
            .chain(std::iter::once(registry_item(app, "app.preferences", free)))
            .collect();
            view.push(Entry::group("theme", "Display theme", themes));
            view.push(Entry::group(
                "panels",
                "Side panels",
                PANELS.iter().map(|(p, id, label)| Entry::item(id, tl!(label), document, None).checked(app.right == Some(*p))).collect(),
            ));
            items = view;
        }
        menus.push(Entry::group(name, name, items));
    }
    menus
}

fn registry_item(app: &PdfCraftApp, id: &str, enabled: bool) -> Entry {
    let Some(spec) = commands::command(id) else { return Entry::item(id, id, false, None) };
    Entry::item(id, &crate::i18n::menu_label(id, spec.label), enabled && app.command_enabled(spec), spec.shortcut)
}

/// Shortcuts enter as real egui input, rather than executing a second action alongside winit.
/// This preserves text-editor undo/copy and the existing form-commit-before-save path.
fn activate(app: &mut PdfCraftApp, ctx: &egui::Context, raw: &mut egui::RawInput, id: &str) {
    let menus = entries(app, ctx);
    let Some(item) = menus.iter().find_map(|menu| menu.find(id)).filter(|item| item.enabled && item.children.is_empty()) else { return };
    if id == "edit.copy" {
        raw.events.push(egui::Event::Copy);
        return;
    }
    if commands::command(id).is_some()
        && let Some(shortcut) = item.shortcut
        && let Some(key) = egui::Key::from_name(shortcut.key)
    {
        let modifiers = egui::Modifiers {
            command: shortcut.command,
            mac_cmd: shortcut.command,
            ctrl: shortcut.mac_ctrl,
            shift: shortcut.shift,
            ..Default::default()
        };
        for pressed in [true, false] {
            raw.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        }
        return;
    }
    if let Some(path) = id.strip_prefix("recent:") {
        app.open_recent(path);
    } else if commands::command(id).is_some() {
        app.execute(id);
    } else if let Some((panel, _, _)) = PANELS.iter().find(|(_, panel_id, _)| *panel_id == id) {
        app.choose_right_panel(Some(*panel));
    } else if let Some(view) = app.active.and_then(|i| app.views.get_mut(i)) {
        match id {
            "zoom.actual" => view.set_zoom(1.0),
            "zoom.page" => view.fit = Fit::Page,
            "zoom.width" => view.fit = Fit::Width,
            "zoom.height" => {
                view.fit = Fit::Height;
                view.goto = Some((view.current, 0.0));
            }
            "zoom.in" => view.zoom_step(true),
            "zoom.out" => view.zoom_step(false),
            "rotate.cw" => view.rotate_view(true),
            "rotate.ccw" => view.rotate_view(false),
            _ => {}
        }
    }
}

#[cfg(target_os = "macos")]
impl PdfCraftApp {
    pub(crate) fn inspect_native_menus(&self) -> crate::control::Reply {
        Ok(macos::inspect())
    }

    pub(crate) fn activate_native_menu(&mut self, ctx: &egui::Context, id: &str) -> Result<Vec<egui::Event>, String> {
        if craft_quit::on_menu_event(ctx, id) {
            return Ok(Vec::new());
        }
        let menus = entries(self, ctx);
        let item = menus.iter().find_map(|m| m.find(id)).ok_or_else(|| format!("Unknown menu item {id:?}; see ui.native_menus"))?;
        if !item.enabled || !item.children.is_empty() {
            return Err(format!("Menu item {id:?} is disabled or is a submenu"));
        }
        let mut raw = egui::RawInput::default();
        activate(self, ctx, &mut raw, id);
        Ok(raw.events)
    }

    /// Install the macOS menu bar from the desktop app creator, on the main thread only.
    /// Headless tests and the other frontends never install a native menu.
    pub fn install_native_menus(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if objc2::MainThreadMarker::new().is_none() {
            return Err("The macOS menu bar must be installed on the main thread".into());
        }
        crate::i18n::set_current(crate::i18n::Lang::from_pref(&self.language));
        self.native_menu_events = Some(macos::install(ctx, &entries(self, ctx)).map_err(|e| format!("Couldn't install the macOS menu bar: {e}"))?);
        Ok(())
    }

    pub(crate) fn native_menu_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let events: Vec<_> = self.native_menu_events.as_ref().map(|rx| rx.try_iter().take(64).collect()).unwrap_or_default();
        for id in events {
            activate(self, ctx, raw, &id);
        }
    }

    pub(crate) fn sync_native_menus(&self, ctx: &egui::Context) {
        if self.native_menu_events.is_some()
            && let Err(e) = macos::sync(&entries(self, ctx))
        {
            log::warn!("Couldn't update the macOS menu bar: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    fn harness() -> Harness<'static, PdfCraftApp> {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
            cc.egui_ctx.set_os(egui::os::OperatingSystem::Mac);
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("form.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            app.set_option("left", "closed").unwrap();
            app.set_option("zoom", "150").unwrap();
            app
        });
        h.run_steps(4);
        h
    }

    fn snapshot(app: &PdfCraftApp) -> Vec<Entry> {
        entries(app, app.ctx.as_ref().unwrap())
    }

    fn find<'a>(menus: &'a [Entry], id: &str) -> &'a Entry {
        menus.iter().find_map(|m| m.find(id)).unwrap()
    }

    fn choose(h: &mut Harness<'static, PdfCraftApp>, id: &str) {
        let ctx = h.state().ctx.clone().unwrap();
        let mut raw = egui::RawInput::default();
        activate(h.state_mut(), &ctx, &mut raw, id);
        for event in raw.events {
            h.event(event);
        }
        h.run_steps(3);
    }

    #[test]
    fn native_menus_include_every_registry_menu_and_view_choice() {
        let h = harness();
        let menus = snapshot(h.state());
        assert_eq!(menus.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["File", "Edit", "Pages", "View", "Help"]);
        for spec in commands::COMMANDS.iter().filter(|s| s.menu.is_some()) {
            let menu = find(&menus, spec.menu.unwrap());
            let item = menu.find(spec.id).unwrap();
            assert_eq!(item.shortcut, spec.shortcut.or((spec.id == "edit.copy").then_some(crate::commands::COPY)), "{}", spec.id);
        }
        for id in
            ["zoom.actual", "zoom.page", "zoom.width", "zoom.height", "zoom.in", "zoom.out", "rotate.cw", "rotate.ccw", "display", "theme", "panels"]
        {
            assert!(find(&menus, "View").find(id).is_some(), "{id}");
        }
    }

    #[test]
    fn native_page_edit_undo_save_and_reopen_use_existing_actions() {
        let mut h = harness();
        let id = h.state().views[0].id;
        let rotation = h.state().session.get(id).unwrap().info.pages[0].rotation;
        assert!(!find(&snapshot(h.state()), "edit.undo").enabled);
        choose(&mut h, "page.rotate");
        assert_eq!(h.state().session.get(id).unwrap().info.pages[0].rotation, (rotation + 90) % 360);
        assert!(find(&snapshot(h.state()), "edit.undo").label.starts_with("Undo "));
        choose(&mut h, "edit.undo");
        assert_eq!(h.state().session.get(id).unwrap().info.pages[0].rotation, rotation, "one click is one edit, not two");
        choose(&mut h, "edit.redo");
        let dir = std::env::temp_dir().join(format!("pdfcraft-native-menu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("saved.pdf");
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        choose(&mut h, "file.save");
        let mut reopened = PdfCraftApp::new();
        reopened.open_bytes("saved.pdf", None, std::fs::read(&path).unwrap()).unwrap();
        let doc = reopened.session.get(reopened.views[0].id).unwrap();
        assert_eq!(doc.info.pages[0].rotation, (rotation + 90) % 360);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn native_menus_follow_document_modal_recent_and_theme_state() {
        let mut h = harness();
        choose(&mut h, "view.theme.dark");
        assert_eq!(find(&snapshot(h.state()), "view.theme.dark").checked, Some(true));
        choose(&mut h, "view.layout.two_up");
        assert_eq!(find(&snapshot(h.state()), "view.layout.two_up").checked, Some(true));
        choose(&mut h, "panel.bookmarks");
        assert_eq!(h.state().right, Some(RightPanel::Bookmarks));
        choose(&mut h, "zoom.height");
        assert_eq!(h.state().views[0].fit, Fit::Height);
        choose(&mut h, "app.preferences");
        assert!(h.state().dialog.is_some());
        assert!(!find(&snapshot(h.state()), "page.rotate").enabled);
        let id = h.state().views[0].id;
        let rotation = h.state().session.get(id).unwrap().info.pages[0].rotation;
        choose(&mut h, "page.rotate");
        assert_eq!(h.state().session.get(id).unwrap().info.pages[0].rotation, rotation);
        h.state_mut().dialog = None;
        choose(&mut h, "file.close");
        assert!(h.state().views.is_empty());
        assert!(!find(&snapshot(h.state()), "file.save").enabled);
        assert!(find(&snapshot(h.state()), "file.close").enabled, "⌘W may close the last window");
        assert!(!find(&snapshot(h.state()), "file.open_recent").enabled);
        choose(&mut h, "recent:/not-in-the-list.pdf");
        assert!(h.state().views.is_empty(), "stale or invented recent entries are ignored");
    }

    #[test]
    fn native_copy_and_undo_belong_to_the_focused_text_editor() {
        let mut h = harness();
        choose(&mut h, "page.rotate");
        choose(&mut h, "edit.find");
        h.event(egui::Event::Text("hello".into()));
        h.run_steps(2);
        assert!(h.state().ctx.as_ref().unwrap().text_edit_focused());
        assert_eq!(find(&snapshot(h.state()), "edit.undo").label, "Undo");
        let id = h.state().views[0].id;
        let rotation = h.state().session.get(id).unwrap().info.pages[0].rotation;
        choose(&mut h, "edit.undo");
        assert_eq!(h.state().session.get(id).unwrap().info.pages[0].rotation, rotation, "don't undo a document behind a text editor");
        assert_eq!(h.state().views[0].find.as_ref().unwrap().query, "", "the focused editor's text is undone instead");
        let ctx = h.state().ctx.clone().unwrap();
        let mut raw = egui::RawInput::default();
        activate(h.state_mut(), &ctx, &mut raw, "edit.copy");
        assert!(matches!(raw.events.as_slice(), [egui::Event::Copy]));
    }

    #[test]
    fn native_recent_files_are_live_and_clear_uses_the_registry() {
        let mut h = harness();
        let path = std::env::temp_dir().join(format!("pdfcraft-native-recent-{}.pdf", std::process::id()));
        std::fs::write(&path, include_bytes!("../tests/data/form.pdf")).unwrap();
        let path_text = path.to_string_lossy().into_owned();
        h.state_mut().recent.push(crate::RecentFile { name: "Recent form".into(), path: path_text.clone(), pages: 1, size: 1 });
        let recent_id = format!("recent:{path_text}");
        assert!(find(&snapshot(h.state()), "file.open_recent").enabled);
        assert_eq!(find(&snapshot(h.state()), &recent_id).label, "Recent form");
        choose(&mut h, &recent_id);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while h.state().views.len() < 2 {
            assert!(std::time::Instant::now() < deadline, "recent file did not finish opening");
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(h.state().session.get(h.state().views[1].id).unwrap().path.as_deref(), Some(path_text.as_str()));
        choose(&mut h, "file.clear_recent");
        assert!(h.state().recent.is_empty());
        assert!(!find(&snapshot(h.state()), "file.open_recent").enabled);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn native_save_commits_the_focused_form_editor_before_writing() {
        let mut h = harness();
        let point = {
            let app = h.state();
            let doc = app.session.get(app.views[0].id).unwrap();
            let field = doc.form.iter().find(|f| f.name == "name").unwrap();
            crate::forms_ui::field_screen_rect(&app.views[0], &doc.info, field, 0).unwrap().center()
        };
        h.hover_at(point);
        h.step();
        h.drag_at(point);
        h.step();
        h.drop_at(point);
        h.run_steps(3);
        h.event(egui::Event::Text("Native menu form".into()));
        h.run_steps(2);
        assert!(h.state().views[0].forms.focus.is_some());
        let path = std::env::temp_dir().join(format!("pdfcraft-native-form-save-{}.pdf", std::process::id()));
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        choose(&mut h, "file.save");
        let mut reopened = PdfCraftApp::new();
        reopened.open_bytes("saved.pdf", None, std::fs::read(&path).unwrap()).unwrap();
        let doc = reopened.session.get(reopened.views[0].id).unwrap();
        assert_eq!(doc.form.iter().find(|f| f.name == "name").unwrap().value, ["Native menu form"]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn native_labels_follow_interface_language() {
        let mut h = harness();
        h.state_mut().set_option("language", "de").unwrap();
        h.run_steps(2);
        let menus = snapshot(h.state());
        assert_eq!(find(&menus, "File").label, "Datei");
        assert_eq!(find(&menus, "file.open").label, crate::i18n::menu_label("file.open", "Open…"));
    }

    #[test]
    fn native_recent_menu_is_bounded_even_for_oversized_restored_lists() {
        let mut h = harness();
        h.state_mut().recent =
            (0..30).map(|i| crate::RecentFile { name: format!("File {i}"), path: format!("/synthetic/{i}.pdf"), pages: 1, size: 1 }).collect();
        let menus = snapshot(h.state());
        let recent = find(&menus, "file.open_recent");
        assert_eq!(recent.children.len(), 13, "twelve files plus Clear Recent Files");
        assert_eq!(recent.children.last().unwrap().id, "file.clear_recent");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_menu_control_uses_the_same_adapter_and_rejects_bad_actions() {
        let mut h = harness();
        let ctx = h.state().ctx.clone().unwrap();
        let client = h.state_mut().attach_control(&ctx);
        let id = h.state().views[0].id;
        let rotation = h.state().session.get(id).unwrap().info.pages[0].rotation;
        let reply = client.send("ui.native_menu", serde_json::json!({"id": "page.rotate"}));
        h.run_steps(5);
        assert_eq!(reply.try_recv().unwrap().unwrap()["activated"], true);
        assert_eq!(h.state().session.get(id).unwrap().info.pages[0].rotation, (rotation + 90) % 360);
        let reply = client.send("ui.native_menu", serde_json::json!({"id": "unknown.command"}));
        h.run_steps(2);
        assert!(reply.try_recv().unwrap().unwrap_err().contains("Unknown menu item"));
        let reply = client.send("ui.native_menu", serde_json::json!({"id": "File"}));
        h.run_steps(2);
        assert!(reply.try_recv().unwrap().unwrap_err().contains("submenu"));
        let reply = client.send("ui.native_menus", serde_json::json!({}));
        h.run_steps(2);
        assert_eq!(reply.try_recv().unwrap().unwrap()["installed"], false, "headless tests never install an AppKit menu");
        let reply = client.send("ui.native_menu", serde_json::json!({"id": craft_quit::QUIT_MENU_ID}));
        h.step();
        assert!(h.output().viewport_output.get(&egui::ViewportId::ROOT).unwrap().commands.contains(&egui::ViewportCommand::Close));
        // The real backend turns the viewport command into this close request.
        h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        h.run_steps(5);
        assert!(reply.try_recv().unwrap().is_ok());
        assert!(h.state().close_request.is_some(), "native Quit asks before discarding the edit");
        assert_eq!(h.state().views.len(), 1);
    }
}
