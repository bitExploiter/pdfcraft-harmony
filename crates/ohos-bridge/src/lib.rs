//! N-API bridge (HarmonyOS / OpenHarmony): ArkTS loads `libpdfcraft_ohos.so` and calls these.
//! Compiles to an empty library on other targets so the workspace still builds everywhere.
//!
//! Errors reach JS as `Error` objects whose message starts with a stable code in brackets:
//! `[E_IO]`, `[E_NOT_PDF]`, `[E_BAD_HANDLE]`, `[E_PAGE_RANGE]`, `[E_RENDER]`.
#![cfg(target_env = "ohos")]

use std::collections::HashMap;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::{AsyncTask, Buffer, Uint8Array};
use napi_ohos::{Env, Task};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

// hilog (libhilog_ndk.z.so): LOG_APP = 0, LOG_INFO = 4.
#[link(name = "hilog_ndk.z")]
unsafe extern "C" {
    fn OH_LOG_Print(kind: i32, level: i32, domain: u32, tag: *const std::ffi::c_char, fmt: *const std::ffi::c_char, ...) -> i32;
}

fn hilog(msg: &str) {
    let msg = std::ffi::CString::new(msg.replace('\0', "")).unwrap_or_default();
    // SAFETY: NUL-terminated C strings; the format consumes exactly one string argument.
    unsafe { OH_LOG_Print(0, 4, 0x0001, c"PdfCraftRust".as_ptr(), c"%{public}s".as_ptr(), msg.as_ptr()) };
}

fn err(code: &str, msg: impl std::fmt::Display) -> napi_ohos::Error {
    napi_ohos::Error::from_reason(format!("[{code}] {msg}"))
}

/// Thumbnails kept per document (keyed by page and size) before the cache is dropped.
const THUMB_CACHE: usize = 256;

pub struct Doc {
    /// Page sizes in points (after /Rotate and /UserUnit).
    sizes: Vec<(f32, f32)>,
    renderer: Mutex<PageRenderer>,
    thumbs: Mutex<HashMap<(u32, u32), Raw>>,
}

#[derive(Clone)]
pub struct Raw {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    non_white: u32,
    millis: u32,
}

static NEXT: AtomicU32 = AtomicU32::new(1);
fn docs() -> &'static Mutex<HashMap<u32, Arc<Doc>>> {
    static DOCS: OnceLock<Mutex<HashMap<u32, Arc<Doc>>>> = OnceLock::new();
    DOCS.get_or_init(Default::default)
}

fn doc(handle: u32) -> napi_ohos::Result<Arc<Doc>> {
    let docs = docs().lock().map_err(|_| err("E_RENDER", "lock poisoned"))?;
    docs.get(&handle).cloned().ok_or_else(|| err("E_BAD_HANDLE", format!("no open document {handle}")))
}

fn open(bytes: Vec<u8>) -> napi_ohos::Result<u32> {
    let t = Instant::now();
    if !bytes.iter().take(1024).collect::<Vec<_>>().windows(5).any(|w| w.iter().copied().eq(b"%PDF-".iter())) {
        return Err(err("E_NOT_PDF", "the file is not a PDF"));
    }
    let bytes = Arc::new(bytes);
    let info = pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| err("E_NOT_PDF", format!("{e:?}")))?;
    let sizes = info.pages.iter().map(|p| (p.width, p.height)).collect::<Vec<_>>();
    let renderer = PageRenderer::new(bytes.clone(), RenderConfig::default());
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    hilog(&format!("PDFCRAFT_RUST open handle={id} bytes={} pages={} ms={}", bytes.len(), sizes.len(), t.elapsed().as_millis()));
    let d = Doc { sizes, renderer: Mutex::new(renderer), thumbs: Mutex::default() };
    docs().lock().map_err(|_| err("E_RENDER", "lock poisoned"))?.insert(id, Arc::new(d));
    Ok(id)
}

/// Opens a PDF from a sandbox path; returns a document handle.
#[napi]
pub fn open_path(path: String) -> napi_ohos::Result<u32> {
    open(std::fs::read(&path).map_err(|e| err("E_IO", format!("{path}: {e}")))?)
}

/// Opens a PDF from bytes; returns a document handle.
#[napi]
pub fn open_bytes(data: Buffer) -> napi_ohos::Result<u32> {
    open(data.to_vec())
}

/// Opens a PDF from a file descriptor (e.g. `fs.openSync(uri).fd` for a DocumentViewPicker URI).
/// The descriptor is duplicated; the caller still owns and closes its own.
#[napi]
pub fn open_fd(fd: i32) -> napi_ohos::Result<u32> {
    if fd < 0 {
        return Err(err("E_IO", "invalid file descriptor"));
    }
    // SAFETY: the caller guarantees `fd` is open for the duration of this call; we only dup it.
    let owned: OwnedFd = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned().map_err(|e| err("E_IO", e))?;
    let mut file = std::fs::File::from(owned);
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).map_err(|e| err("E_IO", e))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| err("E_IO", e))?;
    open(bytes)
}

#[napi]
pub fn page_count(handle: u32) -> napi_ohos::Result<u32> {
    let n = doc(handle)?.sizes.len() as u32;
    hilog(&format!("PDFCRAFT_RUST page_count={n}"));
    Ok(n)
}

#[napi(object)]
pub struct PageSize {
    /// Points (1/72 inch), after /Rotate.
    pub width: f64,
    pub height: f64,
}

