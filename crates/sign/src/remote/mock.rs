//! An in-process signing service for tests (this crate's and other crates', with the `mock`
//! feature): a CSC API v2 or v1 server or an Aruba ARSS server, answering through
//! [`HttpTransport`] with keys of its own and certificates its own CA issued (a root and an
//! intermediate, valid 2000 to 2099 so that no test signing with the machine's clock outlives
//! them).
//!
//! It holds the client to what the protocols require — Basic then Bearer, the v1 or v2 field
//! names, a v1 PIN only for a credential that takes one, v2 `authData` that names only the
//! credential's objects, each once, and satisfies its `auth/expression` (an `XOR` true of exactly
//! one operand), the same digest in `credentials/authorize` and `signatures/signHash` (SCAL 2),
//! an OTP that works once, and only under the object `getChallenge` sent it for, unqualified ARSS
//! elements, `SOAPAction: ""` and the Aruba domain —
//! records every request, keeps a clock its waits move on, and can be told to misbehave
//! ([`Fault`]). It never touches the network.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use roxmltree::{Document, Node};
use serde_json::{Map, Value, json};

use super::{Account, HttpResponse, HttpTransport, MAX_RESPONSE, Protocol, base64, base64_lenient};
use crate::der::{self, tag};
use crate::keys::{DigestAlg, PrivateKey, PublicKey, oid};
use crate::{Certificate, Name, SignError, Time};

/// The account's user name, password, PIN and ARSS domain.
pub const USER: &str = "mario.rossi";
pub const PASSWORD: &str = "correct horse battery";
pub const PIN: &str = "24681357";
pub const DOMAIN: &str = "frtest";

/// Where each kind of mock answers.
pub const CSC_V2_URL: &str = "https://rssp.example/csc/v2";
pub const CSC_V1_URL: &str = "https://rssp.example/csc/v1";
pub const ARSS_URL: &str = "https://arss.example/ArubaSignService/ArubaSignService";

/// The credentials: an RSA-2048 key (CSC: PIN and an OTP sent by SMS), a P-256 key (CSC: an OTP
/// from an app, no PIN), and, for CSC, a credential whose key is disabled and must not be listed.
/// [`MockService::set_auth`] gives a CSC v2 credential other authentication objects.
pub const RSA: &str = "rsa-2048";
pub const P256: &str = "p256";
pub const DISABLED: &str = "disabled";

/// A way for the mock to misbehave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// `info` offers only OAuth sign-in.
    OauthOnly,
    /// The current OTP has expired: an authorization with it is refused.
    OtpExpired,
    /// The first token handed out is refused (401 `invalid_token`) the first time it is used.
    TokenExpired,
    /// The token is refused (401 `invalid_token`) at the first `credentials/authorize`.
    TokenExpiresAtAuthorize,
    /// A wrong PIN or OTP at `credentials/authorize` is answered 401 `invalid_pin`, as some
    /// services answer it.
    WrongPinIs401,
    /// `credentials/authorize` answers 202, and `authorizeCheck` answers 202 this many times
    /// before giving the SAD.
    AuthorizePending(u32),
    /// The SAD is bound to another digest, so `signHash` refuses the one sent (SCAL 2).
    HashNotAuthorized,
    /// Signs another digest than the one sent.
    WrongDigest,
    /// ECDSA signatures come back as raw `r ‖ s` instead of DER.
    RawEcdsa,
    /// RSA signatures are PKCS #1 v1.5 over the bare digest, without the DigestInfo.
    BareDigest,
    /// RSA signatures come back a byte longer than the modulus, with a leading zero (the sign
    /// byte of a Java `BigInteger`).
    ExtraLeadingZero,
    /// Every answer is larger than [`MAX_RESPONSE`].
    Oversized,
    /// `credentials/authorize` refuses, quoting back the PIN, the OTP, the token and the password.
    EchoSecretsOnAuthorize,
    /// `signatures/signHash` (ARSS: `signhash`) refuses, quoting back the SAD, the token, the
    /// password and the OTP.
    EchoSecretsOnSign,
    /// CSC v2 `credentials/list` leaves out `credentialInfos`, so each credential is read apart.
    NoCredentialInfos,
    /// CSC v1 `credentials/list` gives one ID per page.
    Paged,
    /// CSC `credentials/list` starts with [`GHOSTS`] IDs of credentials that don't exist.
    ManyCredentials,
    /// ARSS `listCert` has a nil entry (`xsi:nil`) and lists every certificate twice.
    NilAndRepeated,
    /// CSC v2 `info` lists the deprecated `credentials/sendOTP` too, as a v2.0 service does.
    OffersSendOtp,
    /// Each `credentials/authorizeCheck` takes this many seconds on the mock's clock.
    SlowAuthorizeCheck(u32),
}

/// How many unknown credentials [`Fault::ManyCredentials`] lists.
pub const GHOSTS: usize = 100;

/// One request the mock received.
#[derive(Clone, Debug)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    CscV2,
    CscV1,
    Arss,
}

struct Credential {
    id: &'static str,
    key: PrivateKey,
    certificate: Certificate,
    enabled: bool,
}

/// An authorization a SAD stands for.
struct Grant {
    sad: String,
    credential: String,
    hashes: Vec<String>,
}

#[derive(Default)]
struct State {
    /// Bumped whenever an OTP is used or sent; the OTP is derived from it.
    otp_serial: u32,
    /// The CSC v2 object `getChallenge` sent the current OTP for (an SMS), which takes it alone;
    /// `None`: a code no object was asked for (an app's, or one `sendOTP` sent), which any takes.
    sent_for: Option<String>,
    tokens: Vec<String>,
    /// The token [`Fault::TokenExpired`] refuses once.
    stale: Option<String>,
    stale_done: bool,
    grants: Vec<Grant>,
    /// 202 handles: the grant each confirms, and the polls still to answer 202.
    pending: Vec<(String, String, u32)>,
    serial: u32,
    faults: Vec<Fault>,
    requests: Vec<Request>,
    waits: u32,
    /// The mock's clock: what the client waited, and what slow answers took.
    clock: Duration,
    /// [`Fault::TokenExpiresAtAuthorize`] has refused its authorization.
    authorize_refused: bool,
    /// CSC v2 credentials given other authentication: the ID, the expression and the objects.
    auth: Vec<(String, String, Value)>,
}

