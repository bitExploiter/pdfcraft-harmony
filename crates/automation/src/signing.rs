//! Signature tools: list and validate signatures, create a self-signed digital ID, sign a
//! document (with a local digital ID or a remote signing service's), and manage trusted
//! certificates. Rectangles are points from the top-left of the displayed page.

use std::sync::Arc;

use pdfcraft_engine::sign::remote::{self, Account, AuthRole, HttpTransport, OtpKind, OtpSent, Protocol, RemoteFault, RemoteIdentity, Secrets};
use pdfcraft_engine::sign::{self, Certificate, DigitalId, Modification, Name, PrivateKey};
use pdfcraft_engine::{SignOptions, SignatureInfo, SignatureStatus};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// `remote`'s error as the tool's: one the caller fixes by changing the call (a user name,
/// password, PIN or OTP the service turned down, a secret or the Aruba domain left out, a
/// credential the account doesn't have, every Aruba refusal) is `InvalidArgs`, anything else
/// (an unreachable service, an answer that makes no sense, a signature that doesn't verify) a
/// failure. The sign crate, which words the errors, tells them apart ([`remote::fault_of`]).
fn remote_error(e: impl std::fmt::Display) -> ToolError {
    let m = e.to_string();
    match remote::fault_of(&m) {
        RemoteFault::Caller => bad(m),
        RemoteFault::Service => failed(m),
    }
}

/// The longest value a remote call passes on to the service (address, user name, domain, HSM
/// type, credential, password, PIN, OTP): far more than any real one, and it keeps what a call
/// can have PdfCraft send small.
const MAX_REMOTE_VALUE: usize = 1024;

/// Refuse `value` (never quoted: it may be a secret) when it is longer than [`MAX_REMOTE_VALUE`].
fn short(key: &str, value: &str) -> Result<()> {
    if value.len() > MAX_REMOTE_VALUE { Err(bad(format!("{key} is longer than {MAX_REMOTE_VALUE} bytes"))) } else { Ok(()) }
}

/// Refuse an account no service should be asked about: an address that isn't https (but to
/// this computer), a blank user name, an Aruba account without its domain (which Aruba answers
/// with "Invalid Parameters"), or an overlong value. Checked here as well as by `remote`, so
/// the caller hears of it before anything else, network or none.
fn check_account(account: &Account) -> Result<()> {
    for (key, value) in
        [("url", Some(&account.url)), ("user", Some(&account.user)), ("domain", account.domain.as_ref()), ("hsm", account.hsm.as_ref())]
    {
        value.map_or(Ok(()), |v| short(key, v))?;
    }
    remote::check_url(&account.url).map_err(|e| bad(e.to_string()))?;
    if account.user.trim().is_empty() {
        return Err(bad("user must not be empty"));
    }
    if account.protocol == Protocol::Arss && account.domain.as_deref().is_none_or(|d| d.trim().is_empty()) {
        return Err(bad(remote::NEEDS_DOMAIN));
    }
    Ok(())
}

/// The account and credential a `remote:` id names, checked as `sign_remote_ids` checks its
/// arguments: an id is the caller's to write, not only one `sign_remote_ids` returned.
fn remote_reference(id: &str) -> Result<(Account, String)> {
    if !id.starts_with("remote:") {
        return Err(bad("id must be a remote: id from sign_remote_ids"));
    }
    let (account, credential) = remote::parse_reference(id).map_err(|e| bad(e.to_string()))?;
    check_account(&account)?;
    short("credential", &credential)?;
    Ok((account, credential))
}

/// The signing service password, which every remote call needs.
fn service_password<'a>(a: &'a Args) -> Result<&'a str> {
    let password =
        a.opt_str("password")?.filter(|p| !p.is_empty()).ok_or_else(|| bad("remote signing needs password, the signing service password"))?;
    short("password", password)?;
    Ok(password)
}

/// How many `remote:` ids a sent code is remembered for ([`Automation::remote_sent`]).
const MAX_SENT: usize = 16;

/// A secret for one signature (`pin`, `otp`), when given.
fn remote_secret(a: &Args, key: &str) -> Result<Option<String>> {
    let value = a.opt_str(key)?;
    value.map_or(Ok(()), |v| short(key, v))?;
    Ok(value.map(str::to_string))
}

