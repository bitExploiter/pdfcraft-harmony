//! Document Properties ▸ Additional Metadata: the XMP properties the document information has no
//! entry for (the author's title, the description writer, a rating and the copyright), edited
//! with the Description fields and applied with them as one step, and the whole XMP packet, which
//! can be shown (as Acrobat's Advanced view lists it: by schema, then property), imported,
//! exported or deleted.

use egui::RichText;
use pdfcraft_engine::pdfa::{XmpNode, XmpSchema};
use pdfcraft_engine::{DocId, Document, Edit, XmpField};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// Bytes of the packet the XML source view shows; a larger packet is cut there (export has it all).
const MAX_SHOWN: usize = 256 << 10;

/// The largest .xmp file Import reads: the size the packet check accepts.
#[cfg(not(target_arch = "wasm32"))]
const MAX_IMPORT: u64 = 8 << 20;

/// The .xmp file Import reads: a regular UTF-8 file of at most [`MAX_IMPORT`] bytes. A directory,
/// a device or a FIFO (which would block the frame) is refused before it is opened, and a file
/// that grows past the cap while it is read is refused too.
#[cfg(not(target_arch = "wasm32"))]
fn read_import(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let too_large = || format!("larger than {} MB", MAX_IMPORT >> 20);
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_IMPORT {
        return Err(too_large());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    // Checked again on what was opened, in case the path was replaced after the check above.
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_IMPORT + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_IMPORT {
        return Err(too_large());
    }
    String::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_owned())
}

/// The Additional Metadata fields being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct XmpDraft {
    pub doc: DocId,
    /// The values of [`XmpField::ALL`], in that order, as the engine stores them.
    pub values: [String; 6],
    /// The packet as text for the XML source view, read once when the draft is made.
    packet: Option<String>,
    /// The packet's properties by schema, or why it can't be read.
    schemas: Result<Vec<XmpSchema>, String>,
}

impl XmpDraft {
    pub fn of(id: DocId, doc: &Document) -> Self {
        let bytes = doc.xmp_packet();
        let schemas = bytes.as_deref().map_or(Ok(Vec::new()), |p| pdfcraft_engine::pdfa::xmp_schemas(&String::from_utf8_lossy(p)));
        let packet = bytes.map(|p| {
            let cut = p.get(..MAX_SHOWN).unwrap_or(&p);
            let mut text = String::from_utf8_lossy(cut).into_owned();
            if cut.len() < p.len() {
                text.push('…');
            }
            text
        });
        Self { doc: id, values: XmpField::ALL.map(|f| doc.xmp_field(f).unwrap_or_default()), packet, schemas }
    }

    /// The edits that make the document match the draft.
    pub fn changes(&self, doc: &Document) -> Vec<Edit> {
        XmpField::ALL
            .iter()
            .zip(&self.values)
            .filter(|(f, v)| doc.xmp_field(**f).unwrap_or_default() != v.trim())
            .map(|(f, v)| Edit::SetXmp { field: *f, value: v.clone() })
            .collect()
    }
}

/// What the XMP metadata buttons asked for, done once the dialog has been drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Import,
    Export,
    Delete,
}

fn label(field: XmpField) -> &'static str {
    match field {
        XmpField::AuthorTitle => tl!("Author title"),
        XmpField::DescriptionWriter => tl!("Description writer"),
        XmpField::Rating => tl!("Rating"),
        XmpField::CopyrightStatus => tl!("Copyright status"),
        XmpField::CopyrightNotice => tl!("Copyright notice"),
        XmpField::CopyrightUrl => tl!("Copyright info URL"),
    }
}

/// The choices of a field that takes one of a few values, as (stored value, label).
fn choices(field: XmpField) -> Option<Vec<(&'static str, &'static str)>> {
    match field {
        XmpField::Rating => Some(
            [("", tl!("Not rated")), ("-1", tl!("Rejected"))]
                .into_iter()
                .chain([("0", "0"), ("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")])
                .collect(),
        ),
        XmpField::CopyrightStatus => Some(vec![("", tl!("Unknown")), ("True", tl!("Copyrighted")), ("False", tl!("Public domain"))]),
        _ => None,
    }
}

/// How a stored value reads: a choice's label, else the value itself.
fn shown(field: XmpField, value: &str) -> String {
    choices(field).and_then(|c| c.into_iter().find(|(v, _)| *v == value).map(|(_, l)| l.to_owned())).unwrap_or_else(|| value.to_owned())
}

