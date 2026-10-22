//! Use a certificate in the real shell: draw a signature, create a digital ID, sign, validate.

use egui::{Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::SignatureStatus;
use pdfcraft_sign::SignError;
use pdfcraft_sign::remote::mock::{self, MockService};
use pdfcraft_sign::remote::{HttpResponse, HttpTransport, Protocol};
use pdfcraft_ui_egui::{Dialog, NewIdSource, PdfCraftApp, QuickTool, RemoteAccountDraft, RemoteNeeds, RemoteOtp, RightPanel, SignStep};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 45 >> stream
BT /F1 14 Tf 20 150 Td (Please sign below) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A fresh folder per call: tests run in parallel and must not remove each other's files.
fn dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("pdfcraft-signing-ui-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness(dir: std::path::PathBuf) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("contract.pdf", None, FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.export_dir_override = Some(dir.to_string_lossy().into_owned());
        app.save_override = Some(dir.join("contract_signed.pdf").to_string_lossy().into_owned());
        app
    });
    h.run_steps(6);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    pos2(r.left() + x / 300.0 * r.width(), r.top() + (200.0 - y) / 200.0 * r.height())
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
}

fn field<'a>(h: &'a Harness<'static, PdfCraftApp>, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_label(label).last().unwrap_or_else(|| panic!("no {label:?}"))
}

#[test]
fn ctrl_alt_q_types_an_email_without_quitting() {
    let mut h = harness(dir());
    h.state_mut().execute("sign.digital");
    let (a, b) = (at(&h, 40.0, 90.0), at(&h, 220.0, 40.0));
    drag(&mut h, a, b);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().step, SignStep::Configure);
    field(&h, "Email Address").click();
    h.run_steps(1);
    field(&h, "Email Address").type_text("test");
    h.run_steps(1);
    // The key and text events produced by Ctrl+Alt+Q on German QWERTZ.
    h.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Q,
        physical_key: Some(egui::Key::Q),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers { ctrl: true, command: true, alt: true, ..Default::default() },
    });
    h.input_mut().events.push(egui::Event::Text("@".into()));
    h.step();
    assert!(!h.output().viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::Close)));
    assert_eq!(h.state().sign_draft.as_ref().unwrap().new_id.email, "test@");
    field(&h, "Email Address").type_text("example.org");
    h.run_steps(1);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().new_id.email, "test@example.org");
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
}

#[test]
fn drawing_a_signature_creating_an_id_signing_and_trusting() {
    let dir = dir();
    let mut h = harness(dir.clone());
    assert!(h.state_mut().execute("sign.digital"));
    assert_eq!(h.state().quick_tool, QuickTool::SignArea { certify: false });
    let (a, b) = (at(&h, 40.0, 90.0), at(&h, 220.0, 40.0));
    drag(&mut h, a, b);
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    assert_eq!(h.state().sign_draft.as_ref().map(|d| d.step), Some(SignStep::Configure), "no IDs yet: configure one");

    // Create a new digital ID (P-256 keeps the test fast).
    h.get_by_label("Configure a Digital ID for Signing");
    field(&h, "Name").click();
    h.run_steps(1);
    field(&h, "Name").type_text("Grace Hopper");
    h.state_mut().sign_draft.as_mut().unwrap().new_id.key = 3;
    for (label, text) in [("Password", "secret12"), ("Confirm Password", "secret12")] {
        field(&h, label).click();
        h.run_steps(1);
        field(&h, label).type_text(text);
        h.run_steps(1);
    }
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert_eq!(h.state().digital_ids.len(), 1);
    assert!(std::path::Path::new(&h.state().digital_ids[0].path).exists(), "saved as a .p12");
    h.get_by_label("Grace Hopper");
    h.get_by_label("Continue").click();
    h.run_steps(2);
    h.get_by_label("Sign as \"Grace Hopper\"");

    // A wrong password says so.
    field(&h, "Digital ID password").click();
    h.run_steps(1);
    field(&h, "Digital ID password").type_text("nope");
    h.get_by_label("Sign").click();
    h.run_steps(3);
    h.get_by_label("The password is incorrect.");
    h.state_mut().sign_draft.as_mut().unwrap().password = "secret12".into();
    field(&h, "Reason").click();
    h.run_steps(1);
    field(&h, "Reason").type_text("I approve");
    h.get_by_label("Sign").click();
    h.run_steps(6);

    let s = h.state();
    assert_eq!(s.dialog, None);
    assert!(dir.join("contract_signed.pdf").exists());
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.name, "contract_signed.pdf");
    let sig = doc.signatures.iter().find(|x| x.signed).expect("signed");
    assert_eq!((sig.status, sig.reason.as_deref(), sig.page), (SignatureStatus::Unknown, Some("I approve"), Some(0)));
    assert_eq!(s.right, Some(RightPanel::Signatures));
    h.get_by_label("At least one signature has problems.");

    // Signatures panel: expand, trust the signer → valid.
    h.get_by_label_contains("Signed by Grace Hopper").click();
    h.run_steps(2);
    h.get_by_label("Signature validity is UNKNOWN:");
    // Seed a find so the trust path proves it invalidates the view: trust rebuilds
    // the display copy, and the next frame must draw from it without any zoom change.
    h.state_mut().views[0].find = Some(pdfcraft_ui_egui::canvas::Find { matches: vec![(0, 0..5)], ..Default::default() });
    h.get_by_label("Add to trusted certificates").click();
    h.run_steps(3);
    h.get_by_label("Signed and all signatures are valid.");
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().signatures[0].status, SignatureStatus::Valid);
    assert!(s.views[0].find.as_ref().is_some_and(|f| f.matches.is_empty()), "trust invalidates the view without zooming");
    assert!(!s.session.get(s.views[0].id).unwrap().dirty, "trust alone is not an edit");
    // Show certificate: the Certificate Viewer.
    h.get_by_label("Show certificate…").click();
    h.run_steps(3);
    h.get_by_label("Certificate Viewer");
    assert!(h.get_all_by_label_contains("CN=Grace Hopper").count() >= 2, "issued to and by (self-signed)");
    h.run_steps(2);
    h.get_by_label("Details").click();
    h.run_steps(2);
    h.get_by_label("SHA-256 digest");
    h.run_steps(2);
    h.get_by_label("Trust").click();
    h.run_steps(2);

    h.get_by_label("This certificate is in your list of trusted certificates.");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    // View signed version opens the bytes the signature covers as a new document.
    h.get_by_label("View signed version").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.views.len(), 2);
    let v = s.session.get(s.views[1].id).unwrap();
    assert!(v.name.contains("signed version") && v.signatures.iter().any(|x| x.signed));
    assert!(s.views[0].snapshot.is_none());
    assert!(s.views[1].snapshot.as_ref().is_some_and(|snap| snap.kind == pdfcraft_ui_egui::canvas::SnapshotKind::SignedVersion));
    // A repeat click reuses the snapshot tab instead of opening another identical one.
    h.get_by_label("View signed version").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!((s.views.len(), s.active), (2, Some(1)));
    // A repeat from the original tab reuses it too.
    h.state_mut().active = Some(0);
    h.run_steps(1);
    h.get_by_label("View signed version").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!((s.views.len(), s.active), (2, Some(1)));
    // The snapshot tab says what it is.
    h.get_by_label_contains("You are viewing the signed version of");
    // Closing a clean snapshot closes the tab with no save prompt. The bar's own
    // Close is addressed from its message, because the tab chrome has a Close button
    // with the same label.
    let label = h.get_by_label_contains("You are viewing the signed version of");
    let label_row = label.rect().y_range();
    let mut node = label;
    let mut close = None;
    for _ in 0..4 {
        if let Some(pill) = node.query_by_label("Close") {
            close = Some(pill);
            break;
        }
        let Some(up) = node.parent() else { break };
        node = up;
    }
    let close = close.expect("the notice bar holds its own Close");
    // The bar's Close sits in the bar's row and the tab chrome's sits above it. Without
    // this the walk could reach the chrome's button, which closes the same tab, and the
    // test would pass with the bar's own button never pressed.
    assert!(label_row.contains(close.rect().center().y), "the Close found is the bar's own");
    close.click();
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1);
    assert!(h.state().close_request.is_none());
    // Closing a dirty snapshot asks about saving instead of closing at once.
    h.get_by_label("View signed version").click();
    h.run_steps(3);
    let id = h.state().views[1].id;
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Dirty".into() }).unwrap();
    h.state_mut().views[1].close_requested = true;
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 2);
    h.get_by_label("Save changes to “contract_signed (signed version).pdf” before closing?");
    h.get_by_label("Don't save").click();
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1);
    // Trusted certificates and the ID list persist.
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert_eq!((fresh.digital_ids.len(), fresh.session.trusted_certificates().len()), (1, 1));
}