/// A throwaway digital ID, made afresh for each call: `sign_document` signs a remote signature
/// with it first, in memory, to have the document refuse what it would refuse before the
/// service is asked. Nothing it signs is kept, so it is a P-256 key (the quickest to make, well
/// under a millisecond with its certificate) that no one trusts.
fn rehearsal_id(now: i64) -> Result<DigitalId> {
    let key = PrivateKey::generate_p256().map_err(failed)?;
    let certificate = Certificate::self_signed(&Name::build("PdfCraft", "", "", "", ""), &key, time(now), 1, &[1]).map_err(failed)?;
    Ok(DigitalId { key, certificate, chain: Vec::new(), friendly_name: None })
}

fn remote_id_json(id: &RemoteIdentity) -> Value {
    let c = &id.certificate;
    let otp = match id.auth.otp {
        OtpKind::None => "none",
        OtpKind::Typed => "typed",
        OtpKind::Sent => "sent",
    };
    let auth: Vec<Value> = id
        .auth
        .objects
        .iter()
        .map(|o| {
            let role = match o.role {
                AuthRole::Pin => "pin",
                AuthRole::Otp => "otp",
                AuthRole::Confirm => "confirm",
            };
            json!({ "id": o.id, "type": o.kind, "label": o.label, "format": o.format, "role": role })
        })
        .collect();
    json!({
        "id": id.reference(),
        "name": c.display_name(),
        "issuer": c.issuer.display_name(),
        "email": c.subject.email(),
        "expires": c.not_after.to_string(),
        "pin": id.auth.pin,
        "otp": otp,
        "auth": auth,
        "certificate": cert_json(c),
    })
}

/// Seconds since 1970 → a DER time.
fn time(secs: i64) -> sign::Time {
    sign::Time::from_unix(secs)
}

/// The longest label `sign_document` takes (characters): a label is a few words.
const MAX_LABEL_CHARS: usize = 200;

/// `appearance`: which items the visible signature shows; those not given keep the default
/// (name, date and labels on; reason, location and distinguished name off).
fn appearance_arg(v: &Value) -> Result<sign::Appearance> {
    const KEYS: &str = "name, date, reason, location, distinguished_name, labels";
    let o = v.as_object().ok_or_else(|| bad(format!("appearance must be an object of true/false items ({KEYS})")))?;
    let mut shown = sign::Appearance::default();
    for (key, value) in o {
        let on = value.as_bool().ok_or_else(|| bad(format!("appearance.{key} must be true or false")))?;
        let item = match key.as_str() {
            "name" => &mut shown.name,
            "date" => &mut shown.date,
            "reason" => &mut shown.reason,
            "location" => &mut shown.location,
            "distinguished_name" => &mut shown.distinguished_name,
            "labels" => &mut shown.labels,
            other => return Err(bad(format!("unknown appearance item {other:?} ({KEYS})"))),
        };
        *item = on;
    }
    Ok(shown)
}

/// `labels`: the words of the visible signature's labels; those not given keep the English. Each
/// must be drawable in the signature's font. Nobody sees a preview here, so a label it can't draw
/// is refused even when this signature doesn't show it.
fn labels_arg(v: &Value) -> Result<sign::AppearanceLabels> {
    const KEYS: &str = "signed_by, distinguished_name, reason, location, date";
    let o = v.as_object().ok_or_else(|| bad(format!("labels must be an object of strings ({KEYS})")))?;
    let mut labels = sign::AppearanceLabels::default();
    for (key, value) in o {
        let text = value.as_str().ok_or_else(|| bad(format!("labels.{key} must be a string")))?;
        if text.chars().count() > MAX_LABEL_CHARS {
            return Err(bad(format!("labels.{key} is longer than {MAX_LABEL_CHARS} characters")));
        }
        let label = match key.as_str() {
            "signed_by" => &mut labels.signed_by,
            "distinguished_name" => &mut labels.distinguished_name,
            "reason" => &mut labels.reason,
            "location" => &mut labels.location,
            "date" => &mut labels.date,
            other => return Err(bad(format!("unknown label {other:?} ({KEYS})"))),
        };
        *label = text.to_string();
    }
    if let Some((field, c)) = labels.first_undrawable() {
        return Err(bad(format!(
            "labels.{field}: {}",
            pdfcraft_engine::undrawable_message(&format!("the {field} label of the visible signature"), c)
        )));
    }
    Ok(labels)
}

pub(crate) fn cert_json(c: &Certificate) -> Value {
    json!({
        "name": c.display_name(),
        "subject": c.subject.display(),
        "issuer": c.issuer.display(),
        "email": c.subject.email(),
        "serial": c.serial_hex(),
        "valid_from": c.not_before.to_string(),
        "valid_to": c.not_after.to_string(),
        "key": c.public_key.describe(),
        "self_signed": c.is_self_signed(),
        "sha256": c.fingerprint(),
    })
}

