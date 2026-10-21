//! XMP metadata for PDF/A: the identification schema and the document information, kept
//! consistent (ISO 19005-2 §6.6).

use pdfcraft_cos::Document;

/// The largest XMP packet read; a larger one is treated as missing.
pub(crate) const MAX_XMP_BYTES: usize = 8 * 1024 * 1024;
/// Namespace URIs of the PDF/A and PDF/UA identification schemas.
pub(crate) const PDFA_ID: &str = "http://www.aiim.org/pdfa/ns/id/";
pub(crate) const PDFUA_ID: &str = "http://www.aiim.org/pdfua/ns/id/";

/// Read an identification property by its expanded XML name, not its arbitrary prefix. A packet
/// that is not well-formed XML (an undeclared prefix, an illegal character) or is too deep or wide
/// to parse safely is read by the conventional prefix (`pdfaid:`, `pdfuaid:`) instead, as before.
pub(crate) fn identifier(xmp: &str, namespace: &str, local: &str) -> Option<String> {
    if xmp.len() > MAX_XMP_BYTES {
        return None;
    }
    match parsed_identifier(xmp, namespace, local) {
        Ok(found) => found,
        Err(()) => {
            let prefix = match namespace {
                PDFA_ID => "pdfaid",
                PDFUA_ID => "pdfuaid",
                _ => return None,
            };
            value(xmp, &format!("{prefix}:{local}"))
        }
    }
}

/// [`identifier`] through an XML parser; `Err` when the packet can't be parsed within the limits.
fn parsed_identifier(xmp: &str, namespace: &str, local: &str) -> Result<Option<String>, ()> {
    if !xml_limits_ok(xmp) {
        return Err(());
    }
    let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: 100_000 };
    let doc = roxmltree::Document::parse_with_options(xmp, options).map_err(|_| ())?;
    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        if let Some(value) = node.attribute((namespace, local)) {
            return Ok(Some(value.trim().to_owned()));
        }
        if node.tag_name().namespace() == Some(namespace) && node.tag_name().name() == local {
            return Ok(node.text().map(|value| value.trim().to_owned()));
        }
    }
    Ok(None)
}

/// Why `xmp` can't be stored as a document's XMP packet, if it can't: it must be well-formed XML
/// (no DTD) within the parser's limits, holding an `rdf:RDF` element (XMP Specification Part 1
/// §7.3). A leading byte-order mark and an `xpacket` header and trailer are allowed.
pub fn check_xmp(xmp: &str) -> Result<(), String> {
    parse(xmp, |doc| match rdf_root(doc) {
        Some(_) => Ok(()),
        None => Err("the XMP packet has no rdf:RDF element".into()),
    })
}

/// `xmp` parsed within the limits (no DTD), handed to `then`.
fn parse<T>(xmp: &str, then: impl FnOnce(&roxmltree::Document) -> Result<T, String>) -> Result<T, String> {
    let xmp = xmp.trim_start_matches('\u{feff}');
    if xmp.len() > MAX_XMP_BYTES {
        return Err(format!("an XMP packet is at most {} MB", MAX_XMP_BYTES >> 20));
    }
    if !xml_limits_ok(xmp) {
        return Err("the XMP packet is nested too deeply or has too many attributes".into());
    }
    let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: 100_000 };
    let doc = roxmltree::Document::parse_with_options(xmp, options).map_err(|e| format!("the XMP packet isn't well-formed XML: {e}"))?;
    then(&doc)
}

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// The first `rdf:RDF` element of `doc`.
fn rdf_root<'a, 'input>(doc: &'a roxmltree::Document<'input>) -> Option<roxmltree::Node<'a, 'input>> {
    doc.descendants().find(|n| is_rdf(n, "RDF"))
}

fn is_rdf(n: &roxmltree::Node, local: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(RDF) && n.tag_name().name() == local
}

/// The properties of one schema (namespace) of an XMP packet, as Acrobat's Additional Metadata ▸
/// Advanced lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct XmpSchema {
    /// The prefix the packet binds the namespace to (empty when it has none).
    pub prefix: String,
    pub namespace: String,
    pub properties: Vec<XmpNode>,
}

