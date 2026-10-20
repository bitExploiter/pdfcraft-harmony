//! Native tab tear-off. The document and view move in memory; no temporary PDF or save is used.
use crate::PdfCraftApp;
use pdfcraft_engine::DocId;

/// A small vertical dead zone keeps tab reordering and edge scrolling easy to perform.
pub(crate) fn outside_strip(strip: egui::Rect, viewport: egui::Rect, pointer: egui::Pos2) -> bool {
    pointer.y < strip.top() - 24.0 || pointer.y > strip.bottom() + 24.0 || pointer.x < viewport.left() - 24.0 || pointer.x > viewport.right() + 24.0
}

impl PdfCraftApp {
    /// Native hosts opt in only when they can keep the detached document alive.
    pub fn can_detach_tab(&self, id: DocId) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.pickers.pending() || self.pending_overwrite.is_some() {
            return false;
        }
        self.managed_windows
            && !self.window_close_ready
            && self.detach_request.is_none()
            && self.session.get(id).is_some()
            && self.views.iter().any(|v| v.id == id)
            && !self.modal_open()
            && self.compare.is_none()
            && self.ocr_run.is_none()
            && self.ocr_batch.is_none()
            && self.action_run.is_none()
            && self.optimize_run.is_none()
            && self.export_status.is_none()
            && self.image_import.is_none()
    }

    /// Queue a move by document identity, so reordering cannot transfer the wrong tab.
    pub fn request_detach_tab(&mut self, id: DocId, position: Option<egui::Pos2>) -> bool {
        if !self.can_detach_tab(id) {
            return false;
        }
        self.detach_request = Some((id, position.filter(|p| p.x.is_finite() && p.y.is_finite() && p.x.abs() <= 100_000.0 && p.y.abs() <= 100_000.0)));
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
        true
    }

    /// Transfer ownership only when a native host is ready to display the new viewport.
    #[cfg(not(target_arch = "wasm32"))]
    fn take_detached_tab(&mut self) -> Option<(Self, Option<egui::Pos2>)> {
        let (id, position) = self.detach_request.take()?;
        if !self.can_detach_tab(id) {
            return None;
        }
        let index = self.views.iter().position(|v| v.id == id)?;
        let mut child = Self::new();
        child.restore(&self.persist());
        child.mode = self.mode;
        child.mode_override = self.mode_override;
        child.motion.reduced = self.motion.reduced;
        child.quick_tool = self.quick_tool;
        child.form_preview = self.form_preview;
        child.system_text_scale = self.system_text_scale;
        child.system_clipboard = self.system_clipboard;
        child.integrated_titlebar = self.integrated_titlebar;
        child.window_controls = self.window_controls.clone();
        child.update_source = self.update_source.clone();
        child.printer_source = self.printer_source.clone();
        child.os_key_store_ids = self.os_key_store_ids;
        child.run_inline = self.run_inline;
        child.recovery = self.recovery.clone();
        child.managed_windows = true;
        child.startup_superseded = true;
        child.styled = self.styled;
        child.fonts_ready = self.fonts_ready;
        child.fonts_cjk = self.fonts_cjk;
        // Every allocation/settings operation precedes removing the original document.
        child.session = self.session.detach_document(id)?;
        child.views.push(self.views.remove(index));
        child.active = Some(0);
        if let Some(key) = self.recovery_keys.remove(&id) {
            child.recovery_keys.insert(id, key);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.watch.transfer(id, &mut child.watch);
        self.active = match self.active {
            _ if self.views.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active >= self.views.len() => Some(self.views.len() - 1),
            other => other,
        };
        self.deferred_commands.retain(|(_, doc)| *doc != Some(id));
        Some((child, position))
    }
}