impl State {
    fn otp(&self) -> String {
        format!("{:06}", (u64::from(self.otp_serial) * 7919 + 314_159) % 1_000_000)
    }

    fn has(&self, f: Fault) -> bool {
        self.faults.contains(&f)
    }

    fn next(&mut self, prefix: &str) -> String {
        self.serial = self.serial.wrapping_add(1);
        format!("{prefix}-{}", self.serial)
    }

    /// Check an OTP and spend the current one, right or wrong.
    fn spend_otp(&mut self, given: Option<&str>) -> bool {
        let ok = given == Some(self.otp().as_str()) && !self.has(Fault::OtpExpired);
        self.new_otp(None);
        ok
    }

    /// The current OTP is used or replaced: a new one, sent for `object` (`None`: for none).
    fn new_otp(&mut self, object: Option<String>) {
        self.otp_serial = self.otp_serial.wrapping_add(1);
        self.sent_for = object;
    }
}

/// A signing service in memory. See the module documentation.
pub struct MockService {
    kind: Kind,
    credentials: Vec<Credential>,
    /// The CA: the root, and the intermediate that issues the credentials' certificates.
    root: Certificate,
    intermediate: Certificate,
    state: Mutex<State>,
}

/// Every certificate the mock issues is valid from 2000 to the end of 2099.
const VALID: (Time, Time) =
    (Time { year: 2000, month: 1, day: 1, hour: 0, minute: 0, second: 0 }, Time { year: 2099, month: 12, day: 31, hour: 23, minute: 59, second: 59 });

/// A certificate for `key` named `name`, issued by `issuer` (its name and key) or by itself: a
/// CA that may issue certificates, or one that signs documents.
fn issue(name: &Name, key: &PrivateKey, issuer: Option<(&Name, &PrivateKey)>, ca: bool, serial: &[u8]) -> Result<Certificate, SignError> {
    let (issuer_name, signer) = issuer.unwrap_or((name, key));
    let alg = signer.preferred_digest();
    let sig_alg = signer.signature_algorithm(alg);
    let spki = key.public_key().spki();
    let ext = |o: &str, critical: bool, value: &[u8]| {
        if critical { der::seq(&[&der::oid(o), &der::boolean(true), &der::octets(value)]) } else { der::seq(&[&der::oid(o), &der::octets(value)]) }
    };
    let ski = ext("2.5.29.14", false, &der::octets(&DigestAlg::Sha1.digest(&[&spki])));
    let extensions = if ca {
        // basicConstraints cA; keyUsage keyCertSign and cRLSign (bits 5, 6).
        let usage = der::tlv(tag::BIT_STRING, &[1, 0b0000_0110]);
        der::seq(&[&ext("2.5.29.19", true, &der::seq(&[&der::boolean(true)])), &ext("2.5.29.15", true, &usage), &ski])
    } else {
        // keyUsage digitalSignature and nonRepudiation; extKeyUsage emailProtection and Adobe's
        // document signing, as a signing certificate from a trust service has.
        let usage = der::tlv(tag::BIT_STRING, &[6, 0b1100_0000]);
        let purposes = der::seq(&[&der::oid("1.3.6.1.5.5.7.3.4"), &der::oid("1.2.840.113583.1.1.5")]);
        der::seq(&[&ext("2.5.29.15", true, &usage), &ski, &ext("2.5.29.37", false, &purposes)])
    };
    let tbs = der::seq(&[
        &der::explicit(0, &der::int(2)),
        &der::uint(serial),
        &sig_alg,
        &issuer_name.raw,
        &der::seq(&[&VALID.0.encode(), &VALID.1.encode()]),
        &name.raw,
        &spki,
        &der::explicit(3, &extensions),
    ]);
    let signature = signer.sign(alg, &tbs)?;
    Certificate::parse(&der::seq(&[&tbs, &sig_alg, &der::bit_string(&signature)]))
}

fn reply(status: u16, body: Value) -> HttpResponse {
    HttpResponse { status, body: if body.is_null() { Vec::new() } else { body.to_string().into_bytes() } }
}

fn csc_error(status: u16, error: &str, description: &str) -> HttpResponse {
    reply(status, json!({ "error": error, "error_description": description }))
}

impl MockService {
    /// A CSC API v2 service.
    pub fn csc_v2() -> Result<MockService, SignError> {
        MockService::new(Kind::CscV2)
    }

    /// A CSC API v1 service.
    pub fn csc_v1() -> Result<MockService, SignError> {
        MockService::new(Kind::CscV1)
    }

    /// An Aruba ARSS service.
    pub fn arss() -> Result<MockService, SignError> {
        MockService::new(Kind::Arss)
    }

