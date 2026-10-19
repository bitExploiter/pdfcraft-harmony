//! Best-effort recovery of an existing per-file PDFCraft binding after a successful open/save.
//! Quarantined documents with redirected bindings can be refused by LaunchServices before the
//! app receives an open event (Apple feedback FB19468486). In-app Open remains reachable.
//! Never remove quarantine, set type-wide defaults, or create a new preference on an open.

use std::{io, path::Path};

const OPEN_WITH: &str = "com.apple.LaunchServices.OpenWith";
const QUARANTINE: &str = "com.apple.quarantine";
const BUNDLE_ID: &str = "ai.storyteller.pdfcraft";
const MAX_ATTRIBUTE: usize = 64 * 1024;

/// Preserve these two attributes verbatim on the staging file, before atomic replacement.
/// This includes other applications' bindings: an ordinary save must not erase a preference.
#[derive(Default, PartialEq, Eq)]
pub(crate) struct OpeningMetadata {
    association: Option<Vec<u8>>,
    quarantine: Option<Vec<u8>>,
    identity: Option<(u64, u64)>,
}

impl OpeningMetadata {
    pub(crate) fn read(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt as _;
        let identity = match path.symlink_metadata() {
            Ok(meta) if meta.is_file() => (meta.dev(), meta.ino()),
            Ok(_) => return Ok(Self::default()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let snapshot =
            Self { association: read_attribute(path, OPEN_WITH)?, quarantine: read_attribute(path, QUARANTINE)?, identity: Some(identity) };
        let current = path.symlink_metadata()?;
        if !current.is_file() || (current.dev(), current.ino()) != identity {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "the file changed while reading its opening settings"));
        }
        Ok(snapshot)
    }

    pub(crate) fn apply(&self, staging: &std::fs::File) -> io::Result<()> {
        use xattr::FileExt;
        self.apply_with(|name, value| staging.set_xattr(name, value))
    }

    fn apply_with(&self, mut set: impl FnMut(&str, &[u8]) -> io::Result<()>) -> io::Result<()> {
        for (name, value) in [(OPEN_WITH, &self.association), (QUARANTINE, &self.quarantine)] {
            if let Some(value) = value {
                // The shared writer falls back to fsync for NOTSUP/NOTTY from its flush
                // callback. A metadata error must never be mistaken for that flush failure.
                set(name, value).map_err(|e| io::Error::other(format!("could not preserve {name}: {e}")))?;
            }
        }
        Ok(())
    }

    fn still_matches(&self, path: &Path) -> bool {
        matches!(Self::read(path), Ok(current) if current.eq(self))
    }
}

/// Preserve destination preferences only for explicit PDF document saves. Unreadable or
/// oversized metadata refuses replacement rather than silently discarding quarantine.
pub(crate) fn write_document(
    path: &str,
    bytes: &[u8],
    suffixes: impl IntoIterator<Item = u64>,
    sync_all: impl FnOnce(&std::fs::File) -> io::Result<()>,
) -> io::Result<()> {
    use pdfcraft_platform::staging::{StagingName, write_atomically_with};
    let sync = |file: &std::fs::File| {
        // Snapshot late so settings changed during staging are preserved too.
        let metadata = OpeningMetadata::read(Path::new(path))
            .map_err(|e| io::Error::other(format!("could not read opening settings; save a copy with Save As: {e}")))?;
        metadata.apply(file)?;
        sync_all(file)
    };
    write_atomically_with(Path::new(path), bytes, StagingName::TagThenSuffix, suffixes, sync)
}

fn read_attribute(path: &Path, name: &str) -> io::Result<Option<Vec<u8>>> {
    read_attribute_with(|buffer| rustix::fs::lgetxattr(path, name, buffer))
}

fn read_attribute_with(get: impl FnOnce(&mut [u8]) -> rustix::io::Result<usize>) -> io::Result<Option<Vec<u8>>> {
    use rustix::io::Errno;
    // A fixed-size buffer bounds allocation before the OS reads untrusted file metadata.
    // Do not follow a last-component symlink, including one swapped in after the stat above.
    let mut bytes = vec![0; MAX_ATTRIBUTE];
    match get(bytes.as_mut_slice()) {
        Ok(len) => {
            bytes.truncate(len);
            Ok(Some(bytes))
        }
        Err(Errno::NOATTR | Errno::NOTSUP | Errno::OPNOTSUPP) => Ok(None),
        Err(Errno::RANGE) => Err(io::Error::new(io::ErrorKind::InvalidData, "opening metadata exceeds 64 KiB")),
        Err(e) => Err(e.into()),
    }
}