/// Keep a dropped window's title bar on the monitor, including releases by the taskbar.
#[cfg(target_os = "windows")]
fn visible_position(position: egui::Pos2, scale: f32) -> egui::Pos2 {
    let scale = if scale.is_finite() && (0.01..=100.0).contains(&scale) { scale } else { 1.0 };
    let point = winsafe::POINT { x: (position.x * scale) as i32, y: (position.y * scale) as i32 };
    let monitor = winsafe::HMONITOR::MonitorFromPoint(point, winsafe::co::MONITOR::DEFAULTTONEAREST);
    let Ok(info) = monitor.GetMonitorInfo() else { return position };
    let area = info.rcWork;
    let left = area.left as f32 / scale;
    let top = area.top as f32 / scale;
    egui::pos2(
        position.x.clamp(left, (area.right as f32 / scale - 1100.0).max(left)),
        position.y.clamp(top, (area.bottom as f32 / scale - 800.0).max(top)),
    )
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn visible_position(position: egui::Pos2, _scale: f32) -> egui::Pos2 {
    position
}

/// One desktop process hosts independent native windows, each with its own document session.
/// Closing the original window hides its root viewport while detached windows remain alive.
#[cfg(not(target_arch = "wasm32"))]
pub struct NativeWindows {
    root: PdfCraftApp,
    windows: Vec<Window>,
    next: u64,
    root_hidden: bool,
}

#[cfg(not(target_arch = "wasm32"))]
struct Window {
    id: egui::ViewportId,
    app: PdfCraftApp,
    position: Option<egui::Pos2>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeWindows {
    /// Enable tear-off for a native application without changing its existing open documents.
    pub fn new(mut root: PdfCraftApp) -> Self {
        root.managed_windows = true;
        Self { root, windows: Vec::new(), next: 0, root_hidden: false }
    }

    fn collect(&mut self) {
        let mut arriving = Vec::new();
        if let Some(tab) = self.root.take_detached_tab() {
            arriving.push(tab);
        }
        for window in &mut self.windows {
            if let Some(tab) = window.app.take_detached_tab() {
                arriving.push(tab);
            }
        }
        for (app, position) in arriving {
            self.next = self.next.wrapping_add(1);
            self.windows.push(Window { id: egui::ViewportId::from_hash_of(("pdfcraft-document-window", self.next)), app, position });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl eframe::App for NativeWindows {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if ctx.viewport_id() == egui::ViewportId::ROOT {
            self.root.raw_input_hook(ctx, input);
        } else if let Some(window) = self.windows.iter_mut().find(|w| w.id == ctx.viewport_id()) {
            window.app.raw_input_hook(ctx, input);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.root.save(storage);
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.root_hidden {
            self.root.window_close_ready = false;
            self.root.allow_quit = false;
        }
        self.root.logic(ctx, frame);
        if self.root_hidden && (!self.root.views.is_empty() || !self.root.pending_opens.is_empty()) {
            self.root_hidden = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.root_hidden {
            self.root.ui(ui, frame);
        }
        self.collect();
        for window in &mut self.windows {
            window.app.motion.reduced = self.root.motion.reduced;
            let title = window
                .app
                .active
                .and_then(|i| crate::canvas::tab_label(&window.app, i))
                .map_or_else(|| "PdfCraft".to_owned(), |name| format!("{name} — PdfCraft"));
            let mut builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([1100.0, 760.0]).with_min_inner_size([640.0, 420.0]);
            if let Some(position) = window.position.take() {
                builder = builder.with_position(visible_position(position, ctx.pixels_per_point()));
            }
            ctx.show_viewport_immediate(window.id, builder, |ui, _class| {
                let child_ctx = ui.ctx().clone();
                window.app.logic(&child_ctx, frame);
                ui.push_id(window.id, |ui| window.app.ui(ui, frame));
            });
        }
        self.windows.retain(|w| !w.app.window_close_ready);
        if self.windows.is_empty() && (self.root.window_close_ready || self.root_hidden) {
            // eframe handles Close on the next pass. Do not cancel that final root close:
            // every document has already passed its own Save/Discard/Cancel guard.
            self.root.managed_windows = false;
            self.root.allow_quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.root.window_close_ready && !self.root_hidden {
            for i in (0..self.root.views.len()).rev() {
                self.root.close_tab(i);
            }
            self.root_hidden = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    fn app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.managed_windows = true;
        app.set_option("language", "en").unwrap();
        let bytes = app.session.create_blank(300.0, 400.0, 3).unwrap();
        for name in ["first.pdf", "middle.pdf", "last.pdf"] {
            app.open_bytes(name, None, bytes.as_ref().clone()).unwrap();
        }
        app
    }

    #[test]
    fn tear_off_preserves_working_bytes_undo_redo_view_and_recovery_identity() {
        let mut root = app();
        root.active = Some(1);
        let id = root.views[1].id;
        root.apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "First edit".into() });
        let first = root.session.get(id).unwrap().bytes.clone();
        root.apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Second edit".into() });
        let second = root.session.get(id).unwrap().bytes.clone();
        root.session.undo(id).unwrap();
        let before_transfer = root.session.get(id).unwrap().bytes.clone();
        assert_eq!(before_transfer, first);
        root.views[1].zoom = 1.75;
        root.views[1].current = 2;
        root.recovery_keys.insert(id, "existing-recovery".into());
        let active = root.views[2].id;
        root.active = Some(2);
        assert!(root.request_detach_tab(id, None));
        root.move_tab(1, 0); // A queued request follows identity, not its old index.
        let (mut child, _) = root.take_detached_tab().unwrap();
        assert!(root.session.get(id).is_none());
        assert_eq!(root.active_ids().unwrap().1, active);
        assert_eq!(child.active_ids().unwrap().1, id);
        assert_eq!(child.views[0].zoom, 1.75);
        assert_eq!(child.views[0].current, 2);
        assert_eq!(child.recovery_keys.get(&id).unwrap(), "existing-recovery");
        assert!(!root.recovery_keys.contains_key(&id));
        assert!(root.closed_tabs.is_empty());
        assert!(std::sync::Arc::ptr_eq(&child.session.get(id).unwrap().bytes, &before_transfer));
        assert!(child.session.get(id).unwrap().dirty);
        child.session.redo(id).unwrap();
        assert_eq!(child.session.get(id).unwrap().bytes, second);
        child.session.undo(id).unwrap();
        assert_eq!(child.session.get(id).unwrap().bytes, first);
        assert_eq!(root.views.len(), 2);
    }

    #[test]
    fn tear_off_rejects_unavailable_host_missing_document_and_modal_without_removing_work() {
        let mut root = app();
        let id = root.views[0].id;
        root.managed_windows = false;
        assert!(!root.request_detach_tab(id, None));
        root.managed_windows = true;
        assert!(!root.request_detach_tab(DocId(u64::MAX), None));
        root.close_request = Some(crate::CloseRequest::Tab(id));
        assert!(!root.request_detach_tab(id, None));
        assert_eq!(root.views.len(), 3);
        root.close_request = None;
        assert!(root.request_detach_tab(id, None));
        root.close_request = Some(crate::CloseRequest::Tab(id));
        assert!(root.take_detached_tab().is_none());
        assert_eq!(root.session.docs().len(), 3);
    }

    #[test]
    fn tear_off_close_uses_save_discard_cancel_and_does_not_close_other_windows() {
        let mut root = app();
        let id = root.views[2].id;
        root.apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Keep me".into() });
        assert!(root.execute("file.detach_tab"));
        let (mut child, _) = root.take_detached_tab().unwrap();
        let ctx = egui::Context::default();
        assert!(!child.request_quit(&ctx));
        assert_eq!(child.close_request, Some(crate::CloseRequest::Quit));
        child.resolve_close(&ctx, None);
        assert!(!child.window_close_ready);
        assert!(child.session.get(id).unwrap().dirty);
        assert!(!child.request_quit(&ctx));
        child.resolve_close(&ctx, Some(false));
        assert!(child.window_close_ready);
        assert_eq!(root.session.docs().len(), 2);
        assert!(!root.window_close_ready);
    }

    #[test]
    fn tear_off_final_window_allows_the_native_exit_after_its_save_guard() {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| NativeWindows::new(app()));
        h.run_steps(2);
        assert!(h.state_mut().root.request_quit(&egui::Context::default()));
        h.run_steps(2);
        assert!(!h.state().root.managed_windows, "the final Close must not be canceled on the next native pass");
        assert!(h.state().root.allow_quit);
        assert!(h.state().windows.is_empty());
    }

    fn drag(h: &mut Harness<'static, PdfCraftApp>, to: egui::Pos2, escape: bool) {
        let from = h.get_by_role_and_label(egui::accesskit::Role::Tab, "first.pdf").rect().center();
        h.event(egui::Event::PointerMoved(from));
        h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
        for step in 1..=5 {
            h.event(egui::Event::PointerMoved(from.lerp(to, step as f32 / 5.0)));
            h.run_steps(1);
        }
        if escape {
            h.key_press(egui::Key::Escape);
            h.run_steps(1);
        }
        h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(2);
    }

    #[test]
    fn tear_off_raw_drag_queues_only_outside_strip_and_escape_cancels() {
        for escape in [false, true] {
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app());
            h.run_steps(4);
            let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
            drag(&mut h, egui::pos2(400.0, 180.0), escape);
            assert_eq!(h.state().detach_request.map(|r| r.0), if escape { None } else { Some(ids[0]) });
            assert_eq!(h.state().views.iter().map(|v| v.id).collect::<Vec<_>>(), ids);
        }
    }

    #[test]
    fn tear_off_keeps_horizontal_reordering_and_strip_edge_scrolling() {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app());
        h.run_steps(4);
        let first = h.state().views[0].id;
        let end = h.get_by_role_and_label(egui::accesskit::Role::Tab, "last.pdf").rect().right_center() + egui::vec2(12.0, 0.0);
        drag(&mut h, end, false);
        assert_eq!(h.state().views[2].id, first);
        assert!(h.state().detach_request.is_none());
        let strip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 38.0));
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        assert!(!outside_strip(strip, viewport, egui::pos2(1399.0, 20.0)));
        assert!(outside_strip(strip, viewport, egui::pos2(1430.0, 20.0)));
    }
    #[test]
    fn tear_off_preserves_external_file_change_protection() {
        let mut root = app();
        let id = root.views[0].id;
        let path = std::env::temp_dir().join(format!("pdfcraft-tear-off-watch-{}.pdf", std::process::id()));
        std::fs::write(&path, b"old").unwrap();
        let path_text = path.to_str().unwrap();
        root.watch.known_now(id, path_text);
        std::fs::write(&path, b"external rewrite").unwrap();
        assert!(root.request_detach_tab(id, None));
        let (child, _) = root.take_detached_tab().unwrap();
        assert!(child.watch.disk_differs(id, path_text));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn tear_off_new_documents_have_distinct_autosaves_across_windows() {
        let mut root = app();
        let dir = std::env::temp_dir().join(format!("pdfcraft-tear-off-recovery-{}", std::process::id()));
        root.enable_recovery(crate::RecoveryStore::new(&dir));
        let id = root.views[0].id;
        assert!(root.request_detach_tab(id, None));
        let (mut child, _) = root.take_detached_tab().unwrap();
        let bytes = root.session.create_blank(100.0, 100.0, 1).unwrap();
        for app in [&mut root, &mut child] {
            app.open_bytes("new.pdf", None, bytes.as_ref().clone()).unwrap();
            assert!(app.apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "unsaved".into() }));
            app.autosave_now();
        }
        let a = root.active_ids().unwrap().1;
        let b = child.active_ids().unwrap().1;
        assert_eq!(a, b, "sessions may issue the same local document id");
        assert_ne!(root.recovery_keys.get(&a), child.recovery_keys.get(&b));
        let child_path = dir.join(format!("{}.pdf", child.recovery_keys.get(&b).unwrap()));
        for i in (0..root.views.len()).rev() {
            root.close_tab(i);
        }
        root.shutdown_recovery();
        assert!(child_path.is_file(), "closing the source must not remove the detached autosave");
        for i in (0..child.views.len()).rev() {
            child.close_tab(i);
        }
        drop(child);
        drop(root);
        std::fs::remove_dir(&dir).unwrap();
    }
}
