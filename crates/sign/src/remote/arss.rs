//! Aruba Remote Sign Server (ARSS), from its public WSDL: SOAP 1.1, document/literal wrapped,
//! `SOAPAction: ""`. `listCert` lists the account's certificates, `sendCredential` (type `SMS`)
//! sends the OTP, and `signhash` signs a digest.
//!
//! The schema's `elementFormDefault` is unqualified: only the operation's element is in the
//! service namespace, and its children must be in none (the service ignores qualified ones), so
//! the namespace goes on a prefix and no default `xmlns` is ever declared. Errors come back as
//! ordinary answers with `status` `KO`, not SOAP faults.

use roxmltree::{Document, Node};

use super::{
    Account, AuthNeeds, AuthObject, AuthRole, Confirmation, HttpTransport, NO_CERTIFICATE, NO_RESULT, NO_SIGNATURE, NOT_BASE64, NOT_TEXT, NOT_XML,
    OtpKind, OtpSent, REFUSED_TO_LIST, REFUSED_TO_SEND, REFUSED_TO_SIGN, RemoteIdentity, SOAP_FAULT, Secrets, base64, base64_lenient, exchange,
    missing, none_usable, quote, signing_certificate,
};
use crate::SignError;
use crate::keys::DigestAlg;

/// The service's namespace (the WSDL's `targetNamespace`).
pub(super) const NS: &str = "http://arubasignservice.arubapec.it/";
const SOAP_ENV: &str = "http://schemas.xmlsoap.org/soap/envelope/";

/// The most certificates listed.
const MAX_CERTIFICATES: usize = 64;

/// `s` with the five XML special characters escaped, for element content.
pub(super) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

fn element(name: &str, value: &str) -> String {
    format!("<{name}>{}</{name}>", escape(value))
}

/// The request for `operation` with `children` (unqualified elements).
fn envelope(operation: &str, children: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><soapenv:Envelope xmlns:soapenv=\"{SOAP_ENV}\" xmlns:arss=\"{NS}\"><soapenv:Body><arss:{operation}>{children}</arss:{operation}></soapenv:Body></soapenv:Envelope>"
    )
}

/// The `auth` structure as `tag` (`Identity` in most operations, `identity` in `signhash`): its
/// elements in schema order, the empty ones left out. The user name, HSM type and domain go
/// trimmed, as the account was checked; the secrets as typed.
fn identity(tag: &str, account: &Account, password: &str, otp: Option<&str>) -> String {
    let fields = [
        ("otpPwd", otp),
        ("typeHSM", account.hsm.as_deref().map(str::trim)),
        ("typeOtpAuth", account.domain.as_deref().map(str::trim)),
        ("user", Some(account.user.trim())),
        ("userPWD", Some(password)),
    ];
    let inner: String = fields.iter().filter_map(|(k, v)| v.filter(|v| !v.is_empty()).map(|v| element(k, v))).collect();
    format!("<{tag}>{inner}</{tag}>")
}

/// Send `operation` and return the answer's text, refusing one that isn't UTF-8.
fn call(t: &dyn HttpTransport, account: &Account, operation: &str, children: &str, secrets: &[&str]) -> Result<String, SignError> {
    let headers = [("Content-Type", "text/xml; charset=utf-8"), ("SOAPAction", "\"\"")];
    let r = exchange(t, &account.url, &headers, envelope(operation, children).as_bytes(), secrets)?;
    String::from_utf8(r.body).map_err(|_| SignError::Crypto(format!("{NOT_TEXT} (HTTP {})", r.status)))
}

/// The `return` element of `operation`'s answer, or the fault the service reported.
fn returned<'a>(doc: &'a Document<'a>, operation: &str, secrets: &[&str]) -> Result<Node<'a, 'a>, SignError> {
    let named = |n: &Node<'_, '_>, name: &str| n.is_element() && n.tag_name().name() == name;
    if let Some(fault) = doc.descendants().find(|n| named(n, "Fault")) {
        let text = fault.descendants().find(|n| named(n, "faultstring")).and_then(|n| n.text()).unwrap_or_default();
        return Err(SignError::Crypto(format!("{SOAP_FAULT}: {}", quote(text, secrets))));
    }
    let response = format!("{operation}Response");
    doc.descendants()
        .find(|n| named(n, &response))
        .and_then(|r| r.children().find(|n| named(n, "return")))
        .ok_or_else(|| SignError::Crypto(format!("{NO_RESULT} {operation} result")))
}

/// The text of `node`'s child element `name`, trimmed.
fn child<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children().find(|n| n.is_element() && n.tag_name().name() == name).and_then(|n| n.text()).map(str::trim)
}

