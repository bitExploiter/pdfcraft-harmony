//! Remote signing ("firma remota"): a key held by a signing service, used once per signature
//! with a one-time password (OTP), the way Italian and EU qualified signatures are made.
//!
//! PdfCraft builds the PDF, its byte range and the CMS here and sends the service only the
//! digest of the CMS signed attributes, so the document never leaves the machine. Two
//! protocols: the Cloud Signature Consortium API ([`Protocol::Csc`]: v2, with the v1 dialect
//! tolerated; HTTP Basic sign-in only, OAuth isn't supported yet) and Aruba's Remote Sign Server
//! ([`Protocol::Arss`]: the SOAP `signhash` operation).
//!
//! Like [`TimestampAuthority`](crate::TimestampAuthority), the network is the caller's: this
//! crate never opens a socket, it hands each exchange to an [`HttpTransport`]. Secrets (the
//! service password, the PIN, the OTP, and the tokens a service hands back) are never stored,
//! logged, printed by `Debug` or quoted in an error. Every signature a service returns is
//! verified against the certificate before it is used, so a service that signs the wrong thing
//! fails the signing instead of producing a broken file.
//!
//! # Where the protocols come from
//!
//! Clean room (AGENTS.md §2): only public specifications, the providers' own documents and
//! black-box answers of a public endpoint. No other CSC or ARSS client was read, nor code samples
//! seen in search results.
//!
//! - CSC: the Cloud Signature Consortium's "Architectures and protocols for remote signature
//!   applications", v2.2.0.0 (<https://cloudsignatureconsortium.org/wp-content/uploads/2025/11/csc-api.pdf>),
//!   v2.0.0.2 (<https://cloudsignatureconsortium.org/wp-content/uploads/2023/04/csc-api-v2.0.0.2.pdf>)
//!   and v1.0.4.0 (<https://cloudsignatureconsortium.org/wp-content/uploads/2020/01/CSC_API_V1_1.0.4.0.pdf>),
//!   and its Data Model v1.0.0 (<https://cloudsignatureconsortium.org/wp-content/uploads/2025/10/csc-dm.pdf>),
//!   which v2.2 points to for `signAlgo` and base64. Sections used (v2.2, with v2.0 and v1.0.4 for
//!   the older field names): §7 general rules, §8.1 HTTP Basic and Bearer, §10 errors, §11.1
//!   `info`, §11.2–11.3 `auth/login` and `auth/revoke`, §11.6–11.7 `credentials/list` and
//!   `credentials/info` with `auth/expression`, §11.8 `credentials/authorize` with the types of
//!   authentication object and the out-of-band response, §11.9–11.10 `authorizeCheck` and
//!   `getChallenge`, §11.12 `sendOTP`, §11.13 `signatures/signHash`; v1.0.4 §11.4–11.9 for the
//!   v1 dialect.
//! - ARSS: Aruba's public WSDL (<https://arss.arubapec.it/ArubaSignService/ArubaSignService?wsdl>)
//!   for the messages; for what they mean, Aruba PEC's certification practice statement MGA_A_66
//!   v1.13 (<https://www.pec.it/documents/tc-files/it/21_firmadigitalemanualeoperativo.pdf>: RSA
//!   2048 keys, single-use OTPs) and its Firma Remota manual v0.3
//!   (<https://www.pec.it/documents/tc-files/it/39_firmaremotaguidaattivazioneinstallazioneutilizz.pdf>:
//!   user name, password, then the OTP for each signature). Aruba publishes no API manual, so the
//!   rest was observed: unauthenticated calls to the public endpoint with made-up values (no
//!   account) on 2026-10-21 gave the `status KO` answers the tests use ("Empty Identity", code
//!   0002; "Invalid Parameter. %s" for namespace-qualified children; "Credentials Error", 0003),
//!   and showed that `listCert` without the domain (`typeOtpAuth`) answers "Invalid Parameters"
//!   while a made-up domain answers "Error 0003 - Invalid User Credentials": the domain is
//!   required.

mod arss;
mod csc;
#[cfg(any(test, feature = "mock"))]
pub mod mock;

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey, Scheme};
use crate::{Certificate, DigitalId, SignError};

/// One HTTP exchange; the caller (the app or the CLI) owns the network.
pub trait HttpTransport: Send + Sync {
    /// POST `body` to `url` with `headers`, and return the status and body of whatever comes
    /// back, 4xx and 5xx included: the protocols put their errors in the body. Must not follow
    /// redirects, and should read at most [`MAX_RESPONSE`]` + 1` bytes of a body: the one byte
    /// more is how an answer too large to use is told from one that fits, so never hand back a
    /// body cut at exactly `MAX_RESPONSE`.
    fn post(&self, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, SignError>;

    /// Wait `d` before polling a service again (an authorization the user confirms on a phone).
    /// Tests answer at once.
    fn wait(&self, d: Duration) {
        // The browser can't block; there the polls simply come one after another.
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(d);
        #[cfg(target_arch = "wasm32")]
        let _ = d;
    }

    /// A monotonic clock reading, for how long a service is waited for, or `None` where there
    /// is no clock (the browser: the count of polls alone bounds them there). Tests keep a
    /// clock of their own, which their waits move on.
    fn clock(&self) -> Option<Duration> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            Some(START.get_or_init(std::time::Instant::now).elapsed())
        }
        #[cfg(target_arch = "wasm32")]
        None
    }
}

