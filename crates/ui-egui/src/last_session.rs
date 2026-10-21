//! Reopen the files from the last session (Preferences ▸ Documents and view, #442): which files
//! were open when KreativPdf last closed, the page each showed, the active tab, and the windows
//! beside the main one with their tabs and places. Local only, like the recent-files list, and
//! kept only while the preference is on.

use serde::Serialize;

use crate::PdfCraftApp;

/// More files than anyone keeps open. Settings are untrusted, so a longer list is cut here.
pub const MAX_FILES: usize = 64;

/// More windows beside the main one than anyone keeps open; more are cut, as files are.
pub const MAX_WINDOWS: usize = 16;

/// The files open when KreativPdf last closed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LastSession {
    pub files: Vec<SessionFile>,
    /// The path of the tab that was active in the main window.
    pub active: Option<String>,
    /// The windows beside the main one, in order: [`SessionFile::window`] 1 is the first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<SessionWindow>,
}

/// A window beside the main one.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SessionWindow {
    /// The path of its active tab.
    pub active: Option<String>,
    /// Where it was on screen: x, y, width and height in points.
    pub rect: Option<[f32; 4]>,
}

impl Eq for SessionWindow {}

/// One open file and where the reader was in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionFile {
    /// Absolute, so a file opened from the command line by a relative path is found again.
    pub path: String,
    /// The page the tab showed (0-based).
    pub page: usize,
    /// The window it showed in: 0 the main one, `n` the nth of [`LastSession::windows`].
    #[serde(skip_serializing_if = "is_main")]
    pub window: usize,
}

fn is_main(window: &usize) -> bool {
    *window == 0
}

impl LastSession {
    /// Read what [`PdfCraftApp::persist`] wrote. Malformed entries are skipped and the list is
    /// capped at [`MAX_FILES`].
    pub(crate) fn from_json(v: &serde_json::Value) -> Self {
        let files = v["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|f| {
                        let path = f["path"].as_str().filter(|p| !p.is_empty())?.to_string();
                        // `go_to_page` clamps a page past the end.
                        let page = f["page"].as_u64().and_then(|p| usize::try_from(p).ok()).unwrap_or(0);
                        // A window past the last one there can be goes to the main window.
                        let window = f["window"].as_u64().and_then(|w| usize::try_from(w).ok()).filter(|w| *w <= MAX_WINDOWS).unwrap_or(0);
                        Some(SessionFile { path, page, window })
                    })
                    .take(MAX_FILES)
                    .collect()
            })
            .unwrap_or_default();
        let windows = v["windows"]
            .as_array()
            .map(|windows| {
                windows
                    .iter()
                    .take(MAX_WINDOWS)
                    .map(|w| SessionWindow { active: w["active"].as_str().map(str::to_string), rect: rect(&w["rect"]) })
                    .collect()
            })
            .unwrap_or_default();
        Self { files, active: v["active"].as_str().map(str::to_string), windows }
    }
}

/// Four finite numbers, or `None`.
fn rect(v: &serde_json::Value) -> Option<[f32; 4]> {
    let a = v.as_array().filter(|a| a.len() == 4)?;
    let mut out = [0.0; 4];
    for (o, n) in out.iter_mut().zip(a) {
        // Out of f32's range becomes infinite, and is refused with NaN.
        *o = Some(n.as_f64()? as f32).filter(|f| f.is_finite())?;
    }
    Some(out)
}

/// `path` made absolute against the current directory, without resolving links (and without
/// Windows' `\\?\` prefix, which `canonicalize` adds).
#[cfg(not(target_arch = "wasm32"))]
fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

