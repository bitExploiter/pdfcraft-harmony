//! Explicit document-font discovery. Interface fallbacks never enter this catalogue.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FontFile {
    pub path: PathBuf,
    pub family: String,
    pub face: String,
}

/// Standard user-installed font folders; discovery is performed only on request.
pub fn installed_directories() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        let mut dirs = vec![PathBuf::from("/Library/Fonts")];
        if let Some(home) = home {
            dirs.push(home.join("Library/Fonts"));
        }
        dirs
    } else if cfg!(windows) {
        let mut dirs = std::env::var_os("WINDIR").map(|p| PathBuf::from(p).join("Fonts")).into_iter().collect::<Vec<_>>();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
        dirs
    } else {
        let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
        if let Some(home) = home {
            dirs.extend([home.join(".local/share/fonts"), home.join(".fonts")]);
        }
        dirs
    }
}

/// Bounded, symlink-free scan. Only standalone faces whose embedding permissions allow editable
/// embedding are listed.
/// The caller controls roots (automation resolves confinement before calling this).
pub fn discover(roots: &[PathBuf]) -> Vec<FontFile> {
    use std::io::Read;
    let mut pending: Vec<_> = roots.iter().take(16).map(|r| (r.clone(), 0usize)).collect();
    let mut files = Vec::new();
    let mut entries = 0usize;
    let mut budget = 64u64 << 20;
    let mut seen = std::collections::HashSet::new();
    while let Some((path, depth)) = pending.pop() {
        if entries >= 4096 || budget == 0 {
            break;
        }
        entries += 1;
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() || !seen.insert(path.clone()) {
            continue;
        }
        if meta.is_dir() && depth < 4 {
            if let Ok(children) = std::fs::read_dir(&path) {
                let remaining = 4096usize.saturating_sub(entries.saturating_add(pending.len()));
                pending.extend(children.take(remaining).filter_map(Result::ok).map(|p| (p.path(), depth + 1)));
            }
        } else if meta.is_file() && meta.len() <= (16 << 20) && font_extension(&path) {
            if meta.len() > budget {
                continue;
            }
            budget = budget.saturating_sub(meta.len());
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&path).and_then(|f| f.take((16 << 20) + 1).read_to_end(&mut bytes));
            if read.is_err() || pdfcraft_fonts::embedding::validate_document_input(&bytes).is_err() {
                continue;
            }
            if let Ok((family, face)) = pdfcraft_fonts::embedding::names(&bytes) {
                files.push(FontFile { path, family, face });
            }
        }
    }
    files.sort_by(|a, b| a.face.cmp(&b.face).then(a.path.cmp(&b.path)));
    files
}
/// An installed font chosen by [`installed_font_for`].
#[derive(Clone, Debug, PartialEq)]
pub struct InstalledFont {
    pub family: String,
    pub bytes: Vec<u8>,
}

/// Why [`installed_font_for`] found no font.
#[derive(Clone, Debug, PartialEq)]
pub enum NoInstalledFont {
    /// Installed fonts are turned off (`PDFCRAFT_SYSTEM_FONTS=0`).
    Off,
    /// No usable font is installed.
    NoneInstalled,
    /// The installed font that has the most of the characters (`family`) still lacks `missing`.
    Lacks { family: String, missing: char },
}

impl std::fmt::Display for NoInstalledFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => write!(f, "installed fonts are turned off (PDFCRAFT_SYSTEM_FONTS=0)"),
            Self::NoneInstalled => write!(f, "no installed font can be embedded"),
            Self::Lacks { family, missing } => {
                write!(
                    f,
                    "no installed font has every character of the text; the closest, {family}, has no {missing} (U+{:04X})",
                    u32::from(*missing)
                )
            }
        }
    }
}

/// An installed font that can draw every character of `text`, for an edit no other font can draw
/// (Dhivehi typed into a PDF whose subset lacks the letters). A face of the paragraph's own family
/// (`base_font`, a subset name such as `ABCDEF+Faruma`) comes first, then the others in
/// [`discover`] order. The installed folders are scanned once per process.
pub fn installed_font_for(text: &str, base_font: &str) -> Result<InstalledFont, NoInstalledFont> {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<Vec<FontFile>> = OnceLock::new();
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return Err(NoInstalledFont::Off);
    }
    // The paragraph's family by file name first (`A_FARUMA.TTF` for `ABCDEF+Faruma`): the general
    // scan stops at its byte budget and can miss it in a full system folder.
    let named = named_like(&installed_directories(), &pdfcraft_fonts::embedding::face_key(base_font));
    let named = discover(&named);
    let installed = INSTALLED.get_or_init(|| discover(&installed_directories()));
    let all: Vec<FontFile> = named.iter().chain(installed.iter().filter(|f| !named.iter().any(|n| n.path == f.path))).cloned().collect();
    pick_font_for(&all, text, base_font)
}

/// Font files directly in `dirs` whose file name contains `key` (a [`face_key`]), at most 16.
/// Only names are read here.
///
/// [`face_key`]: pdfcraft_fonts::embedding::face_key
fn named_like(dirs: &[PathBuf], key: &str) -> Vec<PathBuf> {
    if key.len() < 3 {
        return Vec::new();
    }
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.filter_map(Result::ok).take(8192))
        .map(|entry| entry.path())
        .filter(|path| {
            font_extension(path) && path.file_stem().is_some_and(|s| pdfcraft_fonts::embedding::face_key(&s.to_string_lossy()).contains(key))
        })
        .take(16)
        .collect()
}

fn pick_font_for(fonts: &[FontFile], text: &str, base_font: &str) -> Result<InstalledFont, NoInstalledFont> {
    use pdfcraft_fonts::embedding::{covers, face_key};
    let family = face_key(base_font);
    // `A_Faruma` and `MV Faruma` are Faruma too.
    let same = |f: &&FontFile| family.len() >= 3 && (face_key(&f.family).contains(&family) || face_key(&f.face).contains(&family));
    let ordered = fonts.iter().filter(same).chain(fonts.iter().filter(|f| !same(f)));
    let mut chars: Vec<char> = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
    chars.sort_unstable();
    chars.dedup();
    // The font lacking the fewest characters, and the first character it lacks in `text`.
    let mut closest: Option<(usize, String, char)> = None;
    // `discover` read these within its byte budget and validated them; reading them again is
    // bounded by the same budget.
    for f in ordered {
        let Some(bytes) = std::fs::read(&f.path).ok().filter(|b| b.len() <= 16 << 20) else { continue };
        if covers(&bytes, text) {
            return Ok(InstalledFont { family: f.family.clone(), bytes });
        }
        let lacking = chars.iter().filter(|c| !covers(&bytes, &c.to_string())).count();
        if closest.as_ref().is_none_or(|(fewest, ..)| lacking < *fewest)
            && let Some(missing) = text.chars().find(|c| !matches!(c, '\n' | '\r') && !covers(&bytes, &c.to_string()))
        {
            closest = Some((lacking, f.family.clone(), missing));
        }
    }
    Err(closest.map_or(NoInstalledFont::NoneInstalled, |(_, family, missing)| NoInstalledFont::Lacks { family, missing }))
}

fn font_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installed_font_for_an_edit_has_every_character_and_prefers_the_paragraphs_family() {
        let asset = |name: &str| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts")).join(name);
        let file = |name: &str, family: &str| FontFile { path: asset(name), family: family.into(), face: family.replace(' ', "") };
        // Discovery order: Dancing Script before Inter.
        let fonts = [file("DancingScript.ttf", "Dancing Script"), file("Inter-Regular.ttf", "Inter")];
        let picked = |fonts: &[FontFile], text: &str, base: &str| pick_font_for(fonts, text, base).map(|f| f.family);
        // The paragraph's own family wins whenever it has every character.
        assert_eq!(picked(&fonts, "Ab c", "ABCDEF+Inter"), Ok("Inter".into()));
        assert_eq!(picked(&fonts, "Ab c", "ABCDEF+DancingScript"), Ok("Dancing Script".into()));
        assert_eq!(pick_font_for(&fonts, "Ab", "Inter").unwrap().bytes, std::fs::read(asset("Inter-Regular.ttf")).unwrap());
        // Otherwise the first font, in discovery order, that has them all: Dancing Script has no Cyrillic.
        assert_eq!(picked(&fonts, "Ab c", "ABCDEF+Faruma"), Ok("Dancing Script".into()));
        assert_eq!(picked(&fonts, "Жb", "ABCDEF+DancingScript"), Ok("Inter".into()));
        // None has every character: the closest font and the first character it lacks are named.
        assert_eq!(picked(&fonts, "Ж ދިވެހި", ""), Err(NoInstalledFont::Lacks { family: "Inter".into(), missing: 'ދ' }));
        assert!(NoInstalledFont::Lacks { family: "Inter".into(), missing: 'ދ' }.to_string().contains("Inter, has no ދ (U+078B)"));
        assert_eq!(picked(&[], "A", ""), Err(NoInstalledFont::NoneInstalled));
        // A file that has gone since discovery is skipped.
        assert_eq!(picked(&[file("missing.ttf", "Inter")], "A", "Inter"), Err(NoInstalledFont::NoneInstalled));
        // A family name that contains the paragraph's (`A_Faruma` for `Faruma`) counts as its family.
        let renamed = [file("DancingScript.ttf", "Dancing Script"), file("Inter-Regular.ttf", "A_Inter")];
        assert_eq!(picked(&renamed, "Ab", "XYZABC+Inter"), Ok("A_Inter".into()));
    }

    #[test]
    fn fonts_are_found_by_file_name() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts"));
        let names = |key: &str| {
            named_like(std::slice::from_ref(&dir), key).iter().filter_map(|p| p.file_name()?.to_str().map(str::to_owned)).collect::<Vec<_>>()
        };
        let mut inter = names("inter");
        inter.sort();
        assert_eq!(inter, ["Inter-Medium.ttf", "Inter-Regular.ttf", "Inter-SemiBold.ttf"]);
        // Not the licence texts, and nothing for a key too short to mean a family.
        assert!(names("ofl").is_empty());
        assert!(names("in").is_empty());
        assert!(named_like(&[dir.join("missing")], "inter").is_empty());
    }
}
