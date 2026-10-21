//! Windows beside the main one (a tab's Move to New Window). Every document's view stays in the
//! one list, [`PdfCraftApp::views`], marked with the window it shows in ([`DocView::window`]), so
//! everything that finds a document's view by its id (saving, reloading, scripts, quitting) works
//! whichever window shows it. What else differs between windows (the active tab, the workspace,
//! the panels, the palette, a dialog, full screen) is a [`Slice`] of the app's state, swapped in
//! while the window is drawn.
//!
//! [`DocView::window`]: crate::canvas::DocView::window

use pdfcraft_engine::DocId;

use crate::{CloseRequest, Dialog, LeftPanel, Mode, PdfCraftApp, RightPanel, docking};

/// The main window's key ([`PdfCraftApp::current_window`] while it is drawn).
pub const MAIN: u32 = 0;

/// The part of the app's state each window has its own of.
pub(crate) struct Slice {
    /// The window's key: [`PdfCraftApp::current_window`] while it is drawn.
    window: u32,
    /// The active document, by id: tabs close and move between windows while it is swapped out.
    active: Option<DocId>,
    mode: Mode,
    left: LeftPanel,
    left_open: bool,
    right: Option<RightPanel>,
    right_before_read: Option<RightPanel>,
    palette_open: bool,
    palette_query: String,
    dialog: Option<Dialog>,
    close_request: Option<CloseRequest>,
    full_screen: bool,
    docking: docking::Workspace,
    pending_panel_reveals: Vec<docking::Panel>,
    window_title: String,
}

/// A window beside the main one.
pub(crate) struct DocWindow {
    viewport: egui::ViewportId,
    slice: Slice,
    /// Closing the window, the document whose unsaved changes it is asking about.
    closing: Option<DocId>,
}

impl PdfCraftApp {
    /// Exchange the app's per-window state with `s`.
    fn swap_slice(&mut self, s: &mut Slice) {
        let incoming = s.active.and_then(|id| self.views.iter().position(|v| v.id == id));
        s.active = self.active.and_then(|i| self.views.get(i)).map(|v| v.id);
        self.active = incoming;
        std::mem::swap(&mut self.current_window, &mut s.window);
        std::mem::swap(&mut self.mode, &mut s.mode);
        std::mem::swap(&mut self.left, &mut s.left);
        std::mem::swap(&mut self.left_open, &mut s.left_open);
        std::mem::swap(&mut self.right, &mut s.right);
        std::mem::swap(&mut self.right_before_read, &mut s.right_before_read);
        std::mem::swap(&mut self.palette_open, &mut s.palette_open);
        std::mem::swap(&mut self.palette_query, &mut s.palette_query);
        std::mem::swap(&mut self.dialog, &mut s.dialog);
        std::mem::swap(&mut self.close_request, &mut s.close_request);
        std::mem::swap(&mut self.full_screen, &mut s.full_screen);
        std::mem::swap(&mut self.docking, &mut s.docking);
        std::mem::swap(&mut self.pending_panel_reveals, &mut s.pending_panel_reveals);
        std::mem::swap(&mut self.window_title, &mut s.window_title);
        self.settle_active();
    }

    /// Whether the view at `index` shows in the window being drawn.
    pub(crate) fn in_this_window(&self, index: usize) -> bool {
        self.views.get(index).is_some_and(|v| v.window == self.current_window)
    }

    /// The tabs of the window being drawn, in order (indices into [`Self::views`]).
    pub(crate) fn window_tabs(&self) -> Vec<usize> {
        (0..self.views.len()).filter(|&i| self.in_this_window(i)).collect()
    }

    /// Keep the active tab one of this window's: after a tab closed or moved away, the nearest
    /// tab of the window after it (or before, at the end), or Home when it has none.
    pub(crate) fn settle_active(&mut self) {
        let Some(a) = self.active else { return };
        if self.in_this_window(a) {
            return;
        }
        let tabs = self.window_tabs();
        self.active = tabs.iter().copied().find(|&i| i >= a).or_else(|| tabs.last().copied());
    }

    /// The documents active in every window, for what only the shown document may do.
    pub(crate) fn shown_documents(&self) -> Vec<DocId> {
        let mut shown: Vec<DocId> = self.active_ids().map(|(_, id)| id).into_iter().collect();
        shown.extend(self.windows.iter().filter_map(|w| w.slice.active));
        shown
    }

    /// Whether any window besides the main one is open.
    pub fn has_other_windows(&self) -> bool {
        !self.windows.is_empty()
    }

    /// How many windows show documents, the main one included.
    pub fn window_count(&self) -> usize {
        1 + self.windows.len()
    }