#[test]
fn certifying_without_a_visible_signature() {
    let dir = dir().join("certify");
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness(dir.clone());
    // An ID from a file.
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    assert!(h.state_mut().execute("sign.certify_invisible"));
    h.run_steps(2);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        assert_eq!((d.rect, d.certify), (None, Some(2)));
        d.new_id.source = NewIdSource::File;
        d.new_id.file = p12.into();
        d.new_id.file_password = "test".into();
    }
    h.run_steps(1);
    h.get_by_label("Continue").click();
    h.run_steps(2);
    assert_eq!(h.state().digital_ids.len(), 1);
    // The dialog shrinks onto this step. egui centers an area from the previous frame's size,
    // so give the buttons a frame to land before clicking.
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(2);
    h.get_by_label("Certify as \"Test Signer EC\"");
    h.get_by_label("Permitted actions after certifying");
    h.state_mut().sign_draft.as_mut().unwrap().password = "test".into();
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.sign_draft.as_ref().and_then(|d| d.error.clone()), None);
    let doc = s.session.get(s.views[0].id).unwrap();
    let sig = doc.signatures.iter().find(|x| x.signed).unwrap();
    assert_eq!((sig.certify, sig.visible), (Some(2), false));
    h.get_by_label_contains("Certified by Test Signer EC");
}

#[test]
fn save_as_keeps_a_signed_document_incremental() {
    // #2165: Save As to a new file rewrites unsigned documents in full; a signed one is appended
    // to, or its signature would no longer cover the file.
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let digital_id = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap();
    let mut s = pdfcraft_engine::Session::new();
    let id = s.open("contract.pdf", None, std::sync::Arc::new(FIXTURE.to_vec()), None).unwrap();
    let opts = pdfcraft_engine::SignOptions { page: 0, rect: Some([20.0, 20.0, 180.0, 60.0]), ..Default::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    let out = dir().join("copy.pdf");
    let (bytes, cert) = (signed.to_vec(), digital_id.certificate.clone());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("contract.pdf", None, bytes).expect("opens");
        app.session.set_trusted_certificates(vec![cert]);
        app
    });
    h.run_steps(4);
    let status = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().signatures.iter().find(|x| x.signed).map(|x| x.status)
    };
    assert_eq!(status(&h), Some(SignatureStatus::Valid));
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::As), "a signed document can be saved as a copy");
    let saved = std::fs::read(&out).expect("file written");
    assert!(saved.starts_with(&signed), "the signed revision is kept as it was");
    assert_eq!(status(&h), Some(SignatureStatus::Valid));
}

/// The fixture with an empty signature field across the bottom of the page.
const WITH_FIELD: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [6 0 R] >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Sig /T (Approver) /Rect [20 20 280 80] /P 3 0 R /F 4 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn clicking_an_empty_signature_field_signs_it() {
    let dir = dir().join("field");
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness(dir.clone());
    h.state_mut().open_bytes("form.pdf", None, WITH_FIELD.to_vec()).unwrap();
    h.state_mut().active = Some(1);
    h.run_steps(6);
    let p = {
        let r = h.state().views[1].page_screen_rect(0).expect("on screen");
        pos2(r.left() + 150.0 / 300.0 * r.width(), r.top() + (200.0 - 50.0) / 200.0 * r.height())
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.field.clone()).as_deref(), Some("Approver"));
    let entry = {
        let id = pdfcraft_engine::sign::pkcs12::open(
            &std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap(),
            "test",
        )
        .unwrap();
        h.state_mut().add_digital_id(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12"), &id.certificate)
    };
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.selected = Some(entry);
        d.step = SignStep::SignAs;
        d.password = "test".into();
    }
    h.run_steps(2);
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    let sig = s.session.get(s.views[1].id).unwrap().signatures.iter().find(|x| x.field == "Approver").cloned().unwrap();
    assert!(sig.signed && sig.visible);
}

