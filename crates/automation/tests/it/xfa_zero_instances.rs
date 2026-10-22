//! Optional XFA rows through the same tool table used by CLI and MCP.
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Call the public automation table and require its JSON result for lifecycle assertions.
fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// Exercise the zero-row lifecycle through tools, checking rendered pixels as well as persisted values.
#[test]
fn zero_instances_tools_render_fill_undo_save_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    std::fs::write(dir.join("optional.pdf"), pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::optional_rows(0))).unwrap();
    let mut a = Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000);
    let id = tool(&mut a, "doc_open", json!({"path":"optional.pdf"}))["doc"].as_u64().unwrap();
    let rows = |a: &mut Automation, id| {
        tool(a, "form_fields", json!({"doc":id}))["fields"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["name"].as_str().is_some_and(|n| n.starts_with("entry")))
            .count()
    };
    assert_eq!(rows(&mut a, id), 0);
    let (_, _, empty) = a.render_pixels(id, 1, 1.0).unwrap();
    let click = tool(&mut a, "js_run", json!({"doc":id,"field":"add","script":""}));
    assert!(click["error"].is_null(), "{click:?}");
    assert_eq!(rows(&mut a, id), 1);
    tool(&mut a, "form_fill", json!({"doc":id,"values":{"entry":"saved row"}}));
    let (_, _, filled) = a.render_pixels(id, 1, 1.0).unwrap();
    assert_ne!(filled, empty);
    tool(&mut a, "js_run", json!({"doc":id,"field":"remove","script":""}));
    assert_eq!(rows(&mut a, id), 0);
    assert_eq!(a.render_pixels(id, 1, 1.0).unwrap().2, empty, "no ghost row or reserved space");
    tool(&mut a, "edit_undo", json!({"doc":id}));
    assert_eq!(a.render_pixels(id, 1, 1.0).unwrap().2, filled);
    tool(&mut a, "edit_redo", json!({"doc":id}));
    tool(&mut a, "doc_save", json!({"doc":id,"path":"saved.pdf"}));
    let reopened = tool(&mut a, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(rows(&mut a, reopened), 0);
    assert_eq!(a.render_pixels(reopened, 1, 1.0).unwrap().2, empty);
    tool(&mut a, "js_run", json!({"doc":reopened,"field":"add","script":""}));
    assert_eq!(rows(&mut a, reopened), 1);
}
