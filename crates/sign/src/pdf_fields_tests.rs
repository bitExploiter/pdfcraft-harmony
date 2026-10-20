use super::*;
use pdfcraft_testkit::Pdf;

/// A document whose form lists `fields`, with `objects` numbered from 3.
fn form(fields: &str, objects: &[String]) -> Document {
    let pdf = Pdf::new()
        .obj(format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{fields}] >> >>"))
        .obj("<< /Type /Pages /Kids [] /Count 0 >>")
        .objs(objects);
    Document::open(Arc::new(pdf.build())).unwrap()
}

/// A form as writers make them: names join down the tree, a field takes its type from the nearest
/// field above that has one, and named signature fields are listed parents first. A field listed
/// twice, or reached again from below, counts once, and a kid `/Fields` lists as well keeps its
/// full name; a kid may be written in place.
#[test]
fn nested_fields_are_named_down_the_tree() {
    let doc = form(
        "3 0 R 6 0 R 9 0 R 11 0 R 3 0 R 4 0 R",
        &[
            "<< /T (parent) /FT /Sig /Kids [4 0 R 5 0 R] >>".into(),
            "<< /T (a) /Kids [12 0 R 3 0 R] >>".into(),
            "<< /T (b) /FT /Tx >>".into(),
            "<< /T (form) /Kids [7 0 R 8 0 R] >>".into(),
            "<< /T (sig) /FT /Sig /Type /Annot /Subtype /Widget /Rect [0 0 10 10] >>".into(),
            "<< /T (text) /FT /Tx >>".into(),
            "<< /T () /Kids [10 0 R] >>".into(),
            "<< /T (inner) /FT /Sig >>".into(),
            "<< /FT /Sig /Kids [<< /T (direct) >>] >>".into(),
            "<< /Type /Annot /Subtype /Widget /Rect [1 2 3 4] >>".into(),
        ],
    );
    let fields = sig_fields(&doc);
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["parent", "parent.a", "form.sig", "inner", "direct"]);
    let widgets: Vec<Vec<Option<ObjRef>>> = fields.iter().map(|f| f.widgets.iter().map(|(r, _)| *r).collect()).collect();
    assert_eq!(widgets, [vec![], vec![Some(ObjRef::new(12, 0))], vec![Some(ObjRef::new(7, 0))], vec![], vec![]]);
}
