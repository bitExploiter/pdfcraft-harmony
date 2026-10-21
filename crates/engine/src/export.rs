//! Export a PDF ▸ Image and Text (execution plan M10.3, the first formats).
//!
//! [`Exporter`] renders pages to PNG, JPEG or TIFF at a resolution and extracts reading-order
//! text, from a
//! document's working file (so unsaved edits and hidden layers are respected, as on screen).

use std::sync::atomic::{AtomicBool, Ordering};

use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};

use crate::Document;

/// Renders and extracts pages of one document state.
pub struct Exporter {
    renderer: PageRenderer,
    pages: usize,
    /// Each page's displayed size in points, for [`Exporter::dpi_used`].
    sizes: Vec<(f32, f32)>,
}

/// What an export needs from a document, as plain values that can move to a worker thread.
#[derive(Clone)]
pub struct ExportSource {
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub config: pdfcraft_render::RenderConfig,
    pub pages: usize,
    /// Each page's displayed size in points (after `/Rotate` and `/UserUnit`).
    pub sizes: Vec<(f32, f32)>,
}

impl Document {
    /// What an export reads, for exporting on another thread. These are the displayed
    /// bytes, so they may carry missing comment appearances PdfCraft drew. Signature
    /// artwork stays original; viewer validation badges are not document content and
    /// do not enter image/text exports or Print as Image. Saving uses the working file.
    pub fn export_source(&self) -> ExportSource {
        ExportSource {
            bytes: self.display.clone(),
            config: self.config.clone(),
            pages: self.info.pages.len(),
            sizes: self.info.pages.iter().map(|p| (p.width, p.height)).collect(),
        }
    }
}

/// Export all images: the images `pages` (0-based) use, each once, skipping those under
/// `min_side` pixels on their shorter side. JPEGs come out unchanged, other images as PNG.
pub fn extract_images(src: &ExportSource, pages: &[usize], min_side: u32) -> Result<pdfcraft_create::ImageExport, String> {
    let doc = pdfcraft_cos::Document::open_with_password(src.bytes.clone(), src.config.password.as_deref()).map_err(|e| e.to_string())?;
    Ok(pdfcraft_create::extract_images(&doc, pages, min_side))
}

/// The file name for the `index`-th (1-based) exported image: `<stem>_Page_<n>_Image_<index>.<ext>`.
/// The page number has leading zeros for as many digits as the document's `page_count` has, so the
/// files sort in page order (#2208): page 7 of 120 is `Page_007`.
pub fn image_file_name(stem: &str, image: &pdfcraft_create::ExtractedImage, index: usize, page_count: usize) -> String {
    let width = page_count.max(1).to_string().len();
    format!("{stem}_Page_{:0width$}_Image_{index:04}.{}", image.page + 1, image.extension)
}

/// An estimate of the image files Export ▸ Image writes, made before it writes them.
///
/// It is a sample, not the bytes themselves: the pages it measures are drawn and encoded, and
/// their bytes per pixel estimate every page (see [`Exporter::estimate`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SizeEstimate {
    /// Estimated bytes of each page's file, in the order asked for.
    pub pages: Vec<u64>,
    /// The pages measured, in the order asked for (0-based): the first, the middle and the last.
    pub sampled: Vec<usize>,
}

impl SizeEstimate {
    /// Estimated bytes of all the files together.
    pub fn total(&self) -> u64 {
        self.pages.iter().copied().fold(0, u64::saturating_add)
    }
}

/// What [`Exporter::estimate_cancellable`] returns when it is stopped. The estimate it was making
/// would have been thrown away, so it says why instead of reporting one.
const ESTIMATE_CANCELLED: &str = "the size estimate was cancelled";

/// The pages [`Exporter::estimate`] measures: the first, the middle and the last of `pages`, each
/// once, so a short range is measured whole and a long one samples its ends and its middle.
fn sample_of(pages: &[usize]) -> Vec<usize> {
    let mut out = Vec::new();
    for i in [0, pages.len() / 2, pages.len().saturating_sub(1)] {
        if let Some(&page) = pages.get(i)
            && !out.contains(&page)
        {
            out.push(page);
        }
    }
    out
}

impl Exporter {
    /// How many pixels [`Exporter::estimate`] draws of a page at most. A page whose export at the
    /// asked resolution is larger is sampled smaller: bytes per pixel fall slowly with resolution,
    /// so the estimate stays close without drawing tens of megapixels for every setting.
    const ESTIMATE_SAMPLE_PIXELS: f64 = 16.0e6;
    /// The fewest pixels a sample has, along its longer side, however large the page.
    const ESTIMATE_SAMPLE_SIDE: f64 = 512.0;