/// What a service answered.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Only the status and the body's length: a body can carry an access token or the SAD.
impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HttpResponse {{ status: {}, body: <{} bytes> }}", self.status, self.body.len())
    }
}

/// The largest answer read from a signing service (1 MiB): a certificate list is a few KiB.
pub const MAX_RESPONSE: usize = 1 << 20;

/// Aruba's Remote Sign Server, which Aruba's own guide says not to change.
pub const ARSS_ENDPOINT: &str = "https://arss.arubapec.it/ArubaSignService/ArubaSignService";

/// The protocol a signing service speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// The Cloud Signature Consortium API (v2, v1 tolerated).
    Csc,
    /// Aruba Remote Sign Server (SOAP).
    Arss,
}

/// Where and as whom to sign: everything but the secrets. Stored as a [`reference()`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub protocol: Protocol,
    /// CSC: the base address, ending in `/csc/v2` (or `/csc/v1`). ARSS: the endpoint.
    pub url: String,
    pub user: String,
    /// ARSS: the domain Aruba gives with the account (`typeOtpAuth`, the "dominio"), which
    /// Aruba requires. Unused for CSC.
    pub domain: Option<String>,
    /// ARSS: the HSM type (`typeHSM`), when Aruba gives one. Unused for CSC.
    pub hsm: Option<String>,
}

/// A credential of an account that can sign: its certificate and what it asks for.
#[derive(Clone, Debug)]
pub struct RemoteIdentity {
    pub account: Account,
    /// CSC `credentialID`, ARSS `certID`.
    pub credential: String,
    pub certificate: Certificate,
    /// The issuers the service gave, without the signer's certificate.
    pub chain: Vec<Certificate>,
    pub auth: AuthNeeds,
}

impl RemoteIdentity {
    /// The reference that names this identity (see [`reference()`]).
    pub fn reference(&self) -> String {
        reference(&self.account, &self.credential)
    }
}

/// What a signature with a credential asks the user for, besides the service password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthNeeds {
    /// A static PIN.
    pub pin: bool,
    pub otp: OtpKind,
    /// The service's own description of each value, so an interface can label its fields.
    pub objects: Vec<AuthObject>,
    /// Whether a signature is confirmed on the user's other device while it is made.
    pub confirm: Confirmation,
}

/// When a credential's signatures are confirmed on the user's other device (CSC `PasswordOOB`,
/// `ChallengeResponseOOB`) while they are made, the PIN given: what an interface says while it
/// waits for the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    Never,
    /// Only a signature made without the code: the confirmation is the code's alternative.
    WithoutCode,
    /// Every signature, with the code or without.
    Always,
}

/// How the user gets the one-time password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtpKind {
    /// None is asked.
    None,
    /// Read from an authenticator app or a token.
    Typed,
    /// The service sends it (SMS, a call) when asked with [`send_otp`]. Aruba accounts are
    /// listed so whatever their OTP device: a code from an app is typed without sending.
    Sent,
}

/// One value a credential's authorization takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthObject {
    /// The name the value is sent under (CSC `authData` id).
    pub id: String,
    /// The service's type for it (CSC v2 `Password`, `ChallengeResponse`…; `PIN` or `OTP`
    /// for CSC v1 and ARSS).
    pub kind: String,
    pub label: Option<String>,
    /// `A` (letters and digits) or `N` (digits), when the service says.
    pub format: Option<String>,
    pub role: AuthRole,
}

/// Which of the user's secrets an [`AuthObject`] carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthRole {
    Pin,
    Otp,
    /// No value: the user confirms on another device (CSC `PasswordOOB`, `ChallengeResponseOOB`).
    Confirm,
}

/// The secrets for one signature. Never stored, and `Debug` prints none of them.
pub struct Secrets {
    /// The signing service password.
    pub password: String,
    pub pin: Option<String>,
    pub otp: Option<String>,
    /// The OTP is one [`send_otp`] had the service send (Send code), not one read from an app
    /// or a token: of a credential's codes offered as alternatives (an SMS or an app), it goes
    /// under the one `send_otp` had it sent for, and otherwise under the first of them.
    pub otp_sent: bool,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secrets(<redacted>)")
    }
}

impl Secrets {
    /// The secrets given, none empty, for keeping them out of quoted error text.
    fn all(&self) -> Vec<&str> {
        [Some(self.password.as_str()), self.pin.as_deref(), self.otp.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect()
    }

    /// The PIN or OTP as given, `None` when left empty.
    fn pin(&self) -> Option<&str> {
        self.pin.as_deref().filter(|s| !s.is_empty())
    }

    fn otp(&self) -> Option<&str> {
        self.otp.as_deref().filter(|s| !s.is_empty())
    }
}

/// What [`send_otp`] got the service to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OtpSent {
    /// The code is on its way to the user (SMS, a call).
    Sent,
    /// A challenge to show the user, who answers it with the code their device computes.
    Challenge(String),
}

