//! Synthetic regression fixtures for #3211; no external PDF or font assets.
use super::*;

/// Build one original synthetic page with hidden/shown groups, an OCMD and a form.
fn fixture(content: &str, form_oc: &str, config: &str) -> Document {
    let page = stream("", content);
    let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 600 800] {form_oc}"), "BT /F1 12 Tf 40 500 Td (FORM_SECRET) Tj ET");
    build(&[
        &format!("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R 7 0 R] /D << {config} >> >> >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm 8 0 R >> /Properties << /Hidden 6 0 R /Shown 7 0 R /Member 9 0 R >> >> /Contents 4 0 R >>",
        &page,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /OCG /Name (Hidden) >>",
        "<< /Type /OCG /Name (Shown) >>",
        &form,
        "<< /Type /OCMD /OCGs [6 0 R 7 0 R] /P /AllOn >>",
    ])
}

/// Read paragraph text through the same reader Office export uses.
fn texts(doc: &Document) -> Vec<String> {
    text::reading_blocks(doc, 0).unwrap().into_iter().map(|b| b.text).collect()
}

const OFF: &str = "/OFF [6 0 R] /ON [7 0 R]";

#[test]
fn hidden_optional_content_is_absent_from_export_and_edit_text() {
    let doc = fixture(
        "BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET /OC /Hidden BDC BT /F1 12 Tf 40 680 Td (SECRET) Tj ET EMC /OC /Shown BDC BT /F1 12 Tf 40 600 Td (SHOWN) Tj ET EMC",
        "",
        OFF,
    );
    assert_eq!(texts(&doc), ["PUBLIC", "SHOWN"]);
    assert_eq!(text::text_lines(&doc, 0).unwrap().iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["PUBLIC", "SHOWN"]);
}

#[test]
fn hidden_forms_and_forms_called_inside_hidden_spans_are_not_exported() {
    for (content, form_oc, expected) in [
        ("/Fm Do", "/OC 6 0 R", Vec::<&str>::new()),
        ("/OC /Hidden BDC /Fm Do EMC", "", vec![]),
        ("/OC /Shown BDC /Fm Do EMC", "", vec!["FORM_SECRET"]),
        ("/Fm Do", "/OC 7 0 R", vec!["FORM_SECRET"]),
    ] {
        assert_eq!(texts(&fixture(content, form_oc, OFF)), expected);
    }
}

#[test]
fn hidden_show_operators_still_advance_the_visible_text() {
    let hidden = fixture("BT /F1 12 Tf 40 700 Td (BEFORE) Tj /OC /Hidden BDC (SECRET) Tj EMC (AFTER) Tj ET", "", OFF);
    let shown = fixture("BT /F1 12 Tf 40 700 Td (BEFORE) Tj (SECRET) Tj (AFTER) Tj ET", "", OFF);
    let before = text::text_lines(&shown, 0).unwrap();
    let after = text::text_lines(&hidden, 0).unwrap();
    assert!(!after.iter().any(|l| l.text.contains("SECRET")));
    // The final visible run must keep its original PDF position, not close up the hidden gap.
    assert_eq!(after.last().unwrap().rect[2], before.last().unwrap().rect[2]);
}

#[test]
fn nested_hidden_spans_do_not_reveal_text_when_inner_scopes_end() {
    let doc = fixture(
        "/OC /Hidden BDC /OC /Shown BDC BT /F1 12 Tf 40 700 Td (SECRET) Tj ET EMC /Span BMC BT /F1 12 Tf 40 600 Td (STILL_SECRET) Tj ET EMC EMC BT /F1 12 Tf 40 500 Td (PUBLIC) Tj ET",
        "",
        OFF,
    );
    assert_eq!(texts(&doc), ["PUBLIC"]);
    let open = fixture("BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET /OC /Hidden BDC BT /F1 12 Tf 40 600 Td (SECRET) Tj ET", "", OFF);
    assert_eq!(texts(&open), ["PUBLIC"]);
}

#[test]
fn default_off_and_on_off_overrides_match_rendering() {
    let content = "/OC /Hidden BDC BT /F1 12 Tf 40 700 Td (HIDDEN_GROUP) Tj ET EMC /OC /Shown BDC BT /F1 12 Tf 40 500 Td (SHOWN_GROUP) Tj ET EMC";
    assert_eq!(texts(&fixture(content, "", "/BaseState /OFF /ON [7 0 R]")), ["SHOWN_GROUP"]);
    assert!(texts(&fixture(content, "", "/BaseState /OFF /ON [7 0 R] /OFF [7 0 R]")).is_empty());
    assert_eq!(texts(&fixture(content, "", "")), ["HIDDEN_GROUP", "SHOWN_GROUP"]);
}

