//! N-API bridge (HarmonyOS / OpenHarmony): ArkTS loads `libpdfcraft_ohos.so` and calls these.
//! Compiles to an empty library on other targets so the workspace still builds everywhere.
#![cfg(target_env = "ohos")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::{Buffer, Uint8Array};
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

static NEXT: AtomicU32 = AtomicU32::new(1);
fn docs() -> &'static Mutex<HashMap<u32, PageRenderer>> {
    static DOCS: OnceLock<Mutex<HashMap<u32, PageRenderer>>> = OnceLock::new();
    DOCS.get_or_init(Default::default)
}

fn err(msg: impl Into<String>) -> napi_ohos::Error {
    napi_ohos::Error::from_reason(msg.into())
}

fn open(bytes: Vec<u8>) -> napi_ohos::Result<u32> {
    if !bytes.starts_with(b"%PDF") && !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err(err("not a PDF file"));
    }
    let renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    docs().lock().map_err(|_| err("lock poisoned"))?.insert(id, renderer);
    Ok(id)
}

/// Opens a PDF from a sandbox path; returns a document handle.
#[napi]
pub fn open_path(path: String) -> napi_ohos::Result<u32> {
    open(std::fs::read(&path).map_err(|e| err(format!("{path}: {e}")))?)
}

/// Opens a PDF from bytes; returns a document handle.
#[napi]
pub fn open_bytes(data: Buffer) -> napi_ohos::Result<u32> {
    open(data.to_vec())
}

#[napi]
pub fn page_count(handle: u32) -> napi_ohos::Result<u32> {
    let docs = docs().lock().map_err(|_| err("lock poisoned"))?;
    let doc = docs.get(&handle).ok_or_else(|| err("bad handle"))?;
    let n = doc.page_count() as u32;
    hilog(&format!("PDFCRAFT_RUST page_count={n}"));
    Ok(n)
}

#[napi(object)]
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major, stride = width * 4.
    /// A typed array over an ArrayBuffer: OHOS N-API caps Buffer size (a 4.5 MB page failed).
    pub rgba: Uint8Array,
    /// Pixels that are not (near) white: a quick "did anything draw" check.
    pub non_white: u32,
}

/// Renders a page (0-based) at `scale` device pixels per PDF point.
#[napi]
pub fn render_page(handle: u32, page: u32, scale: f64) -> napi_ohos::Result<RenderedPage> {
    let mut docs = docs().lock().map_err(|_| err("lock poisoned"))?;
    let doc = docs.get_mut(&handle).ok_or_else(|| err("bad handle"))?;
    let out = doc.render(RenderRequest { page: page as usize, scale: scale as f32, ..Default::default() });
    if let Some(e) = out.error {
        return Err(err(e));
    }
    let words = out.rgba.into_words();
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for w in words {
        bytes.extend_from_slice(&w.to_ne_bytes());
    }
    let non_white = bytes.chunks_exact(4).filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250).count() as u32;
    hilog(&format!("PDFCRAFT_RUST render page={page} {}x{} bytes={} non_white={non_white}", out.width, out.height, bytes.len()));
    Ok(RenderedPage { width: out.width, height: out.height, rgba: Uint8Array::new(bytes), non_white })
}

#[napi]
pub fn close(handle: u32) {
    if let Ok(mut docs) = docs().lock() {
        docs.remove(&handle);
    }
}

