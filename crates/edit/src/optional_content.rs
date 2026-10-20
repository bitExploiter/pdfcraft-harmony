//! Default optional-content visibility for text reading (PDF 1.7 §4.10).
//! Rendering has its own interpreter; this keeps the COS-based Office/edit reader independent.
use pdfcraft_content::Op;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

const MAX_GROUPS: usize = 65_536;
// Only paragraph/ActualText metadata is capped; visibility uses constant space at any depth.
const MAX_MARKED_METADATA_DEPTH: usize = 256;
const MAX_EXPRESSION_DEPTH: usize = 32;
const MAX_EXPRESSION_WORK: usize = 4096;

/// Shared default group configuration and cache, with constant-space scope visibility.
#[derive(Clone)]
pub(crate) struct Visibility {
    // None means the configuration exceeded the budget: omit optional content rather than
    // publish content whose visibility could not be determined. Ordinary content is unaffected.
    inactive: Rc<Option<HashSet<ObjRef>>>,
    memo: Rc<RefCell<HashMap<ObjRef, bool>>>,
    depth: usize,
    first_hidden: Option<usize>,
    include_hidden: bool,
}

impl Visibility {
    /// Read the default configuration once; form scopes share its immutable group state.
    pub(crate) fn new(doc: &Document) -> Self {
        let inactive = Self::inactive(doc);
        Self { inactive: Rc::new(inactive), memo: Rc::new(RefCell::new(HashMap::new())), depth: 0, first_hidden: None, include_hidden: false }
    }

    /// Audit retained glyphs regardless of the document's default layer configuration.
    pub(crate) fn including_hidden() -> Self {
        Self {
            inactive: Rc::new(Some(HashSet::new())),
            memo: Rc::new(RefCell::new(HashMap::new())),
            depth: 0,
            first_hidden: None,
            include_hidden: true,
        }
    }

    /// Apply BaseState, ON and OFF in order; None marks configuration-budget exhaustion.
    fn inactive(doc: &Document) -> Option<HashSet<ObjRef>> {
        let mut off = HashSet::new();
        let Some(root) = doc.trailer().get(b"Root").map(|o| doc.resolve(o)) else { return Some(off) };
        let Some(oc) = root.as_dict().and_then(|d| d.get(b"OCProperties")).map(|o| doc.resolve(o)) else { return Some(off) };
        let Some(config) = oc.as_dict().and_then(|d| d.get(b"D")).map(|o| doc.resolve(o)) else { return Some(off) };
        let Some(config) = config.as_dict() else { return Some(off) };
        let mut work = 0usize;
        for (value, disabled) in [
            (oc.as_dict().and_then(|d| d.get(b"OCGs")).filter(|_| config.name(b"BaseState") == Some(b"OFF")), true),
            (config.get(b"ON"), false),
            (config.get(b"OFF"), true),
        ] {
            let Some(array) = value.map(|o| doc.resolve(o)) else { continue };
            let Some(array) = array.as_array() else { continue };
            for value in array {
                work = work.saturating_add(1);
                if work > MAX_GROUPS {
                    return None;
                }
                if let Some(r) = value.as_ref() {
                    if disabled {
                        off.insert(r);
                    } else {
                        off.remove(&r);
                    }
                }
            }
        }
        Some(off)
    }

    /// Forms inherit the caller's visibility by being entered only at visible Do operators.
    /// Their own marked-content stack cannot pop the caller's scopes.
    pub(crate) fn fresh(&self) -> Self {
        Self { inactive: self.inactive.clone(), memo: self.memo.clone(), depth: 0, first_hidden: None, include_hidden: self.include_hidden }
    }

    /// Report cumulative visibility without suppressing deeply nested ordinary content.
    pub(crate) fn visible(&self) -> bool {
        self.first_hidden.is_none()
    }

    /// Tell the text reader whether this close belongs to an untracked excess scope.
    pub(crate) fn overflowing(&self) -> bool {
        self.depth > MAX_MARKED_METADATA_DEPTH
    }