#[test]
fn membership_policies_and_single_group_membership_are_respected() {
    use pdfcraft_cos::{ObjRef, Object};
    for (groups, policy, visible) in [
        (vec![6, 7], "AnyOn", true),
        (vec![6, 7], "AllOn", false),
        (vec![6, 7], "AnyOff", true),
        (vec![6, 7], "AllOff", false),
        (vec![6], "AnyOn", false),
        (vec![6], "AnyOff", true),
    ] {
        let mut doc = fixture("/OC /Member BDC BT /F1 12 Tf 40 700 Td (MEMBER) Tj ET EMC", "", OFF);
        doc.update_dict(ObjRef::new(9, 0), |d| {
            d.set(b"P".to_vec(), Object::Name(policy.as_bytes().to_vec()));
            let refs: Vec<_> = groups.iter().map(|n| Object::Ref(ObjRef::new(*n, 0))).collect();
            d.set(b"OCGs".to_vec(), if refs.len() == 1 { refs[0].clone() } else { Object::Array(refs) });
        })
        .unwrap();
        assert_eq!(!texts(&doc).is_empty(), visible, "{groups:?} {policy}");
    }
}

#[test]
fn visibility_expressions_take_precedence_and_malformed_ones_fall_back() {
    use pdfcraft_cos::{ObjRef, Object};
    for (expression, visible) in [
        ("[/Not 6 0 R]", true),
        ("[/And 6 0 R 7 0 R]", false),
        ("[/Or 6 0 R 7 0 R]", true),
        ("[/And [/Not 6 0 R] 7 0 R]", true),
        ("[/Unknown 7 0 R]", false),
        ("[/Not 6 0 R 7 0 R]", false),
        ("[/And]", false),
    ] {
        let mut doc = fixture("/OC /Member BDC BT /F1 12 Tf 40 700 Td (MEMBER) Tj ET EMC", "", OFF);
        let ops = pdfcraft_content::parse(format!("{expression} custom").as_bytes()).ops;
        let value = ops[0].operands[0].clone();
        doc.update_dict(ObjRef::new(9, 0), |d| {
            d.set(b"VE".to_vec(), value);
        })
        .unwrap();
        assert_eq!(!texts(&doc).is_empty(), visible, "{expression}");
        // The same expression also applies to a form's /OC.
        doc.update_dict(ObjRef::new(8, 0), |d| {
            d.set(b"OC".to_vec(), Object::Ref(ObjRef::new(9, 0)));
        })
        .unwrap();
        doc.set(ObjRef::new(4, 0), Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), b"/Fm Do".to_vec())));
        assert_eq!(!texts(&doc).is_empty(), visible, "form {expression}");
    }
}

#[test]
fn actual_text_cannot_reintroduce_hidden_glyphs() {
    let doc = fixture("/Span <</ActualText (PUBLIC SECRET)>> BDC BT /F1 12 Tf 40 700 Td (PUBLIC) Tj /OC /Hidden BDC (SECRET) Tj EMC ET EMC", "", OFF);
    assert_eq!(texts(&doc), ["PUBLIC"]);
    let all_hidden = fixture("/OC /Hidden BDC /Span <</ActualText (LEAK)>> BDC BT /F1 12 Tf 40 700 Td (SECRET) Tj ET EMC EMC", "", OFF);
    assert!(texts(&all_hidden).is_empty());
    let hidden_form = fixture("/Span <</ActualText (PUBLIC FORM_SECRET)>> BDC BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET /Fm Do EMC", "/OC 6 0 R", OFF);
    assert_eq!(texts(&hidden_form), ["PUBLIC"]);
}