#[test]
fn the_fill_and_sign_signature_or_an_image_shows_in_a_digital_signature() {
    use pdfcraft_ui_egui::SignGraphic;
    use pdfcraft_ui_egui::fill_sign::SavedSig;
    let dir = dir().join("graphic");
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness(dir.clone());
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let cert = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap().certificate;
    let sign_as = |h: &mut Harness<'static, PdfCraftApp>, rect: [f64; 4]| {
        h.state_mut().start_signing(0, Some(rect), None, None);
        let entry = h.state_mut().add_digital_id(p12, &cert);
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        (d.selected, d.step, d.password) = (Some(entry), SignStep::SignAs, "test".into());
        h.run_steps(2);
    };

    // Without a saved Fill & Sign signature there is nothing to choose.
    sign_as(&mut h, [20.0, 110.0, 280.0, 140.0]);
    h.get_by_label("Graphic");
    assert!(h.get_by_label("Fill & Sign signature").accesskit_node().is_disabled(), "disabled without one");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    // The saved signature (typed here) takes the name's place.
    h.state_mut().signature = Some(SavedSig::Typed("Grace Hopper".into()));
    sign_as(&mut h, [20.0, 110.0, 280.0, 140.0]);
    h.get_by_label("Fill & Sign signature").click();
    h.run_steps(2);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().graphic, SignGraphic::FillSign);
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.sign_draft.as_ref().and_then(|d| d.error.clone()), None);
    let sig = s.session.get(s.views[0].id).unwrap().signatures.iter().find(|x| x.signed).cloned().unwrap();
    assert!(sig.visible && sig.status == SignatureStatus::Unknown, "{:?}", sig.details);
    // Remembered for the next signature, and across a restart.
    assert_eq!(s.sign_graphic, SignGraphic::FillSign);
    sign_as(&mut h, [20.0, 20.0, 280.0, 60.0]);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().graphic, SignGraphic::FillSign);

    // An image (the picker's result is set directly: the file dialog is native).
    let image = {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(60, 20, image::Rgba([200, 0, 0, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        pdfcraft_engine::SignatureImage::from_bytes(png.get_ref()).unwrap()
    };
    h.state_mut().sign_draft.as_mut().unwrap().graphic = SignGraphic::Image(image.clone());
    h.run_steps(2);
    h.get_by_label("Choose image…");
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.sign_draft.as_ref().and_then(|d| d.error.clone()), None);
    let signed = std::fs::read(dir.join("contract_signed.pdf")).unwrap();
    assert!(signed.windows(6).any(|w| w == b"/Image"), "the image is embedded");
    assert_eq!(s.session.get(s.views[0].id).unwrap().signatures.iter().filter(|x| x.signed).count(), 2);
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&s.persist());
    assert_eq!(fresh.sign_graphic, SignGraphic::Image(image));
    // A saved choice that can't be read back falls back to the name.
    fresh.restore(r#"{"sign_graphic": {"Image": "not base64"}}"#);
    assert_eq!(fresh.sign_graphic, SignGraphic::Name);
}

/// The decoded text of every visible signature appearance (Helvetica forms) in a signed file.
fn stamped(path: &std::path::Path) -> String {
    use pdfcraft_cos::{Document, ObjRef, Object};
    let doc = Document::open(std::sync::Arc::new(std::fs::read(path).unwrap())).unwrap();
    let mut text = String::new();
    for num in doc.object_numbers() {
        if let Object::Stream(s) = &*doc.get(ObjRef { num, generation: 0 })
            && s.dict.name(b"Subtype") == Some(b"Form")
        {
            let content = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
            if content.contains("/Helv") {
                text.push_str(&content);
            }
        }
    }
    text
}

/// A signature signed in German is stamped with German labels. In Simplified Chinese, whose
/// labels the signature's font can't draw, it is stamped with English ones and the dialog says
/// so under the preview: signing neither fails nor writes `?` for them.
#[test]
fn the_signature_labels_follow_the_interface_language() {
    use pdfcraft_ui_egui::i18n::{Lang, tr};
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let cert = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap().certificate;
    let note = "The labels are in English: the signature's font can't draw them in this language.";
    for (code, labels, in_english) in [("de", ["(Digital", "(Grund:", "(Datum:"], false), ("zh-hans", ["(Digitally", "(Reason:", "(Date:"], true)] {
        let dir = dir().join(code);
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = harness(dir.clone());
        let lang = Lang::from_code(code).unwrap();
        h.state_mut().set_option("language", code).unwrap();
        h.state_mut().start_signing(0, Some([20.0, 110.0, 280.0, 180.0]), None, None);
        let entry = h.state_mut().add_digital_id(p12, &cert);
        {
            let d = h.state_mut().sign_draft.as_mut().unwrap();
            (d.selected, d.step, d.password, d.reason) = (Some(entry), SignStep::SignAs, "test".into(), "OK".into());
            d.appearance.reason = true;
        }
        h.run_steps(3);
        assert_eq!(h.query_all_by_label(tr(lang, note)).count(), usize::from(in_english), "{code}: the note under the preview");
        field(&h, tr(lang, "Sign")).click();
        h.run_steps(6);
        assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.error.clone()), None, "{code}");
        let content = stamped(&dir.join("contract_signed.pdf"));
        for label in labels {
            assert!(content.contains(label), "{code}: {label}: {content}");
        }
        assert!(!content.contains('?'), "{code}: {content}");
        assert_eq!(content.contains("Digitally") || content.contains("Reason"), in_english, "{code}: {content}");
    }
}

#[test]
fn os_store_identities_are_not_persisted() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    for path in ["windows:Store signer", "keychain:Store signer", "file-id.p12"] {
        app.digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
            path: path.into(),
            name: "Signer".into(),
            issuer: "Test issuer".into(),
            email: String::new(),
            expires: "2030.01.01".into(),
            unusable: None,
            remote: None,
        });
    }
    let saved = app.persist();
    let settings: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(settings["digital_ids"].as_array().unwrap().len(), 1);
    assert_eq!(settings["digital_ids"][0]["path"], "file-id.p12");
    let mut restored = PdfCraftApp::new();
    restored.set_option("language", "en").unwrap();
    restored.restore(&saved);
    assert_eq!(restored.digital_ids.len(), 1);
    assert_eq!(restored.digital_ids[0].path, "file-id.p12");
}

#[test]
fn windows_store_identity_does_not_ask_for_a_file_password() {
    let mut h = harness(dir());
    h.state_mut().start_signing(0, None, None, None);
    // A synthetic picker entry exercises the dialog without a real Windows identity.
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: "windows:No Such Signer".into(),
        name: "Store signer".into(),
        issuer: "Test issuer".into(),
        email: String::new(),
        expires: "2030.01.01".into(),
        unusable: None,
        remote: None,
    });
    let draft = h.state_mut().sign_draft.as_mut().unwrap();
    draft.step = SignStep::Choose;
    draft.selected = Some(0);
    h.run_steps(3);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    h.get_by_label("Sign as \"Store signer\"");
    assert_eq!(h.query_all_by_label("Digital ID password").count(), 0);
    h.get_by_label("The key is in the Windows certificate store, which may ask to allow PdfCraft to use it.");
    h.get_by_label("Sign").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    assert!(h.state().sign_draft.as_ref().unwrap().error.as_ref().unwrap().contains("Windows certificate store"));
}

/// Certificate names are as long as their owners made them: the picker cuts both lines of a
/// row with "…" instead of painting them past the dialog's edge.
#[test]
fn long_digital_id_names_stay_inside_the_picker() {
    let mut h = harness(dir());
    h.state_mut().start_signing(0, None, None, None);
    let long = "a certificate subject much longer than the picker is wide ".repeat(4);
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: "windows:No Such Signer".into(),
        name: long.clone(),
        issuer: long,
        email: String::new(),
        expires: "2030.01.01".into(),
        unusable: None,
        remote: None,
    });
    h.state_mut().sign_draft.as_mut().unwrap().step = SignStep::Choose;
    h.run_steps(3);
    let right = h.get_by_label("Continue").rect().right();
    let lines: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text.contains("much longer") => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 2, "the name and the details line");
    for line in lines {
        assert!(line.galley.elided, "{}", line.galley.job.text);
        assert!(
            line.pos.x + line.galley.size().x <= right,
            "the line ends at {} but the dialog's buttons end at {right}",
            line.pos.x + line.galley.size().x
        );
    }
}

/// A store certificate that can't sign is shown with its reason (so the user learns why an
/// identity Acrobat lists is missing, issue #179) but is never selected or chosen.
#[test]
fn an_unusable_id_is_shown_with_its_reason_and_cannot_be_chosen() {
    let mut h = harness(dir());
    // Not a `windows:` path: those are replaced by the real store listing when signing starts.
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: "store-test:broken".into(),
        name: "Broken signer".into(),
        issuer: String::new(),
        email: String::new(),
        expires: String::new(),
        unusable: Some("its private key can't be opened: the key is missing from its key store (Windows error 0x80090016)".into()),
        remote: None,
    });
    h.state_mut().start_signing(0, None, None, None);
    h.run_steps(3);
    let draft = h.state().sign_draft.as_ref().unwrap();
    assert_eq!(draft.step, SignStep::Choose, "shown, with its reason, rather than skipped to Configure");
    assert_eq!(draft.selected, None);
    h.get_by_label("Broken signer").click();
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().selected, None);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().step, SignStep::Choose);
    // A usable ID after it is the one selected to begin with.
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: "store-test:fine".into(),
        name: "Working signer".into(),
        issuer: "Test issuer".into(),
        email: String::new(),
        expires: "2030.01.01".into(),
        unusable: None,
        remote: None,
    });
    h.state_mut().start_signing(0, None, None, None);
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().selected, Some(1));
}

