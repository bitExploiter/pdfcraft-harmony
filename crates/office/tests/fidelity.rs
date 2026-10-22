//! Conversion fidelity on a corpus: each PDF in `tests/corpus` (made by WordCraft or DeckCraft from
//! a source in `corpus/src`, see `corpus/README.md`) is converted, and the words of the result are
//! compared, in order, with the words of the source's own text (`<name>.txt`). The share of the
//! source's words found in order (longest common subsequence) may not fall below the file's floor
//! in `corpus/floor.toml`: floors only go up. A second measure (`<file>/paragraphs`) counts the
//! source's paragraphs (table cells and outline lines count as paragraphs) that come out as one
//! whole paragraph, neither split nor run into another. `FIDELITY_WRITE=1` writes the measured
//! values as the new floors.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pdfcraft_office::{Options, Target, convert};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// One part of a zip package, inflated.
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

/// The text of the `<{tag}>` elements in `xml` (`w:t` in Word, `a:t` in PowerPoint), one
/// paragraph per line, entities decoded.
fn xml_text(xml: &str, tag: &str, para: &str) -> String {
    let (open, close, para_end) = (format!("<{tag}"), format!("</{tag}>"), format!("</{para}>"));
    let mut out = String::new();
    let mut rest = xml;
    loop {
        // `<w:t>` or `<w:t xml:space=…>`, not `<w:tbl>` or `<w:txbxContent>`.
        let next_t = rest.match_indices(&open).map(|(i, _)| i).find(|&i| rest[i + open.len()..].starts_with(['>', ' ']));
        let next_p = rest.find(&para_end);
        match (next_t, next_p) {
            (Some(t), p) if p.is_none_or(|p| t < p) => {
                let body = &rest[t..];
                let start = body.find('>').map_or(body.len(), |g| g + 1);
                let end = body.find(&close).unwrap_or(body.len());
                out.push_str(&body[start.min(end)..end]);
                out.push(' ');
                rest = &body[end..];
            }
            (_, Some(p)) => {
                out.push('\n');
                rest = &rest[p + para_end.len()..];
            }
            _ => break,
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string()).filter(|w| !w.is_empty()).collect()
}

/// The length of the longest common subsequence of `a` and `b`.
fn lcs(a: &[String], b: &[String]) -> usize {
    let mut row = vec![0usize; b.len() + 1];
    for x in a {
        let mut diag = 0;
        for (j, y) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = if x == y { diag + 1 } else { up.max(row[j]) };
            diag = up;
        }
    }
    row[b.len()]
}

fn floors(path: &Path) -> BTreeMap<String, f64> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .filter_map(|(k, v)| Some((k.trim().trim_matches('"').to_string(), v.trim().parse().ok()?)))
        .collect()
}

struct Measure {
    name: String,
    words: usize,
    found: usize,
    extra: usize,
    paras: usize,
    paras_found: usize,
}

fn share(found: usize, of: usize) -> f64 {
    if of == 0 { 1.0 } else { found as f64 / of as f64 }
}

impl Measure {
    fn ratio(&self) -> f64 {
        share(self.found, self.words)
    }
    fn para_ratio(&self) -> f64 {
        share(self.paras_found, self.paras)
    }
}

/// A paragraph's words, joined, without a list number (`1.`) before them: what two paragraphs are
/// compared by. Bullets aren't words, and a source's plain text may leave numbering out.
fn para_key(text: &str) -> String {
    let t = text.trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    let t = if (1..=3).contains(&digits) && t[digits..].starts_with(['.', ')']) { &t[digits + 1..] } else { t };
    words(t).join(" ")
}

/// The source's paragraphs: its lines, a table row's cells each on its own.
fn source_paras(truth: &str) -> Vec<String> {
    truth.lines().flat_map(|l| l.split('\t')).map(para_key).filter(|p| !p.is_empty()).collect()
}

