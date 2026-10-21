//! Image drops use Add image, while document and page-assembly drops keep their destinations.
mod support;

use egui_kittest::Harness;
use pdfcraft_ui_egui::{LeftPanel, PdfCraftApp};

#[derive(Debug)]
struct Dropped(std::path::PathBuf, Result<Vec<u8>, String>);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        self.1.clone()
    }
}

fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 120, 60);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&[40, 140, 220].repeat(120 * 60)).unwrap();
    bytes
}

fn harness(documents: usize) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for i in 0..documents {
            let bytes = app.session.create_blank(300.0, 400.0, 2).unwrap();
            app.open_bytes(&format!("doc{i}.pdf"), None, bytes.to_vec()).unwrap();
        }
        app
    });
    h.run_steps(4);
    h
}

fn drop_files(h: &mut Harness<'static, PdfCraftApp>, files: Vec<Dropped>) {
    for file in files {
        h.input_mut().dropped_files.push(std::sync::Arc::new(file));
    }
    h.run_steps(4);
}

#[test]
fn image_drop_inserts_on_the_active_page_and_supports_undo_redo() {
    let mut h = harness(2);
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(4);
    drop_files(&mut h, vec![Dropped("photo.PNG".into(), Ok(png()))]);
    assert_eq!(h.state().views.len(), 2, "an image must not open another tab");
    let app = h.state();
    assert!(app.session.get(app.views[0].id).unwrap().added().is_empty());
    let doc = app.session.get(app.views[1].id).unwrap();
    assert_eq!(doc.added().len(), 1, "the dropped image must become content of the active document");
    let image = &doc.added()[0];
    assert_eq!(image.page, 1);
    assert!(matches!(image.content, pdfcraft_engine::AddedContent::Image(_)));
    assert!(doc.dirty);
    assert_eq!(app.left, LeftPanel::Tool("edit"));
    assert!(app.left_open);
    assert!(app.views[1].content.selected.is_some(), "ready to move or resize");
    let pdfcraft_engine::AddedContent::Image(image) = &image.content else { panic!("expected image") };
    assert!(((image.rect[0] + image.rect[2]) / 2.0 - 150.0).abs() < 0.01);
    assert!(((image.rect[1] + image.rect[3]) / 2.0 - 200.0).abs() < 0.01);
    if let Ok(path) = std::env::var("PDFCRAFT_IMAGE_DROP_SHOT") {
        support::wait_until(&mut h, "image render", |app| !app.render_pending());
        h.render().unwrap().save(path).unwrap();
    }
    assert!(h.state_mut().execute("edit.undo"));
    assert!(h.state().session.get(h.state().views[1].id).unwrap().added().is_empty());
    assert!(h.state_mut().execute("edit.redo"));
    assert_eq!(h.state().session.get(h.state().views[1].id).unwrap().added().len(), 1);
    let app = h.state_mut();
    let id = app.views[1].id;
    let saved = app.session.save_bytes(id).unwrap();
    let reopened = app.session.open("reopened.pdf", None, saved, None).unwrap();
    assert_eq!(app.session.get(reopened).unwrap().added()[0].page, 1);
}

#[test]
fn image_drop_without_a_document_opens_a_tab() {
    let mut h = harness(0);
    drop_files(&mut h, vec![Dropped("photo.png".into(), Ok(png()))]);
    support::wait_until(&mut h, "image document", |app| app.views.len() == 1);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.name, "photo.pdf");
    assert_eq!(doc.info.pages.len(), 1);
    assert!(doc.added().is_empty(), "the image is converted to a new document");
}

#[test]
fn image_drop_batch_continues_after_bad_files_and_pdf_still_opens_a_tab() {
    let mut h = harness(1);
    let pdf = h.state().session.create_blank(300.0, 400.0, 1).unwrap();
    drop_files(
        &mut h,
        vec![
            Dropped("other.pdf".into(), Ok(pdf.to_vec())),
            Dropped("first.png".into(), Ok(png())),
            Dropped("broken.png".into(), Ok(b"broken".to_vec())),
            Dropped("unreadable.png".into(), Err("read failed".into())),
            Dropped("last.jpeg".into(), Ok(png())),
        ],
    );
    support::wait_until(&mut h, "dropped PDF", |app| app.views.len() == 2);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.added().len(), 2, "both readable images belong to the original document");
    assert_eq!(doc.info.pages.len(), 2);
}

#[test]
fn an_image_dropped_on_the_organize_grid_still_inserts_a_page() {
    let mut h = harness(1);
    h.state_mut().views[0].organize = true;
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("photo.png".into(), Ok(png()))]);
    h.run_steps(12);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 3);
    assert!(doc.added().is_empty());
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn every_add_image_extension_is_routed_to_content() {
    let mut h = harness(1);
    // The importer sniffs bytes; the drop router must accept the picker's entire extension set.
    let files = ["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"].map(|ext| Dropped(format!("photo.{ext}").into(), Ok(png())));
    drop_files(&mut h, files.into());
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().added().len(), 10);
}

#[test]
fn image_drop_reads_an_absolute_path_from_disk() {
    let path = std::env::temp_dir().join(format!("pdfcraft-image-drop-{}.png", std::process::id()));
    std::fs::write(&path, png()).unwrap();
    let mut h = harness(1);
    drop_files(&mut h, vec![Dropped(path.clone(), Err("use the native path".into()))]);
    std::fs::remove_file(path).unwrap();
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().added().len(), 1);
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn bad_image_drops_report_an_error_without_changing_the_document() {
    for bytes in [Ok(b"broken".to_vec()), Err("read failed".into())] {
        let mut h = harness(1);
        drop_files(&mut h, vec![Dropped("broken.png".into(), bytes)]);
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        assert!(doc.added().is_empty());
        assert!(!doc.dirty);
        assert!(h.state().toast.is_some());
        assert_eq!(h.state().views.len(), 1);
        assert!(h.state().image_import.is_none());
    }
}

fn browser_drop(h: &Harness<'static, PdfCraftApp>) -> pdfcraft_ui_egui::FileRequest {
    h.state().file_request(pdfcraft_ui_egui::FilePurpose::AddImages { page: 0 }, vec![("first.png".into(), png()), ("last.png".into(), png())])
}

#[test]
fn browser_image_drop_keeps_its_page_and_inserts_the_whole_batch() {
    let mut h = harness(1);
    let request = browser_drop(&h);
    h.state_mut().views[0].current = 1;
    h.state_mut().requests.lock().unwrap().push(request);
    h.run_steps(4);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.added().len(), 2);
    assert!(doc.added().iter().all(|a| a.page == 0));
}

#[test]
fn browser_image_drop_refuses_a_switched_or_edited_document() {
    for switch in [true, false] {
        let mut h = harness(2);
        let request = browser_drop(&h);
        if switch {
            h.state_mut().active = Some(0);
        } else {
            h.state_mut().add_image("intervening.png".into(), png());
        }
        h.state_mut().requests.lock().unwrap().push(request);
        h.run_steps(4);
        assert!(h.state().session.get(h.state().views[0].id).unwrap().added().is_empty());
        assert_eq!(h.state().session.get(h.state().views[1].id).unwrap().added().len(), usize::from(!switch));
        assert!(h.state().toast.as_ref().unwrap().0.contains("document changed"));
    }
}
