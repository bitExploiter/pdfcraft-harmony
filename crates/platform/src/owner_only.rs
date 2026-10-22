//! Owner-only files on Windows (#3922): the `--control` token file must be readable by its user
//! alone, as mode 0600 makes it on Unix.
//!
//! A file created in a folder inherits the folder's access list, and folders such as a new folder
//! at a drive root, `C:\Users\Public` or `C:\ProgramData` let every signed-in user read what is
//! created in them. [`restrict_to_owner`] makes the current user the owner and replaces the
//! inherited list with that user and SYSTEM only (`icacls`). [`read_with_sddl`] reads a file and
//! its security descriptor through one handle. [`foreign_access`] reads the descriptor in SDDL
//! form, which names accounts by SID and so reads the same in every Windows language, and says why
//! another user could have written or could read the file. They run the tools Windows ships rather
//! than calling the security API directly, which would need `unsafe`.

/// Why the file whose security descriptor is `sddl` (`O:` owner and `D:` access list, as
/// `Get-Acl` prints it) can't be trusted as user `me`'s own: it is owned by someone else, it has no
/// access list (everyone may use it), or it lets an account other than `me`, SYSTEM or the
/// Administrators group use it. Accounts are compared by SID; `None` means it is `me`'s alone.
///
/// The rule is OpenSSH's for private keys on Windows: other users and groups such as Users or
/// Authenticated Users must have no access, while SYSTEM and Administrators may.
pub fn foreign_access(sddl: &str, me: &str) -> Option<String> {
    let me = me.trim();
    let owner = section(sddl, "O:");
    match owner {
        Some(o) if same_account(o, me) => {}
        Some(o) => return Some(format!("it is owned by another account ({o})")),
        None => return Some("its owner can't be read".into()),
    }
    let Some(dacl) = section(sddl, "D:") else {
        return Some("it has no access list, so every user may read it".into());
    };
    if dacl.starts_with("NO_ACCESS_CONTROL") {
        return Some("it has no access control, so every user may read it".into());
    }
    for ace in dacl.split('(').skip(1) {
        let ace = ace.split(')').next().unwrap_or_default();
        let parts: Vec<&str> = ace.split(';').collect();
        // `type;flags;rights;object guid;inherited object guid;account SID[;(condition)]`.
        let (Some(kind), Some(flags)) = (parts.first(), parts.get(1)) else { continue };
        // Deny entries take access away, and inherit-only entries apply to children, not the file.
        let allows = matches!(*kind, "A" | "OA" | "XA" | "ZA");
        if !allows || flags.contains("IO") {
            continue;
        }
        // An entry that grants access but names no account can't be shown to be the user's.
        let Some(sid) = parts.get(5).filter(|s| !s.is_empty()) else {
            return Some(format!("its access list has an entry that can't be read ({ace})"));
        };
        if !(same_account(sid, me) || matches!(*sid, "SY" | "S-1-5-18" | "BA" | "S-1-5-32-544" | "OW")) {
            return Some(format!("another account ({sid}) has access to it"));
        }
    }
    None
}

/// `section(sddl, "O:")` is the owner, `"D:"` the access list: the text after the tag up to the
/// next top-level tag.
fn section<'a>(sddl: &'a str, tag: &str) -> Option<&'a str> {
    let start = sddl.find(tag)?.checked_add(tag.len())?;
    let rest = sddl.get(start..)?;
    // The owner and group are bare SIDs; the access lists run until the next `X:` tag outside
    // parentheses.
    let mut depth = 0usize;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && i > 0 => {
                // The tag is the one letter before this colon.
                end = rest.get(..i).and_then(|s| s.char_indices().last()).map_or(i, |(j, _)| j);
                break;
            }
            _ => {}
        }
    }
    rest.get(..end).map(str::trim)
}

/// Whether SDDL account `a` names user SID `me`. `OW` (owner rights) and `CO` are not users.
fn same_account(a: &str, me: &str) -> bool {
    a.eq_ignore_ascii_case(me)
}

