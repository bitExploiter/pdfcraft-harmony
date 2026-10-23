//! Host check: writes a content-rich sample PDF (text, filled shapes, an RGB image) and renders
//! page 1 with pdfcraft-render to PNG, reporting how many pixels are not white.
//! Usage: cargo run -p pdfcraft-ohos-bridge --example sample_and_render -- <out.pdf> <out.png>
use std::sync::Arc;

use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

fn sample_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    // 64x64 RGB gradient image, uncompressed.
    let mut px = Vec::with_capacity(64 * 64 * 3);
    for y in 0..64u32 {
        for x in 0..64u32 {
            px.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, 160]);
        }
    }
    let img_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 64, "Height" => 64,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
        },
        px,
    ));
    let ops = vec![
        Operation::new("rg", vec![0.06.into(), 0.47.into(), 0.39.into()]),
        Operation::new("re", vec![40.into(), 680.into(), 515.into(), 100.into()]),
        Operation::new("f", vec![]),
        Operation::new("BT", vec![]),
        Operation::new("rg", vec![1.into(), 1.into(), 1.into()]),
        Operation::new("Tf", vec!["F1".into(), 36.into()]),
        Operation::new("Td", vec![60.into(), 715.into()]),
        Operation::new("Tj", vec![Object::string_literal("PdfCraft en HarmonyOS")]),
        Operation::new("ET", vec![]),
        Operation::new("BT", vec![]),
        Operation::new("rg", vec![0.into(), 0.into(), 0.into()]),
        Operation::new("Tf", vec!["F1".into(), 18.into()]),
        Operation::new("Td", vec![40.into(), 630.into()]),
        Operation::new("Tj", vec![Object::string_literal("Motor Rust renderizado de forma nativa.")]),
        Operation::new("ET", vec![]),
        Operation::new("rg", vec![0.9.into(), 0.3.into(), 0.1.into()]),
        Operation::new("re", vec![40.into(), 380.into(), 220.into(), 200.into()]),
        Operation::new("f", vec![]),
        Operation::new("q", vec![]),
        Operation::new("cm", vec![240.into(), 0.into(), 0.into(), 240.into(), 300.into(), 360.into()]),
        Operation::new("Do", vec!["Im1".into()]),
        Operation::new("Q", vec![]),
        Operation::new("RG", vec![0.1.into(), 0.2.into(), 0.8.into()]),
        Operation::new("w", vec![6.into()]),
        Operation::new("m", vec![40.into(), 300.into()]),
        Operation::new("l", vec![555.into(), 100.into()]),
        Operation::new("S", vec![]),
    ];
    let content_id = doc.add_object(Stream::new(dictionary! {}, Content { operations: ops }.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "Resources" => dictionary! {
            "Font" => dictionary! { "F1" => font_id },
            "XObject" => dictionary! { "Im1" => img_id },
        },
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! {
        "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
    }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = match args.get(3) {
        Some(input) => std::fs::read(input).unwrap(),
        None => sample_pdf(),
    };
    std::fs::write(&args[1], &bytes).unwrap();
    let mut r = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let out = r.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let rgba: Vec<u8> = out.rgba.into_words().iter().flat_map(|w| w.to_ne_bytes()).collect();
    let non_white = rgba.chunks_exact(4).filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250).count();
    println!("pages={} size={}x{} non_white={} first_px={:?}", r.page_count(), out.width, out.height, non_white, &rgba[..4]);
    image::save_buffer(&args[2], &rgba, out.width, out.height, image::ColorType::Rgba8).unwrap();
}
