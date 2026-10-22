//! Document Properties ▸ Initial View and reading options: how the document opens (navigation
//! panel, page layout, magnification, page; window and interface options) and its language and
//! binding (ISO 32000-2 §7.7.2 Table 29, §12.2 viewer preferences).

use pdfcraft_cos::{Dict, Document, Object, PdfString};

use crate::OrganizeError;

/// The navigation panel shown on opening (`/PageMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Navigation {
    #[default]
    PageOnly,
    Bookmarks,
    Pages,
    Attachments,
    Layers,
}

/// `/PageLayout`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Default,
    SinglePage,
    SinglePageContinuous,
    TwoUp,
    TwoUpContinuous,
    TwoUpCoverPage,
    TwoUpContinuousCoverPage,
}

/// The magnification of the opening destination.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Magnification {
    #[default]
    Default,
    ActualSize,
    FitPage,
    FitWidth,
    FitHeight,
    FitVisible,
    /// Percent (100 = actual size).
    Percent(f64),
}

/// Page scaling option in the print dialog (`/ViewerPreferences /PrintScaling`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrintScaling {
    #[default]
    AppDefault,
    None,
}

/// Paper handling option in the print dialog (`/ViewerPreferences /Duplex`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuplexMode {
    Simplex,
    DuplexFlipShortEdge,
    DuplexFlipLongEdge,
}

/// Print dialog presets stored in `/ViewerPreferences` (ISO 32000-2 §12.5.6.5 Table 155).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrintPresets {
    pub page_scaling: Option<PrintScaling>,
    pub duplex: Option<DuplexMode>,
    pub pick_tray_by_pdf_size: Option<bool>,
    /// 1-based page ranges `[first_1, last_1, first_2, last_2, ...]`. Must have an even length.
    pub print_page_range: Option<Vec<u32>>,
    /// Number of copies (≥ 1).
    pub num_copies: Option<u32>,
}

