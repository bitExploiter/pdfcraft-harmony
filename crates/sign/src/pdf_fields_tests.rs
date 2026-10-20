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

/// Each name's first three characters and its length: enough to tell them apart without
/// printing thousands of bytes when an assertion fails.
fn short(fields: &[Field]) -> Vec<(String, usize)> {
    fields.iter().map(|f| (f.name.chars().take(3).collect(), f.name.len())).collect()
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
    assert!(walk_sig_fields(&doc).1, "a form within the caps is read whole");
}

/// A widget two fields list stays listed under both, whether each lists it in its own `/Kids` or
/// both list one shared array: the status layers count these claims to leave such a widget alone.
#[test]
fn a_widget_two_fields_list_is_listed_under_both() {
    let widget = || "<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] >>".to_string();
    let own = form("3 0 R 4 0 R", &["<< /T (a) /FT /Sig /Kids [5 0 R] >>".into(), "<< /T (b) /FT /Sig /Kids [5 0 R] >>".into(), widget()]);
    let shared =
        form("3 0 R 4 0 R", &["<< /T (a) /FT /Sig /Kids 6 0 R >>".into(), "<< /T (b) /FT /Sig /Kids 6 0 R >>".into(), widget(), "[5 0 R]".into()]);
    for (how, doc) in [("own arrays", own), ("one shared array", shared)] {
        let widgets: Vec<Vec<Option<ObjRef>>> = sig_fields(&doc).iter().map(|f| f.widgets.iter().map(|(r, _)| *r).collect()).collect();
        assert_eq!(widgets, [vec![Some(ObjRef::new(5, 0))], vec![Some(ObjRef::new(5, 0))]], "{how}");
    }
}

/// Every kid's name starts with its parent's. Copied into each kid, a 1 MB name over 100,000 kids
/// is 100 GB. A name over the cap is not read, and neither is anything under it.
///
/// The small form goes first: without the cap it fails on the names it lists, long before the
/// large one could use up the machine's memory.
#[test]
fn a_long_name_over_many_kids_is_not_copied_into_each() {
    for (len, kids) in [(64 << 10, 200), (1 << 20, 100_000)] {
        let doc = form(
            "3 0 R 5 0 R",
            &[
                format!("<< /T 4 0 R /FT /Sig /Kids [{}] >>", "<< /T (x) >> ".repeat(kids)),
                format!("({})", "n".repeat(len)),
                "<< /T (ok) /FT /Sig >>".into(),
            ],
        );
        let (fields, whole) = walk_sig_fields(&doc);
        let names: Vec<String> = fields.into_iter().map(|f| f.name).collect();
        let bytes: usize = names.iter().map(String::len).sum();
        assert!(names == ["ok"], "a {len}-byte name over {kids} kids: {} fields listed, {bytes} bytes of names", names.len());
        assert!(!whole, "the long name was passed over");
    }
}

/// The cap is on the whole name as read, dots included. A partial name stored in more bytes than
/// any name within the cap could take, even as UTF-16, is not read, whatever it trims to.
#[test]
fn names_up_to_the_cap_are_read_and_longer_ones_are_not() {
    let max = MAX_FIELD_NAME_BYTES;
    let utf16 = |n: usize| format!("<FEFF{}>", "0061".repeat(n));
    let doc = form(
        "3 0 R 4 0 R 5 0 R 8 0 R 9 0 R 10 0 R 11 0 R",
        &[
            format!("<< /T ({}) /FT /Sig >>", "a".repeat(max)),
            format!("<< /T ({}) /FT /Sig >>", "b".repeat(max + 1)),
            "<< /T (p) /FT /Sig /Kids [6 0 R 7 0 R] >>".into(),
            format!("<< /T ({}) >>", "c".repeat(max - 2)),
            format!("<< /T ({}) >>", "d".repeat(max - 1)),
            format!("<< /T {} /FT /Sig >>", utf16(max)),
            format!("<< /T {} /FT /Sig >>", utf16(max + 1)),
            format!("<< /T (x{}) /FT /Sig >>", " ".repeat(2 * max + 1)),
            format!("<< /T (y{}) /FT /Sig >>", " ".repeat(2 * max + 2)),
        ],
    );
    let (fields, whole) = walk_sig_fields(&doc);
    let expected = [("aaa", max), ("p", 1), ("p.c", max), ("aaa", max), ("x", 1)].map(|(s, n)| (s.to_string(), n));
    assert_eq!(short(&fields), expected);
    assert!(!whole, "fields passed over for their names");
}

