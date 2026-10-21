//! Windows beside the main one (a tab's Move to New Window, or a tab dragged out of the strip).
//! Every document's view stays in the
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

/// A new window's size when nothing says otherwise, and the smallest a window can be.
const DEFAULT_SIZE: egui::Vec2 = egui::vec2(1200.0, 820.0);
const MIN_SIZE: egui::Vec2 = egui::vec2(820.0, 520.0);

/// Where the pointer is in a window torn off under it: over its tab strip, a little in from the
/// left, so the tab seems to come along.
const TEAR_GRAB: egui::Vec2 = egui::vec2(160.0, 18.0);

/// The viewport that shows the window `key`.
fn viewport_of(key: u32) -> egui::ViewportId {
    if key == MAIN { egui::ViewportId::ROOT } else { egui::ViewportId::from_hash_of(("pdfcraft-window", key)) }
}

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
    /// Where it opens on screen (points), when somewhere in particular: under the pointer for a
    /// tab torn off, or where it was for the last session's.
    placement: Option<egui::Rect>,
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

    /// The windows beside the main one, for the session: each one's key, active document and
    /// place on screen. Drawn now, a window's slice is the app's.
    pub(crate) fn window_layout(&self) -> Vec<(u32, Option<DocId>, Option<egui::Rect>)> {
        let rect = |key: u32| self.window_rects.iter().find(|(k, _)| *k == key).map(|(_, r)| *r);
        let mut layout: Vec<_> = self.windows.iter().map(|w| (w.slice.window, w.slice.active, rect(w.slice.window).or(w.placement))).collect();
        if self.current_window != MAIN {
            let active = self.active.and_then(|i| self.views.get(i)).map(|v| v.id);
            layout.push((self.current_window, active, rect(self.current_window)));
        }
        layout.sort_by_key(|w| w.0);
        layout
    }

    /// The main window's active document, when known: while another window is drawn it is
    /// swapped out, and the main window's first tab stands in.
    pub(crate) fn main_active(&self) -> Option<DocId> {
        if self.current_window == MAIN {
            self.active.and_then(|i| self.views.get(i)).map(|v| v.id)
        } else {
            self.views.iter().find(|v| v.window == MAIN).map(|v| v.id)
        }
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
        self.open_window(&[id], id, None);
    }

    /// Open a window showing the documents `ids` (the first that is open, when `active` isn't),
    /// at `placement` when given, with the workspace and panels of the window being drawn.
    /// Returns its key, or `None` when none of `ids` is open.
    pub(crate) fn open_window(&mut self, ids: &[DocId], active: DocId, placement: Option<egui::Rect>) -> Option<u32> {
        let open: Vec<DocId> = ids.iter().copied().filter(|id| self.views.iter().any(|v| v.id == *id)).collect();
        let active = if open.contains(&active) { active } else { *open.first()? };
        let key = self.next_window;
        self.next_window = self.next_window.saturating_add(1).max(MAIN + 1);
        for view in self.views.iter_mut().filter(|v| open.contains(&v.id)) {
            view.window = key;
        }
        let slice = Slice {
            window: key,
            active: Some(active),
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
        let placement = placement.and_then(sane_placement);
        self.windows.push(DocWindow { viewport: viewport_of(key), placement, slice, closing: None });
        self.settle_active();
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
        Some(key)
    }

    /// Move the document `id` into the window `key` (another than the one being drawn), as its
    /// active tab, and bring that window forward. Nothing happens for a window that isn't open.
    pub(crate) fn move_to_window(&mut self, id: DocId, key: u32) {
        if key == self.current_window || !self.window_rects.iter().any(|(k, _)| *k == key) {
            return;
        }
        if key == MAIN {
            self.move_to_main_window(id);
        } else {
            let Some(view) = self.views.iter_mut().find(|v| v.id == id) else { return };
            view.window = key;
            self.settle_active();
            self.activations.push((key, id));
        }
        if let Some(ctx) = &self.ctx {
            ctx.send_viewport_cmd_to(viewport_of(key), egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
    }

    /// A tab dragged out of the strip and released at `at` (screen points, when known): onto
    /// another window, it moves there; anywhere else, it opens in a window of its own under the
    /// pointer, `size` (this window's) when known. A window's last tab only goes to another
    /// window: tearing it off would just move the window.
    pub(crate) fn tear_off(&mut self, id: DocId, at: Option<egui::Pos2>, size: Option<egui::Vec2>) {
        let own = self.window_rects.iter().find(|(k, _)| *k == self.current_window).map(|(_, r)| *r);
        let target = at
            .filter(|p| !own.is_some_and(|r| r.contains(*p)))
            .and_then(|p| self.window_rects.iter().find(|(k, r)| *k != self.current_window && r.contains(p)).map(|(k, _)| *k));
        if let Some(key) = target {
            self.move_to_window(id, key);
            return;
        }
        if self.window_tabs().len() < 2 {
            return;
        }
        let placement = at.map(|p| egui::Rect::from_min_size(p - TEAR_GRAB, size.unwrap_or(DEFAULT_SIZE)));
        self.open_window(&[id], id, placement);
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
            self.activations.push((MAIN, id));
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
        self.window_rects = std::iter::once(MAIN)
            .chain(self.windows.iter().map(|w| w.slice.window))
            .filter_map(|key| Some((key, ctx.input_for(viewport_of(key), |i| i.viewport().outer_rect)?)))
            .collect();
        // Embedded, a window would be drawn inside this one, with every widget id twice. Only a
        // test harness embeds: the desktop app's windows are its own, and the web opens none.
        let embedded = ctx.embed_viewports();
        let mut windows = std::mem::take(&mut self.windows);
        for w in windows.iter_mut().filter(|_| !embedded) {
            let mut builder = egui::ViewportBuilder::default()
                .with_title(if w.slice.window_title.is_empty() { "KreativPdf" } else { w.slice.window_title.as_str() })
                .with_min_inner_size(MIN_SIZE);
            // Sent once: egui only moves or resizes a window when what it is given changes.
            builder = match w.placement {
                Some(r) => builder.with_position(r.min).with_inner_size(r.size()),
                None => builder.with_inner_size(DEFAULT_SIZE),
            };
            builder = builder.with_drag_and_drop(true).with_decorations(!self.draws_window_frame());
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
        for (key, id) in std::mem::take(&mut self.activations) {
            // Moved on, or closed, since.
            if !self.views.iter().any(|v| v.id == id && v.window == key) {
                continue;
            }
            if key == MAIN {
                self.active = self.views.iter().position(|v| v.id == id);
            } else if let Some(w) = self.windows.iter_mut().find(|w| w.slice.window == key) {
                w.slice.active = Some(id);
            }
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

/// `r` when it can place a window: finite and within reach of any monitor, sized at least the
/// smallest window and at most a very large one. A pointer position and settings are untrusted.
fn sane_placement(r: egui::Rect) -> Option<egui::Rect> {
    const REACH: f32 = 100_000.0;
    let ok = |v: f32| v.is_finite() && v.abs() <= REACH;
    if !(ok(r.min.x) && ok(r.min.y) && ok(r.max.x) && ok(r.max.y)) {
        return None;
    }
    let size = r.size().max(MIN_SIZE).min(egui::vec2(16_384.0, 16_384.0));
    Some(egui::Rect::from_min_size(r.min, size))
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
        assert_eq!(app.activations, [(MAIN, moved)], "made active once the main window is drawn");
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

    fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    /// A tab torn off and released away from every window opens in a window of its own under
    /// the pointer, the size of the one it came from; the main window keeps the others.
    #[test]
    fn a_torn_off_tab_opens_in_a_window_under_the_pointer() {
        let mut app = app_with(3);
        app.window_rects = vec![(MAIN, rect(0.0, 0.0, 1000.0, 700.0))];
        let torn = id(&app, 1);
        app.tear_off(torn, Some(egui::pos2(1400.0, 300.0)), Some(egui::vec2(900.0, 600.0)));
        assert_eq!(app.window_count(), 2);
        assert_eq!(app.window_tabs(), [0, 2]);
        let w = &app.windows[0];
        assert_eq!(w.slice.active, Some(torn));
        assert_eq!(w.placement, Some(rect(1400.0 - TEAR_GRAB.x, 300.0 - TEAR_GRAB.y, 900.0, 600.0)));
        // Released over its own window, below the strip, it opens a window too.
        app.tear_off(id(&app, 2), Some(egui::pos2(500.0, 400.0)), None);
        assert_eq!(app.window_count(), 3);
        assert_eq!(app.windows[1].placement.map(|r| r.size()), Some(DEFAULT_SIZE));
    }

    /// A window's last tab stays: tearing it off would only move the window.
    #[test]
    fn a_windows_last_tab_does_not_tear_off_into_a_window_of_its_own() {
        let mut app = app_with(1);
        app.window_rects = vec![(MAIN, rect(0.0, 0.0, 1000.0, 700.0))];
        app.tear_off(id(&app, 0), Some(egui::pos2(1400.0, 300.0)), None);
        assert!(!app.has_other_windows());
        assert_eq!(app.window_tabs(), [0]);
    }

    /// Released over another window, a torn-off tab moves into it and becomes its active tab
    /// once the windows are drawn; a last tab can go too, and a window that is gone takes none.
    #[test]
    fn a_tab_dropped_on_another_window_moves_into_it() {
        let mut app = app_with(3);
        app.move_to_new_window(id(&app, 0));
        let key = app.windows[0].slice.window;
        app.window_rects = vec![(MAIN, rect(0.0, 0.0, 1000.0, 700.0)), (key, rect(1100.0, 0.0, 800.0, 600.0))];
        let dropped = id(&app, 2);
        app.tear_off(dropped, Some(egui::pos2(1500.0, 300.0)), None);
        assert_eq!(app.window_count(), 2, "no new window");
        assert_eq!(app.views[2].window, key);
        assert_eq!(app.activations, [(key, dropped)]);
        // From that window back onto the main one: its last tab goes, as it can't tear off.
        let mut w = app.windows.remove(0);
        app.swap_slice(&mut w.slice);
        app.tear_off(id(&app, 0), Some(egui::pos2(300.0, 300.0)), None);
        app.swap_slice(&mut w.slice);
        app.windows.insert(0, w);
        assert_eq!(app.views[0].window, MAIN);
        app.activations.clear();
        app.move_to_window(id(&app, 1), 99);
        assert_eq!(app.views[1].window, MAIN, "no window 99");
    }

    /// A window is never placed from a position that can't be on any screen, and never smaller
    /// than the smallest window.
    #[test]
    fn untrusted_placements_are_refused_or_clamped() {
        assert_eq!(sane_placement(rect(f32::NAN, 0.0, 900.0, 600.0)), None);
        assert_eq!(sane_placement(rect(0.0, 1e9, 900.0, 600.0)), None);
        assert_eq!(sane_placement(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(f32::INFINITY, 1.0))), None);
        assert_eq!(sane_placement(rect(-1920.0, 40.0, 10.0, 10.0)), Some(rect(-1920.0, 40.0, MIN_SIZE.x, MIN_SIZE.y)));
    }

    /// The last session remembers the windows beside the main one, their tabs, active tabs and
    /// places, and reopens them so.
    #[test]
    fn the_last_session_reopens_its_windows() {
        let dir = std::env::temp_dir().join(format!("kreativpdf-windows-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = app_with(0);
        let mut paths = Vec::new();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            let path = dir.join(name);
            std::fs::write(&path, app.session.create_blank(200.0, 300.0, 1).unwrap().as_ref()).unwrap();
            let path = path.to_string_lossy().into_owned();
            app.open_path(&path);
            paths.push(path);
        }
        assert_eq!(app.views.len(), 3);
        app.active = Some(0);
        let place = rect(1200.0, 80.0, 900.0, 640.0);
        let key = app.open_window(&[id(&app, 1), id(&app, 2)], id(&app, 2), Some(place)).unwrap();
        app.window_rects = vec![(MAIN, rect(0.0, 0.0, 1000.0, 700.0)), (key, place)];
        let saved = app.open_session();
        assert_eq!(saved.files.iter().map(|f| f.window).collect::<Vec<_>>(), [0, 1, 1]);
        assert_eq!(saved.windows.len(), 1);
        let json = serde_json::to_value(&saved).unwrap();
        let mut later = app_with(0);
        later.reopen_last_session = true;
        later.last_session = crate::last_session::LastSession::from_json(&json);
        later.reopen_last_files(&[]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(later.views.len(), 3);
        assert_eq!(later.window_count(), 2);
        assert_eq!(later.window_tabs(), [0], "a.pdf stays in the main window");
        assert_eq!(later.active, Some(0));
        let w = &later.windows[0];
        assert!(later.views[1].window == w.slice.window && later.views[2].window == w.slice.window);
        assert_eq!(w.slice.active, Some(later.views[2].id), "c.pdf is in front in its window");
        assert_eq!(w.placement, Some(place));
    }
}
