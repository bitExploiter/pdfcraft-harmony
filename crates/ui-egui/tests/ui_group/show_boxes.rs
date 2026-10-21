//! Show art, trim and bleed boxes (Acrobat: Preferences ▸ Page Display): the boxes a page sets
//! are outlined on it, each in its colour, where the page shows them.

use egui::{Color32, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const ART: Color32 = Color32::from_rgb(0xE0, 0x00, 0x00);
const TRIM: Color32 = Color32::from_rgb(0x00, 0xA0, 0x00);
const BLEED: Color32 = Color32::from_rgb(0x00, 0x50, 0xF0);

/// A 300×400 pt media box, cropped by 10 pt, with all three boxes set.
const BOXED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /CropBox [10 10 290 390] /BleedBox [20 30 280 370] /TrimBox [30 40 270 360] /ArtBox [50 60 250 340] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// The same crop, and no bleed, trim or art box: each one defaults to the crop box.
const PLAIN: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /CropBox [10 10 290 390] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Turned a quarter clockwise, with a trim box off centre so turning it shows.
const ROTATED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Rotate 90 /TrimBox [30 40 250 380] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness(pdf: &'static [u8], show: Option<&'static str>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("boxes.pdf", None, pdf.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("layout", "single").unwrap();
        if let Some(show) = show {
            app.set_option("page-boxes", show).unwrap();
        }
        app
    });
    h.run_steps(4);
    h
}

/// The outlines drawn in `color` this frame.
fn outlines(h: &Harness<'static, PdfCraftApp>, color: Color32) -> Vec<Rect> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Rect(r) if r.stroke.color == color && r.stroke.width > 0.0 => Some(r.rect),
            _ => None,
        })
        .collect()
}

/// `page` scaled to the fractions `[left, top, right, bottom]` of its width and height.
fn part(page: Rect, f: [f32; 4]) -> Rect {
    Rect::from_min_max(
        egui::pos2(page.left() + f[0] * page.width(), page.top() + f[1] * page.height()),
        egui::pos2(page.left() + f[2] * page.width(), page.top() + f[3] * page.height()),
    )
}

fn assert_near(got: Rect, want: Rect, what: &str) {
    let off = (got.min - want.min).length().max((got.max - want.max).length());
    assert!(off < 1.0, "{what}: drawn at {got:?}, expected {want:?}");
}

#[test]
fn a_trim_box_inside_the_crop_is_outlined_where_it_is_on_the_page() {
    let h = harness(BOXED, Some("on"));
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    // The page shows the crop box, 280 × 380 pt from (10, 10); y runs down from its top (390).
    let trim = outlines(&h, TRIM);
    assert_eq!(trim.len(), 1, "one trim outline: {trim:?}");
    assert_near(trim[0], part(page, [20.0 / 280.0, 30.0 / 380.0, 260.0 / 280.0, 350.0 / 380.0]), "trim box");
    let bleed = outlines(&h, BLEED);
    assert_eq!(bleed.len(), 1, "one bleed outline: {bleed:?}");
    assert_near(bleed[0], part(page, [10.0 / 280.0, 20.0 / 380.0, 270.0 / 280.0, 360.0 / 380.0]), "bleed box");
    let art = outlines(&h, ART);
    assert_eq!(art.len(), 1, "one art outline: {art:?}");
    assert_near(art[0], part(page, [40.0 / 280.0, 50.0 / 380.0, 240.0 / 280.0, 330.0 / 380.0]), "art box");
}

#[test]
fn a_page_without_its_own_boxes_draws_none() {
    let h = harness(PLAIN, Some("on"));
    for (color, name) in [(TRIM, "trim"), (BLEED, "bleed"), (ART, "art")] {
        assert!(outlines(&h, color).is_empty(), "the default {name} box is the crop box, not drawn");
    }
}

#[test]
fn boxes_on_a_rotated_page_turn_with_it() {
    let h = harness(ROTATED, Some("on"));
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    assert!(page.width() > page.height(), "the page shows turned: {page:?}");
    // Turned clockwise, the user-space left edge (x = 30) is the top and the bottom (y = 40) is
    // the left, each a tenth in; the top (y = 380) is 20 pt from the right, x = 250 at 5/6 down.
    let trim = outlines(&h, TRIM);
    assert_eq!(trim.len(), 1, "one trim outline: {trim:?}");
    assert_near(trim[0], part(page, [40.0 / 400.0, 30.0 / 300.0, 380.0 / 400.0, 250.0 / 300.0]), "rotated trim box");
}

#[test]
fn boxes_are_hidden_until_the_preference_is_on() {
    let h = harness(BOXED, None);
    assert!(!h.state().show_page_boxes, "off by default, as in Acrobat");
    for (color, name) in [(TRIM, "trim"), (BLEED, "bleed"), (ART, "art")] {
        assert!(outlines(&h, color).is_empty(), "no {name} outline with the preference off");
    }
    let h = harness(BOXED, Some("off"));
    assert!(outlines(&h, TRIM).is_empty(), "page-boxes off hides them");
}

#[test]
fn preferences_turn_the_boxes_on_and_the_choice_is_remembered() {
    let mut h = harness(BOXED, None);
    h.state_mut().set_option("dialog", "preferences").unwrap();
    h.run_steps(2);
    let checkbox = egui::accesskit::Role::CheckBox;
    h.get_by_role_and_label(checkbox, "Show art, trim and bleed boxes").scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(checkbox, "Show art, trim and bleed boxes").click();
    h.run_steps(2);
    assert!(h.state().show_page_boxes);
    let saved = h.state().persist();
    let mut next = PdfCraftApp::new();
    next.restore(&saved);
    assert!(next.show_page_boxes, "the preference survives a restart");
}
