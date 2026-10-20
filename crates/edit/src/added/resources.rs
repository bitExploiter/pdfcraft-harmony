//! Reclaim an item's obsolete font mappings without invalidating shared page content.
use super::*;

const MAX_BYTES: usize = 16 << 20;
const MAX_STREAMS: usize = 256;

/// Borrow the page's content array so even deciding to skip a large page needs no copy.
pub(super) fn prune_page(doc: &Document, resources: &mut Dict, page: &pdfcraft_model::Page, old: ObjRef, replacement: Option<&[u8]>) {
    let Some(object) = page.dict.get(b"Contents") else { return };
    let resolved = doc.resolve(object);
    let streams = resolved.as_array().map(Vec::as_slice).unwrap_or_else(|| std::slice::from_ref(object));
    prune_with_annotations(doc, resources, streams, old, replacement, page.dict.get(b"Annots"));
}

/// Pruning is optional: unreadable, malformed or over-budget consumers retain their fonts.
fn prune_with_annotations(
    doc: &Document,
    resources: &mut Dict,
    streams: &[Object],
    old: ObjRef,
    replacement: Option<&[u8]>,
    annotations: Option<&Object>,
) {
    let Object::Stream(stream) = &*doc.get(old) else { return };
    let Ok(data) = stream.decoded_within(MAX_BYTES) else { return };
    let parsed = pdfcraft_content::parse(&data);
    if parsed.skipped != 0 {
        return;
    }
    let Some(mut fonts) = resources.get(b"Font").and_then(|f| doc.dict(f)) else { return };
    let unused: HashSet<Vec<u8>> = parsed
        .ops
        .iter()
        .filter(|op| op.is("Tf"))
        .filter_map(|op| op.name(0))
        .filter(|name| name.starts_with(b"PCAr") || name.starts_with(b"PCFull"))
        .filter(|name| !fonts.get(name).and_then(|f| doc.dict(f)).is_some_and(|f| f.get(b"PCRegisteredFont") == Some(&Object::Bool(true))))
        .map(<[u8]>::to_vec)
        .collect();
    if unused.is_empty() || streams.len() > MAX_STREAMS {
        return;
    }
    let mut walk = Usage {
        doc,
        fonts: &fonts,
        xobjects: resources.get(b"XObject").and_then(|r| doc.dict(r)),
        patterns: resources.get(b"Pattern").and_then(|r| doc.dict(r)),
        states: resources.get(b"ExtGState").and_then(|r| doc.dict(r)),
        unused,
        bytes: MAX_BYTES,
        streams: MAX_STREAMS,
        seen: HashSet::new(),
        seen_fonts: HashSet::new(),
        seen_states: HashSet::new(),
    };
    // /Contents is one token stream: a Tf operand can be in a different piece from its operator.
    let mut joined = Vec::new();
    for object in streams {
        let data = if object.as_ref() == Some(old) {
            let data = replacement.unwrap_or_default();
            if data.len() >= walk.bytes {
                return;
            }
            data.to_vec()
        } else {
            let Some(data) = walk.decode(object) else { return };
            data
        };
        let Some(remaining) = walk.bytes.checked_sub(data.len().saturating_add(1)) else { return };
        walk.bytes = remaining;
        joined.extend_from_slice(&data);
        joined.push(b' ');
    }
    if walk.content(&joined, 0).is_none() || walk.annotations(annotations).is_none() {
        return;
    }
    for name in walk.unused {
        fonts.remove(&name);
    }
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
}

struct Usage<'a> {
    doc: &'a Document,
    fonts: &'a Dict,
    xobjects: Option<Dict>,
    patterns: Option<Dict>,
    states: Option<Dict>,
    unused: HashSet<Vec<u8>>,
    bytes: usize,
    streams: usize,
    seen: HashSet<ObjRef>,
    seen_fonts: HashSet<Vec<u8>>,
    seen_states: HashSet<Vec<u8>>,
}