fn points_to_this_app(data: &[u8], app: &Path) -> bool {
    if data.len() > MAX_ATTRIBUTE {
        return false;
    }
    // Deserialize just the small native schema rather than retaining an arbitrary plist tree.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Binding {
        version: u32,
        path: String,
        // Apple's API sometimes writes only version/path (e.g. an unregistered local bundle).
        // The exact path still has to match the running bundle whose identity we checked.
        bundleidentifier: Option<String>,
    }
    let Ok(binding) = plist::from_bytes::<Binding>(data) else { return false };
    if binding.version != 0 || binding.bundleidentifier.as_deref().is_some_and(|id| id != BUNDLE_ID) || !Path::new(&binding.path).is_absolute() {
        return false;
    }
    // Exact native spelling is intentionally conservative. Never resolve an untrusted path:
    // canonicalize could contact a stale network mount and block the main thread.
    Path::new(&binding.path) == app
}

pub(crate) fn repair(path: &Path) {
    use block2::RcBlock;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSError, NSURL};

    // This API was introduced in macOS 12; the application still supports macOS 11.
    if !objc2::available!(macos = 12.0) || objc2::MainThreadMarker::new().is_none() {
        return;
    }
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return;
    }
    let bundle = NSBundle::mainBundle();
    if bundle.bundleIdentifier().is_none_or(|id| id.to_string() != BUNDLE_ID) {
        return; // Tests, CLI binaries, and unbundled development runs never change associations.
    }
    let app_url = bundle.bundleURL();
    let Some(app_path) = app_url.to_file_path() else { return };
    let metadata = match OpeningMetadata::read(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            log::warn!("PDF association recovery skipped: could not read opening metadata");
            return;
        }
    };
    if metadata.quarantine.is_none() || !metadata.association.as_deref().is_some_and(|data| points_to_this_app(data, &app_path)) {
        return;
    }
    let Some(file_url) = NSURL::from_file_path(path) else { return };
    // Recheck the file identity and both attributes immediately before the request. The native
    // API has no compare-and-set; a later external change can still race its asynchronous work.
    if !metadata.still_matches(path) {
        return;
    }
    let completion = RcBlock::new(|error: *mut NSError| {
        // The safe binding exposes an opaque NSError pointer. Do not dereference it or log paths.
        if error.is_null() {
            log::info!("PDF association recovery completed");
        } else {
            log::warn!("PDF association recovery failed; document open/save remains successful");
        }
    });
    NSWorkspace::sharedWorkspace().setDefaultApplicationAtURL_toOpenFileAtURL_completionHandler(&app_url, &file_url, Some(&completion));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn directory() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("pdfcraft-association-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        p
    }

    fn binding(app: &Path, id: &str, version: u32, binary: bool) -> Vec<u8> {
        let value = plist::Value::Dictionary(
            [
                ("version".to_string(), plist::Value::Integer(version.into())),
                ("path".to_string(), plist::Value::String(app.to_string_lossy().into_owned())),
                ("bundleidentifier".to_string(), plist::Value::String(id.into())),
            ]
            .into_iter()
            .collect(),
        );
        let mut bytes = Vec::new();
        if binary {
            value.to_writer_binary(&mut bytes).unwrap();
        } else {
            value.to_writer_xml(&mut bytes).unwrap();
        }
        bytes
    }

    #[test]
    fn requires_matching_identifier_path_and_native_schema() {
        let app = directory();
        let other = directory();
        for binary in [false, true] {
            assert!(points_to_this_app(&binding(&app, BUNDLE_ID, 0, binary), &app));
            assert!(!points_to_this_app(&binding(&app, "com.apple.Preview", 0, binary), &app));
            assert!(!points_to_this_app(&binding(&other, BUNDLE_ID, 0, binary), &app));
            assert!(!points_to_this_app(&binding(&app, BUNDLE_ID, 1, binary), &app));
        }
        let mut native = plist::from_bytes::<plist::Value>(&binding(&app, BUNDLE_ID, 0, true)).unwrap();
        native.as_dictionary_mut().unwrap().remove("bundleidentifier");
        let mut bytes = Vec::new();
        native.to_writer_binary(&mut bytes).unwrap();
        assert!(points_to_this_app(&bytes, &app), "Apple's path-only binding still identifies this exact bundle");
        assert!(!points_to_this_app(&bytes, &other));
        assert!(!points_to_this_app(b"not a plist", &app));
        assert!(!points_to_this_app(&vec![0; MAX_ATTRIBUTE + 1], &app));
        assert!(!points_to_this_app(&binding(Path::new("relative.app"), BUNDLE_ID, 0, true), &app));
        std::fs::remove_dir_all(app).unwrap();
        std::fs::remove_dir_all(other).unwrap();
    }

    #[test]
    fn atomic_save_preserves_own_other_and_malformed_bindings_and_quarantine() {
        let dir = directory();
        let file = dir.join("file.pdf");
        for data in [binding(&dir, BUNDLE_ID, 0, true), binding(&dir, "com.apple.Preview", 0, true), b"malformed".to_vec()] {
            std::fs::write(&file, b"before").unwrap();
            xattr::set(&file, OPEN_WITH, &data).unwrap();
            xattr::set(&file, QUARANTINE, b"0082;1234;Preview;").unwrap();
            write_document(file.to_str().unwrap(), b"after", [7], std::fs::File::sync_all).unwrap();
            assert_eq!(std::fs::read(&file).unwrap(), b"after");
            assert_eq!(xattr::get(&file, OPEN_WITH).unwrap(), Some(data));
            assert_eq!(xattr::get(&file, QUARANTINE).unwrap(), Some(b"0082;1234;Preview;".to_vec()));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn new_destination_does_not_acquire_an_association() {
        let dir = directory();
        let file = dir.join("new.pdf");
        write_document(file.to_str().unwrap(), b"pdf bytes", [7], std::fs::File::sync_all).unwrap();
        assert!(xattr::get(&file, OPEN_WITH).unwrap().is_none());
        assert!(xattr::get(&file, QUARANTINE).unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn metadata_copy_failure_cannot_be_treated_as_an_unsupported_flush() {
        use pdfcraft_platform::staging::{StagingName, write_atomically_with};
        for errno in [rustix::io::Errno::NOTSUP, rustix::io::Errno::NOTTY] {
            let dir = directory();
            let file = dir.join("out.pdf");
            std::fs::write(&file, b"original PDF").unwrap();
            xattr::set(&file, OPEN_WITH, b"original association").unwrap();
            xattr::set(&file, QUARANTINE, b"0082;1234;Preview;").unwrap();
            let metadata = OpeningMetadata::read(&file).unwrap();
            let result = write_atomically_with(&file, b"new PDF", StagingName::TagThenSuffix, [7], |_| metadata.apply_with(|_, _| Err(errno.into())));
            assert!(result.is_err(), "a refused metadata copy must fail the save, not fall back to fsync");
            assert_eq!(std::fs::read(&file).unwrap(), b"original PDF");
            assert_eq!(xattr::get(&file, OPEN_WITH).unwrap(), Some(b"original association".to_vec()));
            assert_eq!(xattr::get(&file, QUARANTINE).unwrap(), Some(b"0082;1234;Preview;".to_vec()));
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "staging file removed");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn oversized_attribute_is_refused_before_preservation_or_parsing() {
        let dir = directory();
        let file = dir.join("out.pdf");
        std::fs::write(&file, b"original PDF").unwrap();
        xattr::set(&file, OPEN_WITH, &vec![b'x'; MAX_ATTRIBUTE + 1]).unwrap();
        assert!(OpeningMetadata::read(&file).is_err(), "reading, not just parsing, must enforce the limit");
        assert!(write_document(file.to_str().unwrap(), b"new PDF", [7], std::fs::File::sync_all).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"original PDF");
        assert_eq!(xattr::get(&file, OPEN_WITH).unwrap().unwrap().len(), MAX_ATTRIBUTE + 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preflight_refuses_a_replaced_file_even_with_identical_attributes() {
        let dir = directory();
        let file = dir.join("out.pdf");
        let replacement = dir.join("replacement.pdf");
        let prepare = |path: &Path| {
            std::fs::write(path, b"PDF").unwrap();
            xattr::set(path, OPEN_WITH, &binding(&dir, BUNDLE_ID, 0, true)).unwrap();
            xattr::set(path, QUARANTINE, b"0082;1234;Preview;").unwrap();
        };
        prepare(&file);
        let metadata = OpeningMetadata::read(&file).unwrap();
        assert!(metadata.still_matches(&file));
        prepare(&replacement);
        std::fs::rename(replacement, &file).unwrap();
        assert!(!metadata.still_matches(&file), "the file successfully opened is no longer at this path");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preflight_refuses_changed_quarantine_and_symbolic_links() {
        let dir = directory();
        let file = dir.join("out.pdf");
        std::fs::write(&file, b"PDF").unwrap();
        xattr::set(&file, OPEN_WITH, &binding(&dir, BUNDLE_ID, 0, true)).unwrap();
        xattr::set(&file, QUARANTINE, b"0082;1234;Preview;").unwrap();
        let metadata = OpeningMetadata::read(&file).unwrap();
        xattr::set(&file, QUARANTINE, b"0082;5678;another app;").unwrap();
        assert!(!metadata.still_matches(&file));
        let link = dir.join("link.pdf");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        let linked = OpeningMetadata::read(&link).unwrap();
        assert!(linked.association.is_none() && linked.quarantine.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn untrusted_bundle_aliases_are_skipped_without_resolving_them() {
        let dir = directory();
        let app = dir.join("PdfCraft.app");
        std::fs::create_dir(&app).unwrap();
        let alias = dir.join("alias.app");
        std::os::unix::fs::symlink(&app, &alias).unwrap();
        assert!(!points_to_this_app(&binding(&alias, BUNDLE_ID, 0, true), &app));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unrelated_atomic_writes_do_not_read_opening_metadata() {
        let dir = directory();
        let file = dir.join("recovery.pdf");
        std::fs::write(&file, b"old recovery").unwrap();
        xattr::set(&file, OPEN_WITH, &vec![b'x'; MAX_ATTRIBUTE + 1]).unwrap();
        crate::editing::write_atomically(file.to_str().unwrap(), b"new recovery").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new recovery");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_binding_and_packaging_identity_match() {
        // Captured via Apple's API on a synthetic PDF on macOS 27.0.1. Contains only the
        // public installed bundle path, identifier and schema version; no document data.
        let hex = "62706c6973743030d30102030405065776657273696f6e54706174685f101062756e646c656964656e74696669657210005f101a2f4170706c69636174696f6e732f50646643726166742e6170705f101761692e73746f727974656c6c65722e7064666372616674080f171c2f314e0000000000000101000000000000000700000000000000000000000000000068";
        let captured: Vec<u8> =
            hex.as_bytes().as_chunks::<2>().0.iter().map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect();
        assert!(points_to_this_app(&captured, Path::new("/Applications/PdfCraft.app")));
        let package = plist::from_bytes::<plist::Value>(include_bytes!("../../../packaging/macos/Info.plist.in")).unwrap();
        assert_eq!(package.as_dictionary().unwrap().get("CFBundleIdentifier").unwrap().as_string(), Some(BUNDLE_ID));
    }

    #[test]
    fn malformed_plists_and_truncated_native_schemas_do_not_panic() {
        let app = Path::new("/Applications/PdfCraft.app");
        for binary in [false, true] {
            let native = binding(app, BUNDLE_ID, 0, binary);
            for end in 0..native.len() {
                let _ = points_to_this_app(native.get(..end).unwrap(), app);
            }
            for index in 0..native.len() {
                for byte in [0, 0xff, b'<', b'&'] {
                    let mut mutated = native.clone();
                    *mutated.get_mut(index).unwrap() = byte;
                    let _ = points_to_this_app(&mutated, app);
                }
            }
        }
        let mut state = 0x125a_9430_8c97_ae13u64;
        for len in 0..1024 {
            let data: Vec<_> = (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect();
            let _ = points_to_this_app(&data, app);
        }
    }

    #[test]
    fn unsupported_attribute_volumes_are_distinct_from_read_failures() {
        use rustix::io::Errno;
        for errno in [Errno::NOATTR, Errno::NOTSUP, Errno::OPNOTSUPP] {
            assert_eq!(read_attribute_with(|_| Err(errno)).unwrap(), None, "{errno}");
        }
        for errno in [Errno::ACCESS, Errno::IO, Errno::RANGE] {
            assert!(read_attribute_with(|_| Err(errno)).is_err(), "{errno}");
        }
    }
}