    pub fn new(doc: &Document) -> Self {
        Self::from_source(doc.export_source())
    }

    /// An exporter that draws a page too large for the renderer's size limits at the largest
    /// resolution they allow, as the interactive Export does; [`Exporter::dpi_used`] says at what.
    /// A source whose `config.reject_oversize` is set refuses such pages instead.
    pub fn from_source(src: ExportSource) -> Self {
        Self { renderer: PageRenderer::new(src.bytes, src.config), pages: src.pages, sizes: src.sizes }
    }

    /// The resolution page `page` is actually exported at when asked for `dpi`: lower than asked
    /// when the page is too large for the renderer's size limits at that resolution.
    pub fn dpi_used(&self, page: usize, dpi: f64) -> f64 {
        let dpi = dpi.clamp(18.0, 1200.0);
        match self.sizes.get(page) {
            Some(&(w, h)) => f64::from(pdfcraft_render::effective_scale(w, h, (dpi / 72.0) as f32)) * 72.0,
            None => dpi,
        }
    }

    fn check(&self, page: usize) -> Result<(), String> {
        if page < self.pages { Ok(()) } else { Err(format!("page {} does not exist", page + 1)) }
    }

    /// Page `page` (0-based) as a PNG at `dpi`, at most the renderer's size limits allow (see
    /// [`Exporter::dpi_used`]); refused instead when the source asked for `reject_oversize`.
    pub fn png(&mut self, page: usize, dpi: f64) -> Result<Vec<u8>, String> {
        self.check(page)?;
        let r = self.renderer.render(RenderRequest { page, scale: (dpi.clamp(18.0, 1200.0) / 72.0) as f32, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("page {}: {e}", page + 1));
        }
        encode_png(r.width, r.height, &r.rgba)
    }

    /// Page `page` as an image file of `format`, which says the resolution it was drawn at.
    pub fn image(&mut self, page: usize, dpi: f64, format: ImageFormat) -> Result<Vec<u8>, String> {
        self.check(page)?;
        let r = self.renderer.render(RenderRequest { page, scale: (dpi.clamp(18.0, 1200.0) / 72.0) as f32, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("page {}: {e}", page + 1));
        }
        encode_image(r.width, r.height, &r.rgba, format, self.dpi_used(page, dpi))
    }

    /// Estimate the bytes [`Exporter::image`] writes for `pages` at `dpi` in `format`, without
    /// writing them.
    ///
    /// Up to three pages — the first, the middle and the last — are drawn and encoded at the
    /// resolution the export uses, and the bytes per pixel they measure estimate every page,
    /// scaled to that page's own pixel count. A page whose render at the export's resolution would
    /// exceed [`Exporter::ESTIMATE_SAMPLE_PIXELS`] is sampled smaller, which reads a little high
    /// (bytes per pixel fall as resolution rises), so the estimate errs on the large side. The
    /// files can still differ from the estimate, most of all when the pages it did not measure
    /// differ from the ones it did.
    ///
    /// This runs to the end; [`Exporter::estimate_cancellable`] stops early when it is no longer
    /// wanted.
    pub fn estimate(&mut self, pages: &[usize], dpi: f64, format: ImageFormat) -> Result<SizeEstimate, String> {
        self.estimate_cancellable(pages, dpi, format, &AtomicBool::new(false))
    }