/// Issue #1396: an imported digital ID can be removed from Sign with a Digital ID, after a
/// confirmation, and its file stays where it was.
#[test]
fn an_imported_digital_id_can_be_removed() {
    let dir = dir().join("remove");
    std::fs::create_dir_all(&dir).unwrap();
    let p12 = dir.join("signer.p12");
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12"), &p12).unwrap();
    let mut h = harness(dir.clone());
    assert!(h.state_mut().execute("sign.certify_invisible"));
    h.run_steps(2);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.new_id.source = NewIdSource::File;
        d.new_id.file = p12.to_string_lossy().into_owned();
        d.new_id.file_password = "test".into();
    }
    h.run_steps(1);
    h.get_by_label("Continue").click();
    h.run_steps(4);
    assert_eq!(h.state().digital_ids.len(), 1);
    assert_eq!(h.state().sign_draft.as_ref().map(|d| d.step), Some(SignStep::Choose));

    // Remove asks first, and Cancel keeps the ID. The dialog changes size with the question;
    // egui centres it from the previous frame's size, so let it settle before each click.
    h.get_by_label("Remove").click();
    h.run_steps(4);
    h.get_by_label("Remove \"Test Signer EC\" from this list? The file itself is not deleted.");
    // The question's Cancel comes before the dialog's own.
    h.get_all_by_label("Cancel").next().unwrap().click();
    h.run_steps(4);
    assert!(h.query_by_label_contains("from this list?").is_none());
    assert_eq!(h.state().digital_ids.len(), 1);

    h.get_by_label("Remove").click();
    h.run_steps(4);
    h.get_by_label("Remove").click();
    h.run_steps(3);
    assert!(h.state().digital_ids.is_empty());
    assert!(p12.exists(), "the file itself is not deleted");
    // With no IDs left the dialog offers to configure one.
    assert_eq!(h.state().sign_draft.as_ref().map(|d| d.step), Some(SignStep::Configure));
    h.get_by_label("Configure a Digital ID for Signing");
}

/// The app's network for remote signatures: a CSC v2 signing service in memory. Requests run in
/// place (`run_inline`), so each answer is in by the next frame.
fn remote_harness(dir: std::path::PathBuf) -> (Harness<'static, PdfCraftApp>, std::sync::Arc<MockService>) {
    let mock = std::sync::Arc::new(MockService::csc_v2().unwrap());
    let mut h = harness(dir);
    h.state_mut().remote_transport = Some(mock.clone());
    h.state_mut().run_inline = true;
    (h, mock)
}

fn type_into(h: &mut Harness<'static, PdfCraftApp>, label: &str, text: &str) {
    field(h, label).click();
    h.run_steps(1);
    field(h, label).type_text(text);
    h.run_steps(1);
}

/// Configure ▸ Remote signature (OTP) for the mock's CSC account, typed as a user would, and
/// Find certificates: the dialog is back at Choose with them listed.
fn find_remote_certificates(h: &mut Harness<'static, PdfCraftApp>) {
    h.state_mut().start_signing(0, Some([20.0, 20.0, 180.0, 60.0]), None, None);
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().step, SignStep::Configure);
    h.get_by_label("Remote signature (OTP)").click();
    h.run_steps(3);
    // Aruba is offered first, with its address filled in; a CSC provider's must be typed.
    assert_eq!(h.state().sign_draft.as_ref().unwrap().new_id.remote.address, pdfcraft_sign::remote::ARSS_ENDPOINT);
    h.get_by_label("Domain");
    h.get_by_label("Other provider (CSC API)").click();
    h.run_steps(3);
    assert!(h.state().sign_draft.as_ref().unwrap().new_id.remote.address.is_empty());
    assert!(h.query_by_label("Domain").is_none(), "only Aruba has a domain");
    type_into(h, "Service address", mock::CSC_V2_URL);
    type_into(h, "Username", mock::USER);
    type_into(h, "Password", mock::PASSWORD);
    h.get_by_label("Find certificates").click();
    h.run_steps(4);
}

#[test]
fn a_remote_csc_identity_is_found_sends_its_code_and_signs_with_pin_and_otp() {
    let dir = dir();
    let (mut h, mock) = remote_harness(dir.clone());
    // A wrong password at Find certificates says so and adds nothing.
    h.state_mut().start_signing(0, None, None, None);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.new_id.source = NewIdSource::Remote;
        d.new_id.remote = RemoteAccountDraft {
            service: Protocol::Csc,
            address: mock::CSC_V2_URL.into(),
            user: mock::USER.into(),
            domain: String::new(),
            password: "not it".into(),
        };
    }
    // The dialog grows to the remote fields over a few frames (egui centres it from the last
    // frame's size): let it settle before clicking.
    h.run_steps(5);
    h.get_by_label("Find certificates").click();
    h.run_steps(3);
    h.get_by_label("the signing service rejected the user name or password");
    assert!(h.state().digital_ids.is_empty());
    h.state_mut().dialog = None;
    h.run_steps(2);

    find_remote_certificates(&mut h);
    let s = h.state();
    let d = s.sign_draft.as_ref().unwrap();
    assert_eq!((d.step, d.selected), (SignStep::Choose, Some(0)));
    assert!(d.new_id.remote.password.is_empty(), "the service password is dropped once used");
    // The RSA key (PIN and a code sent by SMS) and the P-256 key (a code from an app); not the disabled one.
    let ids: Vec<_> = s.digital_ids.iter().map(|e| (e.path.as_str(), e.name.as_str(), e.remote)).collect();
    let rsa = format!("remote:csc?url=https%3A%2F%2Frssp.example%2Fcsc%2Fv2&user=mario.rossi&credential={}", mock::RSA);
    let p256 = format!("remote:csc?url=https%3A%2F%2Frssp.example%2Fcsc%2Fv2&user=mario.rossi&credential={}", mock::P256);
    assert_eq!(
        ids,
        [
            (rsa.as_str(), "Mario Rossi", Some(RemoteNeeds { pin: true, otp: RemoteOtp::Sent, confirm: false, unless_code: false })),
            (p256.as_str(), "Mario Rossi", Some(RemoteNeeds { pin: false, otp: RemoteOtp::Typed, confirm: false, unless_code: false }))
        ]
    );
    assert_eq!(h.get_all_by_label("Mario Rossi").count(), 2);

    h.get_by_label("Continue").click();
    h.run_steps(3);
    h.get_by_label("Sign as \"Mario Rossi\"");
    assert_eq!(h.query_all_by_label("Digital ID password").count(), 0, "the service password, not a file's");
    for label in ["Password", "PIN", "One-time code (OTP)"] {
        field(&h, label);
    }
    // Send code: the service sends a new code.
    let before = mock.current_otp();
    type_into(&mut h, "Password", mock::PASSWORD);
    h.get_by_label("Send code").click();
    h.run_steps(3);
    h.get_by_label("Code sent");
    assert_ne!(mock.current_otp(), before);
    type_into(&mut h, "PIN", mock::PIN);
    type_into(&mut h, "One-time code (OTP)", &mock.current_otp());
    h.get_by_label("Sign").click();
    h.run_steps(6);

    let s = h.state();
    assert_eq!(s.dialog, None, "{:?}", s.sign_draft.as_ref().and_then(|d| d.error.clone()));
    assert!(dir.join("contract_signed.pdf").exists());
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.name, "contract_signed.pdf");
    assert!(!doc.dirty && doc.can_undo().is_none(), "saved and marked signed");
    let sig = doc.signatures.iter().find(|x| x.signed).expect("signed");
    assert_eq!((sig.status, sig.page), (SignatureStatus::Unknown, Some(0)));
    assert_eq!(sig.certificate.as_ref().map(|c| &c.raw), mock.certificate(mock::RSA).map(|c| c.raw).as_ref());
    assert_eq!(s.right, Some(RightPanel::Signatures));
    // The file on disk is the one in the session, and trusting the service's certificate validates it.
    assert_eq!(std::fs::read(dir.join("contract_signed.pdf")).unwrap(), *doc.bytes);
    h.state_mut().trust_certificate(mock.certificate(mock::RSA).unwrap());
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().signatures[0].status, SignatureStatus::Valid);
}