// What remote signing's errors say. Each sentence is written once, here, and begins the errors
// that say it, which may go on with what the service or the network said (": …", " (…)"). From
// it [`fault_of`] tells who has to act, and [`WORDINGS`] lists those an interface translates.

/// The service turned down the sign-in.
pub const REJECTED_PASSWORD: &str = "the signing service rejected the user name or password";
/// CSC: the PIN was wrong (v1 `invalid_pin`, or a v2 authorization that sent only a PIN).
pub const REJECTED_PIN: &str = "the signing service rejected the PIN";
pub const REJECTED_OTP: &str = "the signing service rejected the OTP (it may have expired)";
/// CSC v2, whose `invalid_authentication_data` doesn't say which of the two was wrong.
pub const REJECTED_PIN_OR_OTP: &str = "the signing service rejected the PIN or the OTP (the OTP may have expired)";
/// Aruba refusals (`status KO`), its description and code following.
pub const REFUSED_TO_LIST: &str = "the signing service refused to list the certificates";
pub const REFUSED_TO_SEND: &str = "the signing service refused to send the OTP";
pub const REFUSED_TO_SIGN: &str = "the signing service refused to sign";
pub const NEEDS_OAUTH: &str = "this signing service can't be used: it needs OAuth sign-in in a browser, which PdfCraft doesn't support yet";
/// An Aruba account without its domain, which Aruba answers with "Invalid Parameters".
pub const NEEDS_DOMAIN: &str = "an Aruba account needs its domain (the \"dominio\" Aruba gives with the account): enter it";
/// The service couldn't be reached: the transport's reason follows in parentheses.
pub const UNREACHABLE: &str = "the signing service is unreachable";

/// The refusals people meet most, for an interface to say in its own language (followed, as
/// they come, by what the service added).
pub const WORDINGS: &[&str] =
    &[REJECTED_PASSWORD, REJECTED_PIN, REJECTED_OTP, REJECTED_PIN_OR_OTP, REFUSED_TO_LIST, REFUSED_TO_SEND, REFUSED_TO_SIGN, NEEDS_OAUTH];

const REFUSED_TO_DESCRIBE: &str = "the signing service refused to describe the certificate";
const NOT_SENT: &str = "the signing service doesn't send this certificate's one-time password: read it from your authenticator app or token";
const CONFIRMED_ELSEWHERE: &str = "this certificate's signatures are confirmed on your other device while signing: there is no code to send";
const NO_OTP: &str = "this certificate signs without a one-time password";
const NEEDS: &str = "this certificate needs";
const NO_CERTIFICATE: &str = "the signing service has no certificate";
const CANT_SIGN: &str = "the signing service's certificate";
const USER_COLON: &str = "the signing service user name can't contain a colon";
const NEEDS_USER: &str = "enter the signing service user name";
const BAD_ADDRESS: &str = "the signing service address";
const OTP_USED: &str = "this remote signature has already used its one-time password: enter a new one to sign again";
/// How [`parse_reference`]'s errors read (`SignError::Malformed`).
const BAD_REFERENCE: &str = "malformed data: remote signing reference";
const TOO_LARGE: &str = "the signing service's answer is larger than 1 MiB, so it wasn't read";
const NOT_JSON: &str = "the signing service's answer isn't JSON";
const NOT_XML: &str = "the signing service's answer isn't XML";
const NOT_TEXT: &str = "the signing service's answer isn't text";
const NO_RESULT: &str = "the signing service's answer has no";
const NO_SIGNATURE: &str = "the signing service's answer carries no signature";
const NOT_BASE64: &str = "the signing service's signature isn't base64";
const SOAP_FAULT: &str = "the signing service reported a fault";
const REFUSED_REQUEST: &str = "the signing service refused the request";
const MISMATCH: &str =
    "the signing service's signature does not match the certificate (it signed something else, or with another key), so it wasn't used";
const BARE_DIGEST: &str = "the signing service signed the bare digest, without the DigestInfo that names its hash algorithm (RSA PKCS #1 v1.5 needs it), so the signature can't be used";
const BAD_ECDSA: &str = "the signing service's ECDSA signature is neither DER nor raw r and s";
const NO_TOKEN: &str = "the signing service signed in without giving an access token";
const SIGN_IN_EXPIRED: &str = "the signing service's sign-in expired before the request went through: try again";
const NO_SAD: &str = "the signing service authorized the signature without giving its activation data (SAD)";
const NOT_CONFIRMED: &str = "the signature wasn't confirmed within a minute";
const SHOWS_CHALLENGE: &str = "the signing service shows a challenge to confirm this signature with, which PdfCraft can't display while signing yet";
const SIGN_IN_UNSUPPORTED: &str = "this signing service only offers sign-in methods PdfCraft doesn't support";
const KEY_CANT_SIGN: &str = "the signing service's key can't sign with";