/// The answer as XML. The parser's message quotes the answer's own names, so it is quoted too.
fn parse<'a>(text: &'a str, secrets: &[&str]) -> Result<Document<'a>, SignError> {
    Document::parse(text).map_err(|e| SignError::Crypto(format!("{NOT_XML} ({})", quote(&e.to_string(), secrets))))
}

/// A refusal (`status` other than `OK`), said by `sentence` with the service's description and
/// code, when it gives them.
fn refused(ret: Node<'_, '_>, sentence: &str, secrets: &[&str]) -> Option<SignError> {
    let status = child(ret, "status").unwrap_or_default();
    if status.eq_ignore_ascii_case("OK") {
        return None;
    }
    let mut msg = sentence.to_string();
    if let Some(d) = child(ret, "description").filter(|d| !d.is_empty()) {
        msg.push_str(&format!(": {}", quote(d, secrets)));
    }
    if let Some(c) = child(ret, "return_code").filter(|c| !c.is_empty()) {
        msg.push_str(&format!(" (code {})", quote(c, secrets)));
    }
    Some(SignError::Crypto(msg))
}

/// What every ARSS signature asks for: the OTP, sent by SMS or read from Aruba's app or a token.
fn needs() -> AuthNeeds {
    let otp = AuthObject { id: "otpPwd".into(), kind: "OTP".into(), label: Some("OTP".into()), format: None, role: AuthRole::Otp };
    AuthNeeds { pin: false, otp: OtpKind::Sent, objects: vec![otp], confirm: Confirmation::Never }
}

pub(super) fn list(t: &dyn HttpTransport, account: &Account, password: &str) -> Result<Vec<RemoteIdentity>, SignError> {
    let text = call(t, account, "listCert", &identity("Identity", account, password, None), &[password])?;
    let doc = parse(&text, &[password])?;
    let ret = returned(&doc, "listCert", &[password])?;
    if let Some(e) = refused(ret, REFUSED_TO_LIST, &[password]) {
        return Err(e);
    }
    let mut out: Vec<RemoteIdentity> = Vec::new();
    let mut skipped = Vec::new();
    let certs = ret.children().filter(|n| n.is_element() && matches!(n.tag_name().name(), "app1" | "app2"));
    for c in certs.take(MAX_CERTIFICATES) {
        // A nil list entry (`xsi:nil`) has neither.
        let (Some(id), Some(value)) = (child(c, "id").filter(|i| !i.is_empty() && i.len() <= 1024), child(c, "value")) else { continue };
        let read = base64_lenient(value).ok_or_else(|| "its certificate isn't base64".to_string()).and_then(|der| signing_certificate(&der));
        match read {
            Ok(certificate) if !out.iter().any(|o| o.credential == id && o.certificate.raw == certificate.raw) => {
                out.push(RemoteIdentity { account: account.clone(), credential: id.to_string(), certificate, chain: Vec::new(), auth: needs() })
            }
            Ok(_) => {}
            Err(why) => skipped.push((id.to_string(), why)),
        }
    }
    if out.is_empty() && !skipped.is_empty() {
        return Err(none_usable(&skipped));
    }
    Ok(out)
}

pub(super) fn send_otp(t: &dyn HttpTransport, account: &Account, password: &str) -> Result<OtpSent, SignError> {
    let children = format!("{}{}", identity("Identity", account, password, None), element("type", "SMS"));
    let text = call(t, account, "sendCredential", &children, &[password])?;
    let doc = parse(&text, &[password])?;
    let ret = returned(&doc, "sendCredential", &[password])?;
    match refused(ret, REFUSED_TO_SEND, &[password]) {
        Some(e) => Err(e),
        None => Ok(OtpSent::Sent),
    }
}

/// A signature in progress: the account, the certificate and the secrets to spend.
pub(super) struct Session {
    account: Account,
    credential: String,
    secrets: Secrets,
}

/// Find `credential` among the account's certificates, ready to sign with `secrets`.
pub(super) fn open(t: &dyn HttpTransport, account: &Account, credential: &str, secrets: Secrets) -> Result<(RemoteIdentity, Session), SignError> {
    if secrets.otp().is_none() {
        return Err(missing("a one-time password (OTP)"));
    }
    let id = list(t, account, &secrets.password)?
        .into_iter()
        .find(|i| i.credential == credential)
        .ok_or_else(|| SignError::Crypto(format!("{NO_CERTIFICATE} {} that can sign for this account", quote(credential, &[]))))?;
    Ok((id, Session { account: account.clone(), credential: credential.to_string(), secrets }))
}

