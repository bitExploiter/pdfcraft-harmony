//! macOS: print through the system's own print panel, for what PdfCraft's Print dialog doesn't
//! offer — PDF services (Save to iCloud, Open in Preview, fax and other workflows, #2157),
//! presets, and the driver's panes.
//!
//! The print-ready PDF that [`crate::impose`] composed is written to an owner-only temporary file
//! and handed to PDFKit in an `osascript` process, which runs the standard print panel on it and
//! waits until it closes. PdfCraft calls no AppKit API itself (PDFKit's bindings are all
//! `unsafe`), as Windows printing once went through PowerShell. The temporary file is removed when
//! the panel closes, whatever was chosen.

use crate::PrintError;

/// The printer name that means "the system print dialog", in the Print dialog's draft and for the
/// `doc_print` tool.
pub const SYSTEM_DIALOG: &str = "system-dialog";

/// Whether this platform has a system print dialog PdfCraft can open.
pub const AVAILABLE: bool = cfg!(target_os = "macos");

/// How the system print dialog closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Print (or a PDF service) was chosen: the job is with macOS.
    Printed,
    /// The person closed the panel without printing.
    Cancelled,
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
/// The AppleScript that runs the print panel: `argv` is the PDF's path and the job's title. It
/// answers `printed`, `cancelled` or `error: …` on stdout.
const SCRIPT: &str = r#"use AppleScript version "2.5"
use framework "AppKit"
use framework "PDFKit"
use scripting additions

on run argv
	set theApp to current application's NSApplication's sharedApplication()
	theApp's setActivationPolicy:(current application's NSApplicationActivationPolicyAccessory)
	set theDoc to current application's PDFDocument's alloc()'s initWithURL:(current application's NSURL's fileURLWithPath:(item 1 of argv))
	if theDoc is missing value then return "error: the print-ready PDF could not be read"
	set theOp to theDoc's printOperationForPrintInfo:(current application's NSPrintInfo's sharedPrintInfo()) scalingMode:(current application's kPDFPrintPageScaleNone) autoRotate:false
	if theOp is missing value then return "error: macOS could not prepare the print job"
	theOp's setJobTitle:(item 2 of argv)
	theOp's setShowsPrintPanel:true
	theOp's setShowsProgressPanel:true
	theApp's activateIgnoringOtherApps:true
	if (theOp's runOperation()) as boolean then return "printed"
	return "cancelled"
end run
"#;

#[cfg(all(unix, not(target_arch = "wasm32")))]
/// The longest job title passed on, in characters.
const TITLE_CHARS: usize = 200;

/// Open the system print dialog on `pdf` (a print-ready PDF) and wait until it closes. Blocks:
/// call it off the UI thread.
#[cfg(target_os = "macos")]
pub fn print(pdf: &[u8], title: &str) -> Result<Outcome, PrintError> {
    print_via(pdf, title, &std::env::temp_dir(), std::process::Command::new("/usr/bin/osascript"))
}

/// [`print`], on another platform: there is no system print dialog to open.
#[cfg(not(target_os = "macos"))]
pub fn print(_pdf: &[u8], _title: &str) -> Result<Outcome, PrintError> {
    Err(PrintError::Spool("the system print dialog is only available on macOS".into()))
}

/// [`print`] with the folder for the temporary PDF and the `osascript` command supplied, so tests
/// need neither a print panel nor macOS. The script goes to the command's stdin (`-`), followed by
/// the PDF's path and the title.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub(crate) fn print_via(pdf: &[u8], title: &str, dir: &std::path::Path, mut osascript: std::process::Command) -> Result<Outcome, PrintError> {
    use std::io::Write as _;
    if pdf.is_empty() {
        return Err(PrintError::NoPages);
    }
    let file = TempPdf::write(dir, pdf)?;
    let spawned = osascript
        .arg("-")
        .arg(&file.0)
        .arg(job_title(title))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = spawned.map_err(|e| PrintError::Spool(format!("couldn't open the system print dialog: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A script that exits without reading all of it is reported by its own output below.
        let _ = stdin.write_all(SCRIPT.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| PrintError::Spool(format!("the system print dialog stopped unexpectedly: {e}")))?;
    drop(file);
    outcome(out.status.success(), &String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr))
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
/// What the script's exit status and output say happened.
pub(crate) fn outcome(success: bool, stdout: &str, stderr: &str) -> Result<Outcome, PrintError> {
    let said = stdout.trim();
    match said {
        "printed" if success => Ok(Outcome::Printed),
        "cancelled" if success => Ok(Outcome::Cancelled),
        _ => {
            let fallback = if success { "no answer from osascript" } else { "osascript stopped with an error" };
            let why = said.strip_prefix("error: ").map(str::trim).filter(|s| !s.is_empty()).or_else(|| last_line(stderr)).unwrap_or(fallback);
            Err(PrintError::Spool(format!("the system print dialog failed: {}", capped(why))))
        }
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
/// The job's title as the print queue shows it: no control characters, at most [`TITLE_CHARS`]
/// characters, never empty.
pub(crate) fn job_title(title: &str) -> String {
    let clean: String = title.chars().filter(|c| !c.is_control()).take(TITLE_CHARS).collect();
    let clean = clean.trim();
    if clean.is_empty() { "PdfCraft".to_string() } else { clean.to_string() }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
fn last_line(s: &str) -> Option<&str> {
    s.lines().map(str::trim).rfind(|l| !l.is_empty())
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
/// At most 300 characters of a message from another process.
fn capped(s: &str) -> String {
    s.chars().take(300).collect()
}

/// A print-ready PDF in a temporary file only its owner can read, removed when dropped.
#[cfg(all(unix, not(target_arch = "wasm32")))]
struct TempPdf(std::path::PathBuf);

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl TempPdf {
    fn write(dir: &std::path::Path, pdf: &[u8]) -> Result<TempPdf, PrintError> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let mut last = None;
        // `create_new` never follows or reuses a file another process put there first.
        for _ in 0..16 {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = dir.join(format!("pdfcraft-print-{}-{nanos}-{n}.pdf", std::process::id()));
            match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
                Ok(mut f) => {
                    let file = TempPdf(path);
                    f.write_all(pdf)
                        .and_then(|()| f.sync_all())
                        .map_err(|e| PrintError::Spool(format!("couldn't write the print-ready PDF: {e}")))?;
                    return Ok(file);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(PrintError::Spool(format!("couldn't write the print-ready PDF: {e}"))),
            }
        }
        Err(PrintError::Spool(format!("couldn't write the print-ready PDF: {}", last.map_or_else(|| "no free name".to_string(), |e| e.to_string()))))
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl Drop for TempPdf {
    fn drop(&mut self) {
        // Already gone (the folder was cleaned) is fine; nothing else can be done here.
        let _ = std::fs::remove_file(&self.0);
    }
}
