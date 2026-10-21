//! Owner-only files on Windows (#3922): the `--control` token file must be readable by its user
//! alone, as mode 0600 makes it on Unix.
//!
//! A file created in a folder inherits the folder's access list, and folders such as a new folder
//! at a drive root, `C:\Users\Public` or `C:\ProgramData` let every signed-in user read what is
//! created in them. [`restrict_to_owner`] replaces the inherited list with the current user and
//! SYSTEM only (`icacls`). [`foreign_access`] reads a file's security descriptor in SDDL form, which
//! names accounts by SID and so reads the same in every Windows language, and says why another
//! user could have written or could read it. Both run the tools Windows ships rather than calling
//! the security API directly, which would need `unsafe`.

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
/// what it inherited from its folder (`icacls /inheritance:r /grant:r`).
#[cfg(windows)]
pub fn restrict_to_owner(path: &std::path::Path) -> std::io::Result<()> {
    let me = current_user_sid()?;
    let out = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("*{me}:(F)"))
        .arg("*S-1-5-18:(F)")
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("icacls could not restrict the file to its owner: {}", String::from_utf8_lossy(&out.stderr).trim())))
    }
}

/// The security descriptor of `path` in SDDL form, owner included (`Get-Acl`). The path reaches
/// PowerShell through an environment variable, never as script text.
#[cfg(windows)]
pub fn sddl_of(path: &std::path::Path) -> std::io::Result<String> {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "(Get-Acl -LiteralPath $env:PDFCRAFT_ACL_PATH).Sddl"])
        .env("PDFCRAFT_ACL_PATH", path)
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && text.contains("O:") {
        Ok(text)
    } else {
        Err(std::io::Error::other("PowerShell Get-Acl could not read the file's owner and access list"))
    }
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
        let before = foreign_access(&sddl_of(&file).unwrap(), &me);
        assert!(before.as_deref().is_some_and(|p| p.contains("BU") || p.contains("S-1-5-32-545")), "Users inherit read access: {before:?}");
        restrict_to_owner(&file).unwrap();
        let sddl = sddl_of(&file).unwrap();
        assert_eq!(foreign_access(&sddl, &me), None, "{sddl}");
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
    fn sections_stop_at_the_next_tag() {
        assert_eq!(section("O:S-1-5-18G:S-1-5-32-544D:(A;;FA;;;SY)", "O:"), Some("S-1-5-18"));
        assert_eq!(section("O:BAG:SYD:P(A;;FA;;;SY)S:(AU;SA;FA;;;WD)", "D:"), Some("P(A;;FA;;;SY)"));
        assert_eq!(section("O:BA", "D:"), None);
    }
}