#[test]
fn a_wrong_one_time_code_is_shown_and_the_dialog_stays_until_the_right_one() {
    let dir = dir();
    let (mut h, mock) = remote_harness(dir.clone());
    find_remote_certificates(&mut h);
    // The P-256 key: a code from an app, so no PIN and nothing to send.
    h.get_all_by_label("Mario Rossi").nth(1).unwrap().click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert!(h.query_by_label("PIN").is_none() && h.query_by_label("Send code").is_none());
    type_into(&mut h, "Password", mock::PASSWORD);
    type_into(&mut h, "One-time code (OTP)", "000000");
    h.get_by_label("Sign").click();
    h.run_steps(4);
    h.get_by_label("the signing service rejected the OTP (it may have expired)");
    let s = h.state();
    assert_eq!((s.dialog, s.sign_draft.as_ref().map(|d| d.step)), (Some(Dialog::Sign), Some(SignStep::SignAs)));
    assert!(!dir.join("contract_signed.pdf").exists());
    assert!(!s.session.get(s.views[0].id).unwrap().is_signed());
    // The next code signs (the wrong try spent the one before).
    h.state_mut().sign_draft.as_mut().unwrap().otp = mock.current_otp();
    h.run_steps(1);
    h.get_by_label("Sign").click();
    h.run_steps(6);
    assert_eq!(h.state().dialog, None);
    assert!(dir.join("contract_signed.pdf").exists());
    let s = h.state();
    let sig = s.session.get(s.views[0].id).unwrap().signatures.iter().find(|x| x.signed).cloned().expect("signed");
    assert_eq!(sig.certificate.map(|c| c.raw), mock.certificate(mock::P256).map(|c| c.raw));
}

/// A transport that holds the requests to one method (`hold`) until they are let through, so
/// the dialog can be seen while the service works.
struct Held {
    mock: std::sync::Arc<MockService>,
    hold: &'static str,
    open: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl HttpTransport for Held {
    fn post(&self, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, SignError> {
        if url.ends_with(self.hold) {
            for _ in 0..3000 {
                if self.open.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        self.mock.post(url, headers, body)
    }
}

#[test]
fn while_the_service_works_the_dialog_says_so_and_waits() {
    let dir = dir();
    let (mut h, mock) = remote_harness(dir.clone());
    // Find certificates.
    let listed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    h.state_mut().remote_transport = Some(std::sync::Arc::new(Held { mock: mock.clone(), hold: "credentials/list", open: listed.clone() }));
    h.state_mut().run_inline = false;
    h.state_mut().start_signing(0, Some([20.0, 20.0, 180.0, 60.0]), None, None);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.new_id.source = NewIdSource::Remote;
        d.new_id.remote = RemoteAccountDraft {
            service: Protocol::Csc,
            address: mock::CSC_V2_URL.into(),
            user: mock::USER.into(),
            domain: String::new(),
            password: mock::PASSWORD.into(),
        };
    }
    h.run_steps(5);
    h.get_by_label("Find certificates").click();
    h.run_steps(3);
    h.get_by_label("Looking for certificates…");
    assert!(h.get_by_label("Find certificates").accesskit_node().is_disabled());
    assert!(h.get_by_label("Use a Digital ID from a file").accesskit_node().is_disabled(), "the choice waits too");
    listed.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..400 {
        h.run_steps(1);
        if !h.state().digital_ids.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().digital_ids.len(), 2);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().step, SignStep::Choose);
    h.run_steps(3);
    // Send code (the RSA key's code comes by SMS, which this service sends on getChallenge).
    let sent = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    h.state_mut().remote_transport = Some(std::sync::Arc::new(Held { mock: mock.clone(), hold: "credentials/getChallenge", open: sent.clone() }));
    h.get_by_label("Continue").click();
    h.run_steps(3);
    h.state_mut().sign_draft.as_mut().unwrap().password = mock::PASSWORD.into();
    h.run_steps(4);
    h.get_by_label("Send code").click();
    h.run_steps(3);
    h.get_by_label("Sending the code…");
    for button in ["Send code", "Sign", "Back"] {
        assert!(h.get_by_label(button).accesskit_node().is_disabled(), "{button}");
    }
    sent.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..400 {
        h.run_steps(1);
        if h.query_by_label("Code sent").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Code sent");
    assert!(h.query_by_label("Sending the code…").is_none());
    h.get_by_label("Back").click();
    h.run_steps(3);
    // Sign.
    let open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    h.state_mut().remote_transport = Some(std::sync::Arc::new(Held { mock: mock.clone(), hold: "signatures/signHash", open: open.clone() }));
    h.state_mut().run_inline = false;
    h.get_all_by_label("Mario Rossi").nth(1).unwrap().click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.password = mock::PASSWORD.into();
        d.otp = mock.current_otp();
    }
    h.run_steps(1);
    h.get_by_label("Sign").click();
    h.run_steps(3);
    h.get_by_label("Signing…");
    assert!(h.get_by_label("Sign").accesskit_node().is_disabled() && h.get_by_label("Back").accesskit_node().is_disabled());
    // What is being signed is settled: nothing in the dialog changes, and no picker opens.
    for control in ["Choose image…", "Name", "Signer's name"] {
        assert!(h.get_by_label(control).accesskit_node().is_disabled(), "{control}");
    }
    assert!(field(&h, "Reason").accesskit_node().is_disabled() && field(&h, "One-time code (OTP)").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Cancel").accesskit_node().is_disabled(), "the dialog can always be closed");
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..400 {
        h.run_steps(1);
        if h.state().dialog.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let s = h.state();
    assert_eq!(s.dialog, None, "{:?}", s.sign_draft.as_ref().and_then(|d| d.error.clone()));
    assert!(dir.join("contract_signed.pdf").exists());
    assert!(s.session.get(s.views[0].id).unwrap().is_signed());
}

/// An answer that comes back too late is never saved: not over an edit made meanwhile (it isn't
/// in the signed file), and not into a dialog closed meanwhile.
#[test]
fn a_late_signature_is_not_saved_over_an_edit_or_into_a_closed_dialog() {
    let dir = dir();
    let (mut h, mock) = remote_harness(dir.clone());
    find_remote_certificates(&mut h);
    let open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    h.state_mut().remote_transport = Some(std::sync::Arc::new(Held { mock: mock.clone(), hold: "signatures/signHash", open: open.clone() }));
    h.state_mut().run_inline = false;
    h.get_all_by_label("Mario Rossi").nth(1).unwrap().click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    let sign = |h: &mut Harness<'static, PdfCraftApp>| {
        {
            let d = h.state_mut().sign_draft.as_mut().unwrap();
            d.password = mock::PASSWORD.into();
            d.otp = mock.current_otp();
        }
        h.run_steps(4);
        h.get_by_label("Sign").click();
        h.run_steps(3);
        h.get_by_label("Signing…");
    };
    // An edit while the service signs: the signed file lacks it, so it isn't saved.
    sign(&mut h);
    let id = h.state().views[0].id;
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Changed".into() }).unwrap();
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..400 {
        h.run_steps(1);
        if h.state().sign_draft.as_ref().is_none_or(|d| d.error.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(3);
    h.get_by_label("The document changed while it was being signed, so the signature wasn't saved.");
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    assert!(!dir.join("contract_signed.pdf").exists());
    assert!(!h.state().session.get(id).unwrap().is_signed() && h.state().session.get(id).unwrap().dirty, "the edit is kept");
    // The dialog closed while the service signs: nothing is saved when it answers.
    open.store(false, std::sync::atomic::Ordering::SeqCst);
    let before = mock.requests().len();
    sign(&mut h);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..400 {
        if mock.requests().iter().skip(before).any(|r| r.url.ends_with("signatures/signHash")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The worker checks the signature and answers just after: give it time to.
    for _ in 0..50 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(mock.requests().iter().skip(before).any(|r| r.url.ends_with("signatures/signHash")), "the service did sign");
    assert_eq!(h.state().dialog, None);
    assert!(!dir.join("contract_signed.pdf").exists());
    assert!(!h.state().session.get(id).unwrap().is_signed());
}

#[test]
fn without_a_network_remote_ids_are_greyed_with_the_reason() {
    let mut h = harness(dir());
    let reference = format!("remote:csc?url=https%3A%2F%2Frssp.example%2Fcsc%2Fv2&user=mario.rossi&credential={}", mock::RSA);
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: reference,
        name: "Mario Rossi".into(),
        issuer: "Mock Trust Services".into(),
        email: String::new(),
        expires: "2035.01.01".into(),
        unusable: None,
        remote: Some(RemoteNeeds { pin: true, otp: RemoteOtp::Sent, confirm: false, unless_code: false }),
    });
    h.state_mut().start_signing(0, None, None, None);
    h.run_steps(3);
    let d = h.state().sign_draft.as_ref().unwrap();
    assert_eq!((d.step, d.selected), (SignStep::Choose, None), "shown, but not chosen");
    let reason = "Signing service  ·  Remote signing isn't available in this build";
    assert!(h.output().shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == reason)), "the row says why");
    h.get_by_label("Mario Rossi").click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(2);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().step, SignStep::Choose);
    // Configure offers it greyed, saying why.
    h.get_by_label("Configure New Digital ID").click();
    h.run_steps(3);
    h.get_by_label("Remote signing isn't available in this build");
    assert!(h.get_by_label("Remote signature (OTP)").accesskit_node().is_disabled());
    h.get_by_label("Remote signature (OTP)").click();
    h.run_steps(2);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().new_id.source, NewIdSource::Create);
    // Reached anyway (automation): Find certificates stays off, and Send code and Sign say why.
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.new_id.source = NewIdSource::Remote;
        d.step = SignStep::Configure;
    }
    h.run_steps(2);
    assert!(h.get_by_label("Find certificates").accesskit_node().is_disabled());
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        (d.step, d.selected, d.password, d.otp) = (SignStep::SignAs, Some(0), "pw".into(), "123456".into());
    }
    h.run_steps(3);
    for button in ["Send code", "Sign"] {
        h.get_by_label(button).click();
        h.run_steps(3);
        assert_eq!(h.state().sign_draft.as_ref().unwrap().error.as_deref(), Some("Remote signing isn't available in this build"), "{button}");
        h.get_by_label("Remote signing isn't available in this build");
        h.state_mut().sign_draft.as_mut().unwrap().error = None;
        h.run_steps(2);
    }
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().is_signed());
}

