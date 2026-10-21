//! Keeping the catalog's XMP metadata (`/Root /Metadata`, ISO 32000-2 §14.3.2) in step with the
//! document information when Document Properties ▸ Description changes it.
//!
//! Files that carry XMP state the title and author twice, and viewers that read XMP first show
//! the packet's value, so changing only `/Info` leaves the file disagreeing with itself. Each
//! information key maps to the XMP property PDF/A pairs it with (ISO 19005-1 §6.7.3, the same
//! mapping `pdfcraft-preflight` writes): Title → `dc:title`, Author → `dc:creator`, Subject →
//! `dc:description`, Keywords → `pdf:Keywords`, Creator → `xmp:CreatorTool`, Producer →
//! `pdf:Producer`.
//!
//! The packet is edited as text, touching only the property that changed: everything else in it
//! (other schemas, padding, unknown properties) stays byte for byte. A file without XMP gets
//! none, and a packet this can't edit safely (not UTF-8, no `rdf:RDF`, too large to decode) is
//! left as it was.
//!
//! Document Properties ▸ Description ▸ Additional Metadata edits the properties XMP holds beyond
//! the document information ([`XmpField`]: the author's title, the description writer, a rating
//! and the copyright), adding a packet when the file has none, and replaces or removes the whole
//! packet ([`replace_xmp`]). Properties are found by their conventional prefix (`dc:`, `xmp:`,
//! `xmpRights:`, `photoshop:`), as the information mirror above finds them.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::OrganizeError;
use crate::pdfx::esc;

/// Decoding limit for an XMP packet (they are a few kB; padding rarely takes them past 100 kB).
const MAX_XMP: usize = 16 << 20;

const DC: &str = "http://purl.org/dc/elements/1.1/";
const PDF: &str = "http://ns.adobe.com/pdf/1.3/";
const XMP: &str = "http://ns.adobe.com/xap/1.0/";
const XMP_RIGHTS: &str = "http://ns.adobe.com/xap/1.0/rights/";
const PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The packet header and trailer (XMP Specification Part 1 §7.3): the `begin` byte-order mark and
/// the fixed `id`, and a trailer that lets other tools rewrite the packet in place.
const PACKET_BEGIN: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n";
const PACKET_END: &str = "\n<?xpacket end=\"w\"?>";

/// A property of Document Properties ▸ Description ▸ Additional Metadata that the document
/// information has no entry for: it lives only in the XMP packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum XmpField {
    /// The author's job title (`photoshop:AuthorsPosition`).
    AuthorTitle,
    /// Who wrote the description (`photoshop:CaptionWriter`).
    DescriptionWriter,
    /// A rating from 0 to 5 stars, or -1 for rejected (`xmp:Rating`).
    Rating,
    /// Whether the document is copyrighted (`xmpRights:Marked`): `True` copyrighted, `False` public
    /// domain, absent unknown.
    CopyrightStatus,
    /// The copyright notice (`dc:rights`).
    CopyrightNotice,
    /// A web page with the copyright and licensing terms (`xmpRights:WebStatement`).
    CopyrightUrl,
}

impl XmpField {
    pub const ALL: [XmpField; 6] =
        [Self::AuthorTitle, Self::DescriptionWriter, Self::Rating, Self::CopyrightStatus, Self::CopyrightNotice, Self::CopyrightUrl];

    /// The XMP property (qualified name, namespace) and how it holds its value.
    fn property(self) -> (&'static str, &'static str, Shape) {
        match self {
            Self::AuthorTitle => ("photoshop:AuthorsPosition", PHOTOSHOP, Shape::Text),
            Self::DescriptionWriter => ("photoshop:CaptionWriter", PHOTOSHOP, Shape::Text),
            Self::Rating => ("xmp:Rating", XMP, Shape::Text),
            Self::CopyrightStatus => ("xmpRights:Marked", XMP_RIGHTS, Shape::Text),
            Self::CopyrightNotice => ("dc:rights", DC, Shape::LangAlt),
            Self::CopyrightUrl => ("xmpRights:WebStatement", XMP_RIGHTS, Shape::Text),
        }
    }