impl PrintPresets {
    /// Returns true if all preset fields are `None`.
    pub fn is_empty(&self) -> bool {
        self.page_scaling.is_none()
            && self.duplex.is_none()
            && self.pick_tray_by_pdf_size.is_none()
            && self.print_page_range.as_ref().is_none_or(Vec::is_empty)
            && self.num_copies.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InitialView {
    pub navigation: Navigation,
    pub layout: Layout,
    pub magnification: Magnification,
    /// 0-based page opened to.
    pub page: usize,
    pub fit_window: bool,
    pub center_window: bool,
    pub full_screen: bool,
    /// Show the document title (not the file name) in the window title.
    pub display_title: bool,
    pub hide_menubar: bool,
    pub hide_toolbar: bool,
    pub hide_window_ui: bool,
    /// `/Lang` (e.g. "en-US").
    pub language: Option<String>,
    /// Right-edge binding (`/Direction /R2L`).
    pub right_to_left: bool,
    /// Print dialog presets (`/ViewerPreferences`).
    pub print_presets: Option<PrintPresets>,
    /// Document-wide base URI for relative links (`/URI` dictionary `/Base` in Document Catalog, ISO 32000-2 §12.6.4.7).
    pub base_url: Option<String>,
}

fn catalog(doc: &Document) -> Result<(pdfcraft_cos::ObjRef, Dict), OrganizeError> {
    let r = doc.root().ok_or(OrganizeError::NoPageTree)?;
    Ok((r, doc.get(r).as_dict().cloned().ok_or(OrganizeError::NoPageTree)?))
}

/// The document's initial view as stored.
pub fn initial_view(doc: &Document) -> InitialView {
    let Ok((_, c)) = catalog(doc) else { return InitialView::default() };
    let name = |d: &Dict, k: &[u8]| d.get(k).map(|o| doc.resolve(o)).and_then(|o| o.as_name().map(<[u8]>::to_vec));
    let mut v = InitialView {
        navigation: match name(&c, b"PageMode").as_deref() {
            Some(b"UseOutlines") => Navigation::Bookmarks,
            Some(b"UseThumbs") => Navigation::Pages,
            Some(b"UseAttachments") => Navigation::Attachments,
            Some(b"UseOC") => Navigation::Layers,
            _ => Navigation::PageOnly,
        },
        full_screen: name(&c, b"PageMode").as_deref() == Some(b"FullScreen"),
        layout: match name(&c, b"PageLayout").as_deref() {
            Some(b"SinglePage") => Layout::SinglePage,
            Some(b"OneColumn") => Layout::SinglePageContinuous,
            Some(b"TwoPageLeft") => Layout::TwoUp,
            Some(b"TwoColumnLeft") => Layout::TwoUpContinuous,
            Some(b"TwoPageRight") => Layout::TwoUpCoverPage,
            Some(b"TwoColumnRight") => Layout::TwoUpContinuousCoverPage,
            _ => Layout::Default,
        },
        language: c.get(b"Lang").map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text())).filter(|s| !s.is_empty()),
        base_url: c.get(b"URI").map(|o| doc.resolve(o)).and_then(|o| match &*o {
            Object::Dict(d) => d.get(b"Base").map(|b| doc.resolve(b)).and_then(|b| b.as_string().map(|s| s.to_text())).filter(|s| !s.is_empty()),
            _ => None,
        }),
        ..InitialView::default()
    };
    if let Some(vp) = c.get(b"ViewerPreferences").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()) {
        let flag = |k: &[u8]| matches!(vp.get(k), Some(Object::Bool(true)));
        v.fit_window = flag(b"FitWindow");
        v.center_window = flag(b"CenterWindow");
        v.display_title = flag(b"DisplayDocTitle");
        v.hide_menubar = flag(b"HideMenubar");
        v.hide_toolbar = flag(b"HideToolbar");
        v.hide_window_ui = flag(b"HideWindowUI");
        v.right_to_left = name(&vp, b"Direction").as_deref() == Some(b"R2L");

        let page_scaling = match name(&vp, b"PrintScaling").as_deref() {
            Some(b"AppDefault") => Some(PrintScaling::AppDefault),
            Some(b"None") => Some(PrintScaling::None),
            _ => None,
        };
        let duplex = match name(&vp, b"Duplex").as_deref() {
            Some(b"Simplex") => Some(DuplexMode::Simplex),
            Some(b"DuplexFlipShortEdge") => Some(DuplexMode::DuplexFlipShortEdge),
            Some(b"DuplexFlipLongEdge") => Some(DuplexMode::DuplexFlipLongEdge),
            _ => None,
        };
        let pick_tray = vp.get(b"PickTrayByPDFSize").map(|o| doc.resolve(o)).and_then(|o| match *o {
            Object::Bool(b) => Some(b),
            _ => None,
        });
        let print_page_range = vp.get(b"PrintPageRange").map(|o| doc.resolve(o)).and_then(|o| match &*o {
            Object::Array(arr) => {
                let nums: Vec<u32> = arr
                    .iter()
                    .filter_map(|x| match *doc.resolve(x) {
                        Object::Int(i) => u32::try_from(i).ok(),
                        _ => None,
                    })
                    .collect();
                if nums.len() == arr.len() && nums.len().is_multiple_of(2) && !nums.is_empty() { Some(nums) } else { None }
            }
            _ => None,
        });
        let num_copies = vp.get(b"NumCopies").map(|o| doc.resolve(o)).and_then(|o| match *o {
            Object::Int(i) => u32::try_from(i).ok().filter(|&n| n >= 1),
            _ => None,
        });
        if page_scaling.is_some() || duplex.is_some() || pick_tray.is_some() || print_page_range.is_some() || num_copies.is_some() {
            v.print_presets = Some(PrintPresets { page_scaling, duplex, pick_tray_by_pdf_size: pick_tray, print_page_range, num_copies });
        }
    }
    // The opening destination: an explicit array, or a GoTo action's.
    let dest = c.get(b"OpenAction").map(|o| doc.resolve(o)).and_then(|o| match &*o {
        Object::Array(a) => Some(a.clone()),
        Object::Dict(d) if d.name(b"S") == Some(b"GoTo") => d.get(b"D").map(|x| doc.resolve(x)).and_then(|x| x.as_array().cloned()),
        _ => None,
    });
    if let Some(d) = dest {
        let pages = crate::walk(doc).unwrap_or_default();
        v.page = d
            .first()
            .and_then(|p| match p {
                Object::Ref(r) => pages.iter().position(|(pr, _)| pr == r),
                Object::Int(i) => usize::try_from(*i).ok(),
                _ => None,
            })
            .unwrap_or(0);
        let kind = d.get(1).and_then(|k| k.as_name().map(<[u8]>::to_vec));
        v.magnification = match kind.as_deref() {
            Some(b"Fit") => Magnification::FitPage,
            Some(b"FitH") => Magnification::FitWidth,
            Some(b"FitV") => Magnification::FitHeight,
            Some(b"FitB" | b"FitBH" | b"FitBV") => Magnification::FitVisible,
            Some(b"XYZ") => match d.get(4).and_then(|z| z.as_f64()) {
                Some(z) if z > 0.0 && (z - 1.0).abs() < 1e-6 => Magnification::ActualSize,
                Some(z) if z > 0.0 => Magnification::Percent(z * 100.0),
                _ => Magnification::Default,
            },
            _ => Magnification::Default,
        };
    }
    v
}