impl Usage<'_> {
    /// Legacy appearances can inherit the page's resources too (ISO 32000-2, 7.8.3):
    /// https://pdf-issues.pdfa.org/32000-2-2020/clause07.html
    fn annotations(&mut self, annotations: Option<&Object>) -> Option<()> {
        if self.unused.is_empty() {
            return Some(());
        }
        let Some(annotations) = annotations else { return Some(()) };
        let resolved = self.doc.resolve(annotations);
        let annotations = resolved.as_array()?;
        if annotations.len() > MAX_STREAMS {
            return None;
        }
        for annotation in annotations {
            let annotation = self.doc.dict(annotation)?;
            if let Some(appearance) = annotation.get(b"DA") {
                let source = self.doc.resolve(appearance);
                let data = &source.as_string()?.bytes;
                self.bytes = self.bytes.checked_sub(data.len())?;
                self.content(data, 0)?;
            }
            let Some(appearance) = annotation.get(b"AP") else { continue };
            let appearance = self.doc.dict(appearance)?;
            for kind in [b"N", b"R", b"D"] {
                let Some(object) = appearance.get(kind) else { continue };
                match self.doc.resolve(object).as_ref() {
                    Object::Stream(_) => self.inherited(object, 0)?,
                    Object::Dict(states) if states.len() <= MAX_STREAMS => {
                        for (_, stream) in states.iter() {
                            self.inherited(stream, 0)?;
                        }
                    }
                    _ => return None,
                }
            }
        }
        Some(())
    }

    fn type3(&mut self, object: &Object, depth: usize) -> Option<()> {
        let font = self.doc.dict(object)?;
        if font.name(b"Subtype") != Some(b"Type3") {
            return Some(());
        }
        if let Some(own) = font.get(b"Resources") {
            return self.doc.dict(own).map(|_| ());
        }
        if let Some(reference) = object.as_ref()
            && !self.seen.insert(reference)
        {
            return Some(());
        }
        let procs = font.get(b"CharProcs").and_then(|p| self.doc.dict(p))?;
        if procs.len() > MAX_STREAMS {
            return None;
        }
        for (_, proc) in procs.iter() {
            self.inherited(proc, depth)?;
        }
        Some(())
    }

    fn decode(&mut self, object: &Object) -> Option<Vec<u8>> {
        self.streams = self.streams.checked_sub(1)?;
        let Object::Stream(stream) = &*self.doc.resolve(object) else { return None };
        stream.decoded_within(self.bytes).ok()
    }

    /// A nested stream with its own resources owns its font references independently. Legacy
    /// forms, patterns and Type 3 charprocs without resources can still use the page's names.
    fn inherited(&mut self, object: &Object, depth: usize) -> Option<()> {
        if let Some(reference) = object.as_ref()
            && !self.seen.insert(reference)
        {
            return Some(());
        }
        let Object::Stream(stream) = &*self.doc.resolve(object) else { return None };
        if let Some(own) = stream.dict.get(b"Resources") {
            return self.doc.dict(own).map(|_| ());
        }
        if depth >= 16 {
            return None;
        }
        let data = self.decode(object)?;
        self.bytes = self.bytes.checked_sub(data.len())?;
        self.content(&data, depth + 1)
    }

    fn content(&mut self, data: &[u8], depth: usize) -> Option<()> {
        let parsed = pdfcraft_content::parse(data);
        if parsed.skipped != 0 {
            return None;
        }
        for op in parsed.ops {
            if self.unused.is_empty() {
                return Some(());
            }
            if op.is("Tf") {
                if op.operands.len() != 2 || op.num(1).is_none_or(|size| !size.is_finite()) {
                    return None;
                }
                let name = op.name(0)?;
                self.unused.remove(name);
                if !self.seen_fonts.insert(name.to_vec()) {
                    continue;
                }
                self.type3(self.fonts.get(name)?, depth)?;
            } else if op.is("gs") {
                if op.operands.len() != 1 {
                    return None;
                }
                let name = op.name(0)?;
                if !self.seen_states.insert(name.to_vec()) {
                    continue;
                }
                let state = self.doc.dict(self.states.as_ref()?.get(name)?)?;
                if let Some(font) = state.get(b"Font") {
                    let source = self.doc.resolve(font);
                    let font = source.as_array()?.first()?;
                    self.type3(font, depth)?;
                }
                if let Some(mask) = state.get(b"SMask") {
                    match self.doc.resolve(mask).as_ref() {
                        Object::Dict(mask) => self.inherited(mask.get(b"G")?, depth)?,
                        Object::Name(name) if name == b"None" => {}
                        _ => return None,
                    }
                }
            } else if op.is("Do") {
                if op.operands.len() != 1 {
                    return None;
                }
                let name = op.name(0)?;
                let object = self.xobjects.as_ref()?.get(name)?.clone();
                let Object::Stream(stream) = &*self.doc.resolve(&object) else { return None };
                if stream.dict.name(b"Subtype") == Some(b"Form") {
                    self.inherited(&object, depth)?;
                }
            } else if (op.is("scn") || op.is("SCN"))
                && let Some(name) = op.operands.last().and_then(Object::as_name)
            {
                let object = self.patterns.as_ref()?.get(name)?.clone();
                if matches!(self.doc.resolve(&object).as_ref(), Object::Stream(_)) {
                    self.inherited(&object, depth)?;
                }
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prune(doc: &Document, resources: &mut Dict, streams: &[Object], old: ObjRef, replacement: Option<&[u8]>) {
        prune_with_annotations(doc, resources, streams, old, replacement, None);
    }

    #[test]
    fn pruning_preserves_annotation_and_graphics_state_consumers() {
        let mut missed = Vec::new();
        for case in 0..4 {
            let (mut doc, mut resources, old) = setup();
            let proc = stream(&mut doc, b"BT /PCFull0 12 Tf <0001> Tj ET");
            let mut annotations = None;
            let mut contents = vec![Object::Ref(old)];
            if case < 2 {
                let mut annot = Dict::new();
                if case == 0 {
                    let mut states = Dict::new();
                    states.set(b"Hover".to_vec(), Object::Ref(proc));
                    let mut ap = Dict::new();
                    ap.set(b"R".to_vec(), Object::Dict(states));
                    annot.set(b"AP".to_vec(), Object::Dict(ap));
                } else {
                    annot.set(b"DA".to_vec(), Object::String(PdfString::literal(b"/PCFull0 12 Tf 0 g".to_vec())));
                }
                annotations = Some(Object::Array(vec![Object::Dict(annot)]));
            } else {
                let mut state = Dict::new();
                if case == 2 {
                    let mut mask = Dict::new();
                    mask.set(b"G".to_vec(), Object::Ref(proc));
                    state.set(b"SMask".to_vec(), Object::Dict(mask));
                } else {
                    let mut procs = Dict::new();
                    procs.set(b"a".to_vec(), Object::Ref(proc));
                    let mut face = Dict::new();
                    face.set(b"Subtype".to_vec(), Object::name("Type3"));
                    face.set(b"CharProcs".to_vec(), Object::Dict(procs));
                    state.set(b"Font".to_vec(), Object::Array(vec![Object::Dict(face), Object::Int(12)]));
                }
                let mut states = Dict::new();
                states.set(b"State".to_vec(), Object::Dict(state));
                resources.set(b"ExtGState".to_vec(), Object::Dict(states));
                contents.push(Object::Ref(stream(&mut doc, b"/State gs")));
            }
            prune_with_annotations(&doc, &mut resources, &contents, old, None, annotations.as_ref());
            if !has_font(&doc, &resources) {
                missed.push(case);
            }
        }
        assert!(missed.is_empty(), "resources still used by AP/DA/soft masks/Type3 graphics states: {missed:?}");
    }

    fn setup() -> (Document, Dict, ObjRef) {
        let mut doc = Document::new_empty();
        let old = stream(&mut doc, b"BT /PCFull0 12 Tf <0001> Tj ET");
        let mut fonts = Dict::new();
        fonts.set(b"PCFull0".to_vec(), Object::Dict(Dict::new()));
        let mut resources = Dict::new();
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        (doc, resources, old)
    }

    fn stream(doc: &mut Document, data: &[u8]) -> ObjRef {
        doc.add(Object::Stream(Stream::flate(Dict::new(), data)))
    }

    fn has_font(doc: &Document, resources: &Dict) -> bool {
        doc.dict(resources.get(b"Font").unwrap()).unwrap().contains(b"PCFull0")
    }

    #[test]
    fn pruning_respects_split_operators_and_escaped_resource_names() {
        let (mut doc, mut resources, old) = setup();
        let prefix = stream(&mut doc, b"BT /PCFull#30");
        let suffix = stream(&mut doc, b"12 Tf <0001> Tj ET");
        prune(&doc, &mut resources, &[Object::Ref(old), Object::Ref(prefix), Object::Ref(suffix)], old, None);
        assert!(has_font(&doc, &resources));
        let literal = stream(&mut doc, b"BT (/PCFull0 12 Tf) Tj ET");
        prune(&doc, &mut resources, &[Object::Ref(old), Object::Ref(literal)], old, None);
        assert!(!has_font(&doc, &resources), "a font-like string is not a resource reference");
    }

    #[test]
    fn pruning_preserves_inherited_form_fonts_and_handles_cycles() {
        let (mut doc, mut resources, old) = setup();
        let mut dict = Dict::new();
        dict.set(b"Subtype".to_vec(), Object::name("Form"));
        let form = doc.add(Object::Stream(Stream::flate(dict.clone(), b"BT /PCFull0 12 Tf <0001> Tj ET /Loop Do")));
        let mut xobjects = Dict::new();
        xobjects.set(b"Loop".to_vec(), Object::Ref(form));
        resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
        let caller = stream(&mut doc, b"/Loop Do");
        let contents = [Object::Ref(old), Object::Ref(caller)];
        prune(&doc, &mut resources, &contents, old, None);
        assert!(has_font(&doc, &resources));
        doc.set(form, Object::Stream(Stream::flate(dict, b"/Loop Do")));
        prune(&doc, &mut resources, &contents, old, None);
        assert!(!has_font(&doc, &resources));
    }

    #[test]
    fn pruning_respects_inherited_pattern_and_type3_consumers() {
        for pattern in [false, true] {
            let (mut doc, mut resources, old) = setup();
            let proc = stream(&mut doc, b"BT /PCFull0 12 Tf <0001> Tj ET");
            let caller = if pattern {
                let mut patterns = Dict::new();
                patterns.set(b"Paint".to_vec(), Object::Ref(proc));
                resources.set(b"Pattern".to_vec(), Object::Dict(patterns));
                stream(&mut doc, b"/Pattern cs /Paint scn 0 0 100 100 re f")
            } else {
                let mut procs = Dict::new();
                procs.set(b"a".to_vec(), Object::Ref(proc));
                let mut face = Dict::new();
                face.set(b"Subtype".to_vec(), Object::name("Type3"));
                face.set(b"CharProcs".to_vec(), Object::Dict(procs));
                let mut fonts = doc.dict(resources.get(b"Font").unwrap()).unwrap();
                fonts.set(b"Other".to_vec(), Object::Dict(face));
                resources.set(b"Font".to_vec(), Object::Dict(fonts));
                stream(&mut doc, b"BT /Other 12 Tf (a) Tj ET")
            };
            prune(&doc, &mut resources, &[Object::Ref(old), Object::Ref(caller)], old, None);
            assert!(has_font(&doc, &resources));
        }
    }

    #[test]
    fn pruning_stops_at_the_inherited_resource_depth_limit() {
        let (mut doc, mut resources, old) = setup();
        let mut xobjects = Dict::new();
        for index in 0..18 {
            let mut dict = Dict::new();
            dict.set(b"Subtype".to_vec(), Object::name("Form"));
            let data = if index == 17 { "BT /PCFull0 12 Tf <0001> Tj ET".into() } else { format!("/Form{} Do", index + 1) };
            let form = doc.add(Object::Stream(Stream::flate(dict, data.as_bytes())));
            xobjects.set(format!("Form{index}").into_bytes(), Object::Ref(form));
        }
        resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
        let caller = stream(&mut doc, b"/Form0 Do");
        prune(&doc, &mut resources, &[Object::Ref(old), Object::Ref(caller)], old, None);
        assert!(has_font(&doc, &resources), "an unvisited consumer may still need the font");
    }

    #[test]
    fn pruning_retains_fonts_when_consumers_cannot_be_proven_unused() {
        for data in [b"(".as_slice(), b"BT /PCFull0 Tf ET", b"BT /Unused /PCFull0 12 Tf <0001> Tj ET"] {
            let (mut doc, mut resources, old) = setup();
            let other = stream(&mut doc, data);
            prune(&doc, &mut resources, &[Object::Ref(old), Object::Ref(other)], old, None);
            assert!(has_font(&doc, &resources));
        }
        let (mut doc, mut resources, old) = setup();
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::name("UnknownFilter"));
        let unreadable = doc.add(Object::Stream(Stream::from_raw(dict, b"?".to_vec())));
        prune(&doc, &mut resources, &[Object::Ref(old), Object::Ref(unreadable)], old, None);
        assert!(has_font(&doc, &resources));
        let too_many = vec![Object::Ref(old); MAX_STREAMS + 1];
        prune(&doc, &mut resources, &too_many, old, None);
        assert!(has_font(&doc, &resources));
    }

    #[test]
    fn pruning_keeps_deliberately_registered_faces() {
        let (doc, mut resources, old) = setup();
        let mut face = Dict::new();
        face.set(b"PCRegisteredFont".to_vec(), Object::Bool(true));
        let mut fonts = Dict::new();
        fonts.set(b"PCFull0".to_vec(), Object::Dict(face));
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        prune(&doc, &mut resources, &[Object::Ref(old)], old, None);
        assert!(has_font(&doc, &resources));
    }
}