/// A property, an array item or a structure field: its name (`dc:title`, or `[1]` for an item,
/// with its `xml:lang` when it has one), its text, and what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct XmpNode {
    pub name: String,
    pub value: Option<String>,
    pub children: Vec<XmpNode>,
}

/// How many properties, items and fields [`xmp_schemas`] lists, and how deep it follows them; what
/// lies beyond is left out, with a `…` node where it was.
const MAX_NODES: usize = 5_000;
const MAX_DEPTH: usize = 16;

/// The properties of `xmp` grouped by schema, in the order the packet first uses each: the
/// properties of every top-level `rdf:Description` (elements and attributes), with arrays
/// (`rdf:Alt`, `rdf:Seq`, `rdf:Bag`) and structures (`rdf:Description`, `rdf:parseType="Resource"`)
/// as children (XMP Specification Part 1 §7). Fails when the packet can't be parsed.
pub fn xmp_schemas(xmp: &str) -> Result<Vec<XmpSchema>, String> {
    parse(xmp, |doc| {
        let rdf = rdf_root(doc).ok_or("the XMP packet has no rdf:RDF element")?;
        let mut schemas: Vec<XmpSchema> = Vec::new();
        let mut budget = MAX_NODES;
        for description in rdf.children().filter(|n| is_rdf(n, "Description")) {
            for (namespace, node) in fields(description, 0, &mut budget) {
                match schemas.iter_mut().find(|s| s.namespace == namespace) {
                    Some(s) => s.properties.push(node),
                    None => {
                        let prefix = description.lookup_prefix(&namespace).unwrap_or_default().to_owned();
                        schemas.push(XmpSchema { prefix, namespace, properties: vec![node] });
                    }
                }
            }
        }
        Ok(schemas)
    })
}

/// The fields of a description or structure: its attributes (other than RDF's and XML's), then its
/// child elements, each with its namespace.
fn fields(node: roxmltree::Node, depth: usize, budget: &mut usize) -> Vec<(String, XmpNode)> {
    let mut out = Vec::new();
    for a in node.attributes() {
        let ns = a.namespace().unwrap_or_default();
        if ns == RDF || ns == XML {
            continue;
        }
        if !take(budget) {
            out.push((ns.to_owned(), more()));
            return out;
        }
        out.push((ns.to_owned(), XmpNode { name: qualified(node, ns, a.name()), value: Some(a.value().trim().to_owned()), children: Vec::new() }));
    }
    for child in node.children().filter(roxmltree::Node::is_element) {
        let ns = child.tag_name().namespace().unwrap_or_default().to_owned();
        if !take(budget) {
            out.push((ns, more()));
            return out;
        }
        let name = qualified(child, &ns, child.tag_name().name());
        out.push((ns, property(child, name, depth + 1, budget)));
    }
    out
}

/// The property (or item) element `el`, named `name`.
fn property(el: roxmltree::Node, name: String, depth: usize, budget: &mut usize) -> XmpNode {
    if depth > MAX_DEPTH {
        return XmpNode { name, value: None, children: vec![more()] };
    }
    if let Some(uri) = el.attribute((RDF, "resource")) {
        return XmpNode { name, value: Some(uri.trim().to_owned()), children: Vec::new() };
    }
    let elements: Vec<_> = el.children().filter(roxmltree::Node::is_element).collect();
    // An array: its items, numbered from 1 as Acrobat numbers them.
    if let [array] = elements.as_slice()
        && (is_rdf(array, "Alt") || is_rdf(array, "Seq") || is_rdf(array, "Bag"))
    {
        let mut children = Vec::new();
        for (i, li) in array.children().filter(|n| is_rdf(n, "li")).enumerate() {
            if !take(budget) {
                children.push(more());
                break;
            }
            let label = match li.attribute((XML, "lang")) {
                Some(lang) => format!("[{}] {lang}", i + 1),
                None => format!("[{}]", i + 1),
            };
            children.push(property(li, label, depth + 1, budget));
        }
        return XmpNode { name, value: None, children };
    }
    // A structure, written as a nested description or with parseType="Resource"; or a simple
    // value whose element also carries qualifiers or shorthand fields as attributes.
    let holder = match elements.as_slice() {
        [d] if is_rdf(d, "Description") => *d,
        _ => el,
    };
    let text: String = el.children().filter(roxmltree::Node::is_text).filter_map(|t| t.text()).collect();
    let text = text.trim();
    let children = fields(holder, depth, budget).into_iter().map(|(_, n)| n).collect();
    XmpNode { name, value: (!text.is_empty()).then(|| text.to_owned()), children }
}