    /// The property's qualified name, such as `dc:rights`.
    pub fn name(self) -> &'static str {
        self.property().0
    }

    /// `value` as this property stores it, or why it can't be stored. Empty means "remove".
    fn check(self, value: &str) -> Result<String, OrganizeError> {
        let value = value.trim();
        match self {
            _ if value.is_empty() => Ok(String::new()),
            Self::Rating => match value.parse::<i8>() {
                Ok(n @ -1..=5) => Ok(n.to_string()),
                _ => Err(OrganizeError::Invalid(format!("a rating is a whole number from 0 to 5, or -1 for rejected, not {value:?}"))),
            },
            Self::CopyrightStatus => match value {
                "True" | "False" => Ok(value.to_owned()),
                _ => Err(OrganizeError::Invalid(format!(
                    "the copyright status is True (copyrighted), False (public domain) or empty (unknown), not {value:?}"
                ))),
            },
            _ => Ok(value.to_owned()),
        }
    }
}

/// How an XMP property holds its value.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// A language alternative: `rdf:Alt` with an `x-default` item.
    LangAlt,
    /// An ordered array: `rdf:Seq` (the authors).
    Seq,
    /// Plain text.
    Text,
}

/// The XMP property (qualified name, namespace) that mirrors a document information key.
fn property(key: &str) -> Option<(&'static str, &'static str, Shape)> {
    Some(match key {
        "Title" => ("dc:title", DC, Shape::LangAlt),
        "Author" => ("dc:creator", DC, Shape::Seq),
        "Subject" => ("dc:description", DC, Shape::LangAlt),
        "Keywords" => ("pdf:Keywords", PDF, Shape::Text),
        "Creator" => ("xmp:CreatorTool", XMP, Shape::Text),
        "Producer" => ("pdf:Producer", PDF, Shape::Text),
        _ => return None,
    })
}

/// After the information entry `key` became `value` (empty: removed), make the catalog's XMP
/// packet say the same. Does nothing for keys XMP doesn't mirror, files without XMP, and packets
/// that can't be edited safely.
pub(crate) fn sync_info(doc: &mut Document, key: &str, value: &str) -> Result<(), OrganizeError> {
    let Some((name, ns, shape)) = property(key) else { return Ok(()) };
    let Some(Packet::Text(r, dict, xmp)) = packet(doc) else { return Ok(()) };
    let Some(updated) = set_property(&xmp, name, ns, shape, value) else { return Ok(()) };
    doc.set(r, Object::Stream(Stream::from_raw(dict, updated.into_bytes())));
    Ok(())
}

/// The catalog's metadata stream as found.
enum Packet {
    /// Its object, its dictionary without filters (as it is written back) and its text.
    Text(ObjRef, Dict, String),
    /// A stream this can't edit: it can't be decoded within [`MAX_XMP`] or isn't UTF-8.
    Unreadable,
}

/// The catalog's metadata stream, or `None` when there is none (or `/Metadata` names something
/// other than a stream, which is left alone).
fn packet(doc: &Document) -> Option<Packet> {
    let r = metadata_ref(doc)?;
    let obj = doc.get(r);
    let Object::Stream(stream) = &*obj else { return None };
    let Some(text) = stream.decoded_within(MAX_XMP).ok().and_then(|b| String::from_utf8(b).ok()) else { return Some(Packet::Unreadable) };
    // Written unfiltered, as PDF/A asks of metadata streams, so other tools can read it.
    let mut dict = stream.dict.clone();
    for k in [&b"Filter"[..], b"DecodeParms", b"DL", b"Length"] {
        dict.remove(k);
    }
    Some(Packet::Text(r, dict, text))
}

/// The object the catalog's `/Metadata` names. A metadata stream is always indirect (§7.3.8); a
/// direct value is left alone.
fn metadata_ref(doc: &Document) -> Option<ObjRef> {
    doc.get(doc.root()?).as_dict()?.reference(b"Metadata")
}

/// The catalog's XMP packet as stored (decoded), for showing or exporting it; `None` when the
/// document has none or it can't be decoded within the limit.
pub fn xmp_packet(doc: &Document) -> Option<Vec<u8>> {
    let r = metadata_ref(doc)?;
    let obj = doc.get(r);
    let Object::Stream(stream) = &*obj else { return None };
    stream.decoded_within(MAX_XMP).ok()
}

