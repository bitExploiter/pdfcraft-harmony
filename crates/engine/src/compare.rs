//! Compare files: the text differences between two open documents, a report, and the
//! differences marked as comments in the newer one.

use std::sync::Arc;

pub use pdfcraft_compare::{Change, Comparison, Kind, Side};
// How far Office export reads into form XObjects; `OfficeExport::truncated_pages` lists the pages
// that draw more.
pub use pdfcraft_edit::{MAX_FORM_DEPTH, MAX_FORM_VISITS};

use crate::{DocId, Edit, EditError, Markup, NewAnnotation, NoteIcon, Session, Shape};

/// Acrobat's compare colours: replaced blue, inserted green, deleted red.
pub fn colour(kind: Kind) -> crate::Rgb {
    match kind {
        Kind::Replaced => [0.2, 0.45, 0.95],
        Kind::Inserted => [0.2, 0.75, 0.3],
        Kind::Deleted => [0.9, 0.25, 0.25],
    }
}

impl crate::Document {
    /// Standards ▸ Verify PDF/A: the rules the document breaks for `level`.
    pub fn pdfa_verify(&self, level: pdfcraft_preflight::Level) -> Vec<pdfcraft_preflight::Issue> {
        self.editor.as_ref().map(|e| pdfcraft_preflight::verify(&e.cos, level)).unwrap_or_default()
    }

    /// The standards the document declares (Standards panel).
    pub fn standards(&self) -> pdfcraft_preflight::Declared {
        self.editor.as_ref().map(|e| pdfcraft_preflight::declared(&e.cos)).unwrap_or_default()
    }

    /// Every word of the document in reading order, with page and box.
    pub fn words(&self) -> Vec<pdfcraft_compare::Word> {
        let config = pdfcraft_render::RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        for (page, info) in self.info.pages.iter().enumerate() {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            if let Some(t) = res.text {
                out.extend(crate::js::page_words(&t, info).into_iter().map(|(text, rect)| pdfcraft_compare::Word { text, page, rect }));
            }
        }
        out
    }
}

impl Session {
    /// Compare files: the text differences from document `old` to document `new`.
    pub fn compare(&self, old: DocId, new: DocId) -> Result<Comparison, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        Ok(pdfcraft_compare::compare(&a.words(), &b.words()))
    }

    /// Visual compare: regions where page n of `new` looks different from page n of `old`
    /// (rendered at `dpi`), as (page, user-space box in `new`).
    pub fn compare_visual(&self, old: DocId, new: DocId, dpi: f32) -> Result<Vec<(usize, [f64; 4])>, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        let renderer = |d: &crate::Document| {
            pdfcraft_render::PageRenderer::new(
                d.bytes.clone(),
                pdfcraft_render::RenderConfig { password: d.password.as_deref().map(Arc::from), ..Default::default() },
            )
        };
        let (mut ra, mut rb) = (renderer(a), renderer(b));
        let scale = dpi.clamp(18.0, 150.0) / 72.0;
        let mut out = Vec::new();
        for (page, info) in b.info.pages.iter().enumerate().take(a.info.pages.len()) {
            let req = pdfcraft_render::RenderRequest { page, scale, ..Default::default() };
            let (x, y) = (ra.render(req), rb.render(req));
            if x.error.is_some() || y.error.is_some() {
                continue;
            }
            let s = y.width as f32 / info.width.max(1e-3);
            for r in pdfcraft_compare::visual_regions((&y.rgba, y.width, y.height), (&x.rgba, x.width, x.height), 24) {
                let p = info.view_to_user(r[0] as f32 / s, r[1] as f32 / s);
                let q = info.view_to_user(r[2] as f32 / s, r[3] as f32 / s);
                out.push((page, [p[0].min(q[0]) as f64, p[1].min(q[1]) as f64, p[0].max(q[0]) as f64, p[1].max(q[1]) as f64]));
            }
        }
        Ok(out)
    }

    /// The compare report as a new PDF (not opened).
    pub fn compare_report(&self, old: DocId, new: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        let c = self.compare(old, new)?;
        let name = |id| self.get(id).map(|d| d.name.clone()).unwrap_or_default();
        self.create_report("Compare Report", &pdfcraft_compare::report(&c, &name(old), &name(new)))
    }

    /// Mark the differences in `new` as comments: highlights over replaced and inserted text
    /// (blue, green) and a note where text was deleted (red), authored "Compare". One undoable
    /// step; returns how many comments were added.
    pub fn mark_differences(&mut self, old: DocId, new: DocId) -> Result<usize, EditError> {
        let c = self.compare(old, new)?;
        let mut edits = Vec::new();
        for ch in &c.changes {
            let color = colour(ch.kind);
            let contents = match ch.kind {
                Kind::Replaced => format!("Replaced: \"{}\" with \"{}\"", ch.old.text, ch.new.text),
                Kind::Inserted => format!("Inserted: \"{}\"", ch.new.text),
                Kind::Deleted => format!("Deleted: \"{}\"", ch.old.text),
            };
            let shape = if ch.new.rects.is_empty() {
                let Some(r) = ch.new.near else { continue };
                Shape::Note { at: [r[0], r[3]], icon: NoteIcon::Note }
            } else {
                Shape::TextMarkup {
                    kind: Markup::Highlight,
                    quads: ch.new.rects.iter().map(|r| [r[0], r[3], r[2], r[3], r[0], r[1], r[2], r[1]]).collect(),
                }
            };
            let mut style = crate::Style::default_for(&shape);
            style.color = color;
            edits.push(Edit::AddAnnotation(NewAnnotation { page: ch.new.page, shape, style, contents, author: "Compare".into() }));
        }
        let n = edits.len();
        if n > 0 {
            self.apply(new, Edit::Batch { label: "Mark differences".into(), edits })?;
        }
        Ok(n)
    }
}