/// No more than `MAX_FIELDS` fields are looked at: the first in `/Fields`, then kids as they are
/// reached, out of whatever room is left.
#[test]
fn the_walk_stops_after_max_fields() {
    let fields = |n: usize| (0..n).map(|i| format!("<< /T (f{i}) /FT /Sig >>")).collect::<String>();
    let (listed, whole) = walk_sig_fields(&form(&fields(MAX_FIELDS), &[]));
    assert_eq!((listed.len(), whole), (MAX_FIELDS, true));
    let (listed, whole) = walk_sig_fields(&form(&fields(MAX_FIELDS + 1), &[]));
    assert_eq!((listed.len(), whole), (MAX_FIELDS, false));
    assert_eq!(listed.last().map(|f| f.name.clone()), Some(format!("f{}", MAX_FIELDS - 1)));
    // The first field's kids take the room that is left, so the second field's kid is not reached.
    let kids = (0..MAX_FIELDS).map(|i| format!("<< /T (k{i}) >>")).collect::<String>();
    let listed =
        sig_fields(&form("3 0 R 4 0 R", &[format!("<< /T (p) /FT /Sig /Kids [{kids}] >>"), "<< /T (q) /FT /Sig /Kids [<< /T (k) >>] >>".into()]));
    assert_eq!(listed.len(), MAX_FIELDS);
    assert_eq!(listed.last().map(|f| f.name.clone()), Some("q".to_string()));
}

/// No more than `MAX_ENTRIES` entries of `/Fields` and the `/Kids` arrays are read, all together.
#[test]
fn the_walk_reads_no_more_than_max_entries() {
    // The field is one entry of /Fields; its widgets take the rest.
    let field = |widgets: usize| form(&format!("<< /T (p) /FT /Sig /Kids [{}] >>", "<< /Subtype /Widget >> ".repeat(widgets)), &[]);
    let (listed, whole) = walk_sig_fields(&field(MAX_ENTRIES - 1));
    assert_eq!((listed[0].widgets.len(), whole), (MAX_ENTRIES - 1, true));
    let (listed, whole) = walk_sig_fields(&field(MAX_ENTRIES));
    assert_eq!((listed[0].widgets.len(), whole), (MAX_ENTRIES - 1, false));
}

/// A `/Kids` array shared by two fields on every level doubles the paths through the tree each
/// level down: 2^41 fields for 40 levels. A field written in place in a shared array is visited
/// once, under the first field that reaches it.
#[test]
fn a_kids_array_shared_by_many_fields_is_walked_once() {
    for levels in [16, 40] {
        let level = |i: usize| {
            if i + 1 < levels {
                format!("[<< /T (a) /Kids {n} 0 R >> << /T (b) /Kids {n} 0 R >>]", n = i + 4)
            } else {
                "[<< /T (a) >> << /T (b) >>]".to_string()
            }
        };
        let doc = form("<< /T (r) /FT /Sig /Kids 3 0 R >>", &(0..levels).map(level).collect::<Vec<_>>());
        assert_eq!(sig_fields(&doc).len(), 2 * levels + 1, "{levels} levels");
    }
}

/// A `/Fields` array that a field also lists as its `/Kids` leads back to itself, one name longer
/// each time round. Each field in it is listed once.
#[test]
fn a_fields_array_listed_again_as_kids_is_walked_once() {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields 3 0 R >> >>")
        .obj("<< /Type /Pages /Kids [] /Count 0 >>")
        .obj("[<< /T (a) /FT /Sig /Kids 3 0 R >>]");
    let doc = Document::open(Arc::new(pdf.build())).unwrap();
    let names: Vec<String> = sig_fields(&doc).into_iter().map(|f| f.name).collect();
    assert_eq!(names, ["a"]);
}

/// A `/Kids` array every field lists is read under each of them, which the entry cap bounds:
/// 20,000 fields over one array of 1,000 widgets would be 20 million entries.
///
/// The small form goes first, as above.
#[test]
fn a_kids_array_many_fields_list_is_read_within_the_cap() {
    for (fields, widgets) in [(200, 1_000), (MAX_FIELDS, 1_000)] {
        let listing = (0..fields).map(|i| format!("<< /T (f{i}) /FT /Sig /Kids 3 0 R >>")).collect::<String>();
        let array = format!("[{}]", (0..widgets).map(|i| format!("{} 0 R ", i + 4)).collect::<String>());
        let objects: Vec<String> = std::iter::once(array).chain((0..widgets).map(|_| "<< /Subtype /Widget >>".to_string())).collect();
        let (listed, whole) = walk_sig_fields(&form(&listing, &objects));
        let read = listed.iter().map(|f| f.widgets.len()).sum::<usize>() + fields;
        assert!(read <= MAX_ENTRIES && !whole, "{fields} fields over {widgets} widgets: {read} entries read");
    }
}

/// A widget every field lists is listed under each as one copy: copied into each, one 1 MB widget
/// under 20,000 fields is 20 GB.
///
/// The small form goes first, as above.
#[test]
fn a_widget_many_fields_list_is_copied_once() {
    for (len, fields) in [(64 << 10, 1_000), (1 << 20, MAX_FIELDS)] {
        let listing = (0..fields).map(|i| format!("<< /T (f{i}) /FT /Sig /Kids [3 0 R] >>")).collect::<String>();
        let doc = form(&listing, &[format!("<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] /Junk ({}) >>", "j".repeat(len))]);
        let listed = sig_fields(&doc);
        assert_eq!(listed.len(), fields);
        let first: &Dict = &listed[0].widgets[0].1;
        let shared = listed.iter().all(|f| f.widgets.len() == 1 && std::ptr::eq::<Dict>(&*f.widgets[0].1, first));
        assert!(shared, "a {len}-byte widget under {fields} fields");
    }
}
