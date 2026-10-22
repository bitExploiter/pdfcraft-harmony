//! macOS desktop host. The root window may be hidden; its event loop stays alive.
//! Each additional document owns an independent PdfCraftApp and a native egui viewport.

use eframe::App;
use pdfcraft_ui_egui::{OsEvent, OsEventsFn, PdfCraftApp, WindowOpen};
use std::sync::mpsc::{Receiver, Sender};

struct DocumentWindow {
    id: egui::ViewportId,
    app: PdfCraftApp,
}

pub struct Desktop {
    root: PdfCraftApp,
    children: Vec<DocumentWindow>,
    receiver: Receiver<WindowOpen>,
    sender: Sender<WindowOpen>,
    os_events: Option<OsEventsFn>,
    next_window: u64,
    quitting: bool,
    settings: String,
}

impl Desktop {
    pub fn new(mut root: PdfCraftApp, receiver: Receiver<WindowOpen>, sender: Sender<WindowOpen>) -> Self {
        let os_events = root.os_events.take();
        let settings = root.persist();
        Self { root, children: Vec::new(), receiver, sender, os_events, next_window: 0, quitting: false, settings }
    }

    fn new_document(&mut self, request: WindowOpen, ctx: &egui::Context) {
        if matches!(request, WindowOpen::Quit) {
            self.quit(ctx);
            return;
        }
        if matches!(request, WindowOpen::CancelQuit) {
            self.quitting = false;
            self.root.close_request = None;
            for window in &mut self.children {
                window.app.close_request = None;
            }
            return;
        }
        let local_file = match &request {
            WindowOpen::Path { path, then } => Some((path.as_str(), *then)),
            WindowOpen::Bytes { path: Some(path), .. } => Some((path.as_str(), None)),
            _ => None,
        };
        if let Some((path, then)) = local_file {
            let existing = if self.root.activate_open_path(path, then) {
                Some(egui::ViewportId::ROOT)
            } else {
                self.children.iter_mut().find_map(|window| window.app.activate_open_path(path, then).then_some(window.id))
            };
            if let Some(id) = existing {
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
                ctx.request_repaint_of(id);
                return;
            }
        }
        if self.root.window_closed || !self.root.window_occupied() {
            self.root.window_closed = false;
            self.root.dialog = None;
            Self::open(&mut self.root, request);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else {
            let mut app = PdfCraftApp::new();
            app.restore(&self.settings);
            app.single_document_window = true;
            app.window_open = Some(self.sender.clone());
            app.system_text_scale = self.root.system_text_scale;
            app.printer_source = self.root.printer_source.clone();
            app.update_source = self.root.update_source.clone();
            app.os_key_store_ids = true;
            // Workers wake the root until this document's own viewport has been created.
            app.prepare_native_window(ctx);
            if let Some(store) = self.root.recovery.clone() {
                app.enable_window_recovery(store);
            }
            Self::open(&mut app, request);
            self.next_window = self.next_window.wrapping_add(1);
            let id = egui::ViewportId::from_hash_of(("document-window", self.next_window));
            self.children.push(DocumentWindow { id, app });
        }
        ctx.request_repaint();
    }

    fn open(app: &mut PdfCraftApp, request: WindowOpen) {
        match request {
            WindowOpen::Path { path, then } => app.open_path_then(&path, then),
            WindowOpen::Bytes { name, path, bytes } => {
                if let Err(error) = app.open_bytes(&name, path, bytes) {
                    app.notify(error);
                }
            }
            WindowOpen::Created { name, bytes } => {
                if let Err(error) = app.open_created_bytes(&name, Ok(bytes)) {
                    app.notify(error);
                }
            }
            WindowOpen::CancelQuit | WindowOpen::Quit => {}
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.root.close_native_window(ctx);
        for window in &mut self.children {
            window.app.close_native_window(ctx);
            ctx.send_viewport_cmd_to(window.id, egui::ViewportCommand::Focus);
        }
        ctx.request_repaint();
    }
}

impl App for Desktop {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.root.raw_input_hook(ctx, input);
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let events = self.os_events.as_mut().map(|poll| poll()).unwrap_or_default();
        for event in events {
            match event {
                OsEvent::Open(paths) => {
                    for path in paths {
                        self.new_document(WindowOpen::Path { path, then: None }, ctx);
                    }
                }
                OsEvent::Quit => self.quit(ctx),
            }
        }
        while let Ok(request) = self.receiver.try_recv() {
            self.new_document(request, ctx);
        }
        self.children.retain(|window| !window.app.window_closed);
        if self.quitting && self.root.window_closed && self.children.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let count = usize::from(!self.root.window_closed) + self.children.len();
        self.root.desktop_dirty = self.root.first_dirty().is_some() || self.children.iter().any(|window| window.app.first_dirty().is_some());
        self.root.desktop_windows = count;
        for window in &mut self.children {
            window.app.desktop_windows = count;
        }
        let was_closed = self.root.window_closed;
        self.root.logic(ctx, frame);
        if was_closed && !self.root.window_closed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if !self.root.window_closed && ctx.input(|input| input.focused) {
            self.settings = self.root.persist();
        }
        // No polling while idle: Finder events and open workers explicitly wake the root.
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Paint the root if the backend requests it (also after a Dock reopen).
        // Closing never infers a reopen from stale visibility in a hidden viewport.
        self.root.ui(ui, frame);
        for window in &mut self.children {
            if window.app.window_closed {
                continue;
            }
            let id = window.id;
            let title = window
                .app
                .active_ids()
                .and_then(|(_, id)| window.app.session.get(id))
                .map_or_else(|| "PdfCraft".to_owned(), |doc| format!("{} — PdfCraft", doc.display_name()));
            let builder = egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([1200.0, 850.0])
                .with_min_inner_size([820.0, 520.0])
                .with_titlebar_shown(true)
                .with_title_shown(true)
                .with_fullsize_content_view(false)
                .with_drag_and_drop(true);
            ctx.show_viewport_immediate(id, builder, |ui, _| {
                let child_ctx = ui.ctx().clone();
                window.app.logic(&child_ctx, frame);
                if !window.app.window_closed {
                    ui.push_id(id, |ui| window.app.ui(ui, frame));
                    if child_ctx.input(|input| input.focused) {
                        self.settings = window.app.persist();
                    }
                } else {
                    child_ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            });
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("pdfcraft", self.settings.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        include_bytes!("../../../crates/ui-egui/tests/data/form.pdf").to_vec()
    }

    fn app() -> (PdfCraftApp, Receiver<WindowOpen>) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = PdfCraftApp::new();
        app.single_document_window = true;
        app.window_open = Some(sender);
        app.open_bytes("first.pdf", None, fixture()).unwrap();
        (app, receiver)
    }

    fn desktop_with_file() -> (Desktop, egui::Context, String) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = PdfCraftApp::new();
        app.single_document_window = true;
        app.window_open = Some(sender.clone());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/ui-egui/tests/data/form.pdf")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        app.open_path(&path);
        (Desktop::new(app, receiver, sender), egui::Context::default(), path)
    }

    #[test]
    fn duplicate_open_reuses_root_and_preserves_unsaved_edits() {
        let (mut desktop, ctx, path) = desktop_with_file();
        desktop.root.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        let id = desktop.root.views[0].id;
        let bytes = desktop.root.session.get(id).unwrap().bytes.clone();
        desktop.new_document(WindowOpen::Path { path, then: None }, &ctx);
        assert!(desktop.children.is_empty(), "same file must reuse its existing window");
        assert_eq!(desktop.root.views[0].id, id);
        assert_eq!(desktop.root.session.get(id).unwrap().bytes, bytes);
        assert!(desktop.root.first_dirty().is_some());
    }

    #[test]
    fn duplicate_open_reuses_child_even_when_root_is_closed() {
        let (mut desktop, ctx, path) = desktop_with_file();
        let child_id = egui::ViewportId::from_hash_of("existing-child");
        let mut child = PdfCraftApp::new();
        child.single_document_window = true;
        child.open_path(&path);
        desktop.root.request_close_tab(0);
        desktop.children.push(DocumentWindow { id: child_id, app: child });
        desktop.new_document(WindowOpen::Path { path, then: None }, &ctx);
        assert!(desktop.root.window_closed, "must find the child before reusing the empty root");
        assert_eq!(desktop.children.len(), 1);
        assert_eq!(desktop.children[0].id, child_id);
    }

    #[test]
    fn duplicate_open_is_coalesced_while_file_is_loading() {
        let (mut desktop, ctx, path) = desktop_with_file();
        desktop.root.request_close_tab(0);
        desktop.root.prepare_native_window(&ctx);
        desktop.new_document(WindowOpen::Path { path: path.clone(), then: None }, &ctx);
        assert!(desktop.root.window_occupied());
        assert!(desktop.root.views.is_empty());
        desktop.new_document(WindowOpen::Path { path, then: None }, &ctx);
        assert!(desktop.children.is_empty(), "a pending open already owns the file");
    }

    #[test]
    fn duplicate_open_from_bytes_reuses_existing_document() {
        let (mut desktop, ctx, path) = desktop_with_file();
        desktop.new_document(WindowOpen::Bytes { name: "form.pdf".into(), path: Some(path), bytes: fixture() }, &ctx);
        assert!(desktop.children.is_empty());
    }

    #[test]
    fn duplicate_open_resolves_alternate_path_spelling() {
        let (mut desktop, ctx, path) = desktop_with_file();
        let alternate = std::path::Path::new(&path).parent().unwrap().join("../data/form.pdf").to_string_lossy().into_owned();
        desktop.new_document(WindowOpen::Path { path: alternate, then: None }, &ctx);
        assert!(desktop.children.is_empty());
    }

    #[test]
    fn same_filename_in_another_folder_and_closed_files_can_open() {
        let (mut desktop, ctx, path) = desktop_with_file();
        desktop.new_document(WindowOpen::Bytes { name: "form.pdf".into(), path: Some(format!("{path}/other/form.pdf")), bytes: fixture() }, &ctx);
        assert_eq!(desktop.children.len(), 1);
        desktop.root.request_close_tab(0);
        desktop.new_document(WindowOpen::Path { path, then: None }, &ctx);
        assert!(!desktop.root.window_closed);
        assert_eq!(desktop.children.len(), 1);
    }

    #[test]
    fn duplicate_open_restores_a_minimized_window() {
        let (mut desktop, ctx, path) = desktop_with_file();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            desktop.new_document(WindowOpen::Path { path: path.clone(), then: None }, ui.ctx());
        });
        let commands = &output.viewport_output.get(&egui::ViewportId::ROOT).unwrap().commands;
        assert!(commands.iter().any(|command| matches!(command, egui::ViewportCommand::Minimized(false))));
        assert!(commands.iter().any(|command| matches!(command, egui::ViewportCommand::Focus)));
        assert!(desktop.children.is_empty());
    }