/// Who has to act on a remote signing error, for a host that reports the two apart (the
/// automation tools: invalid arguments, or a failure).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteFault {
    /// Whoever made the call, by changing it: a user name, password, PIN or OTP the service
    /// turned down, a secret or the Aruba domain left out, a code asked for that the service
    /// doesn't send, a credential the account doesn't have or that PdfCraft can't sign with, an
    /// address or reference that isn't allowed. Every Aruba refusal (`status KO`) counts: its
    /// codes aren't documented and those seen are about what the call sent.
    Caller,
    /// The service or the way to it: unreachable, an answer that makes no sense, a signature
    /// that doesn't verify, a refusal of what PdfCraft computed, something PdfCraft doesn't
    /// support. Also any error that isn't remote signing's.
    Service,
}

/// Who has to act on each sentence above.
const FAULTS: &[(&str, RemoteFault)] = {
    use RemoteFault::{Caller, Service};
    &[
        (REJECTED_PASSWORD, Caller),
        (REJECTED_PIN, Caller),
        (REJECTED_OTP, Caller),
        (REJECTED_PIN_OR_OTP, Caller),
        (REFUSED_TO_LIST, Caller),
        (REFUSED_TO_SEND, Caller),
        (REFUSED_TO_SIGN, Caller),
        (REFUSED_TO_DESCRIBE, Caller),
        (NOT_SENT, Caller),
        (CONFIRMED_ELSEWHERE, Caller),
        (NO_OTP, Caller),
        (NEEDS, Caller),
        (NEEDS_DOMAIN, Caller),
        (NO_CERTIFICATE, Caller),
        (CANT_SIGN, Caller),
        (USER_COLON, Caller),
        (NEEDS_USER, Caller),
        (BAD_ADDRESS, Caller),
        (OTP_USED, Caller),
        (BAD_REFERENCE, Caller),
        (UNREACHABLE, Service),
        (TOO_LARGE, Service),
        (NOT_JSON, Service),
        (NOT_XML, Service),
        (NOT_TEXT, Service),
        (NO_RESULT, Service),
        (NO_SIGNATURE, Service),
        (NOT_BASE64, Service),
        (SOAP_FAULT, Service),
        (REFUSED_REQUEST, Service),
        (MISMATCH, Service),
        (BARE_DIGEST, Service),
        (BAD_ECDSA, Service),
        (NO_TOKEN, Service),
        (SIGN_IN_EXPIRED, Service),
        (NO_SAD, Service),
        (NOT_CONFIRMED, Service),
        (SHOWS_CHALLENGE, Service),
        (NEEDS_OAUTH, Service),
        (SIGN_IN_UNSUPPORTED, Service),
        (KEY_CANT_SIGN, Service),
    ]
};

/// Who has to act on `e`: an error of this module's functions, or of a signature made with a
/// remote digital ID, as a `SignError` or as the text a layer above passed on. The sentence it
/// begins with decides, so the wording and the verdict can't drift apart.
pub fn fault_of(e: impl std::fmt::Display) -> RemoteFault {
    let m = e.to_string();
    // A sentence that begins another (the PIN, and the PIN or the OTP) has the same verdict, so
    // the first that fits decides.
    FAULTS.iter().find(|(s, _)| m.starts_with(s)).map_or(RemoteFault::Service, |(_, f)| *f)
}

/// The reference PdfCraft keeps for a remote identity (a Sign dialog entry, an automation `id`):
/// `remote:csc?url=…&user=…&credential=…` or `remote:arss?user=…&credential=…`, with `domain`,
/// `hsm` and, for ARSS when it isn't [`ARSS_ENDPOINT`], `url` after them. Values are
/// percent-encoded. It holds no secret.
pub fn reference(account: &Account, credential: &str) -> String {
    let mut fields: Vec<(&str, &str)> = Vec::new();
    let protocol = match account.protocol {
        Protocol::Csc => {
            fields.push(("url", &account.url));
            "csc"
        }
        Protocol::Arss => "arss",
    };
    fields.push(("user", &account.user));
    fields.push(("credential", credential));
    if let Some(d) = &account.domain {
        fields.push(("domain", d));
    }
    if let Some(h) = &account.hsm {
        fields.push(("hsm", h));
    }
    if account.protocol == Protocol::Arss && account.url != ARSS_ENDPOINT {
        fields.push(("url", &account.url));
    }
    let query = fields.iter().map(|(k, v)| format!("{k}={}", pct(v))).collect::<Vec<_>>().join("&");
    format!("remote:{protocol}?{query}")
}