    fn new(kind: Kind) -> Result<MockService, SignError> {
        let (root_name, root_key) = (Name::build("Mock Trust Services Root CA", "", "Mock Trust Services", "", "IT"), PrivateKey::generate_p256()?);
        let (ca_name, ca_key) = (Name::build("Mock Trust Services Signing CA", "", "Mock Trust Services", "", "IT"), PrivateKey::generate_p256()?);
        let root = issue(&root_name, &root_key, None, true, &[1])?;
        let intermediate = issue(&ca_name, &ca_key, Some((&root_name, &root_key)), true, &[2])?;
        let name = Name::build("Mario Rossi", "", "Mock Trust Services", "mario.rossi@example.com", "IT");
        let make = |id: &'static str, key: PrivateKey, enabled: bool| -> Result<Credential, SignError> {
            let serial = DigestAlg::Sha256.digest(&[id.as_bytes()]).into_iter().take(8).collect::<Vec<_>>();
            let certificate = issue(&name, &key, Some((&ca_name, &ca_key)), false, &serial)?;
            Ok(Credential { id, key, certificate, enabled })
        };
        let mut credentials = vec![make(RSA, PrivateKey::generate_rsa(2048)?, true)?, make(P256, PrivateKey::generate_p256()?, true)?];
        if kind != Kind::Arss {
            credentials.push(make(DISABLED, PrivateKey::generate_p256()?, false)?);
        }
        Ok(MockService { kind, credentials, root, intermediate, state: Mutex::new(State::default()) })
    }

    /// The account to sign in with (with the wrong password, a sign-in fails).
    pub fn account(&self) -> Account {
        match self.kind {
            Kind::CscV2 => Account { protocol: Protocol::Csc, url: CSC_V2_URL.into(), user: USER.into(), domain: None, hsm: None },
            Kind::CscV1 => Account { protocol: Protocol::Csc, url: CSC_V1_URL.into(), user: USER.into(), domain: None, hsm: None },
            Kind::Arss => Account { protocol: Protocol::Arss, url: ARSS_URL.into(), user: USER.into(), domain: Some(DOMAIN.into()), hsm: None },
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A test that panicked mid-request leaves state another test may still read.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The OTP the service would accept now: it changes once used or sent.
    pub fn current_otp(&self) -> String {
        self.state().otp()
    }

    /// Misbehave this way from now on.
    pub fn fail(&self, f: Fault) {
        self.state().faults.push(f);
    }

    /// Every request received, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.state().requests.clone()
    }

    /// How many times the client waited before polling.
    pub fn waits(&self) -> u32 {
        self.state().waits
    }

    /// The certificate of `credential`, to trust in a validation.
    pub fn certificate(&self, credential: &str) -> Option<Certificate> {
        self.credentials.iter().find(|c| c.id == credential).map(|c| c.certificate.clone())
    }

    /// The mock CA's root, self-signed, which a CSC `"chain"` ends with.
    pub fn root(&self) -> Certificate {
        self.root.clone()
    }

    /// The CA certificate that issued every credential's, which the root issued.
    pub fn intermediate(&self) -> Certificate {
        self.intermediate.clone()
    }

    /// From now on, authorize CSC v2 `credential` with these authentication `objects` (as
    /// `credentials/info` gives them) combined by `expression`. An object whose `id` is `PIN`
    /// takes [`PIN`], any other that takes a value the current OTP (once `getChallenge` sent it
    /// for a `ChallengeResponse`, that one alone), and `PasswordOOB` and `ChallengeResponseOOB`
    /// none (the user confirms on their phone).
    pub fn set_auth(&self, credential: &str, expression: &str, objects: Value) {
        let mut st = self.state();
        st.auth.retain(|(id, ..)| id != credential);
        st.auth.push((credential.to_string(), expression.to_string(), objects));
    }

    /// The expression and the objects a CSC v2 credential is authorized with.
    fn auth_of(&self, st: &State, c: &Credential) -> (String, Value) {
        if let Some((_, expression, objects)) = st.auth.iter().find(|(id, ..)| id == c.id) {
            return (expression.clone(), objects.clone());
        }
        if matches!(c.key.public_key(), PublicKey::Rsa { .. }) {
            let objects = json!([{ "type": "Password", "id": "PIN", "format": "N", "label": "PIN" },
                                 { "type": "ChallengeResponse", "id": "OTP", "format": "N", "label": "SMS code" }]);
            ("PIN AND OTP".into(), objects)
        } else {
            ("OTP".into(), json!([{ "type": "Password", "id": "OTP", "format": "N", "generator": "totp", "label": "Authenticator code" }]))
        }
    }

    fn credential(&self, id: &str) -> Option<&Credential> {
        self.credentials.iter().find(|c| c.id == id)
    }

    /// Sign `digest` (with `alg`) as a service would, or as [`Fault`]s say it misbehaves.
    fn sign(&self, st: &State, c: &Credential, alg: DigestAlg, digest: &[u8]) -> Result<Vec<u8>, SignError> {
        let other;
        let digest = if st.has(Fault::WrongDigest) {
            other = alg.digest(&[digest]);
            &other
        } else {
            digest
        };
        let rsa = matches!(c.key.public_key(), PublicKey::Rsa { .. });
        if rsa && st.has(Fault::BareDigest) {
            return bare_rsa(&c.key, digest);
        }
        let sig = c.key.sign_digest(alg, digest)?;
        match c.key.public_key() {
            PublicKey::P256(_) if st.has(Fault::RawEcdsa) => {
                crate::keys::ecdsa_rs(&sig, 32, &mut Vec::new()).ok_or_else(|| SignError::Crypto("mock: ECDSA signature".into()))
            }
            PublicKey::Rsa { .. } if st.has(Fault::ExtraLeadingZero) => Ok([&[0u8][..], &sig].concat()),
            _ => Ok(sig),
        }
    }

    fn answer_csc(&self, st: &mut State, url: &str, headers: &[(&str, &str)], body: &[u8]) -> HttpResponse {
        let base = if self.kind == Kind::CscV2 { CSC_V2_URL } else { CSC_V1_URL };
        let Some(method) = url.strip_prefix(base).and_then(|m| m.strip_prefix('/')) else {
            return csc_error(404, "invalid_request", "unknown address");
        };
        if header(headers, "Content-Type") != Some("application/json") {
            return csc_error(400, "invalid_request", "not JSON");
        }
        let Ok(Value::Object(b)) = serde_json::from_slice::<Value>(body) else { return csc_error(400, "invalid_request", "malformed JSON") };
        let v2 = self.kind == Kind::CscV2;
        match method {
            "info" => {
                let auth = if st.has(Fault::OauthOnly) { json!(["oauth2code"]) } else { json!(["basic", "oauth2code"]) };
                let mut methods =
                    vec!["auth/login", "auth/revoke", "credentials/list", "credentials/info", "credentials/authorize", "signatures/signHash"];
                if v2 {
                    methods.extend(["credentials/getChallenge", "credentials/authorizeCheck"]);
                }
                if !v2 || st.has(Fault::OffersSendOtp) {
                    methods.push("credentials/sendOTP");
                }
                let specs = if v2 { "2.2.0.0" } else { "1.0.4.0" };
                return reply(
                    200,
                    json!({ "specs": specs, "name": "Mock RSSP", "logo": "https://rssp.example/logo.png", "region": "IT", "lang": "it-IT",
                        "description": "Test service", "authType": auth, "oauth2": "https://rssp.example/", "methods": methods,
                        "signAlgorithms": { "algos": [oid::RSA, "1.2.840.10045.4.3.2"] } }),
                );
            }
            "auth/login" => return self.login(st, headers, &b),
            _ => {}
        }
        // Everything else needs the token.
        let token = header(headers, "Authorization").and_then(|a| a.strip_prefix("Bearer ")).unwrap_or_default().to_string();
        if !st.tokens.contains(&token) {
            return csc_error(401, "invalid_token", "the access token is invalid");
        }
        if st.stale.as_deref() == Some(token.as_str()) && !st.stale_done {
            st.stale_done = true;
            st.tokens.retain(|t| *t != token);
            return csc_error(401, "invalid_token", "the access token has expired");
        }
        let text = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        match method {
            "auth/revoke" => {
                let revoked = text("token");
                st.tokens.retain(|t| *t != revoked);
                reply(204, Value::Null)
            }
            "credentials/list" => {
                let ghosts = if st.has(Fault::ManyCredentials) { GHOSTS } else { 0 };
                let mut ids: Vec<String> = (0..ghosts).map(|i| format!("ghost-{i}")).collect();
                ids.extend(self.credentials.iter().map(|c| c.id.to_string()));
                if v2 && b.get("credentialInfo") == Some(&json!(true)) && !st.has(Fault::NoCredentialInfos) {
                    let infos: Vec<Value> = self.credentials.iter().map(|c| self.csc_info(st, c, &text("certificates"), true)).collect();
                    reply(200, json!({ "credentialIDs": ids, "credentialInfos": infos }))
                } else if !v2 && st.has(Fault::Paged) {
                    let at: usize = text("pageToken").parse().unwrap_or(0);
                    let page: Vec<&String> = ids.iter().skip(at).take(1).collect();
                    let next = at.saturating_add(1);
                    let mut v = json!({ "credentialIDs": page });
                    if let (true, Some(o)) = (next < ids.len(), v.as_object_mut()) {
                        o.insert("nextPageToken".into(), next.to_string().into());
                    }
                    reply(200, v)
                } else {
                    reply(200, json!({ "credentialIDs": ids }))
                }
            }
            "credentials/info" => match self.credential(&text("credentialID")) {
                Some(c) => reply(200, self.csc_info(st, c, &text("certificates"), false)),
                None => csc_error(400, "invalid_request", "Invalid parameter credentialID"),
            },
            "credentials/getChallenge" | "credentials/sendOTP" => {
                let Some(c) = self.credential(&text("credentialID")) else {
                    return csc_error(400, "invalid_request", "Invalid parameter credentialID");
                };
                // A v2 challenge is for one of the credential's objects; the code sent for a
                // ChallengeResponse is that object's alone.
                let mut sent_for = None;
                if v2 && method == "credentials/getChallenge" {
                    let (_, objects) = self.auth_of(st, c);
                    let id = text("authObjectID");
                    let field = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
                    let Some(o) = objects.as_array().into_iter().flatten().find(|o| field(o, "id").as_ref() == Some(&id)) else {
                        return csc_error(400, "invalid_request", "Invalid parameter authObjectID");
                    };
                    if field(o, "type").as_deref() == Some("ChallengeResponse") {
                        sent_for = Some(id);
                    }
                }
                st.new_otp(sent_for);
                reply(204, Value::Null)
            }
            "credentials/authorize" => {
                if st.has(Fault::TokenExpiresAtAuthorize) && !st.authorize_refused {
                    st.authorize_refused = true;
                    return csc_error(401, "invalid_token", "the access token has expired");
                }
                self.authorize(st, &b, &token)
            }
            "credentials/authorizeCheck" => {
                let handle = text("handle");
                if let Some(s) = st.faults.iter().find_map(|f| if let Fault::SlowAuthorizeCheck(s) = f { Some(*s) } else { None }) {
                    st.clock = st.clock.saturating_add(Duration::from_secs(u64::from(s)));
                }
                let Some(p) = st.pending.iter_mut().find(|p| p.0 == handle) else { return csc_error(400, "invalid_request", "Invalid handle") };
                if p.2 > 0 {
                    p.2 -= 1;
                    return reply(202, json!({ "handle": handle }));
                }
                let sad = p.1.clone();
                st.pending.retain(|p| p.0 != handle);
                reply(200, json!({ "SAD": sad, "expiresIn": 300 }))
            }
            "signatures/signHash" => self.sign_hash(st, &b, &token),
            _ => csc_error(501, "invalid_request", "method not implemented"),
        }
    }

    fn login(&self, st: &mut State, headers: &[(&str, &str)], b: &Map<String, Value>) -> HttpResponse {
        if b.get("rememberMe") != Some(&json!(false)) {
            return csc_error(400, "invalid_request", "rememberMe must be false here");
        }
        let given = header(headers, "Authorization").and_then(|a| a.strip_prefix("Basic ")).and_then(base64_lenient);
        if given.as_deref() != Some(format!("{USER}:{PASSWORD}").as_bytes()) {
            return csc_error(400, "authentication_error", "Invalid user or password");
        }
        let token = st.next("AT");
        st.tokens.push(token.clone());
        if st.has(Fault::TokenExpired) && st.stale.is_none() {
            st.stale = Some(token.clone());
        }
        reply(200, json!({ "access_token": token, "expires_in": 3600 }))
    }

    /// A `credentialInfo` in the mock's dialect, with the `certificates` asked for (`none`,
    /// `single`, the default, or `chain`: the signer's, the intermediate's and the root's, in
    /// that order); `listed`: inside `credentials/list`, with its ID.
    fn csc_info(&self, st: &State, c: &Credential, certificates: &str, listed: bool) -> Value {
        let rsa = matches!(c.key.public_key(), PublicKey::Rsa { .. });
        let key = if rsa {
            json!({ "status": if c.enabled { "enabled" } else { "disabled" }, "algo": [oid::RSA], "len": 2048 })
        } else {
            json!({ "status": if c.enabled { "enabled" } else { "disabled" }, "algo": ["1.2.840.10045.4.3.2"], "len": 256, "curve": oid::P256 })
        };
        let certs: Vec<String> = match certificates {
            "none" => Vec::new(),
            "chain" => [&c.certificate, &self.intermediate, &self.root].iter().map(|c| base64(&c.raw)).collect(),
            _ => vec![base64(&c.certificate.raw)],
        };
        let cert = json!({ "status": "valid", "certificates": certs });
        let mut v = if self.kind == Kind::CscV2 {
            let (expression, objects) = self.auth_of(st, c);
            json!({ "key": key, "cert": cert, "auth": { "mode": "explicit", "expression": expression, "objects": objects }, "SCAL": "2", "multisign": 1 })
        } else if rsa {
            json!({ "key": key, "cert": cert, "authMode": "explicit", "PIN": { "presence": "true", "format": "N", "label": "PIN" },
                "OTP": { "presence": "true", "type": "online", "format": "N", "label": "SMS code", "ID": "otp-sms" }, "multisign": 1 })
        } else {
            json!({ "key": key, "cert": cert, "authMode": "explicit", "PIN": { "presence": "false" },
                "OTP": { "presence": "true", "type": "offline", "format": "N", "label": "Authenticator code" }, "multisign": 1 })
        };
        if let (true, Some(o)) = (listed, v.as_object_mut()) {
            o.insert("credentialID".into(), c.id.into());
        }
        v
    }

    fn authorize(&self, st: &mut State, b: &Map<String, Value>, token: &str) -> HttpResponse {
        let v2 = self.kind == Kind::CscV2;
        let Some(c) = b.get("credentialID").and_then(Value::as_str).and_then(|id| self.credential(id)) else {
            return csc_error(400, "invalid_request", "Invalid parameter credentialID");
        };
        if !c.enabled {
            return csc_error(400, "invalid_request", "Key disabled");
        }
        if b.get("numSignatures") != Some(&json!(1)) {
            return csc_error(400, "invalid_request", "Numbers of signatures is too high");
        }
        let (hash_field, wrong_field) = if v2 { ("hashes", "hash") } else { ("hash", "hashes") };
        let hashes: Vec<String> =
            b.get(hash_field).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        if hashes.is_empty() || b.contains_key(wrong_field) {
            return csc_error(400, "invalid_request", "Missing (or invalid type) array parameter hash");
        }
        if v2 && b.get("hashAlgorithmOID").and_then(Value::as_str).and_then(DigestAlg::from_oid).is_none() {
            return csc_error(400, "invalid_request", "Missing hashAlgorithmOID");
        }
        let rsa = matches!(c.key.public_key(), PublicKey::Rsa { .. });
        if v2 {
            if b.contains_key("PIN") || b.contains_key("OTP") {
                return csc_error(400, "invalid_request", "PIN and OTP are v1 parameters");
            }
            if let Err(refused) = self.authorized_v2(st, c, b, token) {
                return refused;
            }
        } else {
            let text = |k: &str| b.get(k).and_then(Value::as_str).map(str::to_string);
            let (pin, otp) = (text("PIN"), text("OTP"));
            // S1 §11.6: a v1 PIN only where the credential's PIN `presence` isn't "false".
            if !rsa && pin.is_some() {
                return csc_error(400, "invalid_request", "This credential takes no PIN");
            }
            if st.has(Fault::EchoSecretsOnAuthorize) {
                let (pin, otp) = (pin.unwrap_or_default(), otp.unwrap_or_default());
                return csc_error(400, "invalid_request", &format!("refused PIN {pin} and OTP {otp} for {USER}:{PASSWORD} with token {token}"));
            }
            let pin_ok = !rsa || pin.as_deref() == Some(PIN);
            let otp_ok = st.spend_otp(otp.as_deref());
            if !pin_ok || !otp_ok {
                if st.has(Fault::WrongPinIs401) {
                    return csc_error(401, "invalid_pin", "Invalid PIN");
                }
                return if pin_ok {
                    csc_error(400, "invalid_otp", "The OTP is invalid or has expired")
                } else {
                    csc_error(400, "invalid_pin", "Invalid PIN")
                };
            }
        }
        let sad = st.next("SAD");
        let bound = if st.has(Fault::HashNotAuthorized) { vec![base64(&[0u8; 32])] } else { hashes };
        st.grants.push(Grant { sad: sad.clone(), credential: c.id.into(), hashes: bound });
        if let Some(polls) = st.faults.iter().find_map(|f| if let Fault::AuthorizePending(n) = f { Some(*n) } else { None }) {
            let handle = st.next("H");
            st.pending.push((handle.clone(), sad, polls));
            return reply(202, json!({ "handle": handle }));
        }
        reply(200, json!({ "SAD": sad, "expiresIn": 300 }))
    }

    /// A v2 `authData` that names only `c`'s objects, each once, with a value exactly where one
    /// is taken, all of them right, and satisfies its expression. The OTP is spent when one was
    /// sent, right or wrong.
    fn authorized_v2(&self, st: &mut State, c: &Credential, b: &Map<String, Value>, token: &str) -> Result<(), HttpResponse> {
        let (expression, objects) = self.auth_of(st, c);
        let objects = objects.as_array().cloned().unwrap_or_default();
        let field = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let mut sent: Vec<(String, Option<String>)> = Vec::new();
        for d in b.get("authData").and_then(Value::as_array).into_iter().flatten() {
            let id = field(d, "id");
            let Some(o) = objects.iter().find(|o| field(o, "id") == id) else {
                return Err(csc_error(400, "invalid_request", "Invalid parameter authData: unknown object"));
            };
            let value = d.get("value").and_then(Value::as_str).map(str::to_string);
            if sent.iter().any(|(s, _)| *s == id) || field(o, "type").ends_with("OOB") == value.is_some() {
                return Err(csc_error(400, "invalid_request", "Invalid parameter authData"));
            }
            sent.push((id, value));
        }
        if st.has(Fault::EchoSecretsOnAuthorize) {
            let values: Vec<&str> = sent.iter().filter_map(|(_, v)| v.as_deref()).collect();
            return Err(csc_error(400, "invalid_request", &format!("refused {} for {USER}:{PASSWORD} with token {token}", values.join(" and "))));
        }
        let otp = st.otp();
        let fresh = !st.has(Fault::OtpExpired);
        // A code sent for one object (an SMS) is no other's (an app's).
        let takes_code = |id: &str| st.sent_for.as_deref().is_none_or(|s| s == id);
        let right = |(id, v): &(String, Option<String>)| match v.as_deref() {
            None => true,
            Some(v) if id == "PIN" => v == PIN,
            Some(v) => v == otp && fresh && takes_code(id),
        };
        let all_right = sent.iter().all(right);
        if sent.iter().any(|(id, v)| v.is_some() && id != "PIN") {
            st.new_otp(None);
        }
        let ids: Vec<String> = objects.iter().map(|o| field(o, "id")).collect();
        let present: Vec<&str> = sent.iter().map(|(id, _)| id.as_str()).collect();
        if all_right && satisfies(&expression, &ids, &present) == Some(true) {
            return Ok(());
        }
        Err(if st.has(Fault::WrongPinIs401) {
            csc_error(401, "invalid_pin", "Invalid PIN")
        } else {
            csc_error(400, "invalid_authentication_data", "Invalid PIN or OTP")
        })
    }

    fn sign_hash(&self, st: &mut State, b: &Map<String, Value>, token: &str) -> HttpResponse {
        let v2 = self.kind == Kind::CscV2;
        let text = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let sad = text("SAD");
        if st.has(Fault::EchoSecretsOnSign) {
            return csc_error(400, "invalid_request", &format!("refused SAD {sad} for {USER}:{PASSWORD} with token {token}"));
        }
        let Some(at) = st.grants.iter().position(|g| g.sad == sad) else { return csc_error(400, "invalid_request", "SAD expired") };
        let Some(c) = self.credential(&text("credentialID")) else { return csc_error(400, "invalid_request", "Invalid parameter credentialID") };
        let (hash_field, alg_field) = if v2 { ("hashes", "hashAlgorithmOID") } else { ("hash", "hashAlgo") };
        let hashes: Vec<String> =
            b.get(hash_field).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        let grant = st.grants.remove(at);
        if grant.credential != c.id || hashes.is_empty() || hashes != grant.hashes {
            return csc_error(400, "invalid_request", "Hash is not authorized by neither SAD nor an access token with scope 'credential'");
        }
        let algo = text("signAlgo");
        let listed = if matches!(c.key.public_key(), PublicKey::Rsa { .. }) { oid::RSA } else { "1.2.840.10045.4.3.2" };
        if algo != listed {
            return csc_error(400, "invalid_request", "Invalid parameter signAlgo");
        }
        let Some(alg) = DigestAlg::from_oid(&text(alg_field)) else {
            return csc_error(400, "invalid_request", "Missing hashAlgo when signAlgo is 1.2.840.113549.1.1.1");
        };
        let mut signatures = Vec::new();
        for h in &hashes {
            let Some(digest) = base64_lenient(h).filter(|d| d.len() == alg.output_len()) else {
                return csc_error(400, "invalid_request", "Invalid digest value");
            };
            match self.sign(st, c, alg, &digest) {
                Ok(sig) => signatures.push(base64(&sig)),
                Err(e) => return csc_error(500, "server_error", &e.to_string()),
            }
        }
        reply(200, json!({ "signatures": signatures }))
    }

    fn answer_arss(&self, st: &mut State, url: &str, headers: &[(&str, &str)], body: &[u8]) -> HttpResponse {
        if url != ARSS_URL {
            return soap_fault("unknown address");
        }
        if header(headers, "SOAPAction") != Some("\"\"") || header(headers, "Content-Type") != Some("text/xml; charset=utf-8") {
            return soap_fault("SOAPAction must be \"\" and the content text/xml");
        }
        let Ok(text) = std::str::from_utf8(body) else { return soap_fault("not UTF-8") };
        let Ok(doc) = Document::parse(text) else { return soap_fault("not XML") };
        let Some(op) =
            doc.descendants().find(|n| n.is_element() && n.tag_name().name() == "Body").and_then(|b| b.children().find(|n| n.is_element()))
        else {
            return soap_fault("no body");
        };
        if op.tag_name().namespace() != Some(super::arss::NS) {
            return soap_fault("the operation is not in the service namespace");
        }
        let name = op.tag_name().name();
        // What the real service answers to qualified children (it doesn't see them).
        if op.descendants().skip(1).any(|n| n.is_element() && n.tag_name().namespace().is_some()) {
            return arss_return(name, "<description>Invalid Parameter. %s</description><return_code>0002</return_code><status>KO</status>");
        }
        let ko = |code: &str, description: &str| {
            arss_return(name, &format!("<description>{description}</description><return_code>{code}</return_code><status>KO</status>"))
        };
        // As the real service answers an account without its domain.
        let no_domain = || arss_return(name, "<description>Invalid Parameters</description><status>KO</status>");
        match name {
            "listCert" => match identity(op.children().find(|n| n.has_tag_name("Identity"))) {
                Identity::Right => {
                    let mut certs: String = self
                        .credentials
                        .iter()
                        .map(|c| format!("<app1><id>{}</id><value>{}</value></app1>", c.id, base64(&c.certificate.raw)))
                        .collect();
                    if st.has(Fault::NilAndRepeated) {
                        let nil = "<app2 xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:nil=\"true\"/>";
                        certs = format!("{nil}{certs}{certs}");
                    }
                    arss_return(name, &format!("{certs}<description>OK</description><status>OK</status>"))
                }
                Identity::NoDomain => no_domain(),
                Identity::WrongDomain => arss_return(name, "<description>Error 0003 - Invalid User Credentials</description><status>KO</status>"),
                Identity::Wrong => arss_return(name, "<description>Invalid Credentials</description><status>KO</status>"),
            },
            "sendCredential" => {
                match identity(op.children().find(|n| n.has_tag_name("Identity"))) {
                    Identity::Right => {}
                    Identity::NoDomain => return no_domain(),
                    Identity::WrongDomain | Identity::Wrong => return ko("0003", "Credentials Error"),
                }
                if child(op, "type") != Some("SMS") {
                    return ko("0002", "Invalid Parameter. type");
                }
                st.new_otp(None);
                arss_return(name, "<description>OK</description><return_code>0000</return_code><status>OK</status>")
            }
            "signhash" => {
                let Some(req) = op.children().find(|n| n.has_tag_name("SignHashRequest")) else { return ko("0002", "Empty request") };
                let Some(id) = req.children().find(|n| n.has_tag_name("identity")) else { return ko("0002", "Empty Identity") };
                match identity(Some(id)) {
                    Identity::Right => {}
                    Identity::NoDomain => return no_domain(),
                    Identity::WrongDomain | Identity::Wrong => return ko("0003", "Credentials Error"),
                }
                if st.has(Fault::EchoSecretsOnSign) {
                    let (pwd, otp) = (child(id, "userPWD").unwrap_or_default(), child(id, "otpPwd").unwrap_or_default());
                    return ko("0099", &super::arss::escape(&format!("refused {USER}:{pwd} with OTP {otp}")));
                }
                if !matches!(child(req, "requirecert"), Some("true" | "false")) {
                    return ko("0002", "Invalid Parameter. requirecert");
                }
                if !st.spend_otp(child(id, "otpPwd")) {
                    return ko("0001", "Invalid OTP");
                }
                let Some(c) = child(req, "certID").and_then(|i| self.credential(i)) else { return ko("0002", "Invalid Parameter. certID") };
                let alg = match child(req, "hashtype") {
                    Some("SHA256") => DigestAlg::Sha256,
                    Some("SHA384") => DigestAlg::Sha384,
                    _ => return ko("0002", "Invalid Parameter. hashtype"),
                };
                let Some(digest) = child(req, "hash").and_then(base64_lenient).filter(|d| d.len() == alg.output_len()) else {
                    return ko("0002", "Invalid Parameter. hash");
                };
                match self.sign(st, c, alg, &digest) {
                    Ok(sig) => arss_return(
                        name,
                        &format!(
                            "<certID>{}</certID><description>OK</description><return_code>0000</return_code><signature>{}</signature><status>OK</status>",
                            c.id,
                            base64(&sig)
                        ),
                    ),
                    Err(_) => ko("0030", "Signature error"),
                }
            }
            _ => soap_fault("unknown operation"),
        }
    }
}