/// The current user's SID (`whoami /user`).
#[cfg(windows)]
pub fn current_user_sid() -> std::io::Result<String> {
    let out = std::process::Command::new("whoami").args(["/user", "/fo", "csv", "/nh"]).output()?;
    if !out.status.success() {
        return Err(std::io::Error::other("whoami /user failed"));
    }
    sid_from_whoami(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| std::io::Error::other("whoami /user printed no SID"))
}

#[cfg(any(test, windows))]
/// The SID in `whoami /user /fo csv /nh` output, `"DOMAIN\user","S-1-5-21-…"`: the last quoted
/// field, when it is one. Anything else (an error message, nothing) gives `None`.
fn sid_from_whoami(text: &str) -> Option<String> {
    let last = text.trim().rsplit(',').next()?.trim().trim_matches('"');
    let digits = last.strip_prefix("S-1-")?;
    // Every part after `S-1-` is a number; an empty remainder is one empty part, so it fails too.
    digits.split('-').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then(|| last.to_string())
}

/// Give `path` an access list of its own that only the current user and SYSTEM are on, dropping
/// what it inherited from its folder, then make that user its owner (`icacls /inheritance:r
/// /grant:r`, then `/setowner`). The owner is set because a file an elevated administrator creates
/// is owned by the Administrators group, not by the user. The list comes first: its owner may always
/// replace it, while setting the owner needs the Full control it grants (a folder may give Modify).
#[cfg(windows)]
pub fn restrict_to_owner(path: &std::path::Path) -> std::io::Result<()> {
    let me = current_user_sid()?;
    icacls(path, &["/inheritance:r".into(), "/grant:r".into(), format!("*{me}:(F)"), "*S-1-5-18:(F)".into()])?;
    icacls(path, &["/setowner".into(), format!("*{me}")])
}

#[cfg(windows)]
fn icacls(path: &std::path::Path, args: &[String]) -> std::io::Result<()> {
    let out = std::process::Command::new("icacls").arg(path).args(args).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("icacls could not restrict the file to its owner: {}", String::from_utf8_lossy(&out.stderr).trim())))
    }
}

/// Open `path` shared for reading only, so that while it is open it can't be replaced, renamed or
/// written, and read through that one handle both its security descriptor in SDDL form (owner and
/// access list) and its first `max + 1` bytes. The descriptor checked is the one of the handle the
/// bytes come from, so there is no moment at which the path could name a different file (a
/// time-of-check to time-of-use race). The path reaches PowerShell through an environment variable,
/// never as script text.
#[cfg(windows)]
pub fn read_with_sddl(path: &std::path::Path, max: u64) -> std::io::Result<(String, Vec<u8>)> {
    // No double quotes: the command line would strip them.
    const SCRIPT: &str = "$fs = [IO.File]::Open($env:PDFCRAFT_ACL_PATH, 'Open', 'Read', 'Read'); try { \
        $sddl = $fs.GetAccessControl().GetSecurityDescriptorSddlForm('Owner, Access'); \
        $max = [int]$env:PDFCRAFT_ACL_MAX + 1; $buf = New-Object byte[] $max; $total = 0; \
        while ($total -lt $max) { $n = $fs.Read($buf, $total, $max - $total); if ($n -le 0) { break }; $total += $n }; \
        [Console]::Out.Write($sddl + [char]10 + [BitConverter]::ToString($buf, 0, $total)) } finally { $fs.Dispose() }";
    // Bounded so the script's `[int]` and buffer stay small; a control file is far below this.
    let max = max.min(1 << 20);
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("PDFCRAFT_ACL_PATH", path)
        .env("PDFCRAFT_ACL_MAX", max.to_string())
        .output()?;
    let read = if out.status.success() { sddl_and_bytes(&String::from_utf8_lossy(&out.stdout)) } else { None };
    read.ok_or_else(|| std::io::Error::other("PowerShell could not read the file with its owner and access list"))
}

/// The security descriptor of `path` in SDDL form, owner included.
#[cfg(windows)]
pub fn sddl_of(path: &std::path::Path) -> std::io::Result<String> {
    read_with_sddl(path, 0).map(|(sddl, _)| sddl)
}