/// The account and credential a [`reference()`] names. Unknown fields are ignored (a later
/// version may add some); the address must pass [`check_url`].
pub fn parse_reference(s: &str) -> Result<(Account, String), SignError> {
    let bad = |why: String| SignError::Malformed(format!("remote signing reference: {why}"));
    let rest = s.strip_prefix("remote:").ok_or_else(|| bad("it doesn't start with remote:".into()))?;
    let (protocol, query) = rest.split_once('?').unwrap_or((rest, ""));
    let protocol = match protocol {
        "csc" => Protocol::Csc,
        "arss" => Protocol::Arss,
        other => return Err(bad(format!("unknown protocol {:?} (csc, arss)", quote(other, &[])))),
    };
    let (mut url, mut user, mut credential, mut domain, mut hsm) = (None, None, None, None, None);
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').ok_or_else(|| bad(format!("{:?} has no value", quote(pair, &[]))))?;
        let value = unpct(value).ok_or_else(|| bad(format!("the {} value isn't percent-encoded UTF-8", quote(key, &[]))))?;
        let slot = match key {
            "url" => &mut url,
            "user" => &mut user,
            "credential" => &mut credential,
            "domain" => &mut domain,
            "hsm" => &mut hsm,
            _ => continue,
        };
        if slot.replace(value).is_some() {
            return Err(bad(format!("{key} is given twice")));
        }
    }
    let url = match (protocol, url) {
        (_, Some(u)) => u,
        (Protocol::Arss, None) => ARSS_ENDPOINT.to_string(),
        (Protocol::Csc, None) => return Err(bad("it has no url".into())),
    };
    check_url(&url)?;
    let user = user.filter(|u| !u.is_empty()).ok_or_else(|| bad("it has no user".into()))?;
    let credential = credential.filter(|c| !c.is_empty()).ok_or_else(|| bad("it has no credential".into()))?;
    Ok((Account { protocol, url, user, domain, hsm }, credential))
}

/// Refuse a signing service address that isn't `https://` (plain `http://` only to this
/// computer: `localhost`, `127.x.x.x`, `[::1]`) or that carries a user name or password.
pub fn check_url(url: &str) -> Result<(), SignError> {
    let bad = |why: &str| Err(SignError::Crypto(format!("{BAD_ADDRESS} {why}")));
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return bad("contains spaces or control characters");
    }
    let lower = url.to_ascii_lowercase();
    let (https, rest) = match (lower.strip_prefix("https://"), lower.strip_prefix("http://")) {
        (Some(r), _) => (true, r),
        (None, Some(r)) => (false, r),
        (None, None) => return bad("must start with https://"),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return bad("must not contain a user name or password");
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((h, p)) => (h, p),
            None => return bad("has an unclosed [ in its host"),
        },
        None => authority.split_at(authority.find(':').unwrap_or(authority.len())),
    };
    if host.is_empty() {
        return bad("has no host name");
    }
    if let Some(p) = port.strip_prefix(':') {
        // Digits only: `u16::from_str` would also take a sign.
        if !p.bytes().all(|b| b.is_ascii_digit()) || !p.parse::<u16>().is_ok_and(|p| p != 0) {
            return bad("has an invalid port");
        }
    } else if !port.is_empty() {
        return bad("has an invalid host");
    }
    let loopback = host == "localhost" || host == "::1" || host.parse::<std::net::Ipv4Addr>().is_ok_and(|a| a.is_loopback());
    if !https && !loopback {
        return bad("must start with https:// (plain http:// is allowed only to this computer)");
    }
    Ok(())
}

/// Refuse an account no request should be sent for: an address [`check_url`] refuses, no user
/// name, or an Aruba account without its domain (which Aruba refuses as "Invalid Parameters").
fn check_account(account: &Account) -> Result<(), SignError> {
    check_url(&account.url)?;
    if account.user.trim().is_empty() {
        return Err(SignError::Crypto(NEEDS_USER.into()));
    }
    if account.protocol == Protocol::Arss && account.domain.as_deref().is_none_or(|d| d.trim().is_empty()) {
        return Err(SignError::Crypto(NEEDS_DOMAIN.into()));
    }
    Ok(())
}

/// Sign in and list the account's credentials that can sign: an RSA, P-256 or P-384 key whose
/// certificate allows signing documents, and that PdfCraft can authorize. Each comes with its
/// certificate and what a signature asks for.
pub fn list(t: &dyn HttpTransport, account: &Account, password: &str) -> Result<Vec<RemoteIdentity>, SignError> {
    check_account(account)?;
    match account.protocol {
        Protocol::Csc => csc::list(t, account, password),
        Protocol::Arss => arss::list(t, account, password),
    }
}

/// Ask the service to send the OTP for `credential` (CSC `getChallenge` or `sendOTP`, ARSS
/// `sendCredential` by SMS). A code from an app or a token needs no sending. Sign with it as
/// [`Secrets::otp_sent`], which puts it under the authentication object it was sent for.
pub fn send_otp(t: &dyn HttpTransport, account: &Account, credential: &str, password: &str) -> Result<OtpSent, SignError> {
    check_account(account)?;
    match account.protocol {
        Protocol::Csc => csc::send_otp(t, account, credential, password),
        Protocol::Arss => arss::send_otp(t, account, password),
    }
}

/// A digital ID whose key signs remotely with `secrets`. It signs in and fetches the
/// certificate now, which the CMS needs before anything is signed (signing-certificate-v2);
/// the OTP is spent by the first signature, and a second one fails.
pub fn digital_id(t: Arc<dyn HttpTransport>, account: &Account, credential: &str, secrets: Secrets) -> Result<DigitalId, SignError> {
    check_account(account)?;
    let (certificate, chain, session) = match account.protocol {
        Protocol::Csc => {
            let (id, session) = csc::open(&*t, account, credential, secrets)?;
            (id.certificate, id.chain, Session::Csc(Box::new(session)))
        }
        Protocol::Arss => {
            let (id, session) = arss::open(&*t, account, credential, secrets)?;
            (id.certificate, id.chain, Session::Arss(session))
        }
    };
    let public = certificate.public_key.clone();
    let key = PrivateKey::external(public.clone(), Arc::new(RemoteKey { transport: t, public, session: Mutex::new(Some(session)) }));
    let friendly_name = Some(certificate.display_name());
    Ok(DigitalId { key, certificate, chain, friendly_name })
}