impl HttpTransport for MockService {
    fn post(&self, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, SignError> {
        let mut st = self.state();
        st.requests.push(Request {
            url: url.to_string(),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: body.to_vec(),
        });
        let r = match self.kind {
            Kind::CscV2 | Kind::CscV1 => self.answer_csc(&mut st, url, headers, body),
            Kind::Arss => self.answer_arss(&mut st, url, headers, body),
        };
        if st.has(Fault::Oversized) {
            return Ok(HttpResponse { status: r.status, body: vec![b' '; MAX_RESPONSE + 1] });
        }
        Ok(r)
    }

    fn wait(&self, d: Duration) {
        let mut st = self.state();
        st.waits = st.waits.saturating_add(1);
        st.clock = st.clock.saturating_add(d);
    }

    fn clock(&self) -> Option<Duration> {
        Some(self.state().clock)
    }
}

/// Whether the object IDs `present` satisfy a CSC `auth/expression` over `all` (empty: an `AND`
/// of every object), read as a strict service would: `AND` before `OR` and `XOR`, an `XOR` true
/// of exactly one of its operands however many there are, and `OR` and `XOR` not mixed without
/// parentheses. `None` when it can't be read.
fn satisfies(expression: &str, all: &[String], present: &[&str]) -> Option<bool> {
    let spaced = expression.replace('(', " ( ").replace(')', " ) ");
    let t: Vec<&str> = spaced.split_whitespace().collect();
    if t.is_empty() {
        return Some(all.iter().all(|id| present.contains(&id.as_str())));
    }
    fn either(t: &[&str], at: &mut usize, present: &[&str]) -> Option<bool> {
        let mut values = vec![both(t, at, present)?];
        let mut operator = None;
        while let Some(&op) = t.get(*at).filter(|op| matches!(**op, "OR" | "XOR")) {
            if operator.is_some_and(|o| o != op) {
                return None;
            }
            operator = Some(op);
            *at += 1;
            values.push(both(t, at, present)?);
        }
        let true_ones = values.iter().filter(|v| **v).count();
        Some(if operator == Some("XOR") { true_ones == 1 } else { true_ones > 0 })
    }
    fn both(t: &[&str], at: &mut usize, present: &[&str]) -> Option<bool> {
        let mut v = one(t, at, present)?;
        while t.get(*at) == Some(&"AND") {
            *at += 1;
            v &= one(t, at, present)?;
        }
        Some(v)
    }
    fn one(t: &[&str], at: &mut usize, present: &[&str]) -> Option<bool> {
        let token = *t.get(*at)?;
        *at += 1;
        if token != "(" {
            return Some(present.contains(&token));
        }
        let v = either(t, at, present)?;
        (t.get(*at) == Some(&")")).then(|| {
            *at += 1;
            v
        })
    }
    let mut at = 0;
    let v = either(&t, &mut at, present)?;
    (at == t.len()).then_some(v)
}

fn header<'a>(headers: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| *v)
}

