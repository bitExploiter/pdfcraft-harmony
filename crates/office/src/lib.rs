//! pdfcraft-office — Convert a PDF ▸ Word (.docx), PowerPoint (.pptx), HTML or RTF (L5).
//!
//! The one entry point other apps open a PDF with: [`convert`] takes the file's bytes and returns
//! an Office file they already read, so a word processor gets paragraphs, headings, tables and
//! pictures and a presentation app one slide per page. PdfCraft's own Export a PDF uses the same
//! reduction ([`pages`]), so the three apps cannot drift apart.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use pdfcraft_render::PageInfo;

/// Biggest file [`convert`] reads (512 MiB). Larger ones are refused before anything is parsed.
pub const MAX_INPUT_BYTES: usize = 512 * 1024 * 1024;

/// What to convert to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Docx,
    Pptx,
    Html,
    Rtf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// The password to open an encrypted PDF with (user or owner password).
    pub password: Option<String>,
    /// Pages to convert (0-based), in this order; `None` converts every page.
    /// [`parse_page_range`] reads them from text such as `1-3, 5`.
    pub pages: Option<Vec<usize>>,
    /// The title written into the Office file; the PDF's own title when `None`.
    pub title: Option<String>,
}

/// A converted PDF.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Converted {
    pub bytes: Vec<u8>,
    /// Pages in the PDF, whether or not all were converted.
    pub page_count: usize,
    /// Pages (0-based) that draw forms nested deeper than [`pdfcraft_edit::MAX_FORM_DEPTH`] or
    /// more of them than [`pdfcraft_edit::MAX_FORM_VISITS`]: reading stops there to stay bounded
    /// on hostile files, so the text and images of those forms are missing.
    pub truncated_pages: Vec<usize>,
    /// Converted pages (0-based) with pictures but no text: scans, whose words only text
    /// recognition can recover.
    pub image_only_pages: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("the file is not a PDF")]
    NotPdf,
    #[error("the PDF is protected by a password")]
    NeedsPassword,
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("the PDF is larger than {} MB", MAX_INPUT_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("{0}")]
    BadPageRange(String),
    #[error("the PDF could not be read: {0}")]
    Damaged(String),
}

/// Does `bytes` hold a PDF? Its header (`%PDF-`) may follow up to 1024 bytes of junk, as
/// readers allow (ISO 32000-2 Annex H).
pub fn is_pdf(bytes: &[u8]) -> bool {
    bytes.get(..bytes.len().min(1024 + 5)).is_some_and(|head| head.windows(5).any(|w| w == b"%PDF-"))
}