    #[test]
    fn duplicate_open_keeps_the_existing_password_prompt() {
        let (mut desktop, ctx, path) = desktop_with_file();
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open("encrypted.pdf", None, std::sync::Arc::new(fixture()), None).unwrap();
        session
            .apply(
                id,
                pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection { open_password: Some("test-password".into()), ..Default::default() }),
            )
            .unwrap();
        let bytes = session.save_bytes(id).unwrap().as_ref().clone();
        desktop.root.request_close_tab(0);
        desktop.root.open_bytes("encrypted.pdf", Some(path.clone()), bytes).unwrap();
        desktop.root.password_prompt.as_mut().unwrap().input = "typed-password".into();
        desktop.new_document(WindowOpen::Path { path, then: None }, &ctx);
        assert!(desktop.children.is_empty());
        assert_eq!(desktop.root.password_prompt.as_ref().unwrap().input, "typed-password");
    }

    #[test]
    fn duplicate_open_recognizes_symlinks_and_hard_links() {
        let (mut desktop, ctx, path) = desktop_with_file();
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/duplicate-file-links-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let symlink = folder.join("symlink.pdf");
        let hard_link = folder.join("hard-link.pdf");
        std::os::unix::fs::symlink(&path, &symlink).unwrap();
        std::fs::hard_link(&path, &hard_link).unwrap();
        for alias in [&symlink, &hard_link] {
            desktop.new_document(WindowOpen::Path { path: alias.to_string_lossy().into_owned(), then: None }, &ctx);
        }
        std::fs::remove_dir_all(folder).unwrap();
        assert!(desktop.children.is_empty());
    }

    #[test]
    fn rapid_path_opens_are_separated_while_the_first_is_still_loading() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = PdfCraftApp::new();
        app.single_document_window = true;
        app.window_open = Some(sender);
        app.prepare_native_window(&egui::Context::default());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/ui-egui/tests/data/form.pdf").canonicalize().unwrap();
        let path = path.to_str().unwrap();
        app.open_path(path);
        assert!(app.window_occupied());
        app.open_path(path);
        assert!(receiver.try_recv().is_err(), "reopening a pending file must not duplicate it");
        let other = format!("{path}.other.pdf");
        app.open_path(&other);
        assert!(matches!(receiver.try_recv().unwrap(), WindowOpen::Path { .. }));
        assert!(app.views.len() <= 1);
    }

    #[test]
    fn reopening_a_file_focuses_its_existing_native_window() {
        let ctx = egui::Context::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut root = PdfCraftApp::new();
        root.single_document_window = true;
        root.window_open = Some(sender.clone());
        root.open_bytes("first.pdf", Some("/test/first.pdf".into()), fixture()).unwrap();
        let mut desktop = Desktop::new(root, receiver, sender);
        desktop.new_document(WindowOpen::Path { path: "/test/first.pdf".into(), then: None }, &ctx);
        assert!(desktop.children.is_empty());
        desktop.new_document(WindowOpen::Bytes { name: "second.pdf".into(), path: Some("/test/second.pdf".into()), bytes: fixture() }, &ctx);
        assert_eq!(desktop.children.len(), 1);
        desktop.new_document(WindowOpen::Path { path: "/test/second.pdf".into(), then: None }, &ctx);
        assert_eq!(desktop.children.len(), 1);
        assert_eq!(desktop.root.views.len(), 1);
        assert_eq!(desktop.children[0].app.views.len(), 1);
    }

    #[test]
    fn separate_windows_keep_distinct_recovery_snapshots() {
        let (mut first, _) = app();
        let (mut second, _) = app();
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/local-recovery-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let store = pdfcraft_ui_egui::RecoveryStore::new(&folder);
        for window in [&mut first, &mut second] {
            window.enable_window_recovery(store.clone());
            window.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
            window.autosave_now();
        }
        assert_eq!(store.list().len(), 2);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn generated_documents_get_a_new_window_and_keep_their_unsaved_state() {
        let (mut original, receiver) = app();
        original.open_created_bytes("Combined.pdf", Ok(std::sync::Arc::new(fixture()))).unwrap();
        assert_eq!(original.views.len(), 1);
        let WindowOpen::Created { name, bytes } = receiver.try_recv().unwrap() else { panic!("expected a new result window") };
        let mut result = PdfCraftApp::new();
        result.single_document_window = true;
        result.open_created_bytes(&name, Ok(bytes)).unwrap();
        assert!(original.first_dirty().is_none());
        assert!(result.first_dirty().is_some());
    }

    #[test]
    fn second_document_is_routed_without_changing_the_first() {
        let (mut app, receiver) = app();
        let first = app.views[0].id;
        app.open_bytes("second.pdf", None, fixture()).unwrap();
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.views[0].id, first);
        assert!(matches!(receiver.try_recv().unwrap(), WindowOpen::Bytes { name, .. } if name == "second.pdf"));
    }

    #[test]
    fn closing_the_last_document_retires_only_its_window() {
        let (mut app, _) = app();
        app.request_close_tab(0);
        assert!(app.window_closed);
        assert!(app.views.is_empty());
        app.open_bytes("reopened.pdf", None, fixture()).unwrap();
        assert!(!app.window_closed);
        assert_eq!(app.views.len(), 1);
    }

    #[test]
    fn unsaved_close_can_be_cancelled_without_losing_work() {
        let (mut app, receiver) = app();
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        assert!(app.first_dirty().is_some());
        app.request_close_tab(0);
        assert!(!app.window_closed);
        assert!(app.close_request.is_some());
        app.resolve_close(&egui::Context::default(), None);
        assert_eq!(app.views.len(), 1);
        assert!(app.first_dirty().is_some());
        assert!(matches!(receiver.try_recv().unwrap(), WindowOpen::CancelQuit));
        app.request_close_tab(0);
        app.resolve_close(&egui::Context::default(), Some(false));
        assert!(app.window_closed);
        assert!(app.views.is_empty());
    }
}
