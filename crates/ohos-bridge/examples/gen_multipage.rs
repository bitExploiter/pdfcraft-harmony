//! Writes a multi-page sample PDF (default 20 pages) for viewer tests: a coloured heading band,
//! body text lines, shapes and an RGB image on every page.
//! Usage: cargo run -p pdfcraft-ohos-bridge --example gen_multipage -- <out.pdf> [pages]
use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pages: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let bold = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica-Bold" });
    let mut px = Vec::with_capacity(64 * 64 * 3);
    for y in 0..64u32 {
        for x in 0..64u32 {
            px.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, 160]);
        }
    }
    let img = doc.add_object(Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 64, "Height" => 64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
        px,
    ));
    let palette = [(0.06, 0.47, 0.39), (0.11, 0.39, 0.88), (0.9, 0.3, 0.1), (0.5, 0.2, 0.7), (0.2, 0.2, 0.25)];
    let lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt.";
    let mut kids = Vec::new();
    for n in 1..=pages {
        let (r, g, b) = palette[(n as usize - 1) % palette.len()];
        let mut ops = vec![
            Operation::new("rg", vec![r.into(), g.into(), b.into()]),
            Operation::new("re", vec![50.into(), 720.into(), 495.into(), 70.into()]),
            Operation::new("f", vec![]),
            Operation::new("BT", vec![]),
            Operation::new("rg", vec![1.into(), 1.into(), 1.into()]),
            Operation::new("Tf", vec!["F2".into(), 28.into()]),
            Operation::new("Td", vec![70.into(), 745.into()]),
            Operation::new("Tj", vec![Object::string_literal(format!("PdfCraft - Page {n} of {pages}"))]),
            Operation::new("ET", vec![]),
            Operation::new("BT", vec![]),
            Operation::new("rg", vec![0.13.into(), 0.13.into(), 0.15.into()]),
            Operation::new("Tf", vec!["F1".into(), 11.into()]),
            Operation::new("TL", vec![16.into()]),
            Operation::new("Td", vec![50.into(), 680.into()]),
        ];
        for i in 0..14 {
            ops.push(Operation::new("Tj", vec![Object::string_literal(format!("{} ({n}.{})", lorem, i + 1))]));
            ops.push(Operation::new("T*", vec![]));
        }
        ops.push(Operation::new("ET", vec![]));
        ops.extend([
            Operation::new("rg", vec![r.into(), g.into(), b.into()]),
            Operation::new("re", vec![50.into(), 200.into(), 200.into(), 200.into()]),
            Operation::new("f", vec![]),
            Operation::new("q", vec![]),
            Operation::new("cm", vec![220.into(), 0.into(), 0.into(), 220.into(), 300.into(), 190.into()]),
            Operation::new("Do", vec!["Im1".into()]),
            Operation::new("Q", vec![]),
            Operation::new("RG", vec![0.6.into(), 0.6.into(), 0.65.into()]),
            Operation::new("w", vec![1.into()]),
            Operation::new("m", vec![50.into(), 120.into()]),
            Operation::new("l", vec![545.into(), 120.into()]),
            Operation::new("S", vec![]),
            Operation::new("BT", vec![]),
            Operation::new("rg", vec![0.4.into(), 0.4.into(), 0.45.into()]),
            Operation::new("Tf", vec!["F1".into(), 10.into()]),
            Operation::new("Td", vec![280.into(), 100.into()]),
            Operation::new("Tj", vec![Object::string_literal(format!("{n}"))]),
            Operation::new("ET", vec![]),
        ]);
        let content = doc.add_object(Stream::new(dictionary! {}, Content { operations: ops }.encode().unwrap()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font, "F2" => bold }, "XObject" => dictionary! { "Im1" => img } },
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        kids.push(Object::from(page));
    }
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => pages as i64 }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    doc.compress();
    doc.save(&args[1]).unwrap();
    println!("wrote {} ({pages} pages)", args[1]);
}