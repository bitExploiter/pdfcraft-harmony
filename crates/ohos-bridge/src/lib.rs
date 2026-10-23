//! N-API bridge (HarmonyOS / OpenHarmony): ArkTS loads `libpdfcraft_ohos.so` and calls these.
//! Compiles to an empty library on other targets so the workspace still builds everywhere.
#![cfg(target_env = "ohos")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::Buffer;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

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
    Ok(doc.page_count() as u32)
}

#[napi(object)]
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major.
    pub rgba: Buffer,
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
    Ok(RenderedPage { width: out.width, height: out.height, rgba: bytes.into() })
}

#[napi]
pub fn close(handle: u32) {
    if let Ok(mut docs) = docs().lock() {
        docs.remove(&handle);
    }
}