#[test]
fn marked_visibility_survives_split_streams() {
    use pdfcraft_cos::{ObjRef, Object, Stream};
    let mut doc = fixture("/OC /Hidden BDC BT /F1 12 Tf 40 700 Td (SECRET) Tj ET EMC BT /F1 12 Tf 40 500 Td (PUBLIC) Tj ET", "", OFF);
    let data = page_content_bytes(&doc, 0);
    let split = data.windows(3).position(|s| s == b"/OC").unwrap() + 3;
    let a = doc.add(Object::Stream(Stream::from_raw(Dict::new(), data[..split].to_vec())));
    let b = doc.add(Object::Stream(Stream::from_raw(Dict::new(), data[split..].to_vec())));
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(a), Object::Ref(b)]));
    })
    .unwrap();
    assert_eq!(texts(&doc), ["PUBLIC"]);
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "PUBLIC");
}

#[test]
fn excessive_marked_depth_cannot_pop_a_hidden_outer_scope() {
    let content = format!(
        "/OC /Hidden BDC {} BT /F1 12 Tf 40 700 Td (SECRET) Tj ET {} EMC BT /F1 12 Tf 40 500 Td (PUBLIC) Tj ET",
        "/Span BMC ".repeat(300),
        "EMC ".repeat(300)
    );
    assert_eq!(texts(&fixture(&content, "", OFF)), ["PUBLIC"]);
}

#[test]
fn form_property_names_resolve_in_the_forms_own_resources() {
    use pdfcraft_cos::ObjRef;
    let mut doc = fixture("/Fm Do", "", OFF);
    let own = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /Properties << /Hidden 7 0 R >> >>",
        "/OC /Hidden BDC BT /F1 12 Tf 40 500 Td (FORM_PUBLIC) Tj ET EMC",
    );
    let parsed = build(&["<< /Type /Catalog >>", &own]);
    doc.set(ObjRef::new(8, 0), (*parsed.get(ObjRef::new(2, 0))).clone());
    assert_eq!(texts(&doc), ["FORM_PUBLIC"]);
}

#[test]
fn excessive_and_cyclic_visibility_expressions_have_bounded_fallback() {
    use pdfcraft_cos::{ObjRef, Object};
    for depth in [40, 80] {
        let mut doc = fixture("/OC /Member BDC BT /F1 12 Tf 40 700 Td (SECRET) Tj ET EMC", "", OFF);
        let mut expression = Object::Ref(ObjRef::new(7, 0));
        for _ in 0..depth {
            expression = Object::Array(vec![Object::Name(b"Not".to_vec()), expression]);
        }
        doc.update_dict(ObjRef::new(9, 0), |d| {
            d.set(b"VE".to_vec(), expression);
        })
        .unwrap();
        assert!(texts(&doc).is_empty());
    }
    let mut doc = fixture("/OC /Member BDC BT /F1 12 Tf 40 700 Td (SECRET) Tj ET EMC", "", OFF);
    let cycle = doc.add(Object::Null);
    doc.set(cycle, Object::Array(vec![Object::Name(b"Not".to_vec()), Object::Ref(cycle)]));
    doc.update_dict(ObjRef::new(9, 0), |d| {
        d.set(b"VE".to_vec(), Object::Ref(cycle));
    })
    .unwrap();
    assert!(texts(&doc).is_empty());
    let wide =
        Object::Array(std::iter::once(Object::Name(b"Or".to_vec())).chain(std::iter::repeat_n(Object::Ref(ObjRef::new(7, 0)), 5000)).collect());
    doc.update_dict(ObjRef::new(9, 0), |d| {
        d.set(b"VE".to_vec(), wide);
    })
    .unwrap();
    assert!(texts(&doc).is_empty());
}

#[test]
fn editing_visible_text_preserves_hidden_operators_and_document_bytes_on_read() {
    let mut doc = fixture("BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET /OC /Hidden BDC BT /F1 12 Tf 40 686 Td (SECRET) Tj ET EMC", "", OFF);
    let original = write_full(&doc, &SaveOptions::default()).unwrap();
    assert_eq!(text::text_blocks(&doc, 0).unwrap().iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["PUBLIC"]);
    assert_eq!(texts(&doc), ["PUBLIC"]);
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), original);
    text::replace_block(&mut doc, 0, 0, "UPDATED").unwrap();
    let reopened = reopen(&doc);
    assert_eq!(texts(&reopened), ["UPDATED"]);
    assert!(
        String::from_utf8_lossy(&page_content_bytes(&reopened, 0)).contains("SECRET"),
        "hidden text stays in its original optional-content scope"
    );
}