    /// [`Exporter::estimate`], given up on when `cancel` is set: it is checked before any page is
    /// drawn and again between samples, so a closed dialog stops drawing pages no one will read.
    /// A single render cannot be interrupted, so a stop waits for the page in hand to finish.
    pub fn estimate_cancellable(&mut self, pages: &[usize], dpi: f64, format: ImageFormat, cancel: &AtomicBool) -> Result<SizeEstimate, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err(ESTIMATE_CANCELLED.to_string());
        }
        for page in pages {
            self.check(*page)?;
        }
        let dpi = dpi.clamp(18.0, 1200.0);
        let sampled = sample_of(pages);
        // The samples' bits per pixel, weighted by the pixels each drew: a sample of a large page
        // counts for as much as the page it stands for.
        let mut bits = 0.0f64;
        let mut drawn = 0.0f64;
        for page in &sampled {
            if cancel.load(Ordering::Relaxed) {
                return Err(ESTIMATE_CANCELLED.to_string());
            }
            let (bytes, pixels) = self.draw_sample(*page, dpi, format)?;
            if pixels > 0.0 {
                bits += bytes as f64 * 8.0;
                drawn += pixels;
            }
        }
        let per_pixel = if drawn > 0.0 { bits / drawn } else { 0.0 };
        let mut estimate = SizeEstimate { pages: Vec::with_capacity(pages.len()), sampled };
        for page in pages {
            let bytes = per_pixel * self.pixels(*page, dpi) / 8.0;
            // A page that drew nothing (an empty page, or one the renderer refused) has no size to
            // report: say zero rather than invent one.
            estimate.pages.push(if bytes.is_finite() && bytes > 0.0 { bytes.round().min(u64::MAX as f64) as u64 } else { 0 });
        }
        Ok(estimate)
    }

    /// Draw and encode page `page` for an estimate: (bytes, pixels drawn).
    fn draw_sample(&mut self, page: usize, dpi: f64, format: ImageFormat) -> Result<(u64, f64), String> {
        let (w, h) = self.page_size(page);
        let scale = self.scale_for(page, dpi);
        let area = f64::from(w.max(1.0)) * f64::from(h.max(1.0));
        // Pixels = area × scale², so this scale keeps a sample within the budget…
        let by_area = (Self::ESTIMATE_SAMPLE_PIXELS / area).sqrt();
        // …unless that would leave fewer pixels than a sample needs to be representative.
        let by_side = Self::ESTIMATE_SAMPLE_SIDE / f64::from(w.max(h).max(1.0));
        let sample_scale = if by_area.is_finite() && by_area > 0.0 { (scale as f64).min(by_area.max(by_side)) as f32 } else { scale };
        let r = self.renderer.render(RenderRequest { page, scale: sample_scale, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("page {}: {e}", page + 1));
        }
        // The sample is thrown away once measured, so it records the export's own resolution:
        // the metadata adds the same few bytes either way, and the encode path stays the one the
        // file will take.
        let bytes = encode_image(r.width, r.height, &r.rgba, format, self.dpi_used(page, dpi))?;
        Ok((bytes.len() as u64, f64::from(r.width) * f64::from(r.height)))
    }

    /// Page `page`'s displayed size in points, or letter paper when the source did not say.
    fn page_size(&self, page: usize) -> (f32, f32) {
        self.sizes.get(page).copied().unwrap_or((612.0, 792.0))
    }

    /// Device pixels per point page `page` is exported at for `dpi` (the renderer's size limits
    /// may lower it; see [`Exporter::dpi_used`]).
    fn scale_for(&self, page: usize, dpi: f64) -> f32 {
        let (w, h) = self.page_size(page);
        pdfcraft_render::effective_scale(w.max(1.0), h.max(1.0), (dpi.clamp(18.0, 1200.0) / 72.0) as f32)
    }

    /// The pixels page `page` covers in its exported file at `dpi`.
    fn pixels(&self, page: usize, dpi: f64) -> f64 {
        let (w, h) = self.page_size(page);
        let scale = self.scale_for(page, dpi);
        f64::from(pdfcraft_render::device_pixels(w, scale)) * f64::from(pdfcraft_render::device_pixels(h, scale))
    }

    /// The reading-order text of a page.
    pub fn text(&mut self, page: usize) -> Result<String, String> {
        self.check(page)?;
        let r = self.renderer.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
        match r.text {
            Some(t) => Ok(t.plain_text()),
            None => Err(format!("page {}: {}", page + 1, r.error.unwrap_or_else(|| "no text layer".into()))),
        }
    }

    /// The text of several pages, separated by form feeds (as `pdftotext` does).
    pub fn text_of(&mut self, pages: &[usize]) -> Result<String, String> {
        let mut out = String::new();
        for (k, p) in pages.iter().enumerate() {
            if k > 0 {
                out.push('\u{c}');
            }
            out.push_str(self.text(*p)?.trim_end());
            out.push('\n');
        }
        Ok(out)
    }
}