    /// Update BMC/BDC/EMC scopes, resolving named OC properties in the current resources.
    pub(crate) fn apply(&mut self, doc: &Document, resources: &Dict, op: &Op) {
        match op.op.as_slice() {
            b"BMC" | b"BDC" => {
                self.depth = self.depth.saturating_add(1);
                // A hidden parent determines all descendants; no nested resource lookup is needed.
                if self.first_hidden.is_some() {
                    return;
                }
                let own = if op.is("BDC") && op.name(0) == Some(b"OC") {
                    op.operands.get(1).is_none_or(|o| {
                        if let Object::Name(name) = o {
                            resources
                                .get(b"Properties")
                                .map(|p| doc.resolve(p))
                                .and_then(|p| p.as_dict().and_then(|d| d.get(name)).map(|o| self.object_visible(doc, o)))
                                .unwrap_or(true)
                        } else {
                            self.object_visible(doc, o)
                        }
                    })
                } else {
                    true
                };
                if !own {
                    self.first_hidden = Some(self.depth);
                }
            }
            b"EMC" => {
                if self.first_hidden == Some(self.depth) {
                    self.first_hidden = None;
                }
                self.depth = self.depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    /// Evaluate an XObject's own OC membership without entering or decoding its stream.
    pub(crate) fn xobject_visible(&self, doc: &Document, xobjects: Option<&Dict>, name: &[u8]) -> bool {
        xobjects.and_then(|d| d.get(name)).is_none_or(|o| {
            let object = doc.resolve(o);
            object.as_dict().and_then(|d| d.get(b"OC")).is_none_or(|oc| self.object_visible(doc, oc))
        })
    }

    /// Evaluate an OCG or OCMD; a valid VE overrides membership policy.
    pub(crate) fn object_visible(&self, doc: &Document, value: &Object) -> bool {
        if self.include_hidden {
            return true;
        }
        let reference = value.as_ref();
        if let Some(reference) = reference
            && let Ok(memo) = self.memo.try_borrow()
            && let Some(visible) = memo.get(&reference)
        {
            return *visible;
        }
        let visible = self.evaluate(doc, value);
        if let Some(reference) = reference
            && let Ok(mut memo) = self.memo.try_borrow_mut()
            && memo.len() < MAX_GROUPS
        {
            memo.insert(reference, visible);
        }
        visible
    }

    /// Evaluate uncached membership with per-expression and per-group work budgets.
    fn evaluate(&self, doc: &Document, value: &Object) -> bool {
        let Some(inactive) = self.inactive.as_ref() else { return false };
        let resolved = doc.resolve(value);
        let Some(dict) = resolved.as_dict() else { return true };
        if dict.name(b"Type") != Some(b"OCMD") {
            return value.as_ref().is_none_or(|r| !inactive.contains(&r));
        }
        if let Some(expression) = dict.get(b"VE") {
            let mut work = MAX_EXPRESSION_WORK;
            if let Some(visible) = self.expression(doc, expression, 0, &mut work) {
                return visible;
            }
        }
        let Some(groups) = dict.get(b"OCGs") else { return true };
        let resolved = doc.resolve(groups);
        let groups = resolved.as_array().map(Vec::as_slice).unwrap_or_else(|| std::slice::from_ref(groups));
        if groups.len() > MAX_GROUPS {
            return false;
        }
        let mut states = groups.iter().filter_map(Object::as_ref).map(|r| !inactive.contains(&r)).peekable();
        if states.peek().is_none() {
            return true;
        }
        match dict.name(b"P") {
            Some(b"AllOn") => states.all(|v| v),
            Some(b"AnyOff") => states.any(|v| !v),
            Some(b"AllOff") => states.all(|v| !v),
            _ => states.any(|v| v),
        }
    }

    /// Evaluate boolean VE nodes within depth/work limits; invalid input requests policy fallback.
    fn expression(&self, doc: &Document, value: &Object, depth: usize, work: &mut usize) -> Option<bool> {
        if depth > MAX_EXPRESSION_DEPTH || *work == 0 {
            return None;
        }
        *work -= 1;
        let inactive = self.inactive.as_ref().as_ref()?;
        let object = doc.resolve(value);
        if object.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"OCG")) {
            return Some(!inactive.contains(&value.as_ref()?));
        }
        let array = object.as_array()?;
        let (operator, operands) = array.split_first()?;
        let Object::Name(operator) = operator else { return None };
        match operator.as_slice() {
            b"Not" if operands.len() == 1 => Some(!self.expression(doc, operands.first()?, depth + 1, work)?),
            b"And" | b"Or" if !operands.is_empty() => {
                let and = operator == b"And";
                let mut result = and;
                // Validate every operand, even after the result is known, within one work budget.
                for operand in operands {
                    let v = self.expression(doc, operand, depth + 1, work)?;
                    result = if and { result && v } else { result || v };
                }
                Some(result)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_cache_reuses_results_across_marked_scopes_and_form_visits() {
        let mut doc = Document::new_empty();
        let group = doc.add(Object::Dict({
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("OCG"));
            d
        }));
        let mut membership = Dict::new();
        membership.set(b"Type".to_vec(), Object::name("OCMD"));
        membership.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(group); MAX_GROUPS]));
        membership.set(b"P".to_vec(), Object::name("AnyOff"));
        let member = doc.add(Object::Dict(membership));
        let mut properties = Dict::new();
        properties.set(b"M".to_vec(), Object::Ref(member));
        let mut resources = Dict::new();
        resources.set(b"Properties".to_vec(), Object::Dict(properties));
        let mut visibility = Visibility::new(&doc);
        let begin = Op::new("BDC", vec![Object::name("OC"), Object::name("M")]);
        visibility.apply(&doc, &resources, &begin);
        assert!(!visibility.visible());
        visibility.apply(&doc, &resources, &Op::new("EMC", vec![]));
        // Test-only mutation probes whether evaluation is repeated. Production constructs
        // a new Visibility for each read of the immutable document, never across edits.
        doc.update_dict(member, |d| d.set(b"P".to_vec(), Object::name("AllOn"))).unwrap();
        for _ in 0..1000 {
            visibility.apply(&doc, &resources, &begin);
            assert!(!visibility.visible(), "the page must reuse the cached result");
            visibility.apply(&doc, &resources, &Op::new("EMC", vec![]));
            assert!(!visibility.fresh().object_visible(&doc, &Object::Ref(member)), "forms share the page's cached result");
        }
        assert!(Visibility::new(&doc).object_visible(&doc, &Object::Ref(member)), "a new read must not retain a stale cache");
    }
}