/// The protocol state a signature continues from: the secrets, and for CSC the sign-in.
enum Session {
    Csc(Box<csc::Session>),
    Arss(arss::Session),
}

/// A remote identity's key. Its session is taken by the first signature: the OTP authorizes one.
struct RemoteKey {
    transport: Arc<dyn HttpTransport>,
    public: PublicKey,
    session: Mutex<Option<Session>>,
}

impl ExternalKey for RemoteKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        // Taking the session is all the lock guards, so a poisoned lock is still sound.
        let session = self.session.lock().unwrap_or_else(PoisonError::into_inner).take().ok_or_else(|| SignError::Crypto(OTP_USED.into()))?;
        let digest = alg.digest(&[msg]);
        let sig = match session {
            Session::Csc(s) => csc::sign(&*self.transport, *s, &self.public, alg, &digest)?,
            Session::Arss(s) => arss::sign(&*self.transport, s, alg, &digest)?,
        };
        checked(&self.public, alg, &digest, sig)
    }
}

/// The service's signature over `digest` as CMS carries it, once it verifies against the
/// certificate's key: RSA as exactly the modulus' length, ECDSA as DER even when the service sent
/// raw `r ‖ s`. Anything else fails here rather than producing a signature that doesn't validate.
fn checked(public: &PublicKey, alg: DigestAlg, digest: &[u8], sig: Vec<u8>) -> Result<Vec<u8>, SignError> {
    let mismatch = || SignError::Crypto(MISMATCH.into());
    match public {
        PublicKey::Rsa { n, e } => {
            // RFC 8017 §8.2.2 wants exactly the modulus' length, and strict validators refuse any
            // other. Some services drop leading zeros; others add one (a Java BigInteger's sign
            // byte). Both are mended here; a longer value with anything but zeros in front isn't.
            let extra = sig.len().saturating_sub(n.len());
            let (front, value) = (sig.get(..extra).unwrap_or_default(), sig.get(extra..).unwrap_or_default());
            if front.iter().any(|b| *b != 0) {
                return Err(mismatch());
            }
            let mut padded = vec![0u8; n.len().saturating_sub(value.len())];
            padded.extend_from_slice(value);
            if public.verify(Scheme::RsaPkcs1, alg, digest, &padded)? {
                return Ok(padded);
            }
            if crate::rsa_pad::pkcs1_bare(n, e, digest, &padded)? {
                return Err(SignError::Crypto(BARE_DIGEST.into()));
            }
            Err(mismatch())
        }
        PublicKey::P256(_) | PublicKey::P384(_) => {
            let field = if matches!(public, PublicKey::P256(_)) { 32 } else { 48 };
            let rs = crate::keys::ecdsa_rs(&sig, field, &mut Vec::new()).ok_or_else(|| SignError::Crypto(BAD_ECDSA.into()))?;
            let der = public.store_signature_der(&rs)?;
            if public.verify(Scheme::Ecdsa, alg, digest, &der)? { Ok(der) } else { Err(mismatch()) }
        }
        other => Err(SignError::Unsupported(format!("signing with {} keys", other.describe()))),
    }
}

/// Send one request, refusing an answer over [`MAX_RESPONSE`]. A transport failure reads as the
/// service being unreachable, with `secrets` kept out of its text.
fn exchange(t: &dyn HttpTransport, url: &str, headers: &[(&str, &str)], body: &[u8], secrets: &[&str]) -> Result<HttpResponse, SignError> {
    let r = t.post(url, headers, body).map_err(|e| SignError::Crypto(format!("{UNREACHABLE} ({})", quote(&e.to_string(), secrets))))?;
    if r.body.len() > MAX_RESPONSE {
        return Err(SignError::Crypto(TOO_LARGE.into()));
    }
    Ok(r)
}

/// Text from a service (an error description) fit to show: each of `secrets` replaced, control
/// characters turned into spaces, cut to 200 characters.
fn quote(text: &str, secrets: &[&str]) -> String {
    const MAX: usize = 200;
    // Longest first: a PIN inside the OTP (or the password) would otherwise leave the rest showing.
    let mut secrets: Vec<&str> = secrets.iter().copied().filter(|s| !s.is_empty()).collect();
    secrets.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    secrets.dedup();
    let mut s = text.to_string();
    for secret in secrets {
        s = s.replace(secret, "<redacted>");
    }
    let mut out: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(MAX).collect();
    if s.chars().nth(MAX).is_some() {
        out.push('…');
    }
    out.trim().to_string()
}

