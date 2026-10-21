//! System print spooler supplied by craft-print. PDF composition and permission checks
//! stay in PdfCraft; these adapters retain the application's print-error surface.

pub use craft_print::{
    Duplex, HANDLED_OPTIONS, HAS_PRINTER_PREFERENCES, Job, Printer, PrinterOption, Scaling, lp_args, lpstat_command, parse_lpoptions, parse_lpstat,
    parse_lpstat_e, parse_ppd, parse_windows_printers, ppd_text, printers, printers_parsed,
};

/// The driver's job options for a printer (CUPS: its PPD, with the queue's current defaults);
/// empty when it has none or on other platforms. Reads files and runs `lpoptions`, so call it
/// when the user asks, not every frame.
pub fn printer_options(printer: &str) -> Vec<PrinterOption> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        printer_options_via(printer, std::path::Path::new("/etc/cups/ppd"), std::process::Command::new("lpoptions"))
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        craft_print::printer_options(printer)
    }
}

/// Read a destination's options with the PPD directory and command supplied, so tests need
/// neither a system queue nor a physical printer. CUPS instances share their queue's PPD but
/// have their own defaults, queried with the complete `queue/instance` destination.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub(crate) fn printer_options_via(printer: &str, ppd_dir: &std::path::Path, mut lpoptions: std::process::Command) -> Vec<PrinterOption> {
    let Some(queue) = queue_of(printer) else { return Vec::new() };
    let mut options = read_ppd(ppd_dir, queue).map(|ppd| parse_ppd(&ppd)).unwrap_or_default();
    let current = lpoptions.args(["-p", printer, "-l"]).output().map(|o| parse_lpoptions(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default();
    for (key, value) in current {
        if let Some(o) = options.iter_mut().find(|o| o.key == key)
            && o.choices.iter().any(|(c, _)| *c == value)
        {
            o.default = value;
        }
    }
    options
}

/// The queue of a CUPS destination (`queue` or `queue/instance`) when both parts are names a
/// PPD can be looked up for. Neither component is a path: dot components, backslashes and any
/// additional separator are rejected before I/O.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn queue_of(printer: &str) -> Option<&str> {
    let (queue, instance) = printer.split_once('/').map_or((printer, None), |(q, i)| (q, Some(i)));
    let valid = |name: &str| {
        !name.is_empty()
            && name.len() <= 64
            && !matches!(name, "." | "..")
            && name.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b'\\' | b':' | b'=' | b'"'))
    };
    (valid(queue) && instance.is_none_or(valid)).then_some(queue)
}

/// The queue's PPD as text, at most 16 MiB of it; `None` when the queue has none.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn read_ppd(ppd_dir: &std::path::Path, queue: &str) -> Option<String> {
    use std::io::Read as _;
    const PPD_CAP: u64 = 16 << 20;
    let file = std::fs::File::open(ppd_dir.join(format!("{queue}.ppd"))).ok()?;
    let mut bytes = Vec::new();
    file.take(PPD_CAP).read_to_end(&mut bytes).ok()?;
    Some(ppd_text(&bytes))
}

/// A paper size the printer's driver offers, listed in the Print dialog after the standard
/// sizes: on CUPS one `*PaperDimension` of the queue's PPD (a label printer's 4 × 6 in, an
/// office printer's envelopes).
#[derive(Clone, Debug, PartialEq)]
pub struct PrinterPaper {
    /// The driver's keyword for it (`Letter`, `w288h432`): what the job's `media` names.
    pub key: String,
    /// What the driver calls it (`US Letter`); the keyword when it gives no name.
    pub label: String,
    /// Width and height in points as the driver feeds it: a label's width is across the feed,
    /// so a 2.25 × 1.25 in label and a 1.25 × 2.25 in one are different stock.
    pub size: (f64, f64),
    /// Whether it is the driver's current default (CUPS: the PPD's `*DefaultPageSize`): the
    /// stock a label printer is set up for, an office printer's tray paper. The Print dialog
    /// starts on it.
    pub default: bool,
}

/// The paper sizes a printer's driver offers (CUPS: its PPD's `*PaperDimension` entries, in the
/// PPD's order); empty when it names none, when the destination is unknown, and on other
/// platforms. Reads a file, so call it when a printer is chosen, not every frame.
pub fn printer_papers(printer: &str) -> Vec<PrinterPaper> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        printer_papers_in(printer, std::path::Path::new("/etc/cups/ppd"))
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        let _ = printer;
        Vec::new()
    }
}