/// The value of `field` in the document's XMP packet: the `x-default` (else the first) item of a
/// language alternative. `None` when the packet has no such property or can't be read.
pub fn xmp_field(doc: &Document, field: XmpField) -> Option<String> {
    let Some(Packet::Text(_, _, xmp)) = packet(doc) else { return None };
    get_property(&xmp, field.name())
}

/// Set `field` to `value` in the document's XMP packet (an empty value removes it), adding a
/// packet that mirrors the document information when the file has none. Fails, changing nothing,
/// when the value is not one the property takes or the packet can't be edited as text.
pub fn set_xmp_field(doc: &mut Document, field: XmpField, value: &str) -> Result<(), OrganizeError> {
    let value = field.check(value)?;
    let (name, ns, shape) = field.property();
    let (r, dict, xmp) = match packet(doc) {
        Some(Packet::Text(r, dict, xmp)) => (Some(r), dict, xmp),
        Some(Packet::Unreadable) => {
            return Err(OrganizeError::Invalid(
                "the document's XMP metadata can't be edited: it isn't UTF-8 text or is too large; replace the packet instead".into(),
            ));
        }
        // Nothing to remove from a file without XMP.
        None if value.is_empty() => return Ok(()),
        None => (None, metadata_dict(), new_packet(doc)),
    };
    let Some(updated) = set_property(&xmp, name, ns, shape, &value) else {
        // Nothing to remove, or no `rdf:RDF` to add the property to.
        return match value.is_empty() {
            true => Ok(()),
            false => Err(OrganizeError::Invalid("the document's XMP metadata has no rdf:RDF element to add the property to".into())),
        };
    };
    store(doc, r, dict, updated)
}

/// Replace the document's XMP packet with `xmp`, or remove it when `xmp` is empty. The text is
/// stored as given (with a byte-order mark and an XML declaration at the start dropped), wrapped
/// in an `xpacket` header and trailer when it has none; checking that it is well-formed XMP is
/// the caller's.
pub fn replace_xmp(doc: &mut Document, xmp: &str) -> Result<(), OrganizeError> {
    let xmp = xmp.trim_start_matches('\u{feff}').trim();
    if xmp.len() > MAX_XMP {
        return Err(OrganizeError::Invalid(format!("an XMP packet is at most {} MB", MAX_XMP >> 20)));
    }
    let existing = match packet(doc) {
        Some(Packet::Text(r, dict, _)) => Some((r, dict)),
        // Replaced whole, so a stream that couldn't be read keeps only its dictionary's type.
        Some(Packet::Unreadable) => metadata_ref(doc).map(|r| (r, metadata_dict())),
        None => None,
    };
    if xmp.is_empty() {
        if existing.is_some() {
            let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
            doc.update_dict(root, |c| {
                c.remove(b"Metadata");
            })?;
        }
        return Ok(());
    }
    // An .xmp file's `<?xml …?>` declaration must be the first thing in a document, so it can't
    // follow the header; the stream is UTF-8, the declaration's default, so it is dropped.
    let xmp = match xmp.strip_prefix("<?xml").filter(|rest| rest.starts_with(|c: char| c.is_ascii_whitespace())) {
        Some(rest) => rest.find("?>").and_then(|end| rest.get(end + 2..)).unwrap_or(xmp).trim_start(),
        None => xmp,
    };
    let text = if xmp.starts_with("<?xpacket") { xmp.to_owned() } else { format!("{PACKET_BEGIN}{xmp}{PACKET_END}") };
    let (r, dict) = existing.map_or((None, metadata_dict()), |(r, d)| (Some(r), d));
    store(doc, r, dict, text)
}

/// Write `xmp` into the metadata stream `r`, or into a new one the catalog names.
fn store(doc: &mut Document, r: Option<ObjRef>, dict: Dict, xmp: String) -> Result<(), OrganizeError> {
    let stream = Object::Stream(Stream::from_raw(dict, xmp.into_bytes()));
    match r {
        Some(r) => doc.set(r, stream),
        None => {
            let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
            let m = doc.add(stream);
            doc.update_dict(root, |c| c.set(b"Metadata".to_vec(), Object::Ref(m)))?;
        }
    }
    Ok(())
}