#[test]
fn configuration_work_budget_omits_optional_text_but_keeps_ordinary_text() {
    use pdfcraft_cos::{ObjRef, Object};
    let mut doc = fixture("BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET /OC /Hidden BDC BT /F1 12 Tf 40 500 Td (SECRET) Tj ET EMC", "", OFF);
    let root = doc.dict(&Object::Ref(ObjRef::new(1, 0))).unwrap();
    let mut oc = doc.dict(root.get(b"OCProperties").unwrap()).unwrap();
    let mut config = Dict::new();
    config.set(b"ON".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(6, 0)); 65_537]));
    oc.set(b"D".to_vec(), Object::Dict(config));
    doc.update_dict(ObjRef::new(1, 0), |d| {
        d.set(b"OCProperties".to_vec(), Object::Dict(oc));
    })
    .unwrap();
    assert_eq!(texts(&doc), ["PUBLIC"]);
}

#[test]
fn indirect_properties_membership_arrays_and_nested_form_visibility() {
    use pdfcraft_cos::{ObjRef, Object, Stream};
    let mut doc = fixture("/OC /Member BDC BT /F1 12 Tf 40 700 Td (SECRET) Tj ET EMC", "", OFF);
    let groups = doc.add(Object::Array(vec![Object::Ref(ObjRef::new(6, 0)), Object::Ref(ObjRef::new(7, 0))]));
    doc.update_dict(ObjRef::new(9, 0), |d| {
        d.set(b"OCGs".to_vec(), Object::Ref(groups));
    })
    .unwrap();
    let mut resources = doc.dict(pdfcraft_model::pages(&doc)[0].dict.get(b"Resources").unwrap()).unwrap();
    let properties = doc.add(resources.get(b"Properties").unwrap().clone());
    resources.set(b"Properties".to_vec(), Object::Ref(properties));
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
    assert!(texts(&doc).is_empty());
    // A visible form enters an inherited-resource child whose own /OC is hidden.
    let mut parent = Dict::new();
    parent.set(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()));
    parent.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
    let child = doc.add(Object::Stream(Stream::from_raw(parent.clone(), b"BT /F1 12 Tf 40 700 Td (NESTED_SECRET) Tj ET".to_vec())));
    doc.update_dict(child, |d| {
        d.set(b"OC".to_vec(), Object::Ref(ObjRef::new(6, 0)));
    })
    .unwrap();
    let mut res = Dict::new();
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(ObjRef::new(5, 0)));
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut xo = Dict::new();
    xo.set(b"Child".to_vec(), Object::Ref(child));
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    parent.set(b"Resources".to_vec(), Object::Dict(res));
    doc.set(ObjRef::new(8, 0), Object::Stream(Stream::from_raw(parent, b"/Child Do BT /F1 12 Tf 40 500 Td (PARENT_PUBLIC) Tj ET".to_vec())));
    doc.set(ObjRef::new(4, 0), Object::Stream(Stream::from_raw(Dict::new(), b"/Fm Do".to_vec())));
    assert_eq!(texts(&doc), ["PARENT_PUBLIC"]);
}

/// An outer replacement must not disclose hidden text discovered only inside a visible form.
#[test]
fn actual_text_around_visible_forms_cannot_reintroduce_their_hidden_text() {
    use pdfcraft_cos::{ObjRef, Object, Stream};
    let mut doc = fixture("/Span <</ActualText (PUBLIC NESTED_SECRET)>> BDC BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET /Fm Do EMC", "", OFF);
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()));
    form.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
    doc.set(
        ObjRef::new(8, 0),
        Object::Stream(Stream::from_raw(
            form,
            b"/OC /Hidden BDC BT /F1 12 Tf 40 600 Td (NESTED_SECRET) Tj ET EMC BT /F1 12 Tf 40 500 Td (FORM_PUBLIC) Tj ET".to_vec(),
        )),
    );
    assert_eq!(texts(&doc), ["PUBLIC", "FORM_PUBLIC"]);
    assert_eq!(text::text_lines(&doc, 0).unwrap().iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["PUBLIC"]);
}