/// `prefix:local` for a name in namespace `ns`, as the packet writes it at `node`.
fn qualified(node: roxmltree::Node, ns: &str, local: &str) -> String {
    match node.lookup_prefix(ns).filter(|p| !p.is_empty()) {
        Some(prefix) => format!("{prefix}:{local}"),
        None => local.to_owned(),
    }
}

/// Count one more node against the budget; false when it is spent.
fn take(budget: &mut usize) -> bool {
    match budget.checked_sub(1) {
        Some(left) => {
            *budget = left;
            true
        }
        None => false,
    }
}

/// Where nodes past the limits were left out.
fn more() -> XmpNode {
    XmpNode { name: "…".into(), value: None, children: Vec::new() }
}

/// Reject deep XML and start tags with too many attributes before roxmltree checks duplicates.
/// Each unquoted `=` in a valid start tag introduces an attribute, including namespace bindings.
/// Every `b[i]` and `hay[p..]` below is guarded by `i < b.len()` / `p < hay.len()`.
fn xml_limits_ok(text: &str) -> bool {
    fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        (from..hay.len()).find(|&p| hay[p..].starts_with(needle))
    }
    let b = text.as_bytes();
    let (mut i, mut depth, mut total_attributes) = (0usize, 0usize, 0usize);
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
        } else if b[i..].starts_with(b"<!--") {
            i = find(b, i + 4, b"-->").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<![CDATA[") {
            i = find(b, i + 9, b"]]>").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<?") {
            i = find(b, i + 2, b"?>").map_or(b.len(), |p| p + 2);
        } else if b[i..].starts_with(b"<!") {
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else if b[i..].starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else {
            depth += 1;
            if depth > 1024 {
                return false;
            }
            // Skip the rest of the start tag, over any quoted attribute value, and remember its last
            // significant byte: a tag that ends in `/` closes itself, so it is one level deep while it
            // is read rather than a level left open.
            i += 1;
            let mut quote = 0u8;
            let mut last = 0u8;
            let mut attributes = 0usize;
            while i < b.len() {
                let c = b[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    i += 1;
                    break;
                } else if c == b'=' {
                    attributes += 1;
                    total_attributes += 1;
                    if attributes > 128 || total_attributes > 4096 {
                        return false;
                    }
                    last = c;
                } else if !c.is_ascii_whitespace() {
                    last = c;
                }
                i += 1;
            }
            if last == b'/' {
                depth -= 1;
            }
        }
    }
    true
}

/// The value of a simple XMP property, written either as an attribute (`pdfaid:part="2"`) or
/// as an element (`<pdfaid:part>2</pdfaid:part>`, or inside an `rdf:Alt`/`rdf:Seq`).
pub fn value(xmp: &str, key: &str) -> Option<String> {
    let attr = format!("{key}=\"");
    if let Some(i) = xmp.find(&attr) {
        let rest = &xmp[i + attr.len()..];
        return rest.find('"').map(|e| unescape(&rest[..e]));
    }
    let attr1 = format!("{key}='");
    if let Some(i) = xmp.find(&attr1) {
        let rest = &xmp[i + attr1.len()..];
        return rest.find('\'').map(|e| unescape(&rest[..e]));
    }
    let open = format!("<{key}>");
    let close = format!("</{key}>");
    let start = xmp.find(&open)? + open.len();
    let end = start + xmp[start..].find(&close)?;
    let inner = &xmp[start..end];
    // rdf:Alt / rdf:Seq: the first rdf:li.
    if let Some(li) = inner.find("<rdf:li") {
        let s = li + inner[li..].find('>')? + 1;
        let e = s + inner[s..].find("</rdf:li>")?;
        return Some(unescape(inner[s..e].trim()));
    }
    Some(unescape(inner.trim()))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A PDF date (`D:YYYYMMDDHHmmSSOHH'mm'`) as an XMP (ISO 8601) date.
pub fn iso_date(pdf: &str) -> Option<String> {
    let d = pdf.trim().trim_start_matches("D:");
    let digits: String = d.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |a: usize, b: usize, def: &str| digits.get(a..b).unwrap_or(def).to_string();
    let (y, mo, da, h, mi, s) = (part(0, 4, "0000"), part(4, 6, "01"), part(6, 8, "01"), part(8, 10, "00"), part(10, 12, "00"), part(12, 14, "00"));
    let rest = &d[digits.len()..];
    let tz = match rest.chars().next() {
        Some('Z') | None => "Z".to_string(),
        Some(sign @ ('+' | '-')) => {
            let t: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            format!("{sign}{}:{}", t.get(0..2).unwrap_or("00"), t.get(2..4).unwrap_or("00"))
        }
        _ => "Z".to_string(),
    };
    Some(format!("{y}-{mo}-{da}T{h}:{mi}:{s}{tz}"))
}

fn info(doc: &Document, key: &str) -> Option<String> {
    let i = doc.trailer().get(b"Info").map(|i| doc.resolve(i))?;
    i.as_dict()?.get(key.as_bytes()).and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text())).filter(|s| !s.is_empty())
}