/// The tab's rows, in the Properties grid (two columns). `draft` is `None` while it is being made.
pub(crate) fn tab(ui: &mut egui::Ui, doc: &Document, draft: Option<&mut XmpDraft>, t: &Tokens) -> Option<Action> {
    let draft = draft?;
    let editable = doc.allows_modification();
    for (i, field) in XmpField::ALL.into_iter().enumerate() {
        let Some(value) = draft.values.get_mut(i) else { continue };
        let l = ui.label(RichText::new(label(field)).color(t.text_muted));
        if !editable {
            let v = shown(field, value);
            ui.add(egui::Label::new(if v.is_empty() { RichText::new("—").color(t.text_faint) } else { RichText::new(v) }).wrap());
        } else if let Some(choices) = choices(field) {
            egui::ComboBox::from_id_salt(("xmp", i)).selected_text(shown(field, value)).show_ui(ui, |ui| {
                for (v, name) in choices {
                    if ui.selectable_label(value == v, name).clicked() {
                        *value = v.to_owned();
                    }
                }
            });
        } else if field == XmpField::CopyrightNotice {
            ui.add(widgets::block(value).desired_rows(3).desired_width(420.0).background_color(t.field).id_salt(("xmp", i))).labelled_by(l.id);
        } else {
            ui.add(widgets::line(value).desired_width(420.0).background_color(t.field).id_salt(("xmp", i))).labelled_by(l.id);
        }
        ui.end_row();
    }
    let mut action = None;
    ui.label(RichText::new(tl!("XMP metadata")).font(theme::semibold(12.5)));
    ui.end_row();
    ui.label("");
    ui.vertical(|ui| {
        if draft.packet.is_none() {
            ui.label(RichText::new(tl!("This document has no XMP metadata.")).color(t.text_muted));
        }
        ui.horizontal(|ui| {
            // The web build has no file reading behind this dialog yet.
            if cfg!(not(target_arch = "wasm32")) && editable && ui.button(tl!("Import…")).clicked() {
                action = Some(Action::Import);
            }
            if draft.packet.is_some() && ui.button(tl!("Export…")).clicked() {
                action = Some(Action::Export);
            }
            if editable && draft.packet.is_some() && ui.button(tl!("Delete")).clicked() {
                action = Some(Action::Delete);
            }
        });
        if let Some(packet) = &draft.packet {
            match &draft.schemas {
                Ok(schemas) => tree(ui, schemas, t),
                // Not well-formed: the source is all there is to show.
                Err(e) => {
                    ui.label(RichText::new(e).color(t.text_muted));
                }
            }
            egui::CollapsingHeader::new(tl!("XML source")).id_salt("xmp-packet").show(ui, |ui| {
                egui::ScrollArea::both().id_salt("xmp-source").max_height(260.0).show(ui, |ui| {
                    // Read-only, but selectable and copyable.
                    ui.add(egui::TextEdit::multiline(&mut packet.as_str()).code_editor().desired_width(f32::INFINITY));
                });
            });
        }
    });
    ui.end_row();
    action
}

/// The packet's properties as Acrobat's Advanced view lists them: a group per schema (its prefix
/// and namespace), each property as `name: value`, arrays and structures opening onto their items.
fn tree(ui: &mut egui::Ui, schemas: &[XmpSchema], t: &Tokens) {
    egui::Frame::new().stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("xmp-tree").max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
            // Tight rows, as a tree reads (the Properties grid spaces its rows wider).
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().interact_size.y = 20.0;
            for (i, schema) in schemas.iter().enumerate() {
                let head = match schema.prefix.as_str() {
                    "" => RichText::new(&schema.namespace).strong(),
                    p => RichText::new(format!("{p}  ·  {}", schema.namespace)).strong(),
                };
                let id = ui.make_persistent_id(("xmp-schema", i));
                egui::CollapsingHeader::new(head).id_salt(id).default_open(true).show(ui, |ui| {
                    for (j, node) in schema.properties.iter().enumerate() {
                        node_row(ui, node, t, id.with(j));
                    }
                });
            }
        });
    });
}

/// One property, item or field; one with children opens onto them. Depth is bounded by the parser.
fn node_row(ui: &mut egui::Ui, node: &XmpNode, t: &Tokens, id: egui::Id) {
    if node.children.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(format!("{}:", node.name)).color(t.text_muted));
            ui.label(node.value.as_deref().unwrap_or_default());
        });
        return;
    }
    let head = match &node.value {
        Some(v) => format!("{}: {v}", node.name),
        None => node.name.clone(),
    };
    egui::CollapsingHeader::new(head).id_salt(id).default_open(true).show(ui, |ui| {
        for (i, child) in node.children.iter().enumerate() {
            node_row(ui, child, t, id.with(i));
        }
    });
}

impl PdfCraftApp {
    /// Do what the XMP metadata buttons asked for.
    pub(crate) fn xmp_action(&mut self, action: Action) {
        match action {
            Action::Import => self.import_xmp_dialog(),
            Action::Export => self.export_xmp_dialog(),
            Action::Delete => {
                if self.apply_edit(Edit::ReplaceXmp { packet: String::new() }) {
                    // Made again from the document on the next frame.
                    self.xmp_draft = None;
                }
            }
        }
    }

    /// Additional Metadata ▸ Import…: replace the packet with an .xmp file's (`save_override` in tests).
    fn import_xmp_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let import = |app: &mut Self, path: std::path::PathBuf| {
                let name = path.display().to_string();
                match read_import(&path) {
                    Ok(packet) => {
                        if app.apply_edit(Edit::ReplaceXmp { packet }) {
                            app.xmp_draft = None;
                        }
                    }
                    Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e)]),
                }
            };
            match self.save_override.clone() {
                Some(p) if p.ends_with(".xmp") || p.ends_with(".xml") => import(self, p.into()),
                Some(_) => {}
                None => {
                    let dialog = rfd::AsyncFileDialog::new().add_filter("XMP", &["xmp", "xml"]).set_title(tl!("Replace XMP metadata"));
                    let target = self.active_ids().map(|(_, id)| id);
                    self.ask_one(crate::pickers::Ask::File(dialog), target, import);
                }
            }
        }
    }

    /// Additional Metadata ▸ Export…: write the packet as stored to an .xmp file.
    fn export_xmp_dialog(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let Some(bytes) = doc.xmp_packet() else { return };
        let stem = crate::files::strip_pdf(&doc.name).to_owned();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let write = move |app: &mut Self, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                Ok(()) => app.notify_fmt("Exported to {path}", &[("path", &path.display().to_string())]),
                Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
            };
            match self.save_override.clone() {
                Some(p) => write(self, p.into()),
                None => {
                    let dialog = rfd::AsyncFileDialog::new()
                        .set_title(tl!("Export XMP metadata"))
                        .set_file_name(format!("{stem}.xmp"))
                        .add_filter("XMP", &["xmp"]);
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = crate::editing::download(&format!("{stem}.xmp"), &bytes);
        }
    }
}