fn child<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children().find(|n| n.has_tag_name(name)).and_then(|n| n.text())
}

/// What an ARSS `auth` element holds.
enum Identity {
    /// The account's user, password and domain.
    Right,
    /// No domain (`typeOtpAuth`).
    NoDomain,
    /// Another domain.
    WrongDomain,
    /// Another user or password.
    Wrong,
}

fn identity(id: Option<Node<'_, '_>>) -> Identity {
    let Some(id) = id else { return Identity::Wrong };
    match child(id, "typeOtpAuth") {
        None => Identity::NoDomain,
        Some(d) if d != DOMAIN => Identity::WrongDomain,
        Some(_) if child(id, "user") == Some(USER) && child(id, "userPWD") == Some(PASSWORD) => Identity::Right,
        Some(_) => Identity::Wrong,
    }
}

fn soap(inner: &str) -> String {
    format!(
        "<?xml version='1.0' encoding='UTF-8'?><S:Envelope xmlns:S=\"http://schemas.xmlsoap.org/soap/envelope/\"><S:Body>{inner}</S:Body></S:Envelope>"
    )
}

fn arss_return(operation: &str, inner: &str) -> HttpResponse {
    let ns = super::arss::NS;
    let body = soap(&format!("<ns2:{operation}Response xmlns:ns2=\"{ns}\"><return>{inner}</return></ns2:{operation}Response>"));
    HttpResponse { status: 200, body: body.into_bytes() }
}