/// Visible images, missing resources and non-stream dictionaries do not invalidate replacement text.
#[test]
fn actual_text_around_non_form_xobjects_is_preserved() {
    use pdfcraft_cos::{ObjRef, Object, Stream};
    for kind in ["image", "dictionary", "missing"] {
        let mut doc = fixture("/Span <</ActualText (ACCESSIBLE)>> BDC BT /F1 12 Tf 40 700 Td (GLYPHS) Tj ET /Fm Do EMC", "", OFF);
        let mut dict = Dict::new();
        dict.set(b"Subtype".to_vec(), Object::Name(if kind == "image" { b"Image".to_vec() } else { b"Form".to_vec() }));
        let object = match kind {
            "image" => Object::Stream(Stream::from_raw(dict, vec![0])),
            "dictionary" => Object::Dict(dict),
            _ => Object::Null,
        };
        doc.set(ObjRef::new(8, 0), object);
        assert_eq!(texts(&doc), ["ACCESSIBLE"], "{kind}");
        assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "ACCESSIBLE", "{kind}");
    }
}

/// Named replacement properties share the form boundary, while visible images retain semantics.
#[test]
fn named_actual_text_around_forms_is_invalidated_but_images_keep_it() {
    use pdfcraft_cos::{ObjRef, Object, PdfString, Stream};
    let mut doc = fixture("/Span /Replacement BDC BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET /Fm Do EMC", "", OFF);
    let mut resources = doc.dict(pdfcraft_model::pages(&doc)[0].dict.get(b"Resources").unwrap()).unwrap();
    let mut properties = doc.dict(resources.get(b"Properties").unwrap()).unwrap();
    let mut replacement = Dict::new();
    replacement.set(b"ActualText".to_vec(), Object::String(PdfString::text("PUBLIC NESTED_SECRET")));
    let replacement = doc.add(Object::Dict(replacement));
    properties.set(b"Replacement".to_vec(), Object::Ref(replacement));
    resources.set(b"Properties".to_vec(), Object::Dict(properties));
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
    let form = doc.dict(&Object::Ref(ObjRef::new(8, 0))).unwrap();
    doc.set(ObjRef::new(8, 0), Object::Stream(Stream::from_raw(form, b"/OC /Hidden BDC BT /F1 12 Tf 40 500 Td (NESTED_SECRET) Tj ET EMC".to_vec())));
    assert_eq!(texts(&doc), ["PUBLIC"]);
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "PUBLIC");
    let mut image = Dict::new();
    image.set(b"Subtype".to_vec(), Object::Name(b"Image".to_vec()));
    doc.set(ObjRef::new(8, 0), Object::Stream(Stream::from_raw(image, vec![0])));
    assert_eq!(texts(&doc), ["PUBLIC NESTED_SECRET"]);
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "PUBLIC NESTED_SECRET");
}

#[test]
fn visible_text_past_256_marked_levels_is_read() {
    for depth in [255, 256, 257, 300] {
        let content = format!("{} BT /F1 12 Tf 40 700 Td (DEEP) Tj ET {}", "/Span BMC ".repeat(depth), "EMC ".repeat(depth));
        let doc = fixture(&content, "", OFF);
        assert_eq!(texts(&doc), ["DEEP"], "depth {depth}");
        assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "DEEP");
    }
}

#[test]
fn deep_hidden_scope_ends_at_its_own_emc() {
    let content = format!(
        "{} /OC /Hidden BDC BT /F1 12 Tf 40 700 Td (SECRET) Tj ET EMC BT /F1 12 Tf 40 600 Td (PUBLIC) Tj ET {}",
        "/Span BMC ".repeat(300),
        "EMC ".repeat(300)
    );
    assert_eq!(texts(&fixture(&content, "", OFF)), ["PUBLIC"]);
}

#[test]
fn unresolvable_oc_properties_stay_visible() {
    for prefix in ["/OC /Nope BDC", "/OC BDC", "/Span /Hidden BDC"] {
        let content = format!("{prefix} BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET EMC");
        assert_eq!(texts(&fixture(&content, "", OFF)), ["PUBLIC"], "{prefix}");
    }
    for groups in [None, Some(Object::Array(vec![]))] {
        let mut doc = fixture("/OC /Member BDC BT /F1 12 Tf 40 700 Td (PUBLIC) Tj ET EMC", "", OFF);
        let mut membership = Dict::new();
        membership.set(b"Type".to_vec(), Object::name("OCMD"));
        membership.set(b"P".to_vec(), Object::name("AllOn"));
        if let Some(groups) = groups {
            membership.set(b"OCGs".to_vec(), groups);
        }
        doc.set(pdfcraft_cos::ObjRef::new(9, 0), Object::Dict(membership));
        assert_eq!(texts(&doc), ["PUBLIC"]);
    }
}