impl Automation {
    fn sig_json(&self, a: &Args, s: &SignatureInfo) -> Result<Value> {
        let doc = self.doc(a)?;
        let rect = match (s.page, s.rect) {
            (Some(p), Some(r)) if s.visible => doc.info.pages.get(p).map(|pi| {
                let (a0, a1) = (pi.user_to_view(r[0] as f32, r[3] as f32), pi.user_to_view(r[2] as f32, r[1] as f32));
                json!([a0[0].min(a1[0]), a0[1].min(a1[1]), a0[0].max(a1[0]), a0[1].max(a1[1])])
            }),
            _ => None,
        };
        let (modification, changes) = match &s.modification {
            Modification::None => ("none", Vec::new()),
            Modification::Allowed(c) => ("allowed", c.clone()),
            Modification::Disallowed(c) => ("disallowed", c.clone()),
        };
        Ok(json!({
            "field": s.field,
            "signed": s.signed,
            "status": if !s.signed { "unsigned" } else { match s.status { SignatureStatus::Valid => "valid", SignatureStatus::Unknown => "unknown", SignatureStatus::Invalid => "invalid" } },
            "summary": s.summary(),
            "signer": s.signer,
            "certificate": s.certificate.as_ref().map(cert_json),
            "chain": s.chain.iter().skip(1).map(cert_json).collect::<Vec<_>>(),
            "date": s.date,
            "reason": s.reason,
            "location": s.location,
            "contact": s.contact,
            "certify": s.certify,
            "page": s.page.map(|p| p + 1),
            "rect": rect,
            "visible": s.visible,
            "revision": s.revision,
            "sub_filter": s.sub_filter,
            "algorithm": s.algorithm,
            "timestamp": s.timestamp,
            "timestamp_time": s.timestamp_time.map(|t| t.to_string()),
            "modification": modification,
            "changes": changes,
            // Scripts read this, so the sentences stay in English, whatever the interface reads in.
            "details": s.details.iter().map(|d| d.message.clone()).collect::<Vec<_>>(),
        }))
    }