#[test]
fn remote_ids_persist_with_what_they_ask_for_but_never_a_secret() {
    let (mut h, mock) = remote_harness(dir());
    find_remote_certificates(&mut h);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    // Secrets typed but not used: still nowhere in the settings.
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.password = mock::PASSWORD.into();
        d.pin = mock::PIN.into();
        d.otp = "917263".into();
    }
    h.run_steps(1);
    let saved = h.state().persist();
    for secret in [mock::PASSWORD, mock::PIN, "917263"] {
        assert!(!saved.contains(secret), "{secret:?} in {saved}");
    }
    let settings: serde_json::Value = serde_json::from_str(&saved).unwrap();
    let ids = settings["digital_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0]["remote"], serde_json::json!({ "pin": true, "otp": "sent" }));
    assert_eq!(ids[1]["remote"], serde_json::json!({ "pin": false, "otp": "typed" }));
    // After a restart the IDs are back, and Sign as asks for the same things.
    let mut fresh = harness(dir());
    fresh.state_mut().remote_transport = Some(mock.clone());
    fresh.state_mut().restore(&saved);
    assert_eq!(fresh.state().digital_ids, h.state().digital_ids);
    fresh.state_mut().start_signing(0, None, None, None);
    fresh.run_steps(3);
    fresh.get_by_label("Continue").click();
    fresh.run_steps(3);
    for label in ["Password", "PIN", "One-time code (OTP)", "Send code"] {
        fresh.get_by_label(label);
    }
    // A remote ID is removed like a file ID; its certificate stays with the service.
    fresh.get_by_label("Back").click();
    fresh.run_steps(3);
    fresh.get_by_label("Remove").click();
    fresh.run_steps(4);
    fresh.get_by_label("Remove \"Mario Rossi\" from this list? The certificate stays with the signing service.");
    fresh.get_by_label("Remove").click();
    fresh.run_steps(3);
    assert_eq!(fresh.state().digital_ids.len(), 1);
}

/// What is typed for one digital ID never goes to another: not a digital ID file's password to a
/// signing service, and not one remote ID's PIN, code or "Code sent" to the next.
#[test]
fn secrets_typed_for_one_id_never_reach_another() {
    use base64::Engine as _;
    const FILE_PASSWORD: &str = "only-for-the-p12-file";
    let (mut h, mock) = remote_harness(dir());
    find_remote_certificates(&mut h);
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let id = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap();
    let file = h.state_mut().add_digital_id(p12, &id.certificate);
    let file_name = h.state().digital_ids[file].name.clone();
    h.run_steps(3);
    // The file's password, typed in its Sign as, then Back to the service's RSA key.
    h.get_by_label(&file_name).click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    type_into(&mut h, "Digital ID password", FILE_PASSWORD);
    h.get_by_label("Back").click();
    h.run_steps(3);
    h.get_all_by_label("Mario Rossi").next().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().password, "", "the file's password is not the service's");
    h.get_by_label("Send code").click();
    h.run_steps(3);
    h.get_by_label("the signing service rejected the user name or password");
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{}:{FILE_PASSWORD}", mock::USER));
    for r in mock.requests() {
        let sent = format!("{:?} {}", r.headers, r.body_text());
        assert!(!sent.contains(FILE_PASSWORD) && !sent.contains(&basic), "{} carried the file's password", r.url);
    }
    // One remote ID's PIN, code and Code sent, then Back to the other.
    type_into(&mut h, "Password", mock::PASSWORD);
    h.get_by_label("Send code").click();
    h.run_steps(3);
    h.get_by_label("Code sent");
    type_into(&mut h, "PIN", mock::PIN);
    type_into(&mut h, "One-time code (OTP)", &mock.current_otp());
    h.get_by_label("Back").click();
    h.run_steps(3);
    h.get_all_by_label("Mario Rossi").nth(1).unwrap().click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    let d = h.state().sign_draft.as_ref().unwrap();
    assert_eq!((d.password.as_str(), d.pin.as_str(), d.otp.as_str(), d.otp_note.as_deref()), ("", "", "", None));
    assert!(h.query_by_label("Code sent").is_none(), "no code was sent for this one");
    // Back to the same ID keeps what was typed for it.
    type_into(&mut h, "Password", mock::PASSWORD);
    h.get_by_label("Back").click();
    h.run_steps(3);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().password, mock::PASSWORD);
}

/// Escape, or a click beside the dialog, closes Sign like Cancel: the password, PIN, code and the
/// account password being configured are gone with it.
#[test]
fn closing_the_sign_dialog_any_way_forgets_its_secrets() {
    let (mut h, _) = remote_harness(dir());
    find_remote_certificates(&mut h);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    type_into(&mut h, "Password", mock::PASSWORD);
    type_into(&mut h, "PIN", mock::PIN);
    type_into(&mut h, "One-time code (OTP)", "917263");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    // A field takes the first Escape (it gives up focus); the dialog the next.
    if h.state().dialog.is_some() {
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
    }
    assert_eq!(h.state().dialog, None);
    assert!(h.state().sign_draft.is_none(), "the secrets stayed in the closed dialog's draft");
    // Configure ▸ Remote signature, the account password typed, then a click beside the dialog.
    h.state_mut().start_signing(0, None, None, None);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.step = SignStep::Configure;
        d.new_id.source = NewIdSource::Remote;
        d.new_id.remote.password = mock::PASSWORD.into();
    }
    h.run_steps(5);
    let beside = pos2(8.0, 890.0);
    h.hover_at(beside);
    h.run_steps(1);
    h.drag_at(beside);
    h.run_steps(1);
    h.drop_at(beside);
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    assert!(h.state().sign_draft.is_none());
}

