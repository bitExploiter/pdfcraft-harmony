//! Synthetic occurrence regressions (XFA 3.3, chapter 7, “Repeating Elements using
//! Occurrence Limits”). No source document or viewer assets are used here.

use crate::layout::{Form, Item, Widget};
use crate::{layout, parse, parse_datasets};

/// Collect interactive geometry from every laid-out page for visibility and spacing assertions.
fn widgets(form: &Form) -> Vec<&Widget> {
    form.pages
        .iter()
        .flat_map(|p| &p.items)
        .filter_map(|item| match item {
            Item::Widget(widget) => Some(widget.as_ref()),
            _ => None,
        })
        .collect()
}

/// Lay out a synthetic body with optional datasets under a single top-to-bottom root.
fn render(body: &str, data: Option<&str>) -> Form {
    let (template, _) = parse(&format!(r#"<template><subform name="form" layout="tb" w="300pt">{body}</subform></template>"#)).unwrap();
    let data = data.map(|xml| {
        parse_datasets(&format!(r#"<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data>{xml}</xfa:data></xfa:datasets>"#))
            .unwrap()
    });
    layout(&template, data.as_ref()).unwrap()
}

/// Zero-instance children consume no space in positioned, flowing, and table containers.
#[test]
fn zero_instances_are_absent_in_every_container_layout() {
    for mode in ["position", "tb", "lr-tb", "rl-tb", "table"] {
        for height in ["", " h=\"80pt\""] {
            let child_mode = if mode == "table" { "row" } else { "tb" };
            let body = format!(
                r#"
                <subform name="box" layout="{mode}" w="300pt"{height}>
                    <subform name="optional" layout="{child_mode}" w="120pt" h="30pt">
                        <occur min="0" initial="0" max="1"/>
                        <field name="absent" w="100pt" h="20pt"><ui><textEdit/></ui></field>
                    </subform>
                    <field name="after" w="100pt" h="20pt"><ui><textEdit/></ui></field>
                </subform>
                <field name="following" w="100pt" h="20pt"><ui><textEdit/></ui></field>"#
            );
            let form = render(&body, None);
            let fields = widgets(&form);
            assert_eq!(fields.len(), 2, "mode={mode}, height={height}: {fields:?}");
            assert!(fields[0].som.ends_with("after[0]"));
            if height.is_empty() {
                assert!((fields[1].rect.y - fields[0].rect.y - 20.0).abs() < 0.01, "zero instance must take no height: {fields:?}");
            }
        }
    }
}

/// Blank merges use initial counts; populated datasets instead obey matching records and bounds.
#[test]
fn occurrence_initial_is_for_blank_forms_and_minimum_for_data_merges() {
    let body = r#"<subform name="row" layout="tb" w="100pt" h="20pt">
        <occur min="0" initial="2" max="3"/>
        <field name="value" w="100pt" h="20pt"><ui><textEdit/></ui></field>
    </subform>"#;
    assert_eq!(widgets(&render(body, None)).len(), 2);
    assert_eq!(widgets(&render(body, Some(""))).len(), 2, "an empty data packet is a blank merge");
    assert_eq!(widgets(&render(body, Some("<form/>"))).len(), 0, "data contains no row");
    assert_eq!(widgets(&render(body, Some("<form><row/></form>"))).len(), 1);
    assert_eq!(widgets(&render(body, Some("<form><row/><row/><row/><row/></form>"))).len(), 3);
    let required = body.replace("min=\"0\"", "min=\"2\"");
    assert_eq!(widgets(&render(&required, Some("<form><row/></form>"))).len(), 2);
}

/// An optional singleton requires matching data; omitted max=0 prohibits instances.
#[test]
fn optional_singleton_is_instantiated_when_its_data_exists() {
    let body = r#"<subform name="row" layout="tb"><occur min="0" initial="0" max="1"/>
        <field name="value" w="100pt" h="20pt"><ui><textEdit/></ui></field>
    </subform>"#;
    assert_eq!(widgets(&render(body, None)).len(), 0);
    assert_eq!(widgets(&render(body, Some("<form><row><value>present</value></row></form>"))).len(), 1);
    let prohibited = body.replace(" max=\"1\"", "");
    assert_eq!(widgets(&render(&prohibited, Some("<form><row/></form>"))).len(), 0, "omitted max defaults to min=0");
}

/// Break directives inside absent instances must neither advance the cursor nor create pages.
#[test]
fn zero_instances_take_no_height_and_do_not_trigger_page_breaks() {
    let body = r#"<field name="before" w="100pt" h="20pt"><ui><textEdit/></ui></field>
        <subform name="row" layout="tb"><occur min="0" initial="0" max="-1"/>
            <breakBefore targetType="pageArea"/><breakAfter targetType="pageArea"/>
            <field name="absent" w="100pt" h="200pt"><ui><textEdit/></ui></field>
        </subform>
        <field name="after" w="100pt" h="20pt"><ui><textEdit/></ui></field>"#;
    let form = render(body, None);
    assert_eq!(form.pages.len(), 1);
    let fields = widgets(&form);
    assert_eq!(fields.len(), 2);
    assert!((fields[1].rect.y - fields[0].rect.y - 20.0).abs() < 0.01);
}

/// A table cell counts its data under its row, not under the enclosing table.
#[test]
fn an_optional_subform_cell_uses_the_rows_data_scope() {
    let body = r#"<subform name="table" layout="table" columnWidths="100pt">
        <subform name="row" layout="row">
            <subform name="cell" layout="tb"><occur min="0" initial="0" max="1"/>
                <field name="value" w="100pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>
        </subform>
    </subform>"#;
    assert_eq!(widgets(&render(body, Some("<form><table><row/></table></form>"))).len(), 0);
    let form = render(body, Some("<form><table><row><cell><value>present</value></cell></row></table></form>"));
    let fields = widgets(&form);
    assert_eq!(fields.len(), 1);
    assert!(fields[0].rect.h >= 20.0, "the row must measure its present cell: {:?}", fields[0]);
}

/// Build a section whose optional child makes it cross the full-page-height threshold.
fn shrinking_section(mode: &str, constraints: &str, height: &str) -> String {
    format!(
        r#"
        <field name="intro" w="300pt" h="300pt"><ui><textEdit/></ui></field>
        <subform name="section" layout="{mode}" w="300pt" {height}>
            {constraints}
            <field name="first" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <field name="second" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <field name="third" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <subform name="optional" w="300pt" h="240pt"><occur min="0" initial="0" max="1"/>
                <field name="extra" w="300pt" h="240pt"><ui><textEdit/></ui></field>
            </subform>
        </subform>"#
    )
}

/// Locate the first section field to distinguish splitting from moving the entire section.
fn first_field_page(form: &Form) -> usize {
    form.pages.iter().position(|page| page.items.iter().any(|item| matches!(item, Item::Widget(field) if field.som.ends_with("first[0]")))).unwrap()
}

/// Shrinking intrinsic horizontal flow below a page height must not introduce an implicit keep.
#[test]
fn removing_the_last_optional_row_does_not_push_a_flowing_section_to_the_next_page() {
    for mode in ["lr-tb", "rl-tb"] {
        let body = shrinking_section(mode, "", "");
        let populated = render(&body, Some("<form><section><optional/></section></form>"));
        let empty = render(&body, Some("<form><section/></form>"));
        assert_eq!(first_field_page(&populated), 0, "a section taller than the page starts in the space left");
        assert_eq!(first_field_page(&empty), 0, "shrinking below a full page must not move the whole section: {mode}");
        assert_eq!(empty.pages.len(), 2);
        for field in widgets(&empty) {
            assert!(field.rect.y >= 36.0 && field.rect.bottom() <= 756.01, "a field is kept whole at the break: {field:?}");
        }
    }
}

/// Explicit keep constraints and fixed heights continue to move fitting sections as a unit.
#[test]
fn explicitly_kept_and_fixed_height_horizontal_sections_still_move_whole() {
    for mode in ["lr-tb", "rl-tb"] {
        for constraint in [r#"<keep intact="contentArea"/>"#, r#"<keep intact="pageArea"/>"#] {
            let form = render(&shrinking_section(mode, constraint, ""), Some("<form><section/></form>"));
            assert_eq!(first_field_page(&form), 1);
        }
        let form = render(&shrinking_section(mode, "", r#"h="540pt""#), Some("<form><section/></form>"));
        assert_eq!(first_field_page(&form), 1);
    }
}

/// Repeated rows must measure optional cells in each row's data scope, including
/// tables measured inside positioned containers and drawn with a fixed height.
#[test]
fn repeated_table_rows_measure_each_optional_cell() {
    for wrapper in ["tb", "position"] {
        for height in ["", " h=\"100pt\""] {
            let body = format!(
                r#"<subform name="box" layout="{wrapper}">
                <subform name="table" layout="table" columnWidths="100pt"{height}>
                    <subform name="row" layout="row"><occur min="0" max="-1"/>
                        <subform name="cell" layout="tb"><occur min="0" initial="0" max="1"/>
                            <field name="value" w="100pt" h="30pt"><ui><textEdit/></ui></field>
                        </subform>
                    </subform>
                </subform>
                <field name="after" w="100pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>"#
            );
            let form = render(
                &body,
                Some(
                    "<form><box><table><row/><row><cell><value>second</value></cell></row><row><cell><value>third</value></cell></row></table></box></form>",
                ),
            );
            let fields = widgets(&form);
            let cells: Vec<_> = fields.iter().filter(|f| f.som.ends_with("value[0]")).collect();
            assert_eq!(cells.len(), 2, "wrapper={wrapper}, height={height}: {fields:?}");
            assert!(cells[0].rect.h >= 30.0 && cells[1].rect.h >= 30.0);
            assert!(cells[1].rect.y >= cells[0].rect.bottom() - 0.01);
            if wrapper == "tb" {
                let after = fields.iter().find(|f| f.som.ends_with("after[0]")).unwrap();
                assert!(after.rect.y >= cells[1].rect.bottom() - 0.01);
            }
        }
    }
}

/// Later optional cells drive page breaks and leave room below repeated headers.
#[test]
fn optional_table_cells_paginate_with_repeated_headers() {
    let body = r#"<pageSet><pageArea name="body">
        <contentArea x="10pt" y="10pt" w="300pt" h="70pt"/>
        <medium short="320pt" long="100pt"/>
        </pageArea></pageSet>
        <subform name="table" layout="table" columnWidths="100pt"><overflow leader="header"/>
            <subform name="header" layout="row"><field name="heading" h="20pt"><ui><textEdit/></ui></field></subform>
            <subform name="row" layout="row"><occur min="0" max="-1"/>
                <subform name="cell" layout="tb"><occur min="0" initial="0" max="1"/>
                    <field name="value" w="100pt" h="40pt"><ui><textEdit/></ui></field>
                </subform>
            </subform>
        </subform>"#;
    let form = render(
        body,
        Some("<form><table><row/><row><cell><value>second</value></cell></row><row><cell><value>third</value></cell></row></table></form>"),
    );
    assert_eq!(form.pages.len(), 2);
    for page in &form.pages {
        let fields: Vec<_> = page
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Widget(w) => Some(w),
                _ => None,
            })
            .collect();
        assert_eq!(fields.len(), 2, "one header and one present cell: {fields:?}");
        assert!(fields[0].som.ends_with("heading[0]"));
        assert!(fields[1].rect.y >= fields[0].rect.bottom() - 0.01);
        assert!(fields[1].rect.bottom() <= 80.01);
    }
}

/// Each repeated non-row container measures its own optional children, including
/// both data orders, nested parents, fixed drawing boxes and horizontal packing.
#[test]
fn repeated_flowing_subforms_measure_their_own_instances() {
    for outer in ["tb", "table", "lr-tb", "rl-tb"] {
        for mode in ["tb", "lr-tb", "rl-tb"] {
            for height in ["", " h=\"120pt\""] {
                for rows in ["<row/><row><opt/></row>", "<row><opt/></row><row/>"] {
                    let body = format!(
                        r#"
                    <subform name="parent" layout="tb" w="300pt"><occur min="0" max="-1"/>
                        <subform name="box" layout="{outer}" w="300pt"{height}>
                            <subform name="row" layout="{mode}" w="300pt"><occur min="0" max="-1"/>
                                <subform name="opt" layout="tb" w="300pt"><occur min="0" initial="0" max="1"/>
                                    <field name="value" w="300pt" h="30pt"><ui><textEdit/></ui></field>
                                </subform>
                                <field name="tail" w="300pt" h="10pt"><ui><textEdit/></ui></field>
                            </subform>
                        </subform>
                        <field name="after" w="300pt" h="10pt"><ui><textEdit/></ui></field>
                    </subform>"#
                    );
                    let reverse = if rows.starts_with("<row/>") { "<row><opt/></row><row/>" } else { "<row/><row><opt/></row>" };
                    let data = format!("<form><parent><box>{rows}</box></parent><parent><box>{reverse}</box></parent></form>");
                    let form = render(&body, Some(&data));
                    let fields = widgets(&form);
                    assert_eq!(fields.len(), 8);
                    for parent in 0..2 {
                        let scoped: Vec<_> = fields.iter().filter(|f| f.som.contains(&format!("parent[{parent}]"))).collect();
                        let tails: Vec<_> = scoped.iter().filter(|f| f.som.ends_with("tail[0]")).collect();
                        let value = scoped.iter().find(|f| f.som.ends_with("value[0]")).unwrap();
                        let after = scoped.iter().find(|f| f.som.ends_with("after[0]")).unwrap();
                        let present = usize::from(value.som.contains("row[1]"));
                        let start = if present == 0 { value.rect.y } else { tails[0].rect.y };
                        assert!(
                            (tails[0].rect.y - start - if present == 0 { 30.0 } else { 0.0 }).abs() < 0.01,
                            "{outer}/{mode}/{height}/{rows}: {scoped:?}"
                        );
                        assert!((tails[1].rect.y - start - 40.0).abs() < 0.01, "{outer}/{mode}/{height}/{rows}: {scoped:?}");
                        assert!(
                            (after.rect.y - start - if height.is_empty() { 50.0 } else { 120.0 }).abs() < 0.01,
                            "{outer}/{mode}/{height}/{rows}: {scoped:?}"
                        );
                    }
                }
            }
        }
    }
}

/// A later, taller horizontal instance must reserve its actual height at a page break.
#[test]
fn repeated_horizontal_subforms_paginate_by_instance_height() {
    for mode in ["lr-tb", "rl-tb"] {
        let body = format!(
            r#"<pageSet><pageArea name="body">
            <contentArea x="10pt" y="10pt" w="300pt" h="60pt"/>
            <medium short="320pt" long="80pt"/>
            </pageArea></pageSet>
            <subform name="row" layout="{mode}" w="300pt"><occur min="0" max="-1"/>
                <keep intact="contentArea"/>
                <subform name="opt" layout="tb" w="300pt"><occur min="0" initial="0" max="1"/>
                    <field name="value" w="300pt" h="40pt"><ui><textEdit/></ui></field>
                </subform>
                <field name="tail" w="300pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>"#
        );
        let form = render(&body, Some("<form><row/><row><opt/></row></form>"));
        assert_eq!(form.pages.len(), 2, "{mode}");
        let fields = widgets(&form);
        assert_eq!(fields.len(), 3);
        assert!((fields[1].rect.y - 10.0).abs() < 0.01);
        assert!((fields[2].rect.y - 50.0).abs() < 0.01);
        assert!(fields.iter().all(|f| f.rect.bottom() <= 70.01));
    }
}

/// Same-named fields use their own data for growth, even under a positioned parent.
#[test]
fn same_named_growing_fields_measure_their_own_values() {
    for parent in ["tb", "position"] {
        let body = format!(
            r#"<subform name="box" layout="{parent}" w="300pt">
            <subform name="content" layout="tb" w="100pt">
                <field name="text" w="100pt"><ui><textEdit multiLine="1"/></ui></field>
                <field name="text" w="100pt"><ui><textEdit multiLine="1"/></ui></field>
                <field name="after" w="100pt" h="10pt"><ui><textEdit/></ui></field>
            </subform></subform>"#
        );
        let form = render(
            &body,
            Some(
                "<form><box><content><text>short</text><text>one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen</text></content></box></form>",
            ),
        );
        let fields = widgets(&form);
        assert_eq!(fields.len(), 3);
        assert!(fields[1].rect.h > fields[0].rect.h + 15.0, "{parent}: {fields:?}");
        assert!((fields[1].rect.y - fields[0].rect.bottom()).abs() < 0.01);
        assert!((fields[2].rect.y - fields[1].rect.bottom()).abs() < 0.01);
    }
}

/// An absent overflow leader must stay absent on every continuation page.
#[test]
fn zero_instance_overflow_leaders_stay_absent() {
    let body = r#"<pageSet><pageArea name="body">
        <contentArea x="10pt" y="10pt" w="300pt" h="70pt"/>
        <medium short="320pt" long="100pt"/></pageArea></pageSet>
        <subform name="table" layout="table" columnWidths="100pt"><overflow leader="header"/>
            <subform name="header" layout="row"><occur min="0" initial="0" max="1"/>
                <field name="heading" h="20pt"><ui><textEdit/></ui></field>
            </subform>
            <subform name="row" layout="row"><occur min="0" max="-1"/>
                <field name="value" h="40pt"><ui><textEdit/></ui></field>
            </subform>
        </subform>"#;
    for header in ["", "<header/>"] {
        let data = format!("<form><table>{header}<row/><row/><row/></table></form>");
        let form = render(body, Some(&data));
        assert_eq!(form.pages.len(), 3);
        for page in &form.pages {
            let fields: Vec<_> = page.items.iter().filter_map(|i| if let Item::Widget(w) = i { Some(w) } else { None }).collect();
            assert_eq!(fields.len(), if header.is_empty() { 1 } else { 2 });
            assert_eq!(fields.iter().any(|f| f.som.contains("header[")), !header.is_empty());
            assert!(fields.iter().all(|f| f.rect.bottom() <= 80.01));
        }
    }
}

/// Unnamed containers use occurrence defaults even when unrelated data is present.
#[test]
fn unnamed_subforms_keep_initial_instances_in_layout_and_runtime() {
    for initial in [0, 1, 2] {
        let body = format!(
            r#"<subform layout="tb" w="100pt"><occur min="0" initial="{initial}" max="2"/>
            <field name="value" w="100pt" h="20pt"><ui><textEdit/></ui></field></subform>"#
        );
        let (tpl, _) = parse(&format!(r#"<template><subform name="form" layout="tb">{body}</subform></template>"#)).unwrap();
        let data = parse_datasets(
            r#"<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><unrelated/></form></xfa:data></xfa:datasets>"#,
        )
        .unwrap();
        // Check runtime independently of geometry: scripts must see exactly the visible instances.
        let live = crate::form_tree(&tpl, Some(&data), &Default::default());
        let mut pending = vec![&live.root];
        let mut values = 0;
        while let Some(n) = pending.pop() {
            values += usize::from(n.name == "value");
            pending.extend(&n.children);
        }
        assert_eq!(values, initial);
        let form = render(&body, Some("<form><unrelated/></form>"));
        assert_eq!(widgets(&form).len(), initial);
    }
}