/// A metadata stream's dictionary (§14.3.2).
fn metadata_dict() -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Metadata"));
    d.set(b"Subtype".to_vec(), Object::name("XML"));
    d
}

/// A packet for a file without one, carrying the document information it mirrors, so a viewer
/// that reads XMP first still finds the title and author.
fn new_packet(doc: &Document) -> String {
    let mut props = String::new();
    for key in ["Title", "Author", "Subject", "Keywords", "Creator", "Producer"] {
        let Some(value) = crate::info(doc, key).filter(|v| !v.trim().is_empty()) else { continue };
        let Some((name, _, shape)) = property(key) else { continue };
        props.push_str(&format!("   {}\n", element(name, shape, value.trim(), "rdf")));
    }
    format!(
        "{PACKET_BEGIN}<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"{RDF}\">\n\
  <rdf:Description rdf:about=\"\" xmlns:dc=\"{DC}\" xmlns:pdf=\"{PDF}\" xmlns:xmp=\"{XMP}\">\n{props}  </rdf:Description>\n\
 </rdf:RDF>\n</x:xmpmeta>{PACKET_END}"
    )
}

/// The text of the property `name` in `xmp`, element or attribute form; for an `rdf:Alt`,
/// `rdf:Seq` or `rdf:Bag`, the `x-default` item, else the first. `None` when it is missing or empty.
fn get_property(xmp: &str, name: &str) -> Option<String> {
    let rdf = rdf_prefix(xmp);
    let text = if let Some((start, end)) = element_span(xmp, name) {
        let element = xmp.get(start..end)?;
        let inner_start = element.find('>')? + 1;
        // `<dc:rights/>`: no value.
        let inner = element.get(inner_start..)?.strip_suffix(&format!("</{name}>"))?;
        match list_item(inner, rdf) {
            Some(item) => item,
            None if inner.contains('<') => return None,
            None => inner,
        }
    } else {
        let (_, value_start, value_end, _) = attribute_span(xmp, name)?;
        xmp.get(value_start..value_end)?
    };
    let text = unescape(text.trim());
    (!text.is_empty()).then_some(text)
}

/// The `x-default` item of the `rdf:li` items in `inner` (RDF bound to the prefix `rdf`), else
/// the first.
fn list_item<'a>(inner: &'a str, rdf: &str) -> Option<&'a str> {
    let (open, close) = (format!("<{rdf}:li"), format!("</{rdf}:li>"));
    let mut first = None;
    let mut from = 0;
    while let Some(i) = inner.get(from..).and_then(|s| s.find(&open)).map(|i| i + from) {
        let after = i + open.len();
        from = after;
        if !inner.get(after..).and_then(|s| s.chars().next()).is_some_and(|c| c == '>' || c.is_whitespace()) {
            continue;
        }
        let tag_end = after + inner.get(after..)?.find('>')?;
        let tag = inner.get(after..tag_end)?;
        let content_end = tag_end + inner.get(tag_end..)?.find(&close)?;
        let content = inner.get(tag_end + 1..content_end)?;
        if tag.contains("xml:lang=\"x-default\"") || tag.contains("xml:lang='x-default'") {
            return Some(content);
        }
        first.get_or_insert(content);
        from = content_end;
    }
    first
}

