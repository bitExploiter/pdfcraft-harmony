//! pdfcraft-office opens password-protected PDFs the way the engine does: the user or the owner
//! password, every algorithm Protect writes, and owner-only files without a password.

use pdfcraft_engine::{Edit, Protection, Session};
use pdfcraft_office::{Error, Options, Target, convert};

fn plain() -> Vec<u8> {
    let body = "BT /F1 14 Tf 72 760 Td (Secret text) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn protect(p: Protection) -> Vec<u8> {
    let mut s = Session::new();
    let id = s.open("plain.pdf", None, std::sync::Arc::new(plain()), None).unwrap();
    s.apply(id, Edit::Protect(p)).unwrap();
    s.save_bytes(id).unwrap().as_ref().clone()
}

fn both(algorithm: pdfcraft_cos::Algorithm) -> Vec<u8> {
    protect(Protection { open_password: Some("kullanici".into()), permissions_password: Some("sahip".into()), algorithm, ..Protection::default() })
}

fn with(password: &str) -> Options {
    Options { password: Some(password.into()), ..Options::default() }
}

fn html_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

const ALGORITHMS: [pdfcraft_cos::Algorithm; 4] =
    [pdfcraft_cos::Algorithm::Rc4_40, pdfcraft_cos::Algorithm::Rc4_128, pdfcraft_cos::Algorithm::Aes128, pdfcraft_cos::Algorithm::Aes256];

#[test]
fn needs_password() {
    for a in ALGORITHMS {
        assert_eq!(convert(&both(a), Target::Html, &Options::default()), Err(Error::NeedsPassword), "{a:?}");
    }
}

#[test]
fn wrong_password() {
    for a in ALGORITHMS {
        assert_eq!(convert(&both(a), Target::Html, &with("yanlis")), Err(Error::WrongPassword), "{a:?}");
    }
}

#[test]
fn user_password_opens() {
    for a in ALGORITHMS {
        let out = convert(&both(a), Target::Html, &with("kullanici")).unwrap_or_else(|e| panic!("{a:?}: {e}"));
        assert!(html_text(&out.bytes).contains("Secret text"), "{a:?}");
    }
}

#[test]
fn owner_password_opens() {
    for a in ALGORITHMS {
        let out = convert(&both(a), Target::Html, &with("sahip")).unwrap_or_else(|e| panic!("{a:?}: {e}"));
        assert!(html_text(&out.bytes).contains("Secret text"), "{a:?}");
    }
}

#[test]
fn owner_only_opens_without_prompt() {
    let pdf = protect(Protection { permissions_password: Some("sahip".into()), ..Protection::default() });
    let out = convert(&pdf, Target::Html, &Options::default()).unwrap();
    assert!(html_text(&out.bytes).contains("Secret text"));
}