/// A value for a [`reference()`]: RFC 3986 unreserved characters as they are, the rest `%XX`.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            // Writing to a String can't fail.
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn unpct(s: &str) -> Option<String> {
    let hex = |b: u8| char::from(b).to_digit(16).and_then(|d| u8::try_from(d).ok());
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let (hi, lo) = (hex(*bytes.get(i + 1)?)?, hex(*bytes.get(i + 2)?)?);
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Base64 as services write it: standard or URL-safe alphabet, padded or not, wrapped or not.
fn base64_lenient(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    use base64::alphabet;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const CONFIG: GeneralPurposeConfig = GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent);
    const STANDARD: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, CONFIG);
    const URL_SAFE: GeneralPurpose = GeneralPurpose::new(&alphabet::URL_SAFE, CONFIG);
    let compact: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    STANDARD.decode(&compact).or_else(|_| URL_SAFE.decode(&compact)).ok()
}

fn base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A credential's certificate, if PdfCraft can sign with it: the key type (RSA, P-256, P-384)
/// and the certificate's key usage decide. The `Err` is why not, as a clause after its name.
fn signing_certificate(der: &[u8]) -> Result<Certificate, String> {
    let c = Certificate::parse(der).map_err(|e| format!("its certificate can't be read: {e}"))?;
    if c.public_key.store_signing_key().is_none() {
        return Err(format!("its {} key can't sign through PdfCraft (RSA, P-256 and P-384 only)", c.public_key.describe()));
    }
    if let Some(why) = c.signing_problem_clause() {
        return Err(why);
    }
    Ok(c)
}

/// The error for a list in which no credential could be used, naming why for the first few.
fn none_usable(skipped: &[(String, String)]) -> SignError {
    let reasons: Vec<String> = skipped.iter().take(3).map(|(id, why)| format!("{}: {why}", quote(id, &[]))).collect();
    SignError::Crypto(format!("{NO_CERTIFICATE} PdfCraft can sign with ({})", reasons.join("; ")))
}