/// Premultiplied RGBA → an image file of `format`. A JPEG or TIFF says it is `dpi` dots per inch,
/// so other programs show and print it at the page's size. A PNG says nothing: its `pHYs` counts
/// pixels per metre, so 72 dpi would read back as 72.009 and a page made from it would shrink.
pub fn encode_image(width: u32, height: u32, premultiplied: &[u8], format: ImageFormat, dpi: f64) -> Result<Vec<u8>, String> {
    match format {
        ImageFormat::Png => encode_png(width, height, premultiplied),
        ImageFormat::Jpeg { quality } => encode_jpeg(width, height, premultiplied, quality, dpi),
        ImageFormat::Tiff => encode_tiff(width, height, premultiplied, dpi),
    }
}

/// Premultiplied RGBA → PNG (straight alpha).
pub fn encode_png(width: u32, height: u32, premultiplied: &[u8]) -> Result<Vec<u8>, String> {
    let mut rgba = premultiplied.to_vec();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Image file formats for Export ▸ Image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    /// Quality 1–100.
    Jpeg {
        quality: u8,
    },
    Tiff,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg { .. } => "jpg",
            ImageFormat::Tiff => "tif",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg { .. } => "JPEG",
            ImageFormat::Tiff => "TIFF",
        }
    }
}

/// Premultiplied RGBA composited over white → RGB.
fn over_white(premultiplied: &[u8]) -> Vec<u8> {
    premultiplied
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let k = 255 - p[3];
            [p[0].saturating_add(k), p[1].saturating_add(k), p[2].saturating_add(k)]
        })
        .collect()
}

/// Premultiplied RGBA → baseline JPEG at `dpi` (pages are opaque: transparency becomes white paper).
pub fn encode_jpeg(width: u32, height: u32, premultiplied: &[u8], quality: u8, dpi: f64) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    use image::codecs::jpeg::{JpegEncoder, PixelDensity};
    let rgb = over_white(premultiplied);
    let mut out = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    // The JFIF density is whole dots per inch.
    enc.set_pixel_density(PixelDensity::dpi(dpi.round() as u16));
    enc.write_image(&rgb, width, height, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Premultiplied RGBA → TIFF (RGB, LZW-compressed) at `dpi`. Written with `tiff` directly:
/// `image`'s `TiffEncoder` always says 1 × 1 with no unit and leaves the strips uncompressed.
/// No predictor: LZW alone is the compression TIFF readers support most widely after none.
pub fn encode_tiff(width: u32, height: u32, premultiplied: &[u8], dpi: f64) -> Result<Vec<u8>, String> {
    use tiff::encoder::{Compression, Rational, TiffEncoder, colortype::RGB8};
    use tiff::tags::{ResolutionUnit, Tag};
    let err = |e: tiff::TiffError| e.to_string();
    let rgb = over_white(premultiplied);
    let mut out = std::io::Cursor::new(Vec::new());
    let mut tiff = TiffEncoder::new(&mut out).map_err(err)?.with_compression(Compression::Lzw);
    let mut image = tiff.new_image::<RGB8>(width, height).map_err(err)?;
    // In hundredths: a page drawn at a lowered resolution (`Exporter::dpi_used`) is rarely a whole number.
    let resolution = Rational { n: (dpi * 100.0).round() as u32, d: 100 };
    let tags = image.encoder();
    tags.write_tag(Tag::ResolutionUnit, ResolutionUnit::Inch).map_err(err)?;
    tags.write_tag(Tag::XResolution, resolution.clone()).map_err(err)?;
    tags.write_tag(Tag::YResolution, resolution).map_err(err)?;
    image.write_data(&rgb).map_err(err)?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod image_name_tests {
    use super::image_file_name;

    fn image(page: usize) -> pdfcraft_create::ExtractedImage {
        pdfcraft_create::ExtractedImage {
            page,
            object: pdfcraft_cos::ObjRef { num: 1, generation: 0 },
            width: 1,
            height: 1,
            extension: "png",
            data: Vec::new(),
        }
    }

    #[test]
    fn page_number_is_padded_to_the_page_count() {
        assert_eq!(image_file_name("doc", &image(0), 1, 1), "doc_Page_1_Image_0001.png");
        assert_eq!(image_file_name("doc", &image(0), 1, 9), "doc_Page_1_Image_0001.png");
        assert_eq!(image_file_name("doc", &image(6), 2, 20), "doc_Page_07_Image_0002.png");
        assert_eq!(image_file_name("doc", &image(6), 3, 120), "doc_Page_007_Image_0003.png");
        assert_eq!(image_file_name("doc", &image(119), 4, 120), "doc_Page_120_Image_0004.png");
        assert_eq!(image_file_name("doc", &image(0), 1, 0), "doc_Page_1_Image_0001.png");
    }
}