    pub(crate) fn sign_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let list: Vec<Value> = doc.signatures.iter().map(|s| self.sig_json(a, s)).collect::<Result<_>>()?;
        let signed = doc.signatures.iter().filter(|s| s.signed).count();
        let all_valid = signed > 0 && doc.signatures.iter().filter(|s| s.signed).all(|s| s.status == SignatureStatus::Valid);
        Ok(json!({ "count": list.len(), "signed": signed, "all_valid": all_valid, "signatures": list }))
    }

    pub(crate) fn sign_id_create(&mut self, a: &Args) -> Result<Value> {
        let name = a.str("name")?.trim().to_string();
        if name.is_empty() {
            return Err(bad("name must not be empty"));
        }
        let password = a.str("password")?;
        if password.chars().count() < sign::pkcs12::MIN_PASSWORD_CHARS {
            return Err(bad(format!("the password must have at least {} characters", sign::pkcs12::MIN_PASSWORD_CHARS)));
        }
        let path = self.resolve(a.str("path")?, true)?;
        let key = match a.opt_str("key")?.unwrap_or("rsa2048") {
            "rsa2048" => PrivateKey::generate_rsa(2048),
            "rsa3072" => PrivateKey::generate_rsa(3072),
            "rsa4096" => PrivateKey::generate_rsa(4096),
            "p256" => PrivateKey::generate_p256(),
            other => return Err(bad(format!("unknown key {other:?} (rsa2048, rsa3072, rsa4096, p256)"))),
        }
        .map_err(failed)?;
        let dn = Name::build(
            &name,
            a.opt_str("unit")?.unwrap_or(""),
            a.opt_str("organization")?.unwrap_or(""),
            a.opt_str("email")?.unwrap_or(""),
            a.opt_str("country")?.unwrap_or(""),
        );
        let years = a.opt_int("years")?.unwrap_or(5).clamp(1, 50) as u32;
        let now = self.session.now_secs();
        let serial = sign::keys::DigestAlg::Sha256.digest(&[name.as_bytes(), &now.to_be_bytes()])[..8].to_vec();
        let cert = Certificate::self_signed(&dn, &key, time(now), years, &serial).map_err(failed)?;
        let id = DigitalId { key, certificate: cert, chain: Vec::new(), friendly_name: Some(name) };
        let p12 = sign::pkcs12::write(&id, password).map_err(failed)?;
        write_atomic(&path, &p12)?;
        Ok(json!({ "path": path.to_string_lossy(), "certificate": cert_json(&id.certificate) }))
    }

    /// The network for remote signing, or why there is none.
    fn remote_transport(&self) -> Result<Arc<dyn HttpTransport>> {
        self.remote.clone().ok_or_else(|| failed("remote signing needs network access, which this host did not enable"))
    }

    /// Sign in to a signing service and list the account's credentials that can sign.
    pub(crate) fn sign_remote_ids(&self, a: &Args) -> Result<Value> {
        let protocol = match a.str("protocol")? {
            "csc" => Protocol::Csc,
            "arss" => Protocol::Arss,
            other => return Err(bad(format!("unknown protocol {other:?} (csc, arss)"))),
        };
        let url = match (protocol, a.opt_str("url")?) {
            (_, Some(url)) => url.to_string(),
            (Protocol::Arss, None) => remote::ARSS_ENDPOINT.to_string(),
            (Protocol::Csc, None) => return Err(bad("protocol csc needs url, the service's CSC API address (ending in /csc/v2 or /csc/v1)")),
        };
        // Trimmed, as they are checked: " dom " is the domain "dom".
        let given = |key: &str| a.opt_str(key).map(|v| v.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string));
        let (domain, hsm) = (given("domain")?, given("hsm")?);
        if protocol == Protocol::Csc && (domain.is_some() || hsm.is_some()) {
            return Err(bad("domain and hsm are for protocol arss (Aruba); a CSC service takes neither"));
        }
        let account = Account { protocol, url, user: a.str("user")?.trim().to_string(), domain, hsm };
        check_account(&account)?;
        let password = service_password(a)?;
        let ids = remote::list(&*self.remote_transport()?, &account, password).map_err(remote_error)?;
        Ok(json!({ "count": ids.len(), "ids": ids.iter().map(remote_id_json).collect::<Vec<_>>() }))
    }

    /// Have the service of a `remote:` id send the OTP for its next signature, which then puts
    /// it under the object it was sent for (of an SMS and an app as alternatives, the SMS).
    pub(crate) fn sign_remote_send_otp(&mut self, a: &Args) -> Result<Value> {
        let (account, credential) = remote_reference(a.str("id")?)?;
        let password = service_password(a)?;
        let sent = remote::send_otp(&*self.remote_transport()?, &account, &credential, password).map_err(remote_error)?;
        let reference = remote::reference(&account, &credential);
        self.remote_sent.retain(|r| *r != reference);
        if self.remote_sent.len() >= MAX_SENT {
            self.remote_sent.remove(0);
        }
        self.remote_sent.push(reference);
        Ok(match sent {
            OtpSent::Sent => json!({ "sent": true }),
            OtpSent::Challenge(challenge) => json!({ "challenge": challenge }),
        })
    }

    /// A `remote:` id's digital ID. Opening it signs in and reads the certificate (the CMS names
    /// it before anything is signed); the OTP is spent by the signature. The OTP counts as sent
    /// when `sign_remote_send_otp` had it sent for this id and no signature has used it since.
    fn open_remote(&self, a: &Args, id: &str) -> Result<DigitalId> {
        let (account, credential) = remote_reference(id)?;
        let otp_sent = self.remote_sent.contains(&remote::reference(&account, &credential));
        let secrets = Secrets { password: service_password(a)?.to_string(), pin: remote_secret(a, "pin")?, otp: remote_secret(a, "otp")?, otp_sent };
        remote::digital_id(self.remote_transport()?, &account, &credential, secrets).map_err(remote_error)
    }

    fn open_id(&self, a: &Args) -> Result<DigitalId> {
        // A signing service's credential: "remote:csc?…" or "remote:arss?…". It names no file,
        // so --root has nothing to confine.
        let id = a.str("id")?;
        if id.starts_with("remote:") {
            return self.open_remote(a, id);
        }
        // A macOS Keychain identity: "keychain:<fingerprint>" or "keychain:<common name>".
        if let Some(r) = a.str("id")?.strip_prefix("keychain:") {
            #[cfg(target_os = "macos")]
            return sign::keychain::find(&format!("keychain:{r}")).map_err(failed);
            #[cfg(not(target_os = "macos"))]
            return Err(failed(format!("keychain:{r}: Keychain identities are only available on macOS")));
        }
        if let Some(r) = a.str("id")?.strip_prefix("windows:") {
            #[cfg(target_os = "windows")]
            return sign::windows::find(&format!("windows:{r}")).map_err(failed);
            #[cfg(not(target_os = "windows"))]
            return Err(failed(format!("windows:{r}: Windows certificate store identities are only available on Windows")));
        }
        let path = self.resolve(a.str("id")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        sign::pkcs12::open(&bytes, a.opt_str("password")?.unwrap_or("")).map_err(|e| match e {
            sign::SignError::WrongPassword => bad("the digital ID password is incorrect"),
            e => failed(e),
        })
    }

    pub(crate) fn sign_document(&mut self, a: &Args) -> Result<Value> {
        let remote = a.str("id")?.starts_with("remote:");
        if !remote && (a.opt_str("pin")?.is_some() || a.opt_str("otp")?.is_some()) {
            return Err(bad("pin and otp are for a remote: id from sign_remote_ids"));
        }
        let doc = self.doc(a)?;
        let doc_id = doc.id;
        let field = a.opt_str("field")?.map(str::to_string);
        let page = match a.opt_int("page")? {
            Some(p) if p >= 1 && (p as usize) <= doc.info.pages.len() => p as usize - 1,
            Some(p) => return Err(bad(format!("page {p} does not exist"))),
            None => 0,
        };
        let rect = match a.nums::<4>("rect")? {
            Some(r) => {
                let p = doc.info.pages.get(page).ok_or_else(|| bad("the document has no page to put the signature on"))?;
                let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
                let u = [u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64];
                if u[2] - u[0] < 4.0 || u[3] - u[1] < 4.0 {
                    return Err(bad("the signature rectangle is too small"));
                }
                Some(u)
            }
            None => None,
        };
        let certify = match a.get("certify") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(match s.as_str() {
                "no_changes" => 1,
                "form_fill" => 2,
                "comments" => 3,
                other => return Err(bad(format!("unknown certify level {other:?} (no_changes, form_fill, comments)"))),
            }),
            Some(v) => Some(v.as_u64().filter(|n| (1..=3).contains(n)).ok_or_else(|| bad("certify must be 1, 2 or 3"))? as u8),
        };
        let graphic = match a.get("graphic") {
            None | Some(Value::Null) => None,
            Some(g) if rect.is_none() && field.is_none() => return Err(bad(format!("graphic {g} needs a visible signature: pass rect or field"))),
            Some(g) => Some(self.signature_graphic(g)?),
        };
        let visible = rect.is_some() || field.is_some();
        let appearance = match a.get("appearance") {
            None | Some(Value::Null) => sign::Appearance::default(),
            Some(_) if !visible => return Err(bad("appearance needs a visible signature: pass rect or field")),
            Some(v) => appearance_arg(v)?,
        };
        let appearance_labels = match a.get("labels") {
            None | Some(Value::Null) => sign::AppearanceLabels::default(),
            Some(_) if !visible => return Err(bad("labels needs a visible signature: pass rect or field")),
            Some(v) => labels_arg(v)?,
        };
        let out = self.resolve(a.str("out")?, true)?;
        let opts = SignOptions {
            field,
            page,
            rect,
            reason: a.opt_str("reason")?.map(str::to_string),
            location: a.opt_str("location")?.map(str::to_string),
            contact: a.opt_str("contact")?.map(str::to_string),
            certify,
            appearance,
            appearance_labels,
            graphic,
            ..SignOptions::default()
        };
        let job = self.session.sign_job(doc_id, opts).map_err(failed)?;
        // A graphic the signer refuses is the caller's to fix.
        let refused = |e| match e {
            pdfcraft_engine::EditError::Sign(m) if m.starts_with("the signature graphic") => bad(m),
            e => failed(e),
        };
        // The digital ID comes last: a remote one signs in to its service when opened, which a
        // call that is wrong anyway shouldn't cost (a CSC sign-in is only revoked by signing).
        // The engine has no check of a signature's preconditions short of signing, so a remote
        // signature is first made in memory with a throwaway local key and thrown away:
        // whatever the document refuses (a field that isn't there or is already signed, its
        // permissions, a pending security change, the graphic) is refused before the service
        // hears of it. It costs one more signature computed here, about an incremental save of
        // the document, with no request and nothing written.
        if remote {
            job.run(&rehearsal_id(self.session.now_secs())?).map_err(refused)?;
        }
        let id = self.open_id(a)?;
        let signed = job.run(&id).map_err(|e| match e {
            // The service turning down the PIN or OTP is the caller's too; a signature that
            // doesn't verify, or a refusal of what PdfCraft computed, is a failure.
            pdfcraft_engine::EditError::Sign(m) if remote && remote::fault_of(&m) == RemoteFault::Caller => bad(m),
            e => refused(e),
        })?;
        if remote {
            // That code is spent: the next one is sent again, or read from an app.
            let (account, credential) = remote_reference(a.str("id")?)?;
            let reference = remote::reference(&account, &credential);
            self.remote_sent.retain(|r| *r != reference);
        }
        write_atomic(&out, &signed)?;
        let path = out.to_string_lossy().into_owned();
        self.session.mark_signed(doc_id, signed.clone(), Some(path.clone())).map_err(failed)?;
        let doc = self.doc(a)?;
        let newest = doc.signatures.iter().filter(|s| s.signed).max_by_key(|s| s.revision).cloned();
        Ok(json!({ "path": path, "bytes": signed.len(), "signature": newest.map(|s| self.sig_json(a, &s)).transpose()? }))
    }

    /// `{"image": path}` (PNG or JPEG), `{"text": name}` (typed in the script font) or
    /// `{"strokes": [[[x, y], …], …]}` (y up; the drawing pad is 1 wide).
    fn signature_graphic(&self, g: &Value) -> Result<sign::Graphic> {
        let kinds = "graphic must be an object with exactly one of image (a PNG or JPEG path), text, or strokes";
        let Some(o) = g.as_object().filter(|o| o.len() == 1) else { return Err(bad(kinds)) };
        if let Some(path) = o.get("image") {
            let path = self.resolve(path.as_str().ok_or_else(|| bad("graphic.image must be a path"))?, false)?;
            let file = std::fs::File::open(&path).map_err(|e| bad(format!("graphic.image {}: {e}", path.display())))?;
            return Ok(pdfcraft_engine::SignatureImage::read(file).map_err(|e| bad(e.to_string()))?.graphic());
        }
        if let Some(text) = o.get("text") {
            let text = text.as_str().ok_or_else(|| bad("graphic.text must be a string"))?;
            // Nobody sees a preview here: refuse what the script font and its fallback would leave out.
            if let Some(c) = pdfcraft_engine::script_missing(text) {
                return Err(bad(format!(
                    "the signature script font can't draw \"{}\" (U+{:04X}) in graphic.text; type it in Latin, Cyrillic or Greek letters, or pass graphic.image instead",
                    c.escape_debug(),
                    u32::from(c)
                )));
            }
            return pdfcraft_engine::typed_signature_graphic(text).ok_or_else(|| {
                bad(format!(
                    "graphic.text {text:?} has nothing the script font can draw (at most {} characters)",
                    pdfcraft_engine::MAX_SIGNATURE_CHARS
                ))
            });
        }
        if let Some(strokes) = o.get("strokes") {
            let strokes: Vec<Vec<[f64; 2]>> =
                serde_json::from_value(strokes.clone()).map_err(|e| bad(format!("graphic.strokes must be strokes of [x, y] points: {e}")))?;
            return Ok(sign::Graphic::Strokes(strokes));
        }
        Err(bad(kinds))
    }

    pub(crate) fn sign_trust(&mut self, a: &Args) -> Result<Value> {
        let mut certs: Vec<Certificate> =
            if a.opt_bool("clear")?.unwrap_or(false) { Vec::new() } else { self.session.trusted_certificates().to_vec() };
        let paths: Vec<String> = match a.get("paths") {
            None => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or_else(|| bad("paths must be an array"))?
                .iter()
                .map(|p| p.as_str().map(str::to_string).ok_or_else(|| bad("paths must be strings")))
                .collect::<Result<_>>()?,
        };
        for p in paths {
            let path = self.resolve(&p, false)?;
            let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
            let lower = p.to_ascii_lowercase();
            let found = if lower.ends_with(".p12") || lower.ends_with(".pfx") {
                let id = sign::pkcs12::open(&bytes, a.opt_str("password")?.unwrap_or("")).map_err(failed)?;
                std::iter::once(id.certificate).chain(id.chain).collect()
            } else {
                sign::x509::load_certificates(&bytes).map_err(failed)?
            };
            for c in found {
                if !certs.iter().any(|t| t.raw == c.raw) {
                    certs.push(c);
                }
            }
        }
        self.session.set_trusted_certificates(certs);
        Ok(json!({ "trusted": self.session.trusted_certificates().iter().map(cert_json).collect::<Vec<_>>() }))
    }
}
