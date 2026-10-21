use super::*;
use pdfcraft_testkit::Pdf;

/// Helvetica in WinAnsi with codes 0x80–0x83 set to ğ ş ı İ, the way PDF writers add the Turkish
/// letters WinAnsi lacks.
const TURKISH_FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding \
     /BaseEncoding /WinAnsiEncoding /Differences [128 /gbreve /scedilla /dotlessi /Idotaccent] >> >>";
const BOLD_FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding << /Type /Encoding \
     /BaseEncoding /WinAnsiEncoding /Differences [128 /gbreve /scedilla /dotlessi /Idotaccent] >> >>";

/// A 2×2 RGB picture.
fn picture() -> Vec<u8> {
    pdfcraft_testkit::stream_obj("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8", [200u8; 12])
}

/// One page per entry: its content stream; pages draw fonts /F1 (regular), /F2 (bold) and
/// picture /Im1. Pages are 595×842 (A4).
fn pdf(pages: &[&[u8]]) -> Vec<u8> {
    let n = pages.len();
    // 1 catalog, 2 pages, 3 font, 4 bold font, 5 picture, then page + content pairs.
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 6 + 2 * i)).collect();
    let mut b = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")))
        .obj(TURKISH_FONT)
        .obj(BOLD_FONT)
        .obj(picture());
    for (i, content) in pages.iter().enumerate() {
        b = b
            .obj(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents {} 0 R \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R >> /XObject << /Im1 5 0 R >> >> >>",
                7 + 2 * i
            ))
            .stream("", content);
    }
    b.build()
}

/// "Başlık" 24 pt bold, then "Gövde metni ğüşıöç İ" 11 pt.
const HEADING_PAGE: &[u8] =
    b"BT /F2 24 Tf 72 760 Td (Ba\x81l\x82k) Tj ET BT /F1 11 Tf 72 720 Td (G\xF6vde metni \x80\xFC\x81\x82\xF6\xE7 \x83) Tj ET";
/// A paragraph and a picture.
const PICTURE_PAGE: &[u8] = b"BT /F1 11 Tf 72 760 Td (Ikinci sayfa) Tj ET q 200 0 0 150 72 500 cm /Im1 Do Q";
/// Only a picture: a scanned page.
const SCAN_PAGE: &[u8] = b"q 451 0 0 698 72 72 cm /Im1 Do Q";

fn sample() -> Vec<u8> {
    pdf(&[HEADING_PAGE, PICTURE_PAGE])
}

/// The text of one file in a zip (Office files deflate their parts).
fn zip_entry(zip: &[u8], name: &str) -> Option<String> {
    use std::io::Read;
    let mut at = 0;
    while let Some(h) = zip.get(at..at + 30) {
        if h[..4] != *b"PK\x03\x04" {
            return None;
        }
        let le16 = |i: usize| usize::from(u16::from_le_bytes([h[i], h[i + 1]]));
        let size = u32::from_le_bytes([h[18], h[19], h[20], h[21]]) as usize;
        let (method, name_len, extra) = (le16(8), le16(26), le16(28));
        let start = at + 30 + name_len + extra;
        let data = zip.get(start..start + size)?;
        if zip.get(at + 30..at + 30 + name_len)? == name.as_bytes() {
            let mut out = String::new();
            if method == 8 {
                flate2::read::DeflateDecoder::new(data).read_to_string(&mut out).ok()?;
            } else {
                out = String::from_utf8_lossy(data).into_owned();
            }
            return Some(out);
        }
        at = start + size;
    }
    None
}

fn docx_text(bytes: &[u8]) -> String {
    zip_entry(bytes, "word/document.xml").unwrap_or_default()
}

#[test]
fn docx_has_text_in_order() {
    let out = convert(&sample(), Target::Docx, &Options::default()).unwrap();
    assert_eq!(out.page_count, 2);
    let xml = docx_text(&out.bytes);
    let heading = xml.find("Başlık").expect("heading");
    let body = xml.find("Gövde metni ğüşıöç İ").expect("body with Turkish letters");
    assert!(heading < body);
}

#[test]
fn pptx_one_slide_per_page() {
    let out = convert(&sample(), Target::Pptx, &Options::default()).unwrap();
    assert!(zip_entry(&out.bytes, "ppt/slides/slide1.xml").is_some());
    assert!(zip_entry(&out.bytes, "ppt/slides/slide2.xml").is_some());
    assert!(zip_entry(&out.bytes, "ppt/slides/slide3.xml").is_none());
}

#[test]
fn page_selection() {
    let out = convert(&sample(), Target::Docx, &Options { pages: Some(vec![1]), ..Options::default() }).unwrap();
    assert_eq!(out.page_count, 2);
    let xml = docx_text(&out.bytes);
    assert!(!xml.contains("Başlık"));
    assert!(xml.contains("Ikinci sayfa"));
}

#[test]
fn bad_page_range() {
    let r = convert(&sample(), Target::Docx, &Options { pages: Some(vec![9]), ..Options::default() });
    assert!(matches!(r, Err(Error::BadPageRange(_))), "{r:?}");
    assert_eq!(parse_page_range("1-3, 5", 6).unwrap(), vec![0, 1, 2, 4]);
    assert_eq!(parse_page_range("3,1,3", 6).unwrap(), vec![2, 0]);
    for bad in ["0", "4-2", "7", "x", "", "1-", "99999999999999999999"] {
        assert!(matches!(parse_page_range(bad, 6), Err(Error::BadPageRange(_))), "{bad}");
    }
}

#[test]
fn not_pdf() {
    assert!(matches!(convert(b"hello", Target::Docx, &Options::default()), Err(Error::NotPdf)));
    assert!(is_pdf(b"\xEF\xBB\xBF%PDF-1.7\n"));
    assert!(is_pdf(&sample()));
    assert!(!is_pdf(b"PK\x03\x04"));
}

#[test]
fn too_large() {
    let mut big = vec![b' '; MAX_INPUT_BYTES + 1];
    big[..8].copy_from_slice(b"%PDF-1.7");
    assert!(matches!(convert(&big, Target::Docx, &Options::default()), Err(Error::TooLarge)));
}

#[test]
fn image_only_page_reported() {
    let out = convert(&pdf(&[HEADING_PAGE, SCAN_PAGE]), Target::Docx, &Options::default()).unwrap();
    assert_eq!(out.image_only_pages, vec![1]);
    assert!(out.truncated_pages.is_empty());
}

#[test]
fn title_option_wins() {
    let out = convert(&sample(), Target::Html, &Options { title: Some("Ödev".into()), ..Options::default() }).unwrap();
    assert!(String::from_utf8_lossy(&out.bytes).contains("<title>Ödev</title>"));
}

#[test]
fn hostile_bytes_never_panic() {
    let good = sample();
    let mut variants = Vec::new();
    for step in 1..=8 {
        variants.push(good[..good.len() * step / 8].to_vec());
    }
    for seed in 0..192usize {
        let mut v = good.clone();
        for i in (seed % 97..v.len()).step_by(97) {
            v[i] = v[i].wrapping_add(seed as u8).rotate_left(3);
        }
        variants.push(v);
    }
    for v in variants {
        for target in [Target::Docx, Target::Pptx] {
            let _ = convert(&v, target, &Options::default());
        }
    }
}

#[test]
fn page_count_without_converting() {
    assert_eq!(page_count(&sample(), None), Ok(2));
    assert_eq!(page_count(b"hello", None), Err(Error::NotPdf));
}