/// The XMP packet for `doc` declaring `level`, from its document information.
pub fn packet(doc: &Document, level: crate::Level) -> String {
    let mut props = String::new();
    if let Some(t) = info(doc, "Title") {
        props.push_str(&format!("   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n", esc(&t)));
    }
    if let Some(a) = info(doc, "Author") {
        props.push_str(&format!("   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n", esc(&a)));
    }
    if let Some(s) = info(doc, "Subject") {
        props.push_str(&format!("   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>\n", esc(&s)));
    }
    if let Some(k) = info(doc, "Keywords") {
        props.push_str(&format!("   <pdf:Keywords>{}</pdf:Keywords>\n", esc(&k)));
    }
    if let Some(p) = info(doc, "Producer") {
        props.push_str(&format!("   <pdf:Producer>{}</pdf:Producer>\n", esc(&p)));
    }
    if let Some(c) = info(doc, "Creator") {
        props.push_str(&format!("   <xmp:CreatorTool>{}</xmp:CreatorTool>\n", esc(&c)));
    }
    if let Some(d) = info(doc, "CreationDate").and_then(|d| iso_date(&d)) {
        props.push_str(&format!("   <xmp:CreateDate>{d}</xmp:CreateDate>\n"));
    }
    if let Some(d) = info(doc, "ModDate").and_then(|d| iso_date(&d)) {
        props.push_str(&format!("   <xmp:ModifyDate>{d}</xmp:ModifyDate>\n"));
    }
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n\
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
    xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">\n\
   <pdfaid:part>{}</pdfaid:part>\n\
   <pdfaid:conformance>B</pdfaid:conformance>\n\
{props}  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>",
        level.part()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_attribute_and_element_forms() {
        let a = "<rdf:Description pdfaid:part=\"2\" pdfaid:conformance='B'/>";
        assert_eq!(value(a, "pdfaid:part").as_deref(), Some("2"));
        assert_eq!(value(a, "pdfaid:conformance").as_deref(), Some("B"));
        let e = "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">A &amp; B</rdf:li></rdf:Alt></dc:title><pdfaid:part>3</pdfaid:part>";
        assert_eq!(value(e, "dc:title").as_deref(), Some("A & B"));
        assert_eq!(value(e, "pdfaid:part").as_deref(), Some("3"));
        assert_eq!(value(e, "pdfuaid:part"), None);
    }

    #[test]
    fn pdf_dates_become_iso_dates() {
        assert_eq!(iso_date("D:20240105093000+01'00'").as_deref(), Some("2024-01-05T09:30:00+01:00"));
        assert_eq!(iso_date("D:20240105").as_deref(), Some("2024-01-05T00:00:00Z"));
        assert_eq!(iso_date("D:20240105120000Z").as_deref(), Some("2024-01-05T12:00:00Z"));
        assert_eq!(iso_date("junk"), None);
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    /// The shape Acrobat's own packets have: one description per schema, element and attribute
    /// forms, a language alternative, an ordered array, a structure and a custom namespace.
    #[test]
    fn packets_are_listed_by_schema_as_acrobat_lists_them() {
        let xmp = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
            <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\
              <xmp:CreateDate>2026-02-13T15:35:27+07:00</xmp:CreateDate><xmp:CreatorTool> Writer &amp; Co </xmp:CreatorTool></rdf:Description>\
            <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
              <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Report</rdf:li><rdf:li xml:lang=\"th\">รายงาน</rdf:li></rdf:Alt></dc:title>\
              <dc:creator><rdf:Seq><rdf:li>Pat</rdf:li></rdf:Seq></dc:creator></rdf:Description>\
            <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:ModifyDate=\"2026-02-14\"/>\
            <rdf:Description rdf:about=\"\" xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\" xmlns:stRef=\"http://ns.adobe.com/xap/1.0/sType/ResourceRef#\">\
              <xmpMM:DerivedFrom rdf:parseType=\"Resource\"><stRef:documentID>uuid:1</stRef:documentID></xmpMM:DerivedFrom>\
              <xmpMM:History><rdf:Seq><rdf:li><rdf:Description stRef:instanceID=\"uuid:2\"/></rdf:li></rdf:Seq></xmpMM:History></rdf:Description>\
            <rdf:Description rdf:about=\"\" xmlns:pm=\"https://example.invalid/ns/stamp/1.0/\" pm:CopyType=\"CUSTODIAN\" pm:Origin=\"Thailand\"/>\
            </rdf:RDF></x:xmpmeta>\n<?xpacket end=\"w\"?>";
        let leaf = |name: &str, value: &str| XmpNode { name: name.into(), value: Some(value.into()), children: vec![] };
        let schemas = xmp_schemas(xmp).unwrap();
        let heads: Vec<_> = schemas.iter().map(|s| (s.prefix.as_str(), s.namespace.as_str(), s.properties.len())).collect();
        assert_eq!(
            heads,
            [
                ("xmp", "http://ns.adobe.com/xap/1.0/", 3),
                ("dc", "http://purl.org/dc/elements/1.1/", 2),
                ("xmpMM", "http://ns.adobe.com/xap/1.0/mm/", 2),
                ("pm", "https://example.invalid/ns/stamp/1.0/", 2)
            ]
        );
        // A schema written in two descriptions is one group, in the packet's order.
        assert_eq!(
            schemas[0].properties,
            [leaf("xmp:CreateDate", "2026-02-13T15:35:27+07:00"), leaf("xmp:CreatorTool", "Writer & Co"), leaf("xmp:ModifyDate", "2026-02-14")]
        );
        let title = &schemas[1].properties[0];
        assert_eq!((title.name.as_str(), &title.value), ("dc:title", &None));
        assert_eq!(title.children, [leaf("[1] x-default", "Report"), leaf("[2] th", "รายงาน")]);
        assert_eq!(schemas[1].properties[1].children, [leaf("[1]", "Pat")]);
        // Structures: parseType="Resource", and a description inside an item.
        assert_eq!(schemas[2].properties[0].children, [leaf("stRef:documentID", "uuid:1")]);
        assert_eq!(schemas[2].properties[1].children[0].children, [leaf("stRef:instanceID", "uuid:2")]);
        assert_eq!(schemas[3].properties, [leaf("pm:CopyType", "CUSTODIAN"), leaf("pm:Origin", "Thailand")]);
    }

    #[test]
    fn listing_is_bounded_and_refuses_what_it_cannot_parse() {
        let rdf = |body: &str| format!("<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:a=\"urn:a\">{body}</rdf:RDF>");
        assert_eq!(xmp_schemas(&rdf("")), Ok(vec![]));
        assert!(xmp_schemas("<x/>").is_err());
        assert!(xmp_schemas("not xml").is_err());
        // Wide: the budget ends the list with a marker.
        let many = rdf(&format!("<rdf:Description>{}</rdf:Description>", "<a:p>v</a:p>".repeat(MAX_NODES + 10)));
        let props = &xmp_schemas(&many).unwrap()[0].properties;
        assert_eq!(props.len(), MAX_NODES + 1);
        assert_eq!(props.last().unwrap().name, "…");
        // Deep: structures past the depth limit are cut.
        let deep = rdf(&format!(
            "<rdf:Description>{}v{}</rdf:Description>",
            "<a:s rdf:parseType=\"Resource\">".repeat(40).replace("Resource\">", "Resource\"><a:t>"),
            "</a:t></a:s>".repeat(40)
        ));
        let mut node = &xmp_schemas(&deep).unwrap()[0].properties[0];
        let mut levels = 0;
        while let Some(next) = node.children.first() {
            node = next;
            levels += 1;
        }
        assert!(levels <= 2 * MAX_DEPTH && node.name == "…", "{levels} {node:?}");
    }

    #[test]
    fn packets_are_checked_before_they_are_stored() {
        let ok = "\u{feff}<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
                  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"/></rdf:RDF></x:xmpmeta>\n<?xpacket end=\"w\"?>";
        assert_eq!(check_xmp(ok), Ok(()));
        // Another prefix for the same namespace is the same element.
        assert_eq!(check_xmp("<r:RDF xmlns:r=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/>"), Ok(()));
        for bad in [
            "",
            "plain text",
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>",
            "<rdf:RDF/>",
            "<rdf:RDF xmlns:rdf=\"urn:other\"/>",
            "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">",
            "<!DOCTYPE x [<!ENTITY e \"e\">]><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/>",
        ] {
            assert!(check_xmp(bad).is_err(), "{bad}");
        }
        let deep =
            format!("<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">{}{}</rdf:RDF>", "<a>".repeat(2000), "</a>".repeat(2000));
        assert!(check_xmp(&deep).unwrap_err().contains("deeply"));
        assert!(check_xmp(&"x".repeat(9 << 20)).unwrap_err().contains("MB"));
    }

    #[test]
    fn attribute_and_namespace_budgets_bound_parser_work() {
        let prefix = r#"<root xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="2" "#;
        let attrs = |count| (0..count).map(|i| format!("x{i}=\"v=1\" ")).collect::<String>();
        let at_limit = format!("{prefix}{}/>", attrs(126));
        assert_eq!(identifier(&at_limit, PDFA_ID, "part").as_deref(), Some("2"));
        let over_limit = format!("{prefix}{}/>", attrs(127));
        assert_eq!(identifier(&over_limit, PDFA_ID, "part"), None);
        let namespaces = (0..129).map(|i| format!("xmlns:n{i}=\"urn:{i}\" ")).collect::<String>();
        assert_eq!(identifier(&format!("<root {namespaces}/>"), PDFA_ID, "part"), None);
        let many_elements = format!("<root>{}</root>", r#"<n a="v"/>"#.repeat(4097));
        assert!(!xml_limits_ok(&many_elements));
        // Not well-formed (the prefix is never declared): read by the conventional prefix, as before.
        let undeclared = r#"<rdf:Description pdfaid:part="3" pdfaid:conformance="u"/>"#;
        assert_eq!(identifier(undeclared, PDFA_ID, "part").as_deref(), Some("3"));
        assert_eq!(identifier(undeclared, PDFA_ID, "conformance").as_deref(), Some("u"));
        let over_bytes = format!("<root>{}</root>", " ".repeat(8 * 1024 * 1024));
        assert_eq!(identifier(&over_bytes, PDFA_ID, "part"), None);
    }
}