/// The error when a secret the credential needs is missing.
fn missing(what: &str) -> SignError {
    SignError::Crypto(format!("{NEEDS} {what} to sign: enter it"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_sentence_says_who_has_to_act() {
        for (sentence, fault) in FAULTS {
            assert_eq!(fault_of(sentence), *fault, "{sentence}");
            assert_eq!(fault_of(format!("{sentence}: what the service added")), *fault, "{sentence}");
            // What fault_of relies on: a sentence that begins another says the same of it.
            for (longer, other) in FAULTS.iter().filter(|(l, _)| l != sentence && l.starts_with(sentence)) {
                assert_eq!(fault, other, "{sentence} begins {longer}");
            }
            assert_eq!(FAULTS.iter().filter(|(s, _)| s == sentence).count(), 1, "{sentence} twice");
        }
        for w in WORDINGS.iter().chain([&NEEDS_DOMAIN, &UNREACHABLE]) {
            assert!(FAULTS.iter().any(|(s, _)| s == w), "{w}");
        }
        assert_eq!(fault_of("an error that isn't remote signing's"), RemoteFault::Service);
        assert_eq!(fault_of(SignError::WrongPassword), RemoteFault::Service);
        assert_eq!(fault_of(parse_reference("remote:arss?credential=c").unwrap_err()), RemoteFault::Caller);
    }

    #[test]
    fn percent_encoding_round_trips_and_rejects_bad_escapes() {
        let s = "a b&c=d%é/~._-";
        assert_eq!(pct(s), "a%20b%26c%3Dd%25%C3%A9%2F~._-");
        assert_eq!(unpct(&pct(s)).as_deref(), Some(s));
        for bad in ["%", "%4", "%G0", "%+1", "%C3"] {
            assert_eq!(unpct(bad), None, "{bad}");
        }
    }

    #[test]
    fn quoting_strips_controls_cuts_long_text_and_hides_secrets() {
        assert_eq!(quote("bad\npassword hunter2\u{7}", &["hunter2"]), "bad password <redacted>");
        let long = "é".repeat(300);
        let q = quote(&long, &[]);
        assert_eq!(q.chars().count(), 201);
        assert!(q.ends_with('…'));
        assert_eq!(quote("x", &[""]), "x");
        // A secret inside another (the PIN in the OTP) doesn't leave the rest of the longer showing.
        assert_eq!(quote("OTP 123456 rejected", &["1234", "123456", "1234"]), "OTP <redacted> rejected");
    }

    /// A transport whose every answer is that many spaces.
    struct Spaces(usize);

    impl HttpTransport for Spaces {
        fn post(&self, _url: &str, _headers: &[(&str, &str)], _body: &[u8]) -> Result<HttpResponse, SignError> {
            Ok(HttpResponse { status: 200, body: vec![b' '; self.0] })
        }
    }

    #[test]
    fn an_answer_of_exactly_the_cap_is_read_and_one_byte_more_is_not() {
        assert_eq!(exchange(&Spaces(MAX_RESPONSE), "https://x", &[], b"", &[]).map(|r| r.body.len()).ok(), Some(MAX_RESPONSE));
        let e = exchange(&Spaces(MAX_RESPONSE + 1), "https://x", &[], b"", &[]).unwrap_err();
        assert_eq!(e.to_string(), "the signing service's answer is larger than 1 MiB, so it wasn't read");
    }

    #[test]
    fn a_response_prints_no_body() {
        let r = HttpResponse { status: 200, body: br#"{"access_token":"AT-1"}"#.to_vec() };
        assert_eq!(format!("{r:?}"), "HttpResponse { status: 200, body: <23 bytes> }");
    }

    #[test]
    fn rsa_signatures_get_exactly_the_modulus_length() {
        let key = PrivateKey::generate_rsa(2048).unwrap();
        let public = key.public_key().clone();
        // One in about 256 signatures starts with a zero byte, which a service may drop.
        let (digest, sig) = (0u32..4096)
            .map(|i| DigestAlg::Sha256.digest(&[&i.to_be_bytes()]))
            .map(|d| {
                let s = key.sign_digest(DigestAlg::Sha256, &d).unwrap();
                (d, s)
            })
            .find(|(_, s)| s.first() == Some(&0))
            .unwrap();
        assert_eq!(sig.len(), 256);
        let mend = |s: Vec<u8>| checked(&public, DigestAlg::Sha256, &digest, s).map_err(|e| e.to_string());
        assert_eq!(mend(sig.clone()), Ok(sig.clone()));
        assert_eq!(mend(sig[1..].to_vec()), Ok(sig.clone()), "a dropped zero is put back");
        assert_eq!(mend([&[0, 0][..], &sig].concat()), Ok(sig.clone()), "added zeros are taken off");
        assert!(mend([&[1][..], &sig].concat()).unwrap_err().contains("does not match the certificate"));
    }

    #[test]
    fn urls_need_https_except_to_this_computer() {
        for ok in [
            "https://rssp.example/csc/v2",
            "HTTPS://rssp.example:8443/csc/v2/",
            "http://127.0.0.1:8080/csc/v2",
            "http://localhost/x",
            "http://[::1]:9/x",
            "https://[2001:db8::1]/csc/v2",
        ] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://rssp.example/csc/v2",
            "ftp://rssp.example",
            "rssp.example",
            "https://",
            "https://user:pw@rssp.example/",
            "https://rssp.example:/x",
            "https://rssp.example:99999/x",
            "https://rssp.example:0/x",
            "https://rssp.example/a b",
            "https://[::1/x",
            "http://localhost.evil.example/",
            "http://127.0.0.1.evil.example/",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lenient_base64_reads_both_alphabets_and_wrapped_text() {
        assert_eq!(base64_lenient("+/8=").as_deref(), Some(&[0xFB, 0xFF][..]));
        assert_eq!(base64_lenient("-_8").as_deref(), Some(&[0xFB, 0xFF][..]));
        assert_eq!(base64_lenient("AA\r\nAA").as_deref(), Some(&[0, 0, 0][..]));
        assert_eq!(base64_lenient("*"), None);
    }

    #[test]
    fn secrets_debug_prints_none_of_them() {
        let s = Secrets { password: "hunter2".into(), pin: Some("1234".into()), otp: Some("987654".into()), otp_sent: true };
        let d = format!("{s:?}");
        assert!(!d.contains("hunter2") && !d.contains("1234") && !d.contains("987654"), "{d}");
        assert_eq!(Secrets { password: String::new(), pin: Some(String::new()), otp: None, otp_sent: false }.all(), Vec::<&str>::new());
    }

    use proptest::prelude::*;

    /// An address [`check_url`] takes: https, a host name or address, maybe a port and a path.
    fn address() -> impl Strategy<Value = String> {
        let host = prop_oneof!["[a-z][a-z0-9-]{0,12}(\\.[a-z]{2,6}){0,2}", Just("[2001:db8::1]".to_string()), Just("127.0.0.1".to_string())];
        (host, proptest::option::of(1u16..), "(/[A-Za-z0-9._~%-]{0,8}){0,3}")
            .prop_map(|(h, port, path)| format!("https://{h}{}{path}", port.map(|p| format!(":{p}")).unwrap_or_default()))
    }

    proptest! {
        #[test]
        fn any_reference_round_trips(
            arss in any::<bool>(),
            url in prop_oneof![address(), Just(ARSS_ENDPOINT.to_string())],
            user in ".+",
            credential in ".+",
            domain in proptest::option::of(".*"),
            hsm in proptest::option::of(".*"),
        ) {
            let protocol = if arss { Protocol::Arss } else { Protocol::Csc };
            let account = Account { protocol, url, user, domain, hsm };
            let back = parse_reference(&reference(&account, &credential)).map_err(|e| e.to_string());
            prop_assert_eq!(back, Ok((account, credential)));
        }

        #[test]
        fn readers_of_typed_or_stored_text_never_panic(
            s in prop_oneof![
                any::<String>(),
                "remote:(csc|arss)?\\??[%:@&=\\[\\]a-z0-9é/.]{0,40}",
                "https?://[%:@\\[\\]a-z0-9é/.?#]{0,30}",
            ]
        ) {
            let _ = parse_reference(&s);
            let _ = check_url(&s);
            let _ = unpct(&s);
        }
    }
}