fn soap_fault(why: &str) -> HttpResponse {
    let body = soap(&format!("<S:Fault><faultcode>S:Client</faultcode><faultstring>{}</faultstring></S:Fault>", super::arss::escape(why)));
    HttpResponse { status: 500, body: body.into_bytes() }
}

/// PKCS #1 v1.5 padding around the bare digest, the mistake [`Fault::BareDigest`] makes. Only
/// the `rsa` crate pads without a DigestInfo; ADR-0009 keeps it away from real private keys,
/// and this one is a throwaway test key.
#[cfg(not(target_arch = "wasm32"))]
fn bare_rsa(key: &PrivateKey, digest: &[u8]) -> Result<Vec<u8>, SignError> {
    use crate::der::Tlv;
    use rsa::{BoxedUint, Pkcs1v15Sign, RsaPrivateKey};
    let bad = || SignError::Crypto("mock: RSA key".into());
    let info = Tlv::parse_all(key.pkcs8())?.children()?;
    let rsa_key = Tlv::parse_all(info.get(2).ok_or_else(bad)?.value)?.children()?;
    let int = |i: usize| rsa_key.get(i).map(|t| BoxedUint::from_be_slice_vartime(t.uint_bytes())).ok_or_else(bad);
    let k = RsaPrivateKey::from_components(int(1)?, int(2)?, int(3)?, vec![int(4)?, int(5)?]).map_err(|_| bad())?;
    k.sign(Pkcs1v15Sign::new_unprefixed(), digest).map_err(|_| bad())
}