/// Export a PDF ▸ Word, PowerPoint, HTML or RTF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeFormat {
    Docx,
    Pptx,
    Html,
    Rtf,
}

impl OfficeFormat {
    pub fn extension(self) -> &'static str {
        match self {
            OfficeFormat::Docx => "docx",
            OfficeFormat::Pptx => "pptx",
            OfficeFormat::Html => "html",
            OfficeFormat::Rtf => "rtf",
        }
    }

    pub fn from_extension(ext: &str) -> Option<OfficeFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "docx" => Some(OfficeFormat::Docx),
            "pptx" => Some(OfficeFormat::Pptx),
            "html" | "htm" => Some(OfficeFormat::Html),
            "rtf" => Some(OfficeFormat::Rtf),
            _ => None,
        }
    }
}

/// A Word, PowerPoint, HTML or RTF export ([`crate::Document::export_office_checked`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OfficeExport {
    /// The exported file.
    pub bytes: Vec<u8>,
    /// Pages (0-based) that draw forms nested deeper than [`MAX_FORM_DEPTH`] or more of them
    /// than [`MAX_FORM_VISITS`]: export stops reading there to stay bounded on hostile files, so
    /// the text and images of those forms are missing from `bytes`.
    pub truncated_pages: Vec<usize>,
}

impl crate::Document {
    /// The pages as paragraphs and images (for Word, HTML and RTF export), including what
    /// form XObjects draw.
    pub fn export_pages(&self) -> Vec<pdfcraft_export::Page> {
        self.export_pages_checked().0
    }

    /// [`Self::export_pages`], and the pages whose forms were left partly unread
    /// ([`OfficeExport::truncated_pages`]).
    fn export_pages_checked(&self) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
        let Some(cos) = self.editor.as_ref().map(|e| &e.cos) else { return (Vec::new(), Vec::new()) };
        let boxes: Vec<pdfcraft_office::PageBox> = self
            .info
            .pages
            .iter()
            .map(|p| pdfcraft_office::PageBox { width: p.width, height: p.height, crop: p.crop, rotation: p.rotation })
            .collect();
        pdfcraft_office::pages(cos, &boxes, None)
    }

    /// The document as a Word, PowerPoint, HTML or RTF file.
    pub fn export_office(&self, format: OfficeFormat) -> Vec<u8> {
        self.export_office_checked(format).bytes
    }

    /// [`Self::export_office`], and the pages whose text it had to leave out
    /// ([`OfficeExport::truncated_pages`]).
    pub fn export_office_checked(&self, format: OfficeFormat) -> OfficeExport {
        let (pages, truncated_pages) = self.export_pages_checked();
        let title = self.info.title.clone().unwrap_or_else(|| self.name.trim_end_matches(".pdf").to_string());
        let target = match format {
            OfficeFormat::Docx => pdfcraft_office::Target::Docx,
            OfficeFormat::Pptx => pdfcraft_office::Target::Pptx,
            OfficeFormat::Html => pdfcraft_office::Target::Html,
            OfficeFormat::Rtf => pdfcraft_office::Target::Rtf,
        };
        let bytes = pdfcraft_office::write(&pages, target, &title);
        OfficeExport { bytes, truncated_pages }
    }
}