/// [`printer_papers`] with the PPD directory supplied, so tests need no system queue.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub(crate) fn printer_papers_in(printer: &str, ppd_dir: &std::path::Path) -> Vec<PrinterPaper> {
    queue_of(printer).and_then(|queue| read_ppd(ppd_dir, queue)).map(|ppd| parse_ppd_papers(&ppd)).unwrap_or_default()
}

/// The largest side a printer paper may have, in points: PDF's own page limit (200 inches).
const MAX_PAPER_SIDE: f64 = 14_400.0;
/// How many papers a PPD may put in the list (PPDs are text files from the printer vendor).
const MAX_PAPERS: usize = 400;

/// The paper sizes a PPD offers: its `*PaperDimension key/label: "width height"` entries in
/// file order and as written, each named by its own label, else its `*PageSize` label, else the
/// keyword, the `*DefaultPageSize` one marked as the default. Custom-size placeholders,
/// translations (`*fr.PaperDimension`), repeated keywords and sizes outside 1 pt to 200 in are
/// left out. Lenient: malformed lines are skipped.
pub fn parse_ppd_papers(ppd: &str) -> Vec<PrinterPaper> {
    let keyword = |s: &str| !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b':' | b'=' | b'"'));
    // `*Prefix key/label: value` → (key, label, value), the label falling back to the key.
    let entry = |line: &str, prefix: &str| -> Option<(String, String, String)> {
        let (head, value) = line.trim_end_matches('\r').strip_prefix(prefix)?.split_once(':')?;
        let head = head.trim();
        let (key, label) = match head.split_once('/') {
            Some((k, l)) if !l.trim().is_empty() => (k.trim(), l.trim()),
            Some((k, _)) => (k.trim(), k.trim()),
            None => (head, head),
        };
        keyword(key).then(|| (key.to_string(), label.to_string(), value.trim().to_string()))
    };
    let mut names: std::collections::BTreeMap<String, String> = Default::default();
    for line in ppd.lines() {
        if let Some((key, label, _)) = entry(line, "*PageSize ")
            && label != key
        {
            names.entry(key).or_insert(label);
        }
    }
    let default_key = ppd
        .lines()
        .find_map(|line| line.trim_end_matches('\r').strip_prefix("*DefaultPageSize:").map(str::trim))
        .filter(|k| keyword(k))
        .map(str::to_string);
    let fits = |v: f64| v.is_finite() && (1.0..=MAX_PAPER_SIDE).contains(&v);
    let mut out: Vec<PrinterPaper> = Vec::new();
    for line in ppd.lines() {
        if out.len() >= MAX_PAPERS {
            break;
        }
        let Some((key, label, value)) = entry(line, "*PaperDimension ") else { continue };
        if key.starts_with("Custom") || out.iter().any(|p| p.key == key) {
            continue;
        }
        let mut sides = value.trim_matches('"').split_whitespace().map(|v| v.parse::<f64>().ok());
        let (Some(Some(w)), Some(Some(h))) = (sides.next(), sides.next()) else { continue };
        if !fits(w) || !fits(h) {
            continue;
        }
        let label = if label == key { names.get(&key).cloned().unwrap_or(label) } else { label };
        let default = default_key.as_deref() == Some(key.as_str());
        out.push(PrinterPaper { key, label, size: (w, h), default });
    }
    out
}

/// The index in `papers` of the paper of this size, within half a point a side: the one fed
/// that way round first (a label's width is its width across the feed), else the one fed the
/// other way round; the first when several match.
pub fn paper_of_size(papers: &[PrinterPaper], size: (f64, f64)) -> Option<usize> {
    let same = |p: &PrinterPaper, (w, h): (f64, f64)| (p.size.0 - w).abs() <= 0.5 && (p.size.1 - h).abs() <= 0.5;
    papers.iter().position(|p| same(p, size)).or_else(|| papers.iter().position(|p| same(p, (size.1, size.0))))
}

pub fn submit(pdf: &[u8], job: &Job) -> Result<String, crate::PrintError> {
    craft_print::submit(pdf, job).map_err(|e| crate::PrintError::Spool(e.to_string()))
}

/// [`submit`], told `(sent, sheets)` as each sheet reaches the spooler (Windows; CUPS takes the
/// whole PDF at once and reports nothing).
pub fn submit_with_progress(pdf: &[u8], job: &Job, progress: &mut dyn FnMut(usize, usize)) -> Result<String, crate::PrintError> {
    craft_print::submit_with_progress(pdf, job, progress).map_err(|e| crate::PrintError::Spool(e.to_string()))
}

pub fn open_printer_preferences(printer: &str) -> Result<(), crate::PrintError> {
    craft_print::open_printer_preferences(printer).map_err(|e| crate::PrintError::Spool(e.to_string()))
}