/// `s` with its XML character and entity references replaced; an unknown or malformed one is
/// kept as written.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let tail = rest.get(i..).unwrap_or_default();
        let reference = tail.get(1..).and_then(|t| t.find(';')).filter(|&n| n <= 10).and_then(|n| tail.get(1..=n));
        let c = reference.and_then(|r| match r {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                let n = r.strip_prefix("#x").map(|h| u32::from_str_radix(h, 16)).or_else(|| r.strip_prefix('#').map(str::parse))?;
                n.ok().and_then(char::from_u32)
            }
        });
        match (c, reference) {
            (Some(c), Some(r)) => {
                out.push(c);
                rest = tail.get(r.len() + 2..).unwrap_or_default();
            }
            _ => {
                out.push('&');
                rest = tail.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

/// `xmp` with the property `name` (namespace `ns`) set to `value`, or removed when `value` is
/// empty; `None` when nothing changes or the packet has no `rdf:RDF` to add the property to.
fn set_property(xmp: &str, name: &str, ns: &str, shape: Shape, value: &str) -> Option<String> {
    let rdf = rdf_prefix(xmp);
    let element = (!value.is_empty()).then(|| element(name, shape, value, rdf));
    // The element form, `<dc:title>…</dc:title>` (or `<dc:title/>`): replaced where it stands, so
    // it keeps the namespace declaration of the description around it.
    if let Some((start, end)) = element_span(xmp, name) {
        let mut out = String::with_capacity(xmp.len());
        out.push_str(xmp.get(..start)?);
        out.push_str(element.as_deref().unwrap_or_default());
        out.push_str(xmp.get(end..)?);
        return Some(out);
    }
    // The attribute form, `pdf:Keywords="…"` on an `rdf:Description`: only simple text can be
    // written there; a structured value moves to an element of its own.
    if let Some((start, value_start, value_end, quote)) = attribute_span(xmp, name) {
        let mut out = String::with_capacity(xmp.len());
        if shape == Shape::Text && !value.is_empty() {
            let escaped = if quote == '"' { esc(value) } else { esc(value).replace('\'', "&apos;") };
            out.push_str(xmp.get(..value_start)?);
            out.push_str(&escaped);
            out.push_str(xmp.get(value_end..)?);
            return Some(out);
        }
        out.push_str(xmp.get(..start)?);
        out.push_str(xmp.get(value_end + 1..)?);
        return match element {
            Some(e) => insert_description(&out, name, ns, &e, rdf),
            None => Some(out),
        };
    }
    insert_description(xmp, name, ns, element.as_deref()?, rdf)
}

/// The property `name` holding `value`, as an element (RDF bound to the prefix `rdf`).
fn element(name: &str, shape: Shape, value: &str, rdf: &str) -> String {
    match shape {
        Shape::LangAlt => format!("<{name}><{rdf}:Alt><{rdf}:li xml:lang=\"x-default\">{}</{rdf}:li></{rdf}:Alt></{name}>", esc(value)),
        Shape::Seq => format!("<{name}><{rdf}:Seq><{rdf}:li>{}</{rdf}:li></{rdf}:Seq></{name}>", esc(value)),
        Shape::Text => format!("<{name}>{}</{name}>", esc(value)),
    }
}

/// `xmp` with a new `rdf:Description` holding `element` (and declaring its namespace) as the last
/// description of the packet (RDF bound to the prefix `rdf`).
fn insert_description(xmp: &str, name: &str, ns: &str, element: &str, rdf: &str) -> Option<String> {
    let prefix = name.split(':').next()?;
    let at = xmp.rfind(&format!("</{rdf}:RDF>"))?;
    // Every description of a packet describes the same resource (XMP: one `rdf:about` value).
    let about = attribute_span(xmp, &format!("{rdf}:about")).and_then(|(_, s, e, _)| xmp.get(s..e)).unwrap_or_default().replace('"', "&quot;");
    let mut out = String::with_capacity(xmp.len() + element.len() + 128);
    out.push_str(xmp.get(..at)?);
    out.push_str(&format!("<{rdf}:Description {rdf}:about=\"{about}\" xmlns:{prefix}=\"{ns}\">{element}</{rdf}:Description>\n"));
    out.push_str(xmp.get(at..)?);
    Some(out)
}

/// The prefix `xmp` binds to the RDF namespace: `rdf` in nearly every packet, and the answer when
/// it binds none (or binds it as the default namespace, which isn't edited here).
fn rdf_prefix(xmp: &str) -> &str {
    let mut from = 0;
    while let Some(i) = xmp.get(from..).and_then(|s| s.find(RDF)).map(|i| i + from) {
        from = i + RDF.len();
        // `xmlns:r="…"`: the namespace quoted, after `=`, after the declared prefix.
        let Some(before) = xmp.get(..i) else { continue };
        let Some(quote) = before.chars().next_back().filter(|c| matches!(c, '"' | '\'')) else { continue };
        if !xmp.get(from..).is_some_and(|s| s.starts_with(quote)) {
            continue;
        }
        let Some(before) = before.strip_suffix(quote).map(str::trim_end).and_then(|b| b.strip_suffix('=')).map(str::trim_end) else {
            continue;
        };
        let rest = before.trim_end_matches(|c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
        let Some(prefix) = before.get(rest.len()..).filter(|p| !p.is_empty()) else { continue };
        if rest.strip_suffix("xmlns:").is_some_and(|r| r.ends_with(char::is_whitespace)) {
            return prefix;
        }
    }
    "rdf"
}

/// Where the element `name` is in `xmp`, from its `<` to past its end tag (or past `/>`).
fn element_span(xmp: &str, name: &str) -> Option<(usize, usize)> {
    let open = format!("<{name}");
    let mut from = 0;
    while let Some(i) = xmp.get(from..).and_then(|s| s.find(&open)).map(|i| i + from) {
        let after = i + open.len();
        from = after;
        // `<dc:title` must end there: not `<dc:titles`.
        match xmp.get(after..).and_then(|s| s.chars().next()) {
            Some('>' | '/') => {}
            Some(c) if c.is_whitespace() => {}
            _ => continue,
        }
        let tag_end = after + xmp.get(after..)?.find('>')?;
        if xmp.get(..tag_end)?.ends_with('/') {
            return Some((i, tag_end + 1));
        }
        let close = format!("</{name}>");
        let end = tag_end + xmp.get(tag_end..)?.find(&close)? + close.len();
        return Some((i, end));
    }
    None
}

/// Where the attribute `name` is in `xmp`: its start (including the whitespace before it), the
/// start and end of its value, and its quote character. Only matches inside a start tag.
fn attribute_span(xmp: &str, name: &str) -> Option<(usize, usize, usize, char)> {
    let mut from = 0;
    while let Some(i) = xmp.get(from..).and_then(|s| s.find(name)).map(|i| i + from) {
        from = i + name.len();
        let before = xmp.get(..i)?;
        // Preceded by whitespace, and inside a tag (the last `<` comes after the last `>`).
        if !before.ends_with(char::is_whitespace) || before.rfind('<') < before.rfind('>') || before.rfind('<').is_none() {
            continue;
        }
        let rest = xmp.get(from..)?;
        let trimmed = rest.trim_start();
        let Some(after_eq) = trimmed.strip_prefix('=') else { continue };
        let after_eq = after_eq.trim_start();
        let Some(quote) = after_eq.chars().next().filter(|c| matches!(c, '"' | '\'')) else { continue };
        let value_start = xmp.len() - after_eq.len() + 1;
        let value_end = value_start + xmp.get(value_start..)?.find(quote)?;
        let start = before.trim_end().len();
        return Some((start, value_start, value_end, quote));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a LibreOffice export: one description per schema, element forms.
    const PACKET: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\n   <pdf:Producer>Writer 1.0</pdf:Producer>\n  </rdf:Description>\n\
  <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Untitled 1</rdf:li></rdf:Alt></dc:title>\n\
   <dc:creator><rdf:Seq><rdf:li>Old Author</rdf:li></rdf:Seq></dc:creator>\n\
   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">Old subject</rdf:li></rdf:Alt></dc:description>\n\
  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>";

    #[test]
    fn element_values_are_replaced_in_place() {
        let out = set_property(PACKET, "dc:title", DC, Shape::LangAlt, "Guide & <draft>").unwrap();
        assert!(out.contains("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Guide &amp; &lt;draft&gt;</rdf:li></rdf:Alt></dc:title>\n"), "{out}");
        assert!(!out.contains("Untitled 1"));
        // Everything else is untouched.
        assert_eq!(out.replace("Guide &amp; &lt;draft&gt;", "Untitled 1"), PACKET);
        let out = set_property(PACKET, "dc:creator", DC, Shape::Seq, "Demo Consulting").unwrap();
        assert!(out.contains("<dc:creator><rdf:Seq><rdf:li>Demo Consulting</rdf:li></rdf:Seq></dc:creator>"), "{out}");
    }

    #[test]
    fn an_empty_value_removes_the_property() {
        let out = set_property(PACKET, "dc:description", DC, Shape::LangAlt, "").unwrap();
        assert!(!out.contains("dc:description"), "{out}");
        assert!(out.contains("Untitled 1") && out.contains("Old Author"));
        // Nothing to remove: no change.
        assert_eq!(set_property(PACKET, "pdf:Keywords", PDF, Shape::Text, ""), None);
    }

    #[test]
    fn a_missing_property_gets_a_description_of_its_own() {
        let out = set_property(PACKET, "pdf:Keywords", PDF, Shape::Text, "alpha, beta").unwrap();
        let added =
            "<rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"><pdf:Keywords>alpha, beta</pdf:Keywords></rdf:Description>\n";
        assert!(out.find(added).is_some_and(|at| at > out.find("<dc:creator>").unwrap() && at < out.find("</rdf:RDF>").unwrap()), "{out}");
        assert_eq!(out.replacen(added, "", 1), PACKET);
        // A packet with no rdf:RDF to add to is left alone.
        assert_eq!(set_property("<x:xmpmeta/>", "pdf:Keywords", PDF, Shape::Text, "alpha"), None);
    }

    #[test]
    fn attribute_values_are_rewritten_where_they_stand() {
        let packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"r\"><rdf:Description rdf:about='uuid:1' \
                      xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmp:CreatorTool='Old \"tool\"' dc:title=\"Odd\"/></rdf:RDF></x:xmpmeta>";
        let out = set_property(packet, "xmp:CreatorTool", XMP, Shape::Text, "Writer's tool").unwrap();
        assert!(out.contains("xmp:CreatorTool='Writer&apos;s tool'"), "{out}");
        // A title can't be an attribute: it moves to an element, in a description for the same resource.
        let out = set_property(packet, "dc:title", DC, Shape::LangAlt, "Guide").unwrap();
        assert!(!out.contains("dc:title=\"Odd\""), "{out}");
        assert!(out.contains("<rdf:Description rdf:about=\"uuid:1\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title><rdf:Alt>"), "{out}");
        let out = set_property(packet, "xmp:CreatorTool", XMP, Shape::Text, "").unwrap();
        assert!(out.contains("xmlns:dc=\"http://purl.org/dc/elements/1.1/\" dc:title=\"Odd\"/>"), "{out}");
    }

    #[test]
    fn lookalike_names_and_text_are_not_matched() {
        let packet = "<rdf:RDF xmlns:rdf=\"r\"><rdf:Description rdf:about=\"\"><dc:titles>keep</dc:titles>\
                      <dc:subject>say dc:title=\"no\"</dc:subject><dc:title/></rdf:Description></rdf:RDF>";
        let out = set_property(packet, "dc:title", DC, Shape::LangAlt, "New").unwrap();
        assert!(out.contains("<dc:titles>keep</dc:titles>") && out.contains("say dc:title=\"no\""), "{out}");
        assert!(out.contains("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">New</rdf:li></rdf:Alt></dc:title></rdf:Description>"), "{out}");
    }

    #[test]
    fn properties_are_read_in_every_form() {
        assert_eq!(get_property(PACKET, "dc:title").as_deref(), Some("Untitled 1"));
        assert_eq!(get_property(PACKET, "dc:creator").as_deref(), Some("Old Author"));
        assert_eq!(get_property(PACKET, "pdf:Producer").as_deref(), Some("Writer 1.0"));
        assert_eq!(get_property(PACKET, "dc:rights"), None);
        // The x-default item of a language alternative, wherever it stands; else the first item.
        let alt = "<dc:rights><rdf:Alt><rdf:li xml:lang=\"de\">Alle Rechte</rdf:li>\
                   <rdf:li xml:lang='x-default'> All &amp; every right &#169; &#x2122; </rdf:li></rdf:Alt></dc:rights>";
        assert_eq!(get_property(alt, "dc:rights").as_deref(), Some("All & every right © ™"));
        let first = "<dc:rights><rdf:Alt><rdf:li xml:lang=\"de\">Alle Rechte</rdf:li></rdf:Alt></dc:rights>";
        assert_eq!(get_property(first, "dc:rights").as_deref(), Some("Alle Rechte"));
        // Attribute form, either quote.
        let attrs = "<rdf:Description xmpRights:Marked='True' xmpRights:WebStatement=\"https://x.test/?a=1&amp;b=2\"/>";
        assert_eq!(get_property(attrs, "xmpRights:Marked").as_deref(), Some("True"));
        assert_eq!(get_property(attrs, "xmpRights:WebStatement").as_deref(), Some("https://x.test/?a=1&b=2"));
        // Empty, self-closing, or structured without items: no value.
        for empty in ["<dc:rights/>", "<dc:rights>  </dc:rights>", "<dc:rights><rdf:Bag/></dc:rights>", "<rdf:Description dc:rights=\"\"/>"] {
            assert_eq!(get_property(empty, "dc:rights"), None, "{empty}");
        }
        // A look-alike item name is not an item.
        assert_eq!(list_item("<rdf:list>no</rdf:list><rdf:li>yes</rdf:li>", "rdf"), Some("yes"));
    }

    #[test]
    fn references_are_unescaped_and_malformed_ones_kept() {
        assert_eq!(unescape("a &lt;b&gt; &quot;c&quot; &apos;d&apos; &amp;amp;"), "a <b> \"c\" 'd' &amp;");
        assert_eq!(unescape("&#65;&#x42;&#x1F600;"), "AB😀");
        for kept in ["&", "&;", "&nbsp;", "&#xD800;", "&#99999999;", "&#x;", "& amp;", "&ampamp", "é&é", "&verylongentityname;"] {
            assert_eq!(unescape(kept), kept);
        }
    }

    #[test]
    fn field_values_are_normalised() {
        assert_eq!(XmpField::Rating.check(" +3 ").unwrap(), "3");
        assert_eq!(XmpField::Rating.check("").unwrap(), "");
        assert_eq!(XmpField::CopyrightStatus.check("False").unwrap(), "False");
        assert_eq!(XmpField::CopyrightNotice.check("  © 2026  ").unwrap(), "© 2026");
        assert!(XmpField::Rating.check("5.5").is_err());
        assert_eq!(XmpField::ALL.map(XmpField::name).len(), 6);
    }

    #[test]
    fn hostile_packets_do_not_panic() {
        for packet in ["", "<", "<dc:title", "<dc:title>", "<dc:title x='", " dc:title=", "<a dc:title=\"", "</rdf:RDF>", "é<dc:title é>"] {
            for value in ["", "v"] {
                let _ = set_property(packet, "dc:title", DC, Shape::LangAlt, value);
                let _ = set_property(packet, "pdf:Keywords", PDF, Shape::Text, value);
            }
            let _ = get_property(packet, "dc:title");
            let _ = list_item(packet, "rdf");
            let _ = unescape(packet);
        }
        for packet in [
            RDF,
            "\"http://www.w3.org/1999/02/22-rdf-syntax-ns#",
            "=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"",
            "é xmlns:é='http://www.w3.org/1999/02/22-rdf-syntax-ns#",
        ] {
            assert_eq!(rdf_prefix(packet), "rdf", "{packet}");
        }
    }

    #[test]
    fn the_rdf_prefix_is_the_one_the_packet_binds() {
        let bound = |decl: &str| format!("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><r:RDF {decl}/></x:xmpmeta>");
        assert_eq!(rdf_prefix(&bound(&format!("xmlns:r=\"{RDF}\""))), "r");
        assert_eq!(rdf_prefix(&bound(&format!("xmlns:r_1 = '{RDF}'"))), "r_1");
        // Not a binding: the default namespace, another attribute, mismatched quotes, a lookalike URI.
        for decl in
            [format!("xmlns=\"{RDF}\""), format!("about=\"{RDF}\""), format!("xmlns:r=\"{RDF}'"), format!("xmlns:r=\"{RDF}x\""), String::new()]
        {
            assert_eq!(rdf_prefix(&bound(&decl)), "rdf", "{decl}");
        }
    }
}