/// A certificate whose signatures are approved on the user's phone says so while the service
/// waits for that, instead of a bare "Signing…". One whose phone is the alternative to a code
/// doesn't when a code was typed: no confirmation is asked then.
#[test]
fn a_signature_confirmed_on_another_device_says_so_while_it_waits() {
    for unless_code in [false, true] {
        let dir = dir();
        let (mut h, mock) = remote_harness(dir.clone());
        find_remote_certificates(&mut h);
        let p256 = h.state().digital_ids[1].remote.unwrap();
        h.state_mut().digital_ids[1].remote = Some(RemoteNeeds { confirm: true, unless_code, ..p256 });
        let open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        h.state_mut().remote_transport = Some(std::sync::Arc::new(Held { mock: mock.clone(), hold: "signatures/signHash", open: open.clone() }));
        h.state_mut().run_inline = false;
        h.get_all_by_label("Mario Rossi").nth(1).unwrap().click();
        h.run_steps(2);
        h.get_by_label("Continue").click();
        h.run_steps(3);
        {
            let d = h.state_mut().sign_draft.as_mut().unwrap();
            d.password = mock::PASSWORD.into();
            d.otp = mock.current_otp();
        }
        h.run_steps(1);
        h.get_by_label("Sign").click();
        h.run_steps(3);
        let (said, not_said) = ("Signing… Confirm the signature on your other device.", "Signing…");
        let (said, not_said) = if unless_code { (not_said, said) } else { (said, not_said) };
        h.get_by_label(said);
        assert!(h.query_by_label(not_said).is_none(), "unless_code {unless_code}");
        open.store(true, std::sync::atomic::Ordering::SeqCst);
        for _ in 0..400 {
            h.run_steps(1);
            if h.state().dialog.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(h.state().dialog, None);
        assert!(dir.join("contract_signed.pdf").exists());
        // Kept with the ID; an ID without it writes nothing new to the settings.
        let saved: serde_json::Value = serde_json::from_str(&h.state().persist()).unwrap();
        assert_eq!(saved["digital_ids"][0]["remote"], serde_json::json!({ "pin": true, "otp": "sent" }));
        let mut kept = serde_json::json!({ "pin": false, "otp": "typed", "confirm": true });
        if unless_code {
            kept["unless_code"] = serde_json::json!(true);
        }
        assert_eq!(saved["digital_ids"][1]["remote"], kept);
    }
}

/// A PIN and a code by SMS or from an app, the app's first: the code Send code had sent signs,
/// under the SMS the service sent it for, not the app's (which the service would refuse it under).
#[test]
fn a_code_sent_by_sms_signs_where_the_app_comes_first() {
    let dir = dir();
    let (mut h, mock) = remote_harness(dir.clone());
    mock.set_auth(
        mock::RSA,
        "PIN AND (APP XOR SMS)",
        serde_json::json!([{ "type": "Password", "id": "PIN", "label": "PIN" }, { "type": "ChallengeResponse", "id": "SMS", "label": "SMS code" },
                           { "type": "Password", "id": "APP", "generator": "totp", "label": "App code" }]),
    );
    find_remote_certificates(&mut h);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    type_into(&mut h, "Password", mock::PASSWORD);
    h.get_by_label("Send code").click();
    h.run_steps(3);
    h.get_by_label("Code sent");
    type_into(&mut h, "PIN", mock::PIN);
    type_into(&mut h, "One-time code (OTP)", &mock.current_otp());
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.dialog, None, "{:?}", s.sign_draft.as_ref().and_then(|d| d.error.clone()));
    assert!(dir.join("contract_signed.pdf").exists());
    let authorized = mock.requests().into_iter().rfind(|r| r.url.ends_with("/credentials/authorize")).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&authorized.body).unwrap();
    assert_eq!(body["authData"][1]["id"], "SMS");
}

/// Aruba, the default service: its domain is required and reaches the account, its code is sent
/// by SMS, and the signature it makes is saved and validates.
#[test]
fn an_aruba_identity_is_found_with_its_domain_sends_the_sms_and_signs() {
    let dir = dir();
    let mock = std::sync::Arc::new(MockService::arss().unwrap());
    let mut h = harness(dir.clone());
    h.state_mut().remote_transport = Some(mock.clone());
    h.state_mut().run_inline = true;
    h.state_mut().start_signing(0, Some([20.0, 20.0, 180.0, 60.0]), None, None);
    h.run_steps(3);
    h.get_by_label("Remote signature (OTP)").click();
    h.run_steps(5);
    // Aruba's address is filled in and can be changed: the mock answers at another one.
    assert_eq!(h.state().sign_draft.as_ref().unwrap().new_id.remote.address, pdfcraft_sign::remote::ARSS_ENDPOINT);
    h.state_mut().sign_draft.as_mut().unwrap().new_id.remote.address = mock::ARSS_URL.into();
    h.run_steps(1);
    type_into(&mut h, "Username", mock::USER);
    type_into(&mut h, "Password", mock::PASSWORD);
    // Aruba refuses an account without its domain ("Invalid Parameters"): Find certificates, and
    // Enter in the password, wait for it, and the empty field says where it comes from.
    assert!(h.get_by_label("Find certificates").accesskit_node().is_disabled());
    let hint = "Required, given by Aruba with the account";
    assert!(h.output().shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == hint)), "the hint shows");
    field(&h, "Password").click();
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    h.get_by_label("Find certificates").click();
    h.run_steps(3);
    assert!(mock.requests().is_empty(), "nothing was asked without the domain");
    assert!(h.state().digital_ids.is_empty());
    assert_eq!(h.state().sign_draft.as_ref().unwrap().error, None, "nor tried");
    type_into(&mut h, "Domain", mock::DOMAIN);
    h.run_steps(2);
    assert!(!h.get_by_label("Find certificates").accesskit_node().is_disabled());
    h.get_by_label("Find certificates").click();
    h.run_steps(4);
    let account = mock.account();
    let rsa = pdfcraft_sign::remote::reference(&account, mock::RSA);
    let s = h.state();
    let d = s.sign_draft.as_ref().unwrap();
    assert_eq!((d.step, d.selected, d.error.as_deref()), (SignStep::Choose, Some(0), None));
    let ids: Vec<_> = s.digital_ids.iter().map(|e| (e.path.clone(), e.remote)).collect();
    let needs = Some(RemoteNeeds { pin: false, otp: RemoteOtp::Sent, confirm: false, unless_code: false });
    assert_eq!(ids, [(rsa.clone(), needs), (pdfcraft_sign::remote::reference(&account, mock::P256), needs)]);
    assert!(rsa.contains("domain=frtest"), "{rsa}");

    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert!(h.query_by_label("PIN").is_none(), "Aruba asks for no PIN");
    type_into(&mut h, "Password", mock::PASSWORD);
    let before = mock.current_otp();
    h.get_by_label("Send code").click();
    h.run_steps(3);
    h.get_by_label("Code sent");
    assert_ne!(mock.current_otp(), before, "the SMS carries a new code");
    // A wrong code: Aruba's own reason, and the dialog stays.
    type_into(&mut h, "One-time code (OTP)", "000000");
    h.get_by_label("Sign").click();
    h.run_steps(4);
    h.get_by_label("the signing service refused to sign: Invalid OTP (code 0001)");
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    h.state_mut().sign_draft.as_mut().unwrap().otp = mock.current_otp();
    h.run_steps(1);
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.dialog, None, "{:?}", s.sign_draft.as_ref().and_then(|d| d.error.clone()));
    let doc = s.session.get(s.views[0].id).unwrap();
    let sig = doc.signatures.iter().find(|x| x.signed).expect("signed");
    assert_eq!(sig.status, SignatureStatus::Unknown);
    assert_eq!(sig.certificate.as_ref().map(|c| &c.raw), mock.certificate(mock::RSA).map(|c| c.raw).as_ref());
    assert_eq!(std::fs::read(dir.join("contract_signed.pdf")).unwrap(), *doc.bytes);
    // Every request named the account with its domain; only the digest was sent to sign.
    let signhash: Vec<_> = mock.requests().into_iter().filter(|r| r.body_text().contains("signhash")).collect();
    assert_eq!(signhash.len(), 2);
    assert!(signhash.iter().all(|r| r.body_text().contains("<typeOtpAuth>frtest</typeOtpAuth>")));
}