/// Whether viewers should show the document title rather than the file name
/// (`/ViewerPreferences /DisplayDocTitle`), without reading the rest of the initial view.
pub fn displays_doc_title(doc: &Document) -> bool {
    let Ok((_, c)) = catalog(doc) else { return false };
    c.get(b"ViewerPreferences")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().map(|vp| matches!(vp.get(b"DisplayDocTitle").map(|x| doc.resolve(x)).as_deref(), Some(Object::Bool(true)))))
        .unwrap_or(false)
}

/// Write the initial view. An opening action that isn't a plain destination (a script, say) is
/// kept unless the page or magnification asks for a destination.
pub fn set_initial_view(doc: &mut Document, v: &InitialView) -> Result<(), OrganizeError> {
    let (root, c) = catalog(doc)?;
    let pages = crate::walk(doc)?;
    if v.page >= pages.len() {
        return Err(OrganizeError::NoSuchPage(v.page));
    }
    let page_ref = pages[v.page].0;
    let mut vp = c.get(b"ViewerPreferences").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    for (k, on) in [
        (&b"FitWindow"[..], v.fit_window),
        (b"CenterWindow", v.center_window),
        (b"DisplayDocTitle", v.display_title),
        (b"HideMenubar", v.hide_menubar),
        (b"HideToolbar", v.hide_toolbar),
        (b"HideWindowUI", v.hide_window_ui),
    ] {
        if on {
            vp.set(k.to_vec(), Object::Bool(true));
        } else {
            vp.remove(k);
        }
    }
    if v.right_to_left {
        vp.set(b"Direction".to_vec(), Object::name("R2L"));
    } else {
        vp.remove(b"Direction");
    }
    if let Some(presets) = &v.print_presets {
        match presets.page_scaling {
            Some(PrintScaling::AppDefault) => {
                vp.set(b"PrintScaling".to_vec(), Object::name("AppDefault"));
            }
            Some(PrintScaling::None) => {
                vp.set(b"PrintScaling".to_vec(), Object::name("None"));
            }
            None => {
                vp.remove(b"PrintScaling");
            }
        }
        match presets.duplex {
            Some(DuplexMode::Simplex) => {
                vp.set(b"Duplex".to_vec(), Object::name("Simplex"));
            }
            Some(DuplexMode::DuplexFlipShortEdge) => {
                vp.set(b"Duplex".to_vec(), Object::name("DuplexFlipShortEdge"));
            }
            Some(DuplexMode::DuplexFlipLongEdge) => {
                vp.set(b"Duplex".to_vec(), Object::name("DuplexFlipLongEdge"));
            }
            None => {
                vp.remove(b"Duplex");
            }
        }
        match presets.pick_tray_by_pdf_size {
            Some(b) => {
                vp.set(b"PickTrayByPDFSize".to_vec(), Object::Bool(b));
            }
            None => {
                vp.remove(b"PickTrayByPDFSize");
            }
        }
        match &presets.print_page_range {
            Some(range) if !range.is_empty() && range.len().is_multiple_of(2) => {
                vp.set(b"PrintPageRange".to_vec(), Object::Array(range.iter().map(|&p| Object::Int(i64::from(p))).collect()));
            }
            _ => {
                vp.remove(b"PrintPageRange");
            }
        }
        match presets.num_copies {
            Some(n) if n >= 1 => {
                vp.set(b"NumCopies".to_vec(), Object::Int(i64::from(n)));
            }
            _ => {
                vp.remove(b"NumCopies");
            }
        }
    } else {
        vp.remove(b"PrintScaling");
        vp.remove(b"Duplex");
        vp.remove(b"PickTrayByPDFSize");
        vp.remove(b"PrintPageRange");
        vp.remove(b"NumCopies");
    }
    let uri_dict = match &v.base_url {
        Some(url) if !url.trim().is_empty() => {
            let mut u = c.get(b"URI").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
            u.set(b"Base".to_vec(), PdfString::text(url.trim()));
            Some(u)
        }
        _ => c.get(b"URI").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).and_then(|mut u| {
            u.remove(b"Base");
            if u.is_empty() || (u.len() == 1 && u.get(b"Type").is_some()) { None } else { Some(u) }
        }),
    };
    let open_action_is_dest = match c.get(b"OpenAction").map(|o| doc.resolve(o)) {
        None => true,
        Some(o) => matches!(&*o, Object::Array(_)) || o.as_dict().is_some_and(|d| d.name(b"S") == Some(b"GoTo")),
    };
    let dest = match v.magnification {
        Magnification::Default if v.page == 0 => None,
        Magnification::Default => Some(vec![Object::Ref(page_ref), Object::name("XYZ"), Object::Null, Object::Null, Object::Null]),
        Magnification::ActualSize => Some(vec![Object::Ref(page_ref), Object::name("XYZ"), Object::Null, Object::Null, Object::Real(1.0)]),
        Magnification::Percent(p) => {
            Some(vec![Object::Ref(page_ref), Object::name("XYZ"), Object::Null, Object::Null, Object::Real((p / 100.0).clamp(0.01, 64.0))])
        }
        Magnification::FitPage => Some(vec![Object::Ref(page_ref), Object::name("Fit")]),
        Magnification::FitWidth => Some(vec![Object::Ref(page_ref), Object::name("FitH"), Object::Null]),
        Magnification::FitHeight => Some(vec![Object::Ref(page_ref), Object::name("FitV"), Object::Null]),
        Magnification::FitVisible => Some(vec![Object::Ref(page_ref), Object::name("FitB")]),
    };
    doc.update_dict(root, |c| {
        let mode = if v.full_screen {
            Some("FullScreen")
        } else {
            match v.navigation {
                Navigation::PageOnly => None,
                Navigation::Bookmarks => Some("UseOutlines"),
                Navigation::Pages => Some("UseThumbs"),
                Navigation::Attachments => Some("UseAttachments"),
                Navigation::Layers => Some("UseOC"),
            }
        };
        match mode {
            Some(m) => c.set(b"PageMode".to_vec(), Object::name(m)),
            None => {
                c.remove(b"PageMode");
            }
        }
        let layout = match v.layout {
            Layout::Default => None,
            Layout::SinglePage => Some("SinglePage"),
            Layout::SinglePageContinuous => Some("OneColumn"),
            Layout::TwoUp => Some("TwoPageLeft"),
            Layout::TwoUpContinuous => Some("TwoColumnLeft"),
            Layout::TwoUpCoverPage => Some("TwoPageRight"),
            Layout::TwoUpContinuousCoverPage => Some("TwoColumnRight"),
        };
        match layout {
            Some(l) => c.set(b"PageLayout".to_vec(), Object::name(l)),
            None => {
                c.remove(b"PageLayout");
            }
        }
        if vp.is_empty() {
            c.remove(b"ViewerPreferences");
        } else {
            c.set(b"ViewerPreferences".to_vec(), Object::Dict(vp.clone()));
        }
        match &v.language {
            Some(l) if !l.trim().is_empty() => c.set(b"Lang".to_vec(), PdfString::text(l.trim())),
            _ => {
                c.remove(b"Lang");
            }
        }
        match uri_dict {
            Some(u) => c.set(b"URI".to_vec(), Object::Dict(u)),
            None => {
                c.remove(b"URI");
            }
        }
        match (&dest, open_action_is_dest) {
            (Some(d), _) => c.set(b"OpenAction".to_vec(), Object::Array(d.clone())),
            (None, true) => {
                c.remove(b"OpenAction");
            }
            // A script or other action stays.
            (None, false) => {}
        }
    })?;
    Ok(())
}