#[cfg(target_arch = "wasm32")]
fn bare_rsa(_key: &PrivateKey, _digest: &[u8]) -> Result<Vec<u8>, SignError> {
    Err(SignError::Unsupported("RSA signing in the browser".into()))
}

#[cfg(test)]
mod tests {
    use super::satisfies;

    #[test]
    fn xor_is_exactly_one_operand_and_or_at_least_one() {
        let all: Vec<String> = ["A", "B", "C"].iter().map(|s| s.to_string()).collect();
        let check = |e: &str, present: &[&str]| satisfies(e, &all, present);
        // Three operands: all three true is odd parity, but not exactly one.
        assert_eq!(check("A XOR B XOR C", &["A", "B", "C"]), Some(false));
        assert_eq!(check("A XOR B XOR C", &["B"]), Some(true));
        assert_eq!(check("A XOR B XOR C", &["A", "C"]), Some(false));
        assert_eq!(check("A XOR B XOR C", &[]), Some(false));
        assert_eq!(check("A OR B OR C", &["A", "B", "C"]), Some(true));
        assert_eq!(check("A OR B OR C", &[]), Some(false));
        // AND binds tighter; parentheses group; OR and XOR don't mix without them.
        assert_eq!(check("A AND B XOR C", &["A", "B", "C"]), Some(false));
        assert_eq!(check("A AND (B XOR C)", &["A", "C"]), Some(true));
        assert_eq!(check("(A OR B) XOR C", &["A", "B"]), Some(true));
        assert_eq!(check("A OR B XOR C", &["A"]), None);
        assert_eq!(check("", &["A", "B", "C"]), Some(true));
        assert_eq!(check("", &["A", "B"]), Some(false));
    }
}