/// A signing service's refusals read in the interface language: the sign crate's sentence for a
/// wrong password, PIN or code, no network, an Aruba account without its domain (what the
/// service or the network adds stays as it came). Signing against the mock keeps the catalog's
/// sentences the sign crate's own.
#[test]
fn a_signing_services_refusals_read_in_the_interface_language() {
    let (mut h, mock) = remote_harness(dir());
    find_remote_certificates(&mut h);
    h.state_mut().set_option("language", "it").unwrap();
    h.run_steps(3);
    // A wrong code (the P-256 key's comes from an app).
    h.get_all_by_label("Mario Rossi").nth(1).unwrap().click();
    h.run_steps(2);
    h.get_by_label("Continua").click();
    h.run_steps(3);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        (d.password, d.otp) = (mock::PASSWORD.into(), "000000".into());
    }
    h.run_steps(1);
    h.get_by_label("Firma").click();
    h.run_steps(4);
    h.get_by_label("il servizio di firma ha rifiutato l'OTP (potrebbe essere scaduto)");
    // A wrong password.
    h.state_mut().sign_draft.as_mut().unwrap().password = "not it".into();
    h.run_steps(1);
    h.get_by_label("Firma").click();
    h.run_steps(4);
    h.get_by_label("il servizio di firma ha rifiutato il nome utente o la password");
    // A wrong PIN with the code (the RSA key's): CSC v2 doesn't say which of the two was wrong.
    h.get_by_label("Indietro").click();
    h.run_steps(3);
    h.get_all_by_label("Mario Rossi").next().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Continua").click();
    h.run_steps(3);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        (d.password, d.pin, d.otp) = (mock::PASSWORD.into(), "0000".into(), mock.current_otp());
    }
    h.run_steps(1);
    h.get_by_label("Firma").click();
    h.run_steps(4);
    h.get_by_label("il servizio di firma ha rifiutato il PIN o l'OTP (l'OTP potrebbe essere scaduto)");
    // No network: the frame is translated, the network's reason is as it came.
    h.state_mut().remote_transport = Some(std::sync::Arc::new(Unreachable));
    h.get_by_label("Firma").click();
    h.run_steps(4);
    h.get_by_label("il servizio di firma non è raggiungibile (connection refused)");
    // Aruba's refusal: the frame, then Aruba's own words.
    let arss = std::sync::Arc::new(MockService::arss().unwrap());
    h.state_mut().remote_transport = Some(arss.clone());
    h.state_mut().dialog = None;
    h.run_steps(2);
    let reference = pdfcraft_sign::remote::reference(&arss.account(), mock::RSA);
    h.state_mut().digital_ids = vec![pdfcraft_ui_egui::DigitalIdEntry {
        path: reference,
        name: "Mario Rossi".into(),
        issuer: "Mock Trust Services".into(),
        email: String::new(),
        expires: "2035.01.01".into(),
        unusable: None,
        remote: Some(RemoteNeeds { pin: false, otp: RemoteOtp::Sent, confirm: false, unless_code: false }),
    }];
    h.state_mut().start_signing(0, None, None, None);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        (d.step, d.password, d.otp) = (SignStep::SignAs, mock::PASSWORD.into(), "000000".into());
    }
    h.run_steps(3);
    h.get_by_label("Firma").click();
    h.run_steps(4);
    h.get_by_label("il servizio di firma ha rifiutato di firmare: Invalid OTP (code 0001)");
    // An Aruba ID saved without its domain: said before anything is sent.
    let before = arss.requests().len();
    let account = pdfcraft_sign::remote::Account { domain: None, ..arss.account() };
    h.state_mut().digital_ids[0].path = pdfcraft_sign::remote::reference(&account, mock::RSA);
    h.state_mut().sign_draft.as_mut().unwrap().error = None;
    h.run_steps(2);
    h.get_by_label("Invia codice").click();
    h.run_steps(4);
    h.get_by_label("un account Aruba richiede il suo dominio (quello che Aruba fornisce con l'account): inseriscilo");
    assert_eq!(arss.requests().len(), before);
}

/// A network that never connects.
struct Unreachable;

impl HttpTransport for Unreachable {
    fn post(&self, _url: &str, _headers: &[(&str, &str)], _body: &[u8]) -> Result<HttpResponse, SignError> {
        Err(SignError::Crypto("connection refused".into()))
    }
}

/// Settings this version can't wholly read lose only what can't be read: a remote ID whose needs
/// it doesn't know (a later version's, or edited by hand) asks for everything, and an entry that
/// isn't one is left out alone, not with every other digital ID.
#[test]
fn unreadable_saved_ids_lose_only_what_cannot_be_read() {
    let reference = format!("remote:csc?url=https%3A%2F%2Frssp.example%2Fcsc%2Fv2&user=mario.rossi&credential={}", mock::RSA);
    let id = |path: &str, remote: serde_json::Value| {
        let mut e = serde_json::json!({ "path": path, "name": "Mario Rossi", "issuer": "Mock", "email": "", "expires": "2035.01.01" });
        if !remote.is_null() {
            e["remote"] = remote;
        }
        e
    };
    let saved = serde_json::json!({ "digital_ids": [
        id("/ids/mario.p12", serde_json::Value::Null),
        id(&reference, serde_json::json!({ "pin": false, "otp": "push" })),
        id(&reference, serde_json::json!({})),
        id(&reference, serde_json::json!("sms")),
        42,
        { "path": "/ids/nameless.p12" },
        id(&reference, serde_json::json!({ "pin": false, "otp": "typed", "confirm": true, "later": 1 })),
    ] });
    let mut h = harness(dir());
    h.state_mut().restore(&saved.to_string());
    let ids: Vec<_> = h.state().digital_ids.iter().map(|e| (e.path.as_str(), e.remote)).collect();
    let everything = Some(RemoteNeeds { pin: true, otp: RemoteOtp::Sent, confirm: false, unless_code: false });
    assert_eq!(
        ids,
        [
            ("/ids/mario.p12", None),
            (reference.as_str(), everything),
            (reference.as_str(), everything),
            (reference.as_str(), everything),
            (reference.as_str(), Some(RemoteNeeds { pin: false, otp: RemoteOtp::Typed, confirm: true, unless_code: false })),
        ]
    );
    // And they are saved again, so the next start has them too.
    let again: serde_json::Value = serde_json::from_str(&h.state().persist()).unwrap();
    assert_eq!(again["digital_ids"].as_array().map(Vec::len), Some(5));
}