impl PdfCraftApp {
    /// The files open now. Documents that were never saved to a file are left out (crash
    /// recovery keeps their unsaved work), and so is everything on the web, which has no paths.
    pub(crate) fn open_session(&self) -> LastSession {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = |v: &crate::DocView| self.session.get(v.id).and_then(|d| d.path.as_deref()).map(absolute);
            let path_of = |id: pdfcraft_engine::DocId| self.views.iter().find(|v| v.id == id).and_then(path);
            let layout = self.window_layout();
            let index = |key: u32| layout.iter().position(|w| w.0 == key).map_or(0, |i| i + 1);
            let files = self
                .views
                .iter()
                .filter_map(|v| Some(SessionFile { path: path(v)?, page: v.current, window: index(v.window) }))
                .take(MAX_FILES)
                .collect();
            let active = self.main_active().and_then(path_of);
            let windows = layout
                .iter()
                .map(|(_, active, rect)| SessionWindow {
                    active: active.and_then(path_of),
                    rect: rect.map(|r| [r.min.x, r.min.y, r.width(), r.height()]),
                })
                .collect();
            LastSession { files, active, windows }
        }
        #[cfg(target_arch = "wasm32")]
        LastSession::default()
    }

    /// What to save as the last session: the files open when the quit began, once the app is
    /// really quitting (unsaved tabs close one by one before that), else the files open now.
    pub(crate) fn session_to_save(&self) -> LastSession {
        match &self.quit_session {
            Some(s) if self.allow_quit => s.clone(),
            _ => self.open_session(),
        }
    }

    /// A quit is about to close its first tab: remember everything that is still open.
    pub(crate) fn note_quit_session(&mut self) {
        if self.quit_session.is_none() {
            self.quit_session = Some(self.open_session());
        }
    }

    /// The quit was cancelled: what is open from now on is the session again.
    pub(crate) fn forget_quit_session(&mut self) {
        self.quit_session = None;
    }

    /// At startup, with the preference on: reopen the last session's files at the pages they
    /// showed and bring its active tab forward. Files that are gone are skipped, and so are
    /// `skip` (files the app is about to open anyway, e.g. from the command line) and documents
    /// the Recovery dialog offers, so neither opens twice.
    ///
    /// Only one password prompt can wait at a time: the first encrypted file asks for its
    /// password and later ones stay closed (they are in the recent-files list).
    pub fn reopen_last_files(&mut self, skip: &[String]) {
        let last = std::mem::take(&mut self.last_session);
        #[cfg(not(target_arch = "wasm32"))]
        if self.reopen_last_session {
            let skip: Vec<String> =
                skip.iter().map(|p| absolute(p)).chain(self.recoverable.iter().filter_map(|m| m.path.as_deref().map(absolute))).collect();
            let mut active = None;
            // Each window beside the main one: its documents, and the one active in it.
            let mut windows: Vec<(Vec<pdfcraft_engine::DocId>, Option<pdfcraft_engine::DocId>)> = vec![(Vec::new(), None); last.windows.len()];
            let mut prompt = self.password_prompt.take();
            for f in &last.files {
                let open = |app: &Self| {
                    app.views.iter().any(|v| app.session.get(v.id).and_then(|d| d.path.as_deref()).map(absolute).as_deref() == Some(f.path.as_str()))
                };
                if skip.contains(&f.path) || !std::path::Path::new(&f.path).is_file() || open(self) {
                    continue;
                }
                let before = self.views.len();
                self.open_path(&f.path);
                if let Some(p) = self.password_prompt.take() {
                    prompt.get_or_insert(p);
                }
                if self.views.len() > before
                    && let Some(view) = self.views.last_mut()
                {
                    view.go_to_page(f.page);
                    let id = view.id;
                    match f.window.checked_sub(1) {
                        None if last.active.as_ref() == Some(&f.path) => active = Some(id),
                        None => {}
                        Some(w) => {
                            // A file of a window the list doesn't describe goes in a window with
                            // no particular place.
                            if w >= windows.len() {
                                windows.resize(w + 1, (Vec::new(), None));
                            }
                            if let Some((ids, front)) = windows.get_mut(w) {
                                ids.push(id);
                                if last.windows.get(w).and_then(|s| s.active.as_ref()) == Some(&f.path) {
                                    *front = Some(id);
                                }
                            }
                        }
                    }
                }
            }
            self.password_prompt = prompt;
            for (w, (ids, front)) in windows.iter().enumerate() {
                let Some(&first) = ids.first() else { continue };
                let place =
                    last.windows.get(w).and_then(|s| s.rect).map(|[x, y, wd, ht]| egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(wd, ht)));
                self.open_window(ids, front.unwrap_or(first), place);
            }
            if let Some(i) = active.and_then(|id| self.views.iter().position(|v| v.id == id)) {
                self.active = Some(i);
            }
            self.settle_active();
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (last, skip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_sessions_are_skipped_and_capped() {
        let v = serde_json::json!({
            "files": [{"path": 5}, {"path": ""}, "x", {"path": "/a.pdf", "page": -3}, {"path": "/b.pdf", "page": 1e300}, {"path": "/c.pdf", "page": 7}],
            "active": 3,
        });
        let s = LastSession::from_json(&v);
        let pages: Vec<(&str, usize)> = s.files.iter().map(|f| (f.path.as_str(), f.page)).collect();
        assert_eq!(pages, [("/a.pdf", 0), ("/b.pdf", 0), ("/c.pdf", 7)]);
        assert_eq!(s.active, None);
        let many: Vec<serde_json::Value> = (0..1000).map(|i| serde_json::json!({"path": format!("/{i}.pdf")})).collect();
        assert_eq!(LastSession::from_json(&serde_json::json!({"files": many})).files.len(), MAX_FILES);
        assert_eq!(LastSession::from_json(&serde_json::json!("garbage")), LastSession::default());
    }

    #[test]
    fn malformed_windows_are_skipped_and_capped() {
        let v = serde_json::json!({
            "files": [{"path": "/a.pdf", "window": 1}, {"path": "/b.pdf", "window": 9999}, {"path": "/c.pdf", "window": "x"}],
            "windows": [
                {"active": "/a.pdf", "rect": [10, 20, 900, 600]},
                {"rect": [1, 2, 3]},
                {"rect": [1e300, 0, 900, 600]},
                "garbage",
            ],
        });
        let s = LastSession::from_json(&v);
        assert_eq!(s.files.iter().map(|f| f.window).collect::<Vec<_>>(), [1, 0, 0], "an impossible window is the main one");
        assert_eq!(s.windows.len(), 4);
        assert_eq!(s.windows[0], SessionWindow { active: Some("/a.pdf".into()), rect: Some([10.0, 20.0, 900.0, 600.0]) });
        assert!(s.windows[1..].iter().all(|w| *w == SessionWindow::default()), "{:?}", s.windows);
        let many: Vec<serde_json::Value> = (0..100).map(|_| serde_json::json!({})).collect();
        assert_eq!(LastSession::from_json(&serde_json::json!({"windows": many})).windows.len(), MAX_WINDOWS);
    }
}