fn measure(name: &str, pdf: &[u8], truth: &str, target: Target) -> Measure {
    let out = convert(pdf, target, &Options::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let text = match target {
        Target::Docx => xml_text(&zip_entry(&out.bytes, "word/document.xml").unwrap_or_default(), "w:t", "w:p"),
        _ => (1..=out.page_count)
            .map(|n| xml_text(&zip_entry(&out.bytes, &format!("ppt/slides/slide{n}.xml")).unwrap_or_default(), "a:t", "a:p"))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let (want, got) = (words(truth), words(&text));
    let found = lcs(&want, &got);
    let mut out_paras: Vec<String> = text.lines().map(para_key).filter(|p| !p.is_empty()).collect();
    // `FIDELITY_SHOW=<file>` prints that file's paragraphs as converted, to see what went wrong.
    if std::env::var("FIDELITY_SHOW").is_ok_and(|v| v == name) {
        out_paras.iter().for_each(|p| println!("¶ {p}"));
    }
    let src = source_paras(truth);
    let paras_found = src
        .iter()
        .filter(|p| match out_paras.iter().position(|o| o == *p) {
            Some(i) => {
                out_paras.remove(i);
                true
            }
            None => false,
        })
        .count();
    Measure { name: name.to_string(), words: want.len(), found, extra: got.len().saturating_sub(found), paras: src.len(), paras_found }
}

#[test]
fn fidelity_floor() {
    let dir = corpus();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".pdf").map(str::to_string))
        .collect();
    names.sort();
    assert!(names.len() >= 12, "the corpus has {} PDFs", names.len());
    let mut results = Vec::new();
    for name in &names {
        let pdf = std::fs::read(dir.join(format!("{name}.pdf"))).unwrap();
        let truth = std::fs::read_to_string(dir.join(format!("{name}.txt"))).unwrap_or_else(|_| panic!("{name}.txt: the source's text"));
        results.push(measure(&format!("{name}.docx"), &pdf, &truth, Target::Docx));
        if name.starts_with("deck-") {
            results.push(measure(&format!("{name}.pptx"), &pdf, &truth, Target::PptxPages));
        }
    }
    println!("{:<24} {:>6} {:>6} {:>6} {:>7} {:>6} {:>6} {:>7}", "file", "words", "found", "extra", "ratio", "paras", "whole", "ratio");
    for m in &results {
        println!(
            "{:<24} {:>6} {:>6} {:>6} {:>7.3} {:>6} {:>6} {:>7.3}",
            m.name,
            m.words,
            m.found,
            m.extra,
            m.ratio(),
            m.paras,
            m.paras_found,
            m.para_ratio()
        );
    }
    let measures: Vec<(String, f64)> =
        results.iter().flat_map(|m| [(m.name.clone(), m.ratio()), (format!("{}/paragraphs", m.name), m.para_ratio())]).collect();
    let floor_path = dir.join("floor.toml");
    if std::env::var_os("FIDELITY_WRITE").is_some() {
        let mut out = String::from(
            "# Lowest accepted share of each source's words found, in order, after converting its PDF.\n# Only raise these: `FIDELITY_WRITE=1 cargo test -p pdfcraft-office --test fidelity` writes the measured values.\n",
        );
        for (name, v) in &measures {
            out.push_str(&format!("\"{name}\" = {:.3}\n", (v * 1000.0).floor() / 1000.0));
        }
        std::fs::write(&floor_path, out).unwrap();
        return;
    }
    let floor = floors(&floor_path);
    let mut below = Vec::new();
    for (name, v) in &measures {
        match floor.get(name) {
            Some(f) if v + 1e-9 < *f => below.push(format!("{name}: {v:.3} < floor {f:.3}")),
            Some(_) => {}
            None => below.push(format!("{name}: no floor in floor.toml")),
        }
    }
    assert!(below.is_empty(), "fidelity fell: {below:#?}");
}

#[test]
fn lcs_counts_words_in_order() {
    let w = |s: &str| words(s);
    assert_eq!(lcs(&w("a b c d"), &w("a c b d")), 3);
    assert_eq!(lcs(&w("Öğrenci, no."), &w("öğrenci no Öğrenci no")), 2);
    assert_eq!(
        xml_text("<w:p><w:r><w:t xml:space=\"preserve\">A &amp; B</w:t></w:r></w:p><w:p><w:r><w:t>C</w:t></w:r></w:p>", "w:t", "w:p").trim(),
        "A & B \nC"
    );
}