/// Pages named like `1-3, 5` (1-based, inclusive ranges, separated by commas or semicolons) as
/// 0-based indices, in the order given and each once. Every page must exist in `count`.
pub fn parse_page_range(text: &str, count: usize) -> Result<Vec<usize>, Error> {
    let bad = || Error::BadPageRange(format!("“{text}” isn't a page range of this PDF (pages 1–{count}): use numbers and ranges such as 1-3, 5"));
    let page = |s: &str| -> Result<usize, Error> {
        let n: usize = s.trim().parse().map_err(|_| bad())?;
        if n == 0 || n > count { Err(bad()) } else { Ok(n - 1) }
    };
    let mut out = Vec::new();
    for part in text.split([',', ';']).map(str::trim) {
        if part.is_empty() {
            return Err(bad());
        }
        let (from, to) = match part.split_once('-') {
            Some((a, b)) => (page(a)?, page(b)?),
            None => {
                let p = page(part)?;
                (p, p)
            }
        };
        if from > to {
            return Err(bad());
        }
        for p in from..=to {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// The pages of `cos` (`only`: these 0-based indices, in this order) as paragraphs and images in
/// display space (points, y up, after `/Rotate` and `/UserUnit`), including what form XObjects
/// draw, and the pages whose forms were left partly unread. `info` is the renderer's page list
/// for the same document.
pub fn pages(cos: &pdfcraft_cos::Document, info: &[PageInfo], only: Option<&[usize]>) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
    let all: Vec<usize> = (0..info.len()).collect();
    let mut truncated = Vec::new();
    let pages = only
        .unwrap_or(&all)
        .iter()
        .filter_map(|&i| Some((i, info.get(i)?)))
        .map(|(i, info)| {
            // Export uses the visible page in points, y up. Raw PDF coordinates include
            // CropBox offsets and precede /Rotate and /UserUnit.
            let visible_rect = |r: [f64; 4]| {
                let a = info.user_to_view(r[0] as f32, r[1] as f32);
                let b = info.user_to_view(r[2] as f32, r[3] as f32);
                [a[0].min(b[0]) as f64, (info.height - a[1].max(b[1])) as f64, a[0].max(b[0]) as f64, (info.height - a[1].min(b[1])) as f64]
            };
            let crop_width = f64::from(info.crop[2] - info.crop[0]);
            let display_width = f64::from(if info.rotation % 180 == 0 { info.width } else { info.height });
            let unit = if crop_width > 0.0 && crop_width.is_finite() && display_width.is_finite() { display_width / crop_width } else { 1.0 };
            let reading = pdfcraft_edit::reading_blocks_checked(cos, i).unwrap_or_default();
            if reading.truncated {
                truncated.push(i);
            }
            let blocks = reading
                .blocks
                .into_iter()
                .filter(|b| !b.text.trim().is_empty())
                .map(|b| {
                    let f = b.base_font.to_ascii_lowercase();
                    pdfcraft_export::Block {
                        text: b.text,
                        rect: visible_rect(b.rect),
                        size: b.size * unit,
                        bold: f.contains("bold") || f.contains("black") || f.contains("heavy"),
                        italic: f.contains("italic") || f.contains("oblique"),
                        color: b.color,
                        rotation: b.rotation - f64::from(info.rotation),
                    }
                })
                .collect();
            let images = pdfcraft_edit::reading_images(cos, i)
                .unwrap_or_default()
                .iter()
                .filter_map(|im| {
                    let (ext, bytes) = pdfcraft_create::image_file(cos, im.object?).ok()?;
                    Some(pdfcraft_export::Image {
                        ext: if ext == "jpg" { "jpg" } else { "png" },
                        bytes,
                        rect: visible_rect(im.rect),
                        size: [im.matrix[0].hypot(im.matrix[1]) * unit, im.matrix[2].hypot(im.matrix[3]) * unit],
                        rotation: im.matrix[1].atan2(im.matrix[0]).to_degrees() - f64::from(info.rotation),
                    })
                })
                .collect();
            pdfcraft_export::Page { width: info.width as f64, height: info.height as f64, blocks, images }
        })
        .collect();
    (pages, truncated)
}

/// `pages` as a file of `target`'s format, titled `title`.
pub fn write(pages: &[pdfcraft_export::Page], target: Target, title: &str) -> Vec<u8> {
    match target {
        Target::Docx => pdfcraft_export::docx(pages, title),
        Target::Pptx => pdfcraft_export::pptx(pages, title),
        Target::Html => pdfcraft_export::html(pages, title).into_bytes(),
        Target::Rtf => pdfcraft_export::rtf(pages).into_bytes(),
    }
}

/// Convert the PDF in `bytes` to `target`. Pages that can't be read lose their text or images
/// rather than failing the whole file; a file that can't be opened at all is an [`Error`].
pub fn convert(bytes: &[u8], target: Target, opts: &Options) -> Result<Converted, Error> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Error::TooLarge);
    }
    if !is_pdf(bytes) {
        return Err(Error::NotPdf);
    }
    // The last-resort guard (AGENTS.md §4): a bug on one hostile file is an error, not a crash
    // of the app that asked.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| convert_unguarded(bytes, target, opts))).unwrap_or_else(|p| {
        let why = p.downcast_ref::<&str>().map(|s| (*s).to_string()).or_else(|| p.downcast_ref::<String>().cloned());
        Err(Error::Damaged(why.unwrap_or_else(|| "unknown error".into())))
    })
}

fn convert_unguarded(bytes: &[u8], target: Target, opts: &Options) -> Result<Converted, Error> {
    let data = Arc::new(bytes.to_vec());
    let password = opts.password.as_deref();
    let info = pdfcraft_render::inspect(data.clone(), password).map_err(open_error)?;
    let cos = pdfcraft_cos::Document::open_with_password(data, password).map_err(|e| Error::Damaged(e.to_string()))?;
    let page_count = info.pages.len();
    if let Some(only) = &opts.pages
        && let Some(missing) = only.iter().find(|&&p| p >= page_count)
    {
        return Err(Error::BadPageRange(format!("page {} isn't in this PDF (pages 1–{page_count})", missing.saturating_add(1))));
    }
    let (pages, truncated_pages) = pages(&cos, &info.pages, opts.pages.as_deref());
    let indices: Vec<usize> = opts.pages.clone().unwrap_or_else(|| (0..page_count).collect());
    let image_only_pages = pages.iter().zip(&indices).filter(|(p, _)| p.blocks.is_empty() && !p.images.is_empty()).map(|(_, &i)| i).collect();
    let title = opts.title.clone().or_else(|| info.title.clone()).unwrap_or_default();
    Ok(Converted { bytes: write(&pages, target, &title), page_count, truncated_pages, image_only_pages })
}

fn open_error(e: pdfcraft_render::OpenError) -> Error {
    match e {
        pdfcraft_render::OpenError::NeedsPassword => Error::NeedsPassword,
        pdfcraft_render::OpenError::WrongPassword => Error::WrongPassword,
        other => Error::Damaged(other.to_string()),
    }
}

#[cfg(test)]
mod tests;