    /// Move the document `id` into a window of its own (a tab's Move to New Window), where it is
    /// the active tab, with the workspace and panels the window it came from has.
    pub fn move_to_new_window(&mut self, id: DocId) {
        let Some(index) = self.views.iter().position(|v| v.id == id) else { return };
        let key = self.next_window;
        self.next_window = self.next_window.saturating_add(1).max(MAIN + 1);
        if let Some(view) = self.views.get_mut(index) {
            view.window = key;
        }
        let slice = Slice {
            window: key,
            active: Some(id),
            mode: self.mode,
            left: self.left,
            left_open: self.left_open,
            right: self.right,
            right_before_read: self.right_before_read,
            palette_open: false,
            palette_query: String::new(),
            dialog: None,
            close_request: None,
            full_screen: false,
            docking: self.docking.clone(),
            pending_panel_reveals: Vec::new(),
            window_title: String::new(),
        };
        self.windows.push(DocWindow { viewport: egui::ViewportId::from_hash_of(("pdfcraft-window", key)), slice, closing: None });
        self.settle_active();
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// Move the document `id` back into the main window, as its active tab.
    pub fn move_to_main_window(&mut self, id: DocId) {
        let Some(view) = self.views.iter_mut().find(|v| v.id == id) else { return };
        view.window = MAIN;
        if self.current_window == MAIN {
            self.active = self.views.iter().position(|v| v.id == id);
        } else {
            self.settle_active();
            // The main window's active tab is swapped out while this one is drawn.
            self.main_activation = Some(id);
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// Bring every window's tabs into the main window and close the others (before quitting or
    /// closing everything, which then go through every document in the main window).
    pub(crate) fn merge_windows(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        for view in &mut self.views {
            view.window = MAIN;
        }
        self.windows.clear();
    }

    /// Draw the windows beside the main one, each with its own slice of the app's state, and
    /// close those left without a tab.
    pub(crate) fn draw_windows(&mut self, ctx: &egui::Context) {
        let mut windows = std::mem::take(&mut self.windows);
        for w in &mut windows {
            let mut builder = egui::ViewportBuilder::default()
                .with_title(if w.slice.window_title.is_empty() { "KreativPdf" } else { w.slice.window_title.as_str() })
                .with_inner_size([1200.0, 820.0])
                .with_min_inner_size([820.0, 520.0])
                .with_drag_and_drop(true)
                .with_decorations(!self.draws_window_frame());
            if self.integrated_titlebar {
                builder = builder.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
            }
            ctx.show_viewport_immediate(w.viewport, builder, |ui, _class| {
                self.swap_slice(&mut w.slice);
                let ctx = ui.ctx().clone();
                if ctx.input(|i| i.viewport().close_requested()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    w.closing = None;
                    self.close_window_tabs(w);
                } else if let Some(id) = w.closing
                    && self.close_request.is_none()
                {
                    // The prompt was answered: closed (or saved and closed), go on to the next;
                    // still open (cancelled, or a save still being written), stop.
                    if self.views.iter().any(|v| v.id == id) {
                        w.closing = None;
                    } else {
                        self.close_window_tabs(w);
                    }
                }
                self.shortcuts(&ctx);
                self.draw(ui);
                self.swap_slice(&mut w.slice);
            });
        }
        // Windows opened while these were drawn (a tab moved on from one of them).
        windows.append(&mut self.windows);
        let views = &self.views;
        // Close all and Quit merge the windows into the main one (`merge_windows`): a prompt they
        // left in a window that goes with it is the main window's now.
        let mut carried = None;
        windows.retain(|w| {
            let keep = views.iter().any(|v| v.window == w.slice.window);
            if !keep && carried.is_none() {
                carried = w.slice.close_request.filter(|r| !matches!(r, CloseRequest::Tab(_)));
            }
            keep
        });
        if self.close_request.is_none() {
            self.close_request = carried;
        }
        self.windows = windows;
        if let Some(id) = self.main_activation.take()
            && let Some(i) = self.views.iter().position(|v| v.id == id && v.window == MAIN)
        {
            self.active = Some(i);
        }
    }

    /// Close the window being drawn (`w`, swapped in): its documents without unsaved changes
    /// close at once; the first with unsaved changes asks, as closing its tab does, and the
    /// window goes on with the rest once it is answered. A window left without a tab closes.
    fn close_window_tabs(&mut self, w: &mut DocWindow) {
        for i in (0..self.views.len()).rev() {
            if self.in_this_window(i) && !self.has_unsaved_work(i) {
                self.close_tab(i);
            }
        }
        self.settle_active();
        if let Some(i) = self.window_tabs().first().copied()
            && let Some(id) = self.views.get(i).map(|v| v.id)
        {
            self.active = Some(i);
            self.close_request = Some(CloseRequest::Tab(id));
            w.closing = Some(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with `n` one-page documents, `0.pdf` to `n-1.pdf`, the first active.
    fn app_with(n: usize) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for i in 0..n {
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes(&format!("{i}.pdf"), None, bytes.as_ref().clone()).unwrap();
        }
        app.active = Some(0);
        app
    }

    fn id(app: &PdfCraftApp, i: usize) -> DocId {
        app.views[i].id
    }

    /// Move to New Window: the tab leaves the main window's strip for a window of its own, where
    /// it is active; the main window goes on with its next tab. Move to Main Window brings it
    /// back as the main window's active tab, and the empty window goes.
    #[test]
    fn a_tab_moves_into_a_window_of_its_own_and_back() {
        let mut app = app_with(3);
        app.active = Some(1);
        let moved = id(&app, 1);
        app.move_to_new_window(moved);
        assert_eq!(app.window_count(), 2);
        assert_ne!(app.views[1].window, MAIN);
        assert_eq!(app.window_tabs(), [0, 2], "the main window keeps the others");
        assert_eq!(app.active, Some(2), "and moves on to the next of them");
        let mut w = app.windows.remove(0);
        app.swap_slice(&mut w.slice);
        assert_eq!(app.window_tabs(), [1]);
        assert_eq!(app.active_ids().map(|(_, d)| d), Some(moved), "active in its window");
        app.move_to_main_window(moved);
        assert!(app.window_tabs().is_empty());
        assert_eq!(app.active, None);
        app.swap_slice(&mut w.slice);
        assert_eq!(app.window_tabs(), [0, 1, 2]);
        assert_eq!(app.main_activation, Some(moved), "made active once the main window is drawn");
    }

    /// Each window keeps its active document by id: a tab closing in one window (shifting the
    /// list) leaves the other window's active document as it was.
    #[test]
    fn a_window_keeps_its_active_document_when_tabs_close_elsewhere() {
        let mut app = app_with(4);
        let (moved, last) = (id(&app, 1), id(&app, 3));
        app.active = Some(3);
        app.move_to_new_window(moved);
        let mut w = app.windows.remove(0);
        app.swap_slice(&mut w.slice);
        // In the other window: close the document before the main window's active one.
        let i = app.views.iter().position(|v| v.id == moved).unwrap();
        app.close_tab(i);
        app.swap_slice(&mut w.slice);
        assert_eq!(app.active_ids().map(|(_, d)| d), Some(last), "still the same document");
    }

    /// Closing another window closes its documents; one with unsaved changes asks first, in
    /// that window, and nothing of it is lost.
    #[test]
    fn closing_a_window_closes_its_tabs_and_asks_about_unsaved_ones() {
        let mut app = app_with(3);
        let (a, b) = (id(&app, 1), id(&app, 2));
        app.move_to_new_window(a);
        let mut w = app.windows.remove(0);
        app.swap_slice(&mut w.slice);
        // Move `b` in beside `a`, and give it unsaved changes.
        if let Some(v) = app.views.iter_mut().find(|v| v.id == b) {
            v.window = app.current_window;
        }
        app.active = app.views.iter().position(|v| v.id == b);
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        app.close_window_tabs(&mut w);
        assert!(app.session.get(a).is_none(), "the saved document closed");
        assert!(app.session.get(b).is_some(), "the unsaved one stays");
        assert_eq!(app.close_request, Some(CloseRequest::Tab(b)), "and is asked about");
        assert_eq!(w.closing, Some(b));
        app.swap_slice(&mut w.slice);
        assert_eq!(app.close_request, None, "in its own window, not the main one");
    }

    /// Close all brings every window's tabs into the main window first, so every document is
    /// asked about there.
    #[test]
    fn close_all_merges_the_windows_into_the_main_one() {
        let mut app = app_with(2);
        let moved = id(&app, 1);
        app.move_to_new_window(moved);
        app.active = app.views.iter().position(|v| v.id == moved);
        // Unsaved changes in the moved document (made active only to edit it).
        app.current_window = app.views[1].window;
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        app.current_window = MAIN;
        app.close_all();
        assert!(!app.has_other_windows());
        assert!(app.views.iter().all(|v| v.window == MAIN));
        assert_eq!(app.close_request, Some(CloseRequest::All));
    }

    /// Next and previous tab go through the window's own tabs only.
    #[test]
    fn next_tab_stays_in_its_window() {
        let mut app = app_with(4);
        let moved = id(&app, 2);
        app.move_to_new_window(moved);
        app.active = Some(1);
        app.execute("view.next_tab");
        assert_eq!(app.active, Some(3), "past the tab in the other window");
        app.execute("view.next_tab");
        assert_eq!(app.active, Some(0), "and round");
    }
}