/// `signhash` over `digest`. The certificate isn't asked back (`requirecert` false): it was read
/// before signing, for the signed attributes.
pub(super) fn sign(t: &dyn HttpTransport, s: Session, alg: DigestAlg, digest: &[u8]) -> Result<Vec<u8>, SignError> {
    let hashtype = match alg {
        DigestAlg::Sha256 => "SHA256",
        DigestAlg::Sha384 => "SHA384",
        DigestAlg::Sha512 => "SHA512",
        other => return Err(SignError::Unsupported(format!("{} for new signatures", other.name()))),
    };
    let request = [
        element("certID", &s.credential),
        element("hash", &base64(digest)),
        element("hashtype", hashtype),
        identity("identity", &s.account, &s.secrets.password, s.secrets.otp()),
        element("requirecert", "false"),
    ]
    .concat();
    let secrets = s.secrets.all();
    let text = call(t, &s.account, "signhash", &format!("<SignHashRequest>{request}</SignHashRequest>"), &secrets)?;
    let doc = parse(&text, &secrets)?;
    let ret = returned(&doc, "signhash", &secrets)?;
    let signature = child(ret, "signature").filter(|s| !s.is_empty());
    // Only KO is documented as failure; anything else with a signature is checked like a success.
    let status = child(ret, "status").unwrap_or_default();
    match (signature, status.eq_ignore_ascii_case("KO")) {
        (Some(sig), false) => base64_lenient(sig).ok_or_else(|| SignError::Crypto(NOT_BASE64.into())),
        _ => Err(refused(ret, REFUSED_TO_SIGN, &secrets).unwrap_or_else(|| SignError::Crypto(NO_SIGNATURE.into()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::Protocol;

    fn account() -> Account {
        Account { protocol: Protocol::Arss, url: crate::remote::ARSS_ENDPOINT.into(), user: "a<b".into(), domain: Some("dom&1".into()), hsm: None }
    }

    #[test]
    fn identity_is_in_schema_order_and_escaped() {
        assert_eq!(
            identity("identity", &account(), "p\"w'", Some("123")),
            "<identity><otpPwd>123</otpPwd><typeOtpAuth>dom&amp;1</typeOtpAuth><user>a&lt;b</user><userPWD>p&quot;w&apos;</userPWD></identity>"
        );
        // What the account check took (trimmed, not blank) is what goes; a blank HSM type is left out.
        let spaced = Account { user: " a ".into(), domain: Some(" dom ".into()), hsm: Some(" ".into()), ..account() };
        assert_eq!(
            identity("Identity", &spaced, " pw ", None),
            "<Identity><typeOtpAuth>dom</typeOtpAuth><user>a</user><userPWD> pw </userPWD></Identity>"
        );
    }

    #[test]
    fn children_are_unqualified() {
        let e = envelope("listCert", &identity("Identity", &account(), "pw", None));
        let doc = Document::parse(&e).unwrap();
        let op = doc.descendants().find(|n| n.tag_name().name() == "listCert").unwrap();
        assert_eq!(op.tag_name().namespace(), Some(NS));
        assert!(op.descendants().skip(1).filter(|n| n.is_element()).all(|n| n.tag_name().namespace().is_none()));
    }

    #[test]
    fn faults_and_refusals_are_reported() {
        let fault = format!(
            "<S:Envelope xmlns:S=\"{SOAP_ENV}\"><S:Body><S:Fault><faultcode>S:Server</faultcode><faultstring>boom pw</faultstring></S:Fault></S:Body></S:Envelope>"
        );
        let doc = Document::parse(&fault).unwrap();
        assert_eq!(returned(&doc, "signhash", &["pw"]).unwrap_err().to_string(), "the signing service reported a fault: boom <redacted>");
        let ko = "<Envelope><Body><signhashResponse><return><description>Empty Identity</description><return_code>0002</return_code><status>KO</status></return></signhashResponse></Body></Envelope>";
        let doc = Document::parse(ko).unwrap();
        let ret = returned(&doc, "signhash", &[]).unwrap();
        assert_eq!(refused(ret, REFUSED_TO_SIGN, &[]).unwrap().to_string(), "the signing service refused to sign: Empty Identity (code 0002)");
        // A DTD (an entity expansion bomb) is never read.
        assert!(parse("<!DOCTYPE x [<!ENTITY a \"aaaa\">]><x>&a;</x>", &[]).is_err());
        // The parser's message names what it read: a huge name is cut, a secret hidden.
        let e = parse(&format!("<x></hunter2{}>", "a".repeat(100_000)), &["hunter2"]).unwrap_err().to_string();
        assert!(e.len() < 300 && e.contains("<redacted>") && !e.contains("hunter2"), "{e}");
    }
}