/// A page's displayed size in points, so the UI can lay out before rendering.
#[napi]
pub fn page_size(handle: u32, page: u32) -> napi_ohos::Result<PageSize> {
    let d = doc(handle)?;
    let (w, h) = *d.sizes.get(page as usize).ok_or_else(|| err("E_PAGE_RANGE", format!("page {page} of {}", d.sizes.len())))?;
    Ok(PageSize { width: w as f64, height: h as f64 })
}

#[napi(object)]
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major, stride = width * 4. A typed array over an ArrayBuffer:
    /// OHOS N-API caps Buffer size (a 4.5 MB page failed as a Buffer).
    pub rgba: Uint8Array,
    /// Pixels that are not (near) white: a quick "did anything draw" check.
    pub non_white: u32,
    /// Time spent rendering in Rust (0 when served from the thumbnail cache).
    pub millis: u32,
}

fn render_raw(d: &Doc, page: u32, scale: f32, preview: bool) -> napi_ohos::Result<Raw> {
    if page as usize >= d.sizes.len() {
        return Err(err("E_PAGE_RANGE", format!("page {page} of {}", d.sizes.len())));
    }
    let t = Instant::now();
    let out = d
        .renderer
        .lock()
        .map_err(|_| err("E_RENDER", "lock poisoned"))?
        .render(RenderRequest { page: page as usize, scale, preview, ..Default::default() });
    if let Some(e) = out.error {
        return Err(err("E_RENDER", e));
    }
    let mut rgba = Vec::with_capacity(out.rgba.len());
    for w in out.rgba.into_words() {
        rgba.extend_from_slice(&w.to_ne_bytes());
    }
    let non_white = rgba.chunks_exact(4).filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250).count() as u32;
    Ok(Raw { width: out.width, height: out.height, rgba, non_white, millis: t.elapsed().as_millis() as u32 })
}

pub struct RenderTask {
    doc: Arc<Doc>,
    page: u32,
    /// `Some(max_side)` renders a cached preview-quality thumbnail.
    thumb: Option<u32>,
    scale: f32,
}

impl Task for RenderTask {
    type Output = Raw;
    type JsValue = RenderedPage;

    fn compute(&mut self) -> napi_ohos::Result<Raw> {
        let Some(max_side) = self.thumb else {
            let raw = render_raw(&self.doc, self.page, self.scale, false)?;
            hilog(&format!("PDFCRAFT_RUST render page={} {}x{} ms={} non_white={}", self.page, raw.width, raw.height, raw.millis, raw.non_white));
            return Ok(raw);
        };
        let key = (self.page, max_side);
        if let Some(hit) = self.doc.thumbs.lock().ok().and_then(|c| c.get(&key).cloned()) {
            return Ok(Raw { millis: 0, ..hit });
        }
        let (w, h) = *self.doc.sizes.get(self.page as usize).ok_or_else(|| err("E_PAGE_RANGE", format!("page {}", self.page)))?;
        let scale = max_side as f32 / w.max(h).max(1.0);
        let raw = render_raw(&self.doc, self.page, scale, true)?;
        hilog(&format!("PDFCRAFT_RUST thumb page={} {}x{} ms={}", self.page, raw.width, raw.height, raw.millis));
        if let Ok(mut c) = self.doc.thumbs.lock() {
            if c.len() >= THUMB_CACHE {
                c.clear();
            }
            c.insert(key, raw.clone());
        }
        Ok(raw)
    }

    fn resolve(&mut self, _env: Env, raw: Raw) -> napi_ohos::Result<RenderedPage> {
        Ok(RenderedPage { width: raw.width, height: raw.height, rgba: Uint8Array::new(raw.rgba), non_white: raw.non_white, millis: raw.millis })
    }
}

/// Renders a page (0-based) at `scale` device pixels per point, off the UI thread (Promise).
#[napi(ts_return_type = "Promise<RenderedPage>")]
pub fn render_page(handle: u32, page: u32, scale: f64) -> napi_ohos::Result<AsyncTask<RenderTask>> {
    Ok(AsyncTask::new(RenderTask { doc: doc(handle)?, page, thumb: None, scale: scale as f32 }))
}

/// Renders a cached preview thumbnail whose longer side is `max_side` pixels (Promise).
#[napi(ts_return_type = "Promise<RenderedPage>")]
pub fn render_thumbnail(handle: u32, page: u32, max_side: u32) -> napi_ohos::Result<AsyncTask<RenderTask>> {
    Ok(AsyncTask::new(RenderTask { doc: doc(handle)?, page, thumb: Some(max_side.clamp(16, 1024)), scale: 0.0 }))
}

/// Synchronous render (kept for the Part 1 test).
#[napi]
pub fn render_page_sync(handle: u32, page: u32, scale: f64) -> napi_ohos::Result<RenderedPage> {
    let mut t = RenderTask { doc: doc(handle)?, page, thumb: None, scale: scale as f32 };
    let raw = t.compute()?;
    Ok(RenderedPage { width: raw.width, height: raw.height, rgba: Uint8Array::new(raw.rgba), non_white: raw.non_white, millis: raw.millis })
}

/// Drops a document and its caches. Renders already in flight finish on their own copy.
#[napi]
pub fn close(handle: u32) -> bool {
    docs().lock().map(|mut d| d.remove(&handle).is_some()).unwrap_or(false)
}

/// Drops every open document.
#[napi]
pub fn close_all() {
    if let Ok(mut d) = docs().lock() {
        d.clear();
    }
}