/// [`read_with_sddl`]'s output: the SDDL, a newline, then the bytes as `[BitConverter]::ToString`
/// prints them (`7B-22-…`, empty for none). Anything else gives `None`.
#[cfg(any(test, windows))]
fn sddl_and_bytes(text: &str) -> Option<(String, Vec<u8>)> {
    let (sddl, hex) = text.split_once('\n')?;
    let sddl = sddl.trim();
    if !sddl.contains("O:") {
        return None;
    }
    let hex = hex.trim();
    if hex.is_empty() {
        return Some((sddl.to_string(), Vec::new()));
    }
    // Two hex digits each; `from_str_radix` alone would also take a sign, as in `+7`.
    let byte = |pair: &str| (pair.len() == 2 && pair.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u8::from_str_radix(pair, 16).ok()).flatten();
    let bytes = hex.split('-').map(byte).collect::<Option<Vec<u8>>>()?;
    Some((sddl.to_string(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn the_users_own_file_with_system_and_administrators_is_trusted() {
        assert_eq!(foreign_access(&format!("O:{ME}G:{ME}D:P(A;;FA;;;{ME})(A;;FA;;;SY)"), ME), None);
        assert_eq!(foreign_access(&format!("O:{ME}D:PAI(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;{ME})"), ME), None);
        // Inherit-only entries and deny entries don't grant access to the file itself.
        assert_eq!(foreign_access(&format!("O:{ME}D:(A;OICIIO;GA;;;CO)(D;;FA;;;BU)(A;;FA;;;{ME})"), ME), None);
    }

    #[test]
    fn other_owners_readers_and_missing_lists_are_refused() {
        let other = "S-1-5-21-1-2-3-1002";
        assert!(foreign_access(&format!("O:{other}D:(A;;FA;;;{ME})"), ME).unwrap().contains("owned by another account"));
        // Users (BU) and Authenticated Users (AU), as a folder at a drive root passes on.
        assert!(foreign_access(&format!("O:{ME}D:AI(A;ID;FA;;;{ME})(A;ID;0x1301bf;;;AU)"), ME).unwrap().contains("AU"));
        assert!(foreign_access(&format!("O:{ME}D:(A;;FR;;;BU)(A;;FA;;;{ME})"), ME).unwrap().contains("BU"));
        assert!(foreign_access(&format!("O:{ME}D:(A;;FA;;;{other})"), ME).is_some());
        assert!(foreign_access(&format!("O:{ME}G:SY"), ME).unwrap().contains("no access list"));
        assert!(foreign_access(&format!("O:{ME}D:NO_ACCESS_CONTROL"), ME).is_some());
        assert!(foreign_access("D:(A;;FA;;;SY)", ME).unwrap().contains("owner"));
    }

    /// A conditional entry can hold a `:` in a string; the access list doesn't end there, so the
    /// entry after it is still checked.
    #[test]
    fn a_colon_inside_an_entry_does_not_end_the_access_list() {
        let sddl = format!(r#"O:{ME}D:(XA;;FA;;;{ME};(@User.dept == "a:b"))(A;;FR;;;BU)"#);
        assert!(foreign_access(&sddl, ME).is_some_and(|p| p.contains("BU")), "{sddl}");
    }

    /// A conditional entry for the user is the user's; one that grants access but whose account
    /// can't be read is refused rather than skipped.
    #[test]
    fn conditional_and_malformed_entries() {
        assert_eq!(foreign_access(&format!(r#"O:{ME}D:(XA;;FA;;;{ME};(@User.dept == "x"))"#), ME), None);
        assert!(foreign_access(&format!("O:{ME}D:(A;;FA;;;{ME})(A;;FR)"), ME).is_some_and(|p| p.contains("can't be read")));
        assert!(foreign_access(&format!("O:{ME}D:(A;;FA;;;)"), ME).is_some_and(|p| p.contains("can't be read")));
        // A conditional entry grants access like a plain one: another account on it is refused.
        let other = format!(r#"O:{ME}D:(A;;FA;;;{ME})(XA;;FR;;;BU;(@User.dept == "x"))"#);
        assert!(foreign_access(&other, ME).is_some_and(|p| p.contains("BU")), "{other}");
    }

    #[test]
    fn the_sid_is_read_from_whoamis_output_and_nothing_else() {
        assert_eq!(sid_from_whoami("\"desk\\ada\",\"S-1-5-21-1-2-3-1001\"\r\n").as_deref(), Some("S-1-5-21-1-2-3-1001"));
        for bad in [
            "",
            "ERROR: Access is denied.",
            "\"desk\\ada\",\"\"",
            "\"desk\\ada\",\"S-1-\"",
            "\"S-1-5-x\"",
            "\"desk\\ada\"",
            "\"desk\\ada\",\"1-5-21-7\"",
        ] {
            assert_eq!(sid_from_whoami(bad), None, "{bad:?}");
        }
    }

    /// The real tools: a file created in a folder that lets Users read what's in it inherits that
    /// access; after `restrict_to_owner` only the current user and SYSTEM are left.
    #[cfg(windows)]
    #[test]
    fn a_file_in_a_shared_folder_is_restricted_to_its_owner() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-owner-only-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = std::process::Command::new("icacls").arg(&dir).arg("/grant").arg("*S-1-5-32-545:(OI)(CI)R").output().unwrap();
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
        let file = dir.join("ctl.json");
        std::fs::write(&file, b"{}").unwrap();
        let me = current_user_sid().unwrap();
        // Users inherit read access, so the file is refused. (Run elevated, the file is also owned
        // by Administrators, which is the reason given first.)
        let before = sddl_of(&file).unwrap();
        assert!(before.contains(";BU)") || before.contains(";S-1-5-32-545)"), "Users inherit read access: {before}");
        assert!(foreign_access(&before, &me).is_some(), "{before}");
        restrict_to_owner(&file).unwrap();
        let sddl = sddl_of(&file).unwrap();
        assert_eq!(section(&sddl, "O:"), Some(me.as_str()), "{sddl}");
        assert_eq!(foreign_access(&sddl, &me), None, "{sddl}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder may give its user Modify but not Full control, so a file created there doesn't let
    /// its owner change the owner (no `WRITE_OWNER`). An owner can always replace the access list,
    /// though, and the user's Full control on it then allows setting the owner.
    #[cfg(windows)]
    #[test]
    fn a_file_in_a_folder_granting_only_modify_is_still_restricted() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-owner-only-modify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let me = current_user_sid().unwrap();
        let modify = std::process::Command::new("icacls")
            .arg(&dir)
            .args(["/inheritance:r", "/grant:r"])
            .arg(format!("*{me}:(OI)(CI)M"))
            .arg("*S-1-5-18:(OI)(CI)F")
            .output()
            .unwrap();
        assert!(modify.status.success(), "{}", String::from_utf8_lossy(&modify.stderr));
        let file = dir.join("ctl.json");
        std::fs::write(&file, b"{}").unwrap();
        restrict_to_owner(&file).unwrap();
        let sddl = sddl_of(&file).unwrap();
        assert_eq!((section(&sddl, "O:"), foreign_access(&sddl, &me)), (Some(me.as_str()), None), "{sddl}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An `OWNER RIGHTS` entry replaces what an owner may do implicitly. Granting only *write
    /// owner* that way, the access list can't be replaced although the owner can still be set: the
    /// file isn't taken as restricted.
    #[cfg(windows)]
    #[test]
    fn a_file_whose_access_list_cant_be_replaced_is_not_restricted() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-owner-only-ow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let me = current_user_sid().unwrap();
        let file = dir.join("ctl.json");
        std::fs::write(&file, b"{}").unwrap();
        let rights = std::process::Command::new("icacls")
            .arg(&file)
            .args(["/inheritance:r", "/grant:r"])
            .arg(format!("*{me}:(R)"))
            .arg("*S-1-3-4:(WO)")
            .output()
            .unwrap();
        assert!(rights.status.success(), "{}", String::from_utf8_lossy(&rights.stderr));
        assert!(restrict_to_owner(&file).is_err(), "the access list could not be replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When the tools fail, the functions say so: nothing is taken as restricted or as checked.
    #[cfg(windows)]
    #[test]
    fn tool_failures_are_errors() {
        let missing = std::env::temp_dir().join(format!("pdfcraft-owner-only-missing-{}", std::process::id())).join("none.json");
        assert!(restrict_to_owner(&missing).is_err(), "icacls on a missing file");
        assert!(sddl_of(&missing).is_err(), "Get-Acl on a missing file");
        let me = current_user_sid().unwrap();
        assert!(me.starts_with("S-1-5-") && me.split('-').count() >= 4, "{me}");
        assert!(!me.contains('"') && !me.contains('\\'), "the SID alone, not the account name: {me}");
    }

    #[test]
    fn the_descriptor_and_bytes_are_read_from_the_scripts_output_and_nothing_else() {
        let sddl = format!("O:{ME}D:P(A;;FA;;;{ME})");
        assert_eq!(sddl_and_bytes(&format!("{sddl}\n7B-0A-ff\r\n")), Some((sddl.clone(), vec![0x7B, 0x0A, 0xFF])));
        assert_eq!(sddl_and_bytes(&format!("{sddl}\n")), Some((sddl.clone(), Vec::new())));
        for bad in [
            String::new(),
            sddl.clone(),
            "D:P(A;;FA;;;SY)\n7B".into(),
            format!("{sddl}\n7B-2"),
            format!("{sddl}\n7B--22"),
            format!("{sddl}\n7G"),
            format!("{sddl}\n+7-22"),
        ] {
            assert_eq!(sddl_and_bytes(&bad), None, "{bad:?}");
        }
    }

    /// The bytes come with the descriptor of the same handle, capped at `max + 1` so an oversized
    /// file shows, and the restricted file is owned by the user.
    #[cfg(windows)]
    #[test]
    fn the_file_is_read_with_its_descriptor_through_one_handle() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-owner-only-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ctl.json");
        std::fs::write(&file, br#"{"port":1}"#).unwrap();
        restrict_to_owner(&file).unwrap();
        let me = current_user_sid().unwrap();
        let (sddl, bytes) = read_with_sddl(&file, 64).unwrap();
        assert_eq!(section(&sddl, "O:"), Some(me.as_str()), "{sddl}");
        assert_eq!((foreign_access(&sddl, &me), bytes.as_slice()), (None, &br#"{"port":1}"#[..]));
        assert_eq!(read_with_sddl(&file, 3).unwrap().1, br#"{"po"#);
        // A limit past what the script's `[int]` holds is capped, not handed to it.
        assert_eq!(read_with_sddl(&file, u64::MAX).unwrap().1, br#"{"port":1}"#);
        assert!(read_with_sddl(&dir.join("none.json"), 64).is_err());
        // The handle shares the file for reading only: a file someone has open for writing (or
        // could rename or delete) isn't read, and so can't change between the check and the read.
        let writer = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        assert!(read_with_sddl(&file, 64).is_err(), "a file open for writing is not read");
        drop(writer);
        assert!(read_with_sddl(&file, 64).is_ok());
        // When the owner can't be set, the file isn't taken as restricted, although its access
        // list could still be replaced (an owner may always change that).
        // Everyone is denied it, an entry `/grant:r` for the user leaves in place.
        let deny = std::process::Command::new("icacls").arg(&file).arg("/deny").arg("*S-1-1-0:(WO)").output().unwrap();
        assert!(deny.status.success(), "{}", String::from_utf8_lossy(&deny.stderr));
        assert!(restrict_to_owner(&file).is_err(), "setting the owner failed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sections_stop_at_the_next_tag() {
        assert_eq!(section("O:S-1-5-18G:S-1-5-32-544D:(A;;FA;;;SY)", "O:"), Some("S-1-5-18"));
        assert_eq!(section("O:BAG:SYD:P(A;;FA;;;SY)S:(AU;SA;FA;;;WD)", "D:"), Some("P(A;;FA;;;SY)"));
        assert_eq!(section("O:BA", "D:"), None);
    }
}
