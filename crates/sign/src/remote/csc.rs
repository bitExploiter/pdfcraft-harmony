//! The Cloud Signature Consortium API, v2 (2.0–2.2) with the v1 (1.0.4) dialect tolerated:
//! `info`, HTTP Basic `auth/login`, `credentials/list` and `credentials/info`, the OTP through
//! `credentials/getChallenge` or `credentials/sendOTP`, `credentials/authorize` and
//! `signatures/signHash`. The field names that differ between the versions (v1 `hash`, `PIN`,
//! `OTP`; v2 `hashes`, `hashAlgorithmOID`, `authData`) follow the major version `info.specs`
//! gives. OAuth sign-in isn't supported yet and is reported as such.

use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{
    Account, AuthNeeds, AuthObject, AuthRole, CANT_SIGN, CONFIRMED_ELSEWHERE, Confirmation, HttpResponse, HttpTransport, KEY_CANT_SIGN, NEEDS_OAUTH,
    NO_OTP, NO_SAD, NO_SIGNATURE, NO_TOKEN, NOT_CONFIRMED, NOT_JSON, NOT_SENT, OtpKind, OtpSent, REFUSED_REQUEST, REFUSED_TO_DESCRIBE, REJECTED_OTP,
    REJECTED_PASSWORD, REJECTED_PIN, REJECTED_PIN_OR_OTP, RemoteIdentity, SHOWS_CHALLENGE, SIGN_IN_EXPIRED, SIGN_IN_UNSUPPORTED, Secrets, USER_COLON,
    base64, base64_lenient, exchange, missing, none_usable, quote, signing_certificate,
};
use crate::keys::{DigestAlg, PublicKey, oid};
use crate::{Certificate, SignError};

/// The most credentials listed, and certificates read per credential.
const MAX_CREDENTIALS: usize = 64;
const MAX_CERTIFICATES: usize = 16;
/// How long an authorization confirmed on another device is waited for: 30 polls 2 s apart, and
/// never past a minute however slowly the service answers them.
const POLLS: u32 = 30;
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_FOR: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Version {
    V1,
    V2,
}

/// What `info` says about the service.
struct Service {
    /// The base address without a trailing slash; method names follow it.
    base: String,
    version: Version,
    methods: Vec<String>,
}

impl Service {
    /// Whether the service lists `method`; one that lists none may have any.
    fn offers(&self, method: &str) -> bool {
        self.methods.is_empty() || self.lists(method)
    }

    fn lists(&self, method: &str) -> bool {
        self.methods.iter().any(|m| m == method)
    }
}

fn oauth() -> String {
    "it needs OAuth sign-in in a browser, which PdfCraft doesn't support yet".into()
}

fn service(t: &dyn HttpTransport, account: &Account) -> Result<Service, SignError> {
    let base = account.url.trim_end_matches('/').to_string();
    let r = exchange(t, &format!("{base}/info"), &[("Content-Type", "application/json")], b"{}", &[])?;
    let v = answer(&r).map_err(|e| refusal(e, &[]))?;
    let specs = v.get("specs").and_then(Value::as_str).unwrap_or_default();
    let version = match specs.split('.').next().unwrap_or_default() {
        "1" => Version::V1,
        "" if base.ends_with("/v1") => Version::V1,
        _ => Version::V2,
    };
    if let Some(types) = v.get("authType").and_then(Value::as_array) {
        let types: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
        if !types.iter().any(|t| t.eq_ignore_ascii_case("basic")) {
            if types.iter().any(|t| t.starts_with("oauth2")) {
                return Err(SignError::Crypto(NEEDS_OAUTH.into()));
            }
            return Err(SignError::Crypto(format!("{SIGN_IN_UNSUPPORTED} ({})", quote(&types.join(", "), &[]))));
        }
    }
    let methods = v.get("methods").and_then(Value::as_array).map(|m| m.iter().filter_map(Value::as_str).take(256).map(str::to_string).collect());
    Ok(Service { base, version, methods: methods.unwrap_or_default() })
}

/// An answer that isn't a usable success: the status, and the service's error code and
/// description as it wrote them.
struct Refused {
    status: u16,
    error: String,
    description: Option<String>,
    /// A success whose body isn't JSON.
    garbled: bool,
}

/// The JSON of a successful answer (2xx; an empty body reads as `{}`), or what was refused.
fn answer(r: &HttpResponse) -> Result<Value, Refused> {
    let refused = |error: String, description: Option<String>, garbled: bool| Refused { status: r.status, error, description, garbled };
    if (200..300).contains(&r.status) {
        if r.body.iter().all(u8::is_ascii_whitespace) {
            return Ok(json!({}));
        }
        return serde_json::from_slice(&r.body).map_err(|_| refused(String::new(), None, true));
    }
    let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Err(refused(text("error").unwrap_or_default(), text("error_description"), false))
}

/// What the user is told when the service refuses a request: what it means, then the service's
/// own description with `secrets` kept out.
fn refusal(r: Refused, secrets: &[&str]) -> SignError {
    if r.garbled {
        return SignError::Crypto(NOT_JSON.into());
    }
    let code = quote(&r.error, secrets);
    let what = match code.as_str() {
        "authentication_error" => REJECTED_PASSWORD.to_string(),
        "invalid_pin" => REJECTED_PIN.to_string(),
        "invalid_otp" => REJECTED_OTP.to_string(),
        "" => format!("{REFUSED_REQUEST} (HTTP {})", r.status),
        code => format!("{REFUSED_REQUEST} ({code})"),
    };
    described(what, r.description, secrets)
}

/// `what` happened, then the service's own description of it with `secrets` kept out.
fn described(what: String, description: Option<String>, secrets: &[&str]) -> SignError {
    SignError::Crypto(match description.map(|d| quote(&d, secrets)) {
        Some(d) if !d.is_empty() => format!("{what}: {d}"),
        _ => what,
    })
}

/// A token the service no longer accepts: its error says so, or it is a bare 401 with no error
/// code at all (a gateway in front of the service). A 401 that names another error is that
/// error: some services answer a wrong PIN with 401 `invalid_pin`.
fn token_refused(r: &HttpResponse) -> bool {
    let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    match v.get("error").and_then(Value::as_str) {
        Some(code) => matches!(code, "invalid_token" | "expired_token"),
        None => r.status == 401,
    }
}

/// The methods never sent twice, even with a renewed token: they carry the PIN and the OTP, or
/// have the service send the user a code, so a second could count as one more wrong PIN
/// towards a lock, or void the code the user just got.
const SENT_ONCE: [&str; 3] = ["credentials/authorize", "credentials/sendOTP", "credentials/getChallenge"];

/// A signed-in conversation with the service: the Bearer token, renewed once when the service
/// stops accepting it.
struct Client<'a> {
    t: &'a dyn HttpTransport,
    service: &'a Service,
    user: &'a str,
    password: &'a str,
    token: String,
    renewed: bool,
}

impl<'a> Client<'a> {
    fn login(t: &'a dyn HttpTransport, service: &'a Service, user: &'a str, password: &'a str) -> Result<Client<'a>, SignError> {
        let token = login(t, service, user, password)?;
        Ok(Client { t, service, user, password, token, renewed: false })
    }

    /// `extra` and the secrets of the conversation itself, to keep out of quoted text.
    fn hidden<'s>(&'s self, extra: &[&'s str]) -> Vec<&'s str> {
        let mut all = extra.to_vec();
        all.extend([self.password, self.token.as_str()]);
        all
    }

    fn refused(&self, e: Refused, extra: &[&str]) -> SignError {
        refusal(e, &self.hidden(extra))
    }

    /// POST `body` to `method` with the token; an expired one is renewed (once) and the request
    /// sent again, except for [`SENT_ONCE`] methods, which fail instead.
    fn call(&mut self, method: &str, body: &Value, extra: &[&str]) -> Result<HttpResponse, SignError> {
        let url = format!("{}/{method}", self.service.base);
        let body = body.to_string();
        loop {
            let bearer = format!("Bearer {}", self.token);
            let headers = [("Content-Type", "application/json"), ("Authorization", bearer.as_str())];
            let r = exchange(self.t, &url, &headers, body.as_bytes(), &self.hidden(extra))?;
            if self.renewed || !token_refused(&r) {
                return Ok(r);
            }
            if SENT_ONCE.contains(&method) {
                return Err(SignError::Crypto(SIGN_IN_EXPIRED.into()));
            }
            self.renewed = true;
            self.token = login(self.t, self.service, self.user, self.password)?;
        }
    }

    /// [`Self::call`], and the JSON of a successful answer.
    fn json(&mut self, method: &str, body: &Value, extra: &[&str]) -> Result<Value, SignError> {
        let r = self.call(method, body, extra)?;
        answer(&r).map_err(|e| self.refused(e, extra))
    }

    /// End the session, as the spec recommends. Best effort: whatever it was for is done.
    fn revoke(mut self) {
        let body = json!({ "token": self.token, "token_type_hint": "access_token" });
        // Never sign in again only to sign out.
        self.renewed = true;
        let _ = self.call("auth/revoke", &body, &[]);
    }
}

/// `auth/login` with HTTP Basic; the access token. The user name goes trimmed, as the account
/// was checked; the password as typed.
fn login(t: &dyn HttpTransport, service: &Service, user: &str, password: &str) -> Result<String, SignError> {
    let user = user.trim();
    // RFC 7617: the user-id of Basic credentials can't contain a colon.
    if user.contains(':') {
        return Err(SignError::Crypto(USER_COLON.into()));
    }
    let encoded = base64(format!("{user}:{password}").as_bytes());
    let basic = format!("Basic {encoded}");
    // The header decodes to the password, so a service that echoes it back mustn't show it either.
    let hidden = [password, basic.as_str(), encoded.as_str()];
    let headers = [("Content-Type", "application/json"), ("Authorization", basic.as_str())];
    let r = exchange(t, &format!("{}/auth/login", service.base), &headers, br#"{"rememberMe":false}"#, &hidden)?;
    let v = answer(&r).map_err(|e| {
        if !e.garbled && (e.status == 401 || e.error == "authentication_error") {
            SignError::Crypto(REJECTED_PASSWORD.into())
        } else {
            refusal(e, &hidden)
        }
    })?;
    match v.get("access_token").and_then(Value::as_str) {
        Some(token) if !token.is_empty() => Ok(token.to_string()),
        _ => Err(SignError::Crypto(NO_TOKEN.into())),
    }
}

/// A credential as `credentials/info` describes it.
struct Credential {
    certificate: Certificate,
    chain: Vec<Certificate>,
    auth: AuthNeeds,
    /// Which of the authentication objects an authorization sends.
    rule: Rule,
    /// `key/algo`: the signature algorithms the key signs with.
    algos: Vec<String>,
}

/// Which of a credential's authentication objects an authorization sends.
#[derive(Debug)]
enum Rule {
    /// v1: the PIN and the OTP each when given; `pin` and `otp` when its `presence` is "true",
    /// and the service can't do without it.
    V1 { pin: bool, otp: bool },
    /// v2: what `auth/expression` asks for.
    V2(Expr),
}

/// Whether the service sends the code of `o` when asked for it (`credentials/getChallenge`):
/// what Send code asks for, and so where a code the user had sent goes.
fn sent_when_asked(o: &AuthObject) -> bool {
    o.role == AuthRole::Otp && o.kind == "ChallengeResponse"
}

impl Credential {
    /// The objects an authorization with `secrets` sends, or the error for a secret the
    /// credential can't do without. A code the service sent ([`Secrets::otp_sent`]) goes under
    /// the object [`Self::to_send`] had it sent for, both choosing with [`Expr::choose`] alike.
    fn chosen(&self, secrets: &Secrets) -> Result<Vec<&AuthObject>, SignError> {
        let given = |role: AuthRole| match role {
            AuthRole::Pin => secrets.pin().is_some(),
            AuthRole::Otp => secrets.otp().is_some(),
            AuthRole::Confirm => true,
        };
        let sent = |o: &AuthObject| secrets.otp_sent && sent_when_asked(o);
        let objects = &self.auth.objects;
        let rule = match &self.rule {
            Rule::V1 { pin, otp } => {
                if *pin && secrets.pin().is_none() {
                    return Err(missing("its PIN"));
                }
                if *otp && secrets.otp().is_none() {
                    return Err(missing("a one-time password (OTP)"));
                }
                return Ok(objects.iter().filter(|o| given(o.role)).collect());
            }
            Rule::V2(rule) => rule,
        };
        if let Some((chosen, _)) = rule.choose(objects, &given, &sent) {
            return Ok(chosen);
        }
        // Name the secret that would have done; when neither alone would, the PIN first.
        let pin_does = rule.choose(objects, &|r| r == AuthRole::Pin || given(r), &sent).is_some();
        let otp_does = rule.choose(objects, &|r| r == AuthRole::Otp || given(r), &sent).is_some();
        Err(if pin_does || (!otp_does && secrets.pin().is_none()) { missing("its PIN") } else { missing("a one-time password (OTP)") })
    }

    /// The object Send code has the service send the code for (`credentials/getChallenge`): the
    /// one [`Self::chosen`] fills with a code the service sent, the PIN given too. `None` when
    /// that is no object the service sends a code for: v1, or a v2 `Password` code (one that
    /// `credentials/sendOTP` may send, or an app's).
    fn to_send(&self) -> Option<&AuthObject> {
        let Rule::V2(rule) = &self.rule else { return None };
        let (chosen, _) = rule.choose(&self.auth.objects, &|_| true, &sent_when_asked)?;
        chosen.into_iter().find(|o| sent_when_asked(o))
    }
}

/// Read a `credentialInfo`; the `Err` is why the credential can't sign through PdfCraft.
fn parse_credential(v: &Value, service: &Service) -> Result<Credential, String> {
    let key = v.get("key");
    if key.and_then(|k| k.get("status")).and_then(Value::as_str) == Some("disabled") {
        return Err("its key is disabled".into());
    }
    let algos: Vec<String> = key
        .and_then(|k| k.get("algo"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(32).map(str::to_string).collect())
        .unwrap_or_default();
    let certs: Vec<&str> = v
        .get("cert")
        .and_then(|c| c.get("certificates"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(MAX_CERTIFICATES).collect())
        .unwrap_or_default();
    let (leaf, rest) = certs.split_first().ok_or("the service gave no certificate for it")?;
    let certificate = signing_certificate(&base64_lenient(leaf).ok_or("its certificate isn't base64")?)?;
    // A key that signs with nothing PdfCraft can ask for is no use, and finding that out only
    // when signing would spend the code the service sent for it.
    sign_algo(&certificate.public_key, certificate.public_key.preferred_digest(), &algos)
        .map_err(|what| format!("its key can't sign with {what}"))?;
    // The rest is the chain, end-entity first; an issuer that doesn't read is left out.
    let chain =
        rest.iter().filter_map(|c| base64_lenient(c)).filter_map(|d| Certificate::parse(&d).ok()).filter(|c| c.raw != certificate.raw).collect();
    let (auth, rule) = match service.version {
        Version::V2 => auth_v2(v.get("auth"), service)?,
        Version::V1 => auth_v1(v)?,
    };
    Ok(Credential { certificate, chain, auth, rule, algos })
}

/// Whether the id or the label of an authentication object names `word` (lower case).
fn names(id: &str, label: Option<&str>, word: &str) -> bool {
    [Some(id), label].into_iter().flatten().any(|s| s.to_ascii_lowercase().contains(word))
}

/// Words in a Password's id or label that make it a one-time code: whatever else it says
/// (`ONE_TIME`: "otp" covers "totp"), or unless it is also named the PIN (`CODE`: a "Codice
/// PIN", a "PIN code" or a "Token PIN" is the PIN, a "Codice" or a "Token code" a code).
const ONE_TIME: &[&str] = &["otp", "one-time", "one time", "sms"];
const CODE: &[&str] = &["codice", "code", "token"];

/// v2 `auth`: `mode`, the authentication `objects` and the `expression` that combines them
/// (S22 §11.6–11.7, the object types in §11.8). A "Password" with a `generator`, or whose id
/// or label says it is a one-time code ([`ONE_TIME`], [`CODE`]), is a one-time password,
/// whether or not the credential has a PIN as well. Of the other Passwords, one is the PIN (the
/// one named so, else the first), and the rest are codes too: a credential that asks for a PIN
/// and a second value would otherwise get the PIN twice, and never show a field for the code.
///
/// A Password OTP counts as sent when the service lists `credentials/sendOTP`, generator or
/// not: the spec's own example is a "totp" Password whose code "you received by SMS", and a code
/// that never comes can't be typed, while one from an app simply needs no sending.
fn auth_v2(auth: Option<&Value>, service: &Service) -> Result<(AuthNeeds, Rule), String> {
    if auth.and_then(|a| a.get("mode")).and_then(Value::as_str) == Some("oauth2code") {
        return Err(oauth());
    }
    let mut objects = Vec::new();
    for o in auth.and_then(|a| a.get("objects")).and_then(Value::as_array).into_iter().flatten().take(16) {
        let text = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(kind), Some(id)) = (text("type"), text("id")) else { continue };
        let label = text("label");
        let named = |words: &[&str]| words.iter().any(|w| names(&id, label.as_deref(), w));
        let role = match kind.as_str() {
            "Password" if text("generator").is_some() || named(ONE_TIME) => AuthRole::Otp,
            "Password" if named(CODE) && !named(&["pin"]) => AuthRole::Otp,
            // Which of these is the PIN is decided once all are read.
            "Password" => AuthRole::Pin,
            "ChallengeResponse" => AuthRole::Otp,
            "PasswordOOB" | "ChallengeResponseOOB" => AuthRole::Confirm,
            other => return Err(format!("it asks for an authentication method PdfCraft doesn't know ({})", quote(other, &[]))),
        };
        objects.push(AuthObject { id, kind, label, format: text("format"), role });
    }
    let pin_at = {
        let mut pins = objects.iter().enumerate().filter(|(_, o)| o.role == AuthRole::Pin);
        let first = pins.clone().next().map(|(i, _)| i);
        pins.find(|(_, o)| names(&o.id, o.label.as_deref(), "pin")).map(|(i, _)| i).or(first)
    };
    for (i, o) in objects.iter_mut().enumerate() {
        if o.role == AuthRole::Pin && Some(i) != pin_at {
            o.role = AuthRole::Otp;
        }
    }
    // A v2.0 service has no getChallenge: it sends a Password OTP with sendOTP.
    let password_otp = if service.lists("credentials/sendOTP") { OtpKind::Sent } else { OtpKind::Typed };
    let otp = objects.iter().fold(OtpKind::None, |k, o| {
        stronger(
            k,
            match (o.role, o.kind.as_str()) {
                (AuthRole::Otp, "ChallengeResponse") => OtpKind::Sent,
                (AuthRole::Otp, _) => password_otp,
                _ => OtpKind::None,
            },
        )
    });
    let expression = auth.and_then(|a| a.get("expression")).and_then(Value::as_str).unwrap_or_default();
    let ids: Vec<&str> = objects.iter().map(|o| o.id.as_str()).collect();
    let rule = Expr::read(expression, &ids)?;
    let pin = objects.iter().any(|o| o.role == AuthRole::Pin);
    // Whether a signature with the PIN, and the code or not, confirms on another device.
    let confirms = |code: bool| rule.choose(&objects, &|r| code || r != AuthRole::Otp, &|_| false).is_some_and(|(_, away)| away);
    let confirm = match (confirms(true), confirms(false)) {
        (true, _) => Confirmation::Always,
        (false, true) => Confirmation::WithoutCode,
        (false, false) => Confirmation::Never,
    };
    Ok((AuthNeeds { pin, otp, objects, confirm }, Rule::V2(rule)))
}

/// A v2 `auth/expression` (S22 §11.6–11.7): authentication object IDs joined by `AND`, `OR` and
/// `XOR`, grouped in parentheses. An empty one is an `AND` of every object.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Expr {
    Id(String),
    All(Vec<Expr>),
    /// `OR` or `XOR`: one operand is enough, and only one is ever sent, as `XOR` requires.
    One(Vec<Expr>),
}

/// The most tokens and the deepest nesting an expression is read with: far more than any
/// credential needs.
const MAX_TOKENS: usize = 64;
const MAX_DEPTH: usize = 8;

impl Expr {
    /// Read `text`, which may name only `ids`. The spec gives the operators no precedence, so
    /// `AND` binds tighter, as usual; `OR` and `XOR` are alike for what is sent.
    fn read(text: &str, ids: &[&str]) -> Result<Expr, String> {
        let spaced = text.replace('(', " ( ").replace(')', " ) ");
        let tokens: Vec<&str> = spaced.split_whitespace().collect();
        if tokens.is_empty() {
            return Ok(Expr::All(ids.iter().map(|id| Expr::Id(id.to_string())).collect()));
        }
        let unreadable = || format!("its authorization rule can't be read ({})", quote(text, &[]));
        if tokens.len() > MAX_TOKENS {
            return Err(unreadable());
        }
        let mut r = Reader { tokens, at: 0, ids };
        let e = r.either(0).map_err(|named| named.unwrap_or_else(unreadable))?;
        if r.at != r.tokens.len() {
            return Err(unreadable());
        }
        Ok(e)
    }

    /// What an authorization sends to satisfy this, given which secrets the user gave: for
    /// each alternative one operand whose values are all given, the first of the best. One that
    /// asks for no confirmation on another device is better than one that does, and then one
    /// with an object that `for_code` says the code is for (the one the service sent it for)
    /// than one without. `None` when nothing the user gave satisfies it. The `bool` says
    /// whether a confirmation on another device is in it, which `for_code` never changes.
    fn choose<'o>(
        &self,
        objects: &'o [AuthObject],
        given: &dyn Fn(AuthRole) -> bool,
        for_code: &dyn Fn(&AuthObject) -> bool,
    ) -> Option<(Vec<&'o AuthObject>, bool)> {
        match self {
            Expr::Id(id) => {
                let o = objects.iter().find(|o| &o.id == id)?;
                match o.role {
                    AuthRole::Confirm => Some((vec![o], true)),
                    role => given(role).then(|| (vec![o], false)),
                }
            }
            Expr::All(terms) => {
                let (mut all, mut elsewhere) = (Vec::<&AuthObject>::new(), false);
                for t in terms {
                    let (some, away) = t.choose(objects, given, for_code)?;
                    for o in some {
                        if !all.iter().any(|a| a.id == o.id) {
                            all.push(o);
                        }
                    }
                    elsewhere |= away;
                }
                Some((all, elsewhere))
            }
            Expr::One(terms) => {
                let rank = |(chosen, away): &(Vec<&AuthObject>, bool)| (*away, !chosen.iter().any(|o| for_code(o)));
                terms.iter().filter_map(|t| t.choose(objects, given, for_code)).min_by_key(rank)
            }
        }
    }
}

/// Reads an expression's tokens; an `Err` with a reason names an ID that isn't an object's, and
/// one without is a malformed expression.
struct Reader<'a> {
    tokens: Vec<&'a str>,
    at: usize,
    ids: &'a [&'a str],
}

/// The operators, read in any case.
const AND: &[&str] = &["AND"];
const OR: &[&str] = &["OR", "XOR"];

impl Reader<'_> {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.at).copied()
    }

    /// Whether the next token is one of `words`.
    fn next_is(&self, words: &[&str]) -> bool {
        self.peek().is_some_and(|t| words.iter().any(|w| t.eq_ignore_ascii_case(w)))
    }

    /// `either := all (("OR" | "XOR") all)*`
    fn either(&mut self, depth: usize) -> Result<Expr, Option<String>> {
        let mut terms = vec![self.all(depth)?];
        while self.next_is(OR) {
            self.at += 1;
            terms.push(self.all(depth)?);
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap_or(Expr::All(Vec::new())) } else { Expr::One(terms) })
    }

    /// `all := operand ("AND" operand)*`
    fn all(&mut self, depth: usize) -> Result<Expr, Option<String>> {
        let mut terms = vec![self.operand(depth)?];
        while self.next_is(AND) {
            self.at += 1;
            terms.push(self.operand(depth)?);
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap_or(Expr::All(Vec::new())) } else { Expr::All(terms) })
    }

    /// `operand := ID | "(" either ")"`
    fn operand(&mut self, depth: usize) -> Result<Expr, Option<String>> {
        if self.next_is(AND) || self.next_is(OR) {
            return Err(None);
        }
        let token = self.peek().ok_or(None)?.to_string();
        self.at += 1;
        match token.as_str() {
            "(" if depth < MAX_DEPTH => {
                let e = self.either(depth + 1)?;
                if self.peek() != Some(")") {
                    return Err(None);
                }
                self.at += 1;
                Ok(e)
            }
            "(" | ")" => Err(None),
            id if self.ids.contains(&id) => Ok(Expr::Id(token)),
            id => Err(Some(format!("its authorization rule names {}, which isn't one of its authentication objects", quote(id, &[])))),
        }
    }
}

/// The one of two OTP kinds that asks more of the user: one that's sent over one that's typed.
fn stronger(a: OtpKind, b: OtpKind) -> OtpKind {
    let rank = |k: OtpKind| match k {
        OtpKind::None => 0,
        OtpKind::Typed => 1,
        OtpKind::Sent => 2,
    };
    if rank(b) > rank(a) { b } else { a }
}

/// v1 `authMode`, `PIN` and `OTP` (S1 §11.5). Their `presence` is a string ("true", "false",
/// "optional"); a boolean is read too.
fn auth_v1(v: &Value) -> Result<(AuthNeeds, Rule), String> {
    match v.get("authMode").and_then(Value::as_str) {
        Some("oauth2code") => return Err(oauth()),
        Some("implicit") => {
            let needs = AuthNeeds { pin: false, otp: OtpKind::None, objects: Vec::new(), confirm: Confirmation::Never };
            return Ok((needs, Rule::V1 { pin: false, otp: false }));
        }
        _ => {}
    }
    let presence = |o: Option<&Value>| match o.and_then(|o| o.get("presence")) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => "false".into(),
    };
    let text = |o: Option<&Value>, k: &str| o.and_then(|o| o.get(k)).and_then(Value::as_str).map(str::to_string);
    let (pin, otp) = (v.get("PIN"), v.get("OTP"));
    let (pin_presence, otp_presence) = (presence(pin), presence(otp));
    let mut objects = Vec::new();
    if pin_presence != "false" {
        objects.push(AuthObject {
            id: "PIN".into(),
            kind: "PIN".into(),
            label: text(pin, "label"),
            format: text(pin, "format"),
            role: AuthRole::Pin,
        });
    }
    let mut otp_kind = OtpKind::None;
    if otp_presence != "false" {
        otp_kind = if text(otp, "type").as_deref() == Some("online") { OtpKind::Sent } else { OtpKind::Typed };
        objects.push(AuthObject {
            id: "OTP".into(),
            kind: "OTP".into(),
            label: text(otp, "label"),
            format: text(otp, "format"),
            role: AuthRole::Otp,
        });
    }
    let rule = Rule::V1 { pin: pin_presence == "true", otp: otp_presence == "true" };
    Ok((AuthNeeds { pin: pin_presence != "false", otp: otp_kind, objects, confirm: Confirmation::Never }, rule))
}

/// `credentials/info` for one credential: the transport's error, or the service's answer.
fn info(c: &mut Client<'_>, credential: &str) -> Result<Result<Value, Refused>, SignError> {
    let r = c.call("credentials/info", &json!({ "credentialID": credential, "certificates": "chain", "certInfo": false, "authInfo": true }), &[])?;
    Ok(answer(&r))
}

/// `credentials/info` for one credential, which must be able to sign.
fn read_credential(c: &mut Client<'_>, credential: &str) -> Result<Credential, SignError> {
    let v = info(c, credential)?.map_err(|e| {
        // The request carries nothing of the caller's but the credential's name, so one the
        // service calls invalid names a credential the account doesn't have.
        if e.garbled || e.error != "invalid_request" {
            return c.refused(e, &[]);
        }
        described(format!("{REFUSED_TO_DESCRIBE} {} (invalid_request)", quote(credential, &[])), e.description, &c.hidden(&[]))
    })?;
    parse_credential(&v, c.service).map_err(|why| SignError::Crypto(format!("{CANT_SIGN} {} can't sign: {why}", quote(credential, &[]))))
}

/// The IDs `credentials/list` gives (v1 pages through `nextPageToken`), and the v2
/// `credentialInfos` when the service sends them.
fn credential_ids(c: &mut Client<'_>) -> Result<(Vec<String>, Vec<Value>), SignError> {
    let ids_of = |v: &Value| -> Vec<String> {
        v.get("credentialIDs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
    };
    if c.service.version == Version::V2 {
        let body = json!({ "credentialInfo": true, "certificates": "chain", "certInfo": false, "authInfo": true });
        let v = c.json("credentials/list", &body, &[])?;
        let infos = v.get("credentialInfos").and_then(Value::as_array).cloned().unwrap_or_default();
        return Ok((ids_of(&v), infos));
    }
    let mut ids = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..8 {
        let mut body = Map::new();
        body.insert("maxResults".into(), MAX_CREDENTIALS.into());
        if let Some(p) = page {
            body.insert("pageToken".into(), p.into());
        }
        let v = c.json("credentials/list", &Value::Object(body), &[])?;
        ids.extend(ids_of(&v));
        page = v.get("nextPageToken").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_string);
        if page.is_none() || ids.len() >= MAX_CREDENTIALS {
            break;
        }
    }
    Ok((ids, Vec::new()))
}

pub(super) fn list(t: &dyn HttpTransport, account: &Account, password: &str) -> Result<Vec<RemoteIdentity>, SignError> {
    let service = service(t, account)?;
    let mut c = Client::login(t, &service, &account.user, password)?;
    let listed = list_signed_in(&mut c, account);
    c.revoke();
    listed
}

fn list_signed_in(c: &mut Client<'_>, account: &Account) -> Result<Vec<RemoteIdentity>, SignError> {
    let (ids, infos) = credential_ids(c)?;
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    // An ID too long to be one is a broken answer, not a credential to send back.
    for id in ids.into_iter().filter(|id| !id.is_empty() && id.len() <= 1024).take(MAX_CREDENTIALS) {
        let given = infos.iter().find(|i| i.get("credentialID").and_then(Value::as_str) == Some(id.as_str())).cloned();
        // One credential the service won't describe (locked, say) doesn't hide the others; a
        // service that stops answering ends the list, rather than costing a timeout for each.
        let v = match given.map_or_else(|| info(c, &id), |v| Ok(Ok(v)))? {
            Ok(v) => v,
            Err(e) => {
                skipped.push((id, c.refused(e, &[]).to_string()));
                continue;
            }
        };
        match parse_credential(&v, c.service) {
            Ok(cred) => out.push(RemoteIdentity {
                account: account.clone(),
                credential: id,
                certificate: cred.certificate,
                chain: cred.chain,
                auth: cred.auth,
            }),
            Err(why) => skipped.push((id, why)),
        }
    }
    if out.is_empty() && !skipped.is_empty() {
        return Err(none_usable(&skipped));
    }
    Ok(out)
}

pub(super) fn send_otp(t: &dyn HttpTransport, account: &Account, credential: &str, password: &str) -> Result<OtpSent, SignError> {
    let service = service(t, account)?;
    let mut c = Client::login(t, &service, &account.user, password)?;
    let sent = send_signed_in(&mut c, credential);
    c.revoke();
    sent
}

fn send_signed_in(c: &mut Client<'_>, credential: &str) -> Result<OtpSent, SignError> {
    let cred = read_credential(c, credential)?;
    if let Some(o) = cred.to_send() {
        return challenge(c, credential, &o.id);
    }
    if cred.auth.otp == OtpKind::Sent && c.service.offers("credentials/sendOTP") {
        c.json("credentials/sendOTP", &json!({ "credentialID": credential }), &[])?;
        return Ok(OtpSent::Sent);
    }
    Err(SignError::Crypto(
        if cred.auth.otp != OtpKind::None {
            NOT_SENT
        } else if cred.auth.objects.iter().any(|o| o.role == AuthRole::Confirm) {
            // Signing asks for that confirmation (getChallenge, then authorize); one asked for now
            // would only be replaced by it.
            CONFIRMED_ELSEWHERE
        } else {
            NO_OTP
        }
        .into(),
    ))
}

/// `credentials/getChallenge`: a challenge to show (200), or the code sent out of band (201,
/// 204; the spec says both).
fn challenge(c: &mut Client<'_>, credential: &str, object: &str) -> Result<OtpSent, SignError> {
    let v = c.json("credentials/getChallenge", &json!({ "credentialID": credential, "authObjectID": object }), &[])?;
    Ok(match v.get("challenge").and_then(Value::as_str) {
        Some(ch) if !ch.is_empty() => OtpSent::Challenge(quote(ch, &[])),
        _ => OtpSent::Sent,
    })
}

/// A signature in progress: signed in, with the credential read and the secrets to spend.
pub(super) struct Session {
    service: Service,
    user: String,
    credential: String,
    cred: Credential,
    secrets: Secrets,
    token: String,
}

/// Sign in and read `credential`, ready to sign with `secrets`, which must hold what it needs.
pub(super) fn open(t: &dyn HttpTransport, account: &Account, credential: &str, secrets: Secrets) -> Result<(RemoteIdentity, Session), SignError> {
    let service = service(t, account)?;
    let mut c = Client::login(t, &service, &account.user, &secrets.password)?;
    let cred = match read_credential(&mut c, credential).and_then(|cred| {
        // Refuse a signature the service would refuse for a missing PIN or OTP.
        cred.chosen(&secrets)?;
        Ok(cred)
    }) {
        Ok(cred) => cred,
        Err(e) => {
            c.revoke();
            return Err(e);
        }
    };
    let token = std::mem::take(&mut c.token);
    let id = RemoteIdentity {
        account: account.clone(),
        credential: credential.to_string(),
        certificate: cred.certificate.clone(),
        chain: cred.chain.clone(),
        auth: cred.auth.clone(),
    };
    Ok((id, Session { service, user: account.user.clone(), credential: credential.to_string(), cred, secrets, token }))
}

/// Authorize the one signature over `digest` and sign it: the SAD is bound to these exact
/// digest bytes (SCAL 2), so `credentials/authorize` and `signatures/signHash` get the same.
pub(super) fn sign(t: &dyn HttpTransport, s: Session, public: &PublicKey, alg: DigestAlg, digest: &[u8]) -> Result<Vec<u8>, SignError> {
    let Session { service, user, credential, cred, secrets, token } = s;
    let mut c = Client { t, service: &service, user: &user, password: &secrets.password, token, renewed: false };
    let signed = sign_signed_in(&mut c, &credential, &cred, &secrets, public, alg, digest);
    c.revoke();
    signed
}

fn sign_signed_in(
    c: &mut Client<'_>,
    credential: &str,
    cred: &Credential,
    secrets: &Secrets,
    public: &PublicKey,
    alg: DigestAlg,
    digest: &[u8],
) -> Result<Vec<u8>, SignError> {
    let hash = base64(digest);
    // Before anything is authorized: a signHash the service would refuse spends the OTP for nothing.
    let algo = sign_algo(public, alg, &cred.algos).map_err(|what| SignError::Crypto(format!("{KEY_CANT_SIGN} {what}")))?;
    let chosen = cred.chosen(secrets)?;
    // A confirmation on another device that this signature uses (none when the user gave a code
    // that does instead): the service sends a challenge there. One that answers with the
    // challenge (200), for PdfCraft to show, can't be served: signing has nowhere to show it,
    // and the confirmation would be waited for in vain.
    for o in chosen.iter().filter(|o| o.kind == "ChallengeResponseOOB") {
        if let OtpSent::Challenge(_) = challenge(c, credential, &o.id)? {
            return Err(SignError::Crypto(SHOWS_CHALLENGE.into()));
        }
    }
    let sad = authorize(c, credential, &chosen, secrets, alg, &hash)?;
    let hidden: Vec<&str> = secrets.all().into_iter().chain([sad.as_str()]).collect();
    let body = match c.service.version {
        Version::V2 => json!({ "credentialID": credential, "SAD": sad, "hashes": [hash], "hashAlgorithmOID": alg.oid(), "signAlgo": algo }),
        Version::V1 => json!({ "credentialID": credential, "SAD": sad, "hash": [hash], "hashAlgo": alg.oid(), "signAlgo": algo }),
    };
    let v = c.json("signatures/signHash", &body, &hidden)?;
    let sig = v.get("signatures").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str);
    sig.and_then(base64_lenient).ok_or_else(|| SignError::Crypto(NO_SIGNATURE.into()))
}

/// `credentials/authorize` for one signature over `hash` (base64) with the `chosen` objects:
/// the SAD, waiting while the user confirms on another device (202, then `authorizeCheck`).
fn authorize(
    c: &mut Client<'_>,
    credential: &str,
    chosen: &[&AuthObject],
    secrets: &Secrets,
    alg: DigestAlg,
    hash: &str,
) -> Result<String, SignError> {
    let hidden = secrets.all();
    let mut body = Map::new();
    body.insert("credentialID".into(), credential.into());
    body.insert("numSignatures".into(), 1.into());
    match c.service.version {
        Version::V2 => {
            body.insert("hashes".into(), json!([hash]));
            body.insert("hashAlgorithmOID".into(), alg.oid().into());
            let data: Vec<Value> = chosen
                .iter()
                .filter_map(|o| match o.role {
                    AuthRole::Pin => secrets.pin().map(|v| json!({ "id": o.id, "value": v })),
                    AuthRole::Otp => secrets.otp().map(|v| json!({ "id": o.id, "value": v })),
                    AuthRole::Confirm => Some(json!({ "id": o.id })),
                })
                .collect();
            if !data.is_empty() {
                body.insert("authData".into(), Value::Array(data));
            }
        }
        Version::V1 => {
            body.insert("hash".into(), json!([hash]));
            // S1 §11.6: each only when the credential asks for it (`presence` not "false").
            for o in chosen {
                let (field, value) = match o.role {
                    AuthRole::Pin => ("PIN", secrets.pin()),
                    AuthRole::Otp => ("OTP", secrets.otp()),
                    AuthRole::Confirm => continue,
                };
                if let Some(v) = value {
                    body.insert(field.into(), v.into());
                }
            }
        }
    }
    let r = c.call("credentials/authorize", &Value::Object(body), &hidden)?;
    let v = answer(&r).map_err(|e| auth_refused(c, e, chosen, &hidden))?;
    if r.status != 202 {
        return sad_of(&v);
    }
    let mut handle = v.get("handle").and_then(Value::as_str).unwrap_or_default().to_string();
    let late = deadline(c.t, POLL_FOR);
    for _ in 0..POLLS {
        if late() {
            break;
        }
        c.t.wait(POLL_INTERVAL);
        let r = c.call("credentials/authorizeCheck", &json!({ "handle": handle }), &hidden)?;
        let v = answer(&r).map_err(|e| auth_refused(c, e, chosen, &hidden))?;
        if r.status != 202 {
            return sad_of(&v);
        }
        if let Some(h) = v.get("handle").and_then(Value::as_str) {
            handle = h.to_string();
        }
    }
    Err(SignError::Crypto(NOT_CONFIRMED.into()))
}

/// A test of whether `d` has gone by since it was made, on `t`'s clock. Without one (the
/// browser) it never has, and the count of polls alone bounds them.
fn deadline(t: &dyn HttpTransport, d: Duration) -> impl Fn() -> bool + '_ {
    let end = t.clock().and_then(|now| now.checked_add(d));
    move || end.is_some_and(|end| t.clock().is_some_and(|now| now >= end))
}

/// An authorization the service refused. v2 can't tell which value was wrong
/// (`invalid_authentication_data`), so the message names what was sent.
fn auth_refused(c: &Client<'_>, e: Refused, chosen: &[&AuthObject], hidden: &[&str]) -> SignError {
    if e.error != "invalid_authentication_data" {
        return c.refused(e, hidden);
    }
    let sent = |role: AuthRole| chosen.iter().any(|o| o.role == role);
    SignError::Crypto(
        match (sent(AuthRole::Pin), sent(AuthRole::Otp)) {
            (true, true) => REJECTED_PIN_OR_OTP,
            (true, false) => REJECTED_PIN,
            (false, true) => REJECTED_OTP,
            // Only a confirmation went: there is nothing to name.
            (false, false) => return c.refused(e, hidden),
        }
        .into(),
    )
}

fn sad_of(v: &Value) -> Result<String, SignError> {
    match v.get("SAD").and_then(Value::as_str) {
        Some(sad) if !sad.is_empty() => Ok(sad.to_string()),
        _ => Err(SignError::Crypto(NO_SAD.into())),
    }
}

/// The `signAlgo` for a signature with `alg`, which must be one the key lists in `key/algo`
/// (S1 §11.9, S20 §11.10) when it lists any. RSA PKCS #1 v1.5 is `rsaEncryption` with the hash
/// named apart, as every example in the spec has it, or the combined `sha…WithRSAEncryption`;
/// ECDSA is the matching `ecdsa-with-SHA…`, or `ecPublicKey` with the hash named apart. Neither
/// listed (only RSA-PSS, say, or another hash) makes the credential unusable when it is listed,
/// and fails before the OTP is spent on an authorization. The `Err` is what the key can't sign
/// with, as it follows "can't sign with".
fn sign_algo(public: &PublicKey, alg: DigestAlg, algos: &[String]) -> Result<&'static str, String> {
    let rsa = matches!(public, PublicKey::Rsa { .. });
    let (apart, combined) = match (rsa, alg) {
        (true, DigestAlg::Sha256) => (oid::RSA, "1.2.840.113549.1.1.11"),
        (true, DigestAlg::Sha384) => (oid::RSA, "1.2.840.113549.1.1.12"),
        (true, DigestAlg::Sha512) => (oid::RSA, "1.2.840.113549.1.1.13"),
        (false, DigestAlg::Sha256) => (oid::EC, "1.2.840.10045.4.3.2"),
        (false, DigestAlg::Sha384) => (oid::EC, "1.2.840.10045.4.3.3"),
        (false, DigestAlg::Sha512) => (oid::EC, "1.2.840.10045.4.3.4"),
        (_, other) => return Err(format!("{} (PdfCraft signs with SHA-256, SHA-384 or SHA-512)", other.name())),
    };
    // RSA names the hash apart where it can; ECDSA's own OID says it.
    let (first, second) = if rsa { (apart, combined) } else { (combined, apart) };
    let listed = |o: &str| algos.iter().any(|a| a == o);
    if algos.is_empty() || listed(first) {
        return Ok(first);
    }
    if listed(second) {
        return Ok(second);
    }
    Err(format!("{} and {} (it lists {})", if rsa { "RSA PKCS #1 v1.5" } else { "ECDSA" }, alg.name(), quote(&algos.join(", "), &[])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2(methods: &[&str]) -> Service {
        Service { base: String::new(), version: Version::V2, methods: methods.iter().map(|m| m.to_string()).collect() }
    }

    /// The roles of a v2 credential's objects, in order.
    fn roles(needs: &AuthNeeds) -> Vec<(&str, AuthRole)> {
        needs.objects.iter().map(|o| (o.id.as_str(), o.role)).collect()
    }

    /// The IDs an authorization of the credential `auth` describes sends with a PIN and an OTP
    /// given or not; `None` when it can't be authorized with them.
    fn sent(auth: &Value, pin: bool, otp: bool) -> Option<Vec<String>> {
        let (needs, rule) = auth_v2(Some(auth), &v2(&[])).unwrap();
        let Rule::V2(rule) = rule else { panic!("a v2 rule") };
        let given = |r: AuthRole| match r {
            AuthRole::Pin => pin,
            AuthRole::Otp => otp,
            AuthRole::Confirm => true,
        };
        rule.choose(&needs.objects, &given, &|_| false).map(|(o, _)| o.iter().map(|o| o.id.clone()).collect())
    }

    #[test]
    fn v2_objects_are_told_apart() {
        let auth = json!({ "mode": "explicit", "expression": "PIN AND OTP", "objects": [
            { "type": "Password", "id": "PIN", "format": "N", "label": "PIN" },
            { "type": "ChallengeResponse", "id": "OTP", "label": "SMS code" },
        ]});
        let (needs, _) = auth_v2(Some(&auth), &v2(&[])).unwrap();
        assert!(needs.pin);
        assert_eq!(needs.otp, OtpKind::Sent);
        assert_eq!(roles(&needs), [("PIN", AuthRole::Pin), ("OTP", AuthRole::Otp)]);
        assert_eq!(sent(&auth, true, true), Some(vec!["PIN".into(), "OTP".into()]));
        assert_eq!(sent(&auth, true, false), None);
        // A generator makes a Password an OTP read from a device; an alternative sends one operand.
        let auth = json!({ "expression": "PIN OR code", "objects": [
            { "type": "Password", "id": "PIN" }, { "type": "Password", "id": "code", "generator": "totp" },
        ]});
        let (needs, _) = auth_v2(Some(&auth), &v2(&[])).unwrap();
        assert_eq!((needs.pin, needs.otp), (true, OtpKind::Typed));
        assert_eq!(sent(&auth, true, false), Some(vec!["PIN".into()]));
        assert_eq!(sent(&auth, false, true), Some(vec!["code".into()]));
        assert_eq!(sent(&auth, true, true), Some(vec!["PIN".into()]), "one of the two, not both");
        assert_eq!(sent(&auth, false, false), None);
        // An OTP-named Password on a v2.0 service that lists sendOTP is sent.
        let auth = json!({ "objects": [{ "type": "Password", "id": "OTP" }] });
        assert_eq!(auth_v2(Some(&auth), &v2(&["credentials/sendOTP"])).unwrap().0.otp, OtpKind::Sent);
        assert_eq!(auth_v2(Some(&auth), &v2(&[])).unwrap().0.otp, OtpKind::Typed);
        assert!(auth_v2(Some(&json!({ "mode": "oauth2code" })), &v2(&[])).unwrap_err().contains("OAuth"));
        assert!(auth_v2(Some(&json!({ "objects": [{ "type": "Biometric", "id": "x" }] })), &v2(&[])).is_err());
    }

    /// The credential of the spec's own `credentials/list` example (S22 p.61): a "totp" Password
    /// whose code arrives by SMS, from a service whose `info` example lists sendOTP.
    #[test]
    fn the_specs_sms_totp_example_is_sent() {
        let auth = json!({ "mode": "explicit", "expression": "PIN AND OTP", "objects": [
            { "type": "Password", "id": "PIN", "format": "N", "label": "PIN", "description": "Please enter the signature PIN" },
            { "type": "Password", "id": "OTP", "format": "N", "generator": "totp", "label": "Mobile OTP",
              "description": "Please enter the 6 digit code you received by SMS" },
        ]});
        let (needs, _) = auth_v2(Some(&auth), &v2(&["credentials/list", "credentials/sendOTP", "signatures/signHash"])).unwrap();
        assert_eq!((needs.pin, needs.otp), (true, OtpKind::Sent));
        assert_eq!(roles(&needs), [("PIN", AuthRole::Pin), ("OTP", AuthRole::Otp)]);
        assert_eq!(sent(&auth, true, true), Some(vec!["PIN".into(), "OTP".into()]));
        // Without sendOTP there is no way to ask for it: the code comes from the generator.
        assert_eq!(auth_v2(Some(&auth), &v2(&["credentials/getChallenge"])).unwrap().0.otp, OtpKind::Typed);
    }

    #[test]
    fn a_second_password_is_a_code_not_a_second_pin() {
        let auth = json!({ "expression": "PIN AND CODICE", "objects": [
            { "type": "Password", "id": "PIN", "label": "PIN" }, { "type": "Password", "id": "CODICE", "label": "Codice SMS" },
        ]});
        let (needs, _) = auth_v2(Some(&auth), &v2(&["credentials/sendOTP"])).unwrap();
        assert_eq!(roles(&needs), [("PIN", AuthRole::Pin), ("CODICE", AuthRole::Otp)]);
        assert_eq!((needs.pin, needs.otp), (true, OtpKind::Sent));
        // The one named PIN is the PIN, wherever it is; with none named, the first.
        let auth =
            json!({ "objects": [{ "type": "Password", "id": "C1", "label": "Codice" }, { "type": "Password", "id": "P", "label": "Il tuo PIN" }] });
        assert_eq!(roles(&auth_v2(Some(&auth), &v2(&[])).unwrap().0), [("C1", AuthRole::Otp), ("P", AuthRole::Pin)]);
        let auth = json!({ "objects": [{ "type": "Password", "id": "A" }, { "type": "Password", "id": "B" }] });
        assert_eq!(roles(&auth_v2(Some(&auth), &v2(&[])).unwrap().0), [("A", AuthRole::Pin), ("B", AuthRole::Otp)]);
    }

    /// A lone Password the service names as a one-time code is the OTP, not the PIN, in any
    /// case; one named the PIN, or named nothing, is the PIN.
    #[test]
    fn a_password_named_as_a_code_is_the_otp_even_alone() {
        let lone = |id: &str, label: &str| {
            let auth = json!({ "objects": [{ "type": "Password", "id": id, "label": label }] });
            let (needs, _) = auth_v2(Some(&auth), &v2(&[])).unwrap();
            (needs.pin, needs.otp, roles(&needs)[0].1)
        };
        for (id, label) in [
            ("CODICE", "Codice SMS"),
            ("c1", "Codice"),
            ("Otp", ""),
            ("x", "TOTP"),
            ("x", "One-Time Password"),
            ("x", "one time code"),
            ("x", "Your code"),
            ("TOKEN", "Token"),
            ("x", "SMS"),
        ] {
            assert_eq!(lone(id, label), (false, OtpKind::Typed, AuthRole::Otp), "{id} {label}");
        }
        for (id, label) in [("PIN", "PIN"), ("pwd", "Codice PIN"), ("pwd", "PIN code"), ("x", "Token PIN"), ("password", "Password"), ("P", "")] {
            assert_eq!(lone(id, label), (true, OtpKind::None, AuthRole::Pin), "{id} {label}");
        }
        // Beside one that isn't, and two that are with no PIN at all.
        let auth = json!({ "objects": [{ "type": "Password", "id": "pw", "label": "Password" }, { "type": "Password", "id": "sms" }] });
        assert_eq!(roles(&auth_v2(Some(&auth), &v2(&[])).unwrap().0), [("pw", AuthRole::Pin), ("sms", AuthRole::Otp)]);
        let auth = json!({ "objects": [{ "type": "Password", "id": "sms" }, { "type": "Password", "id": "pw", "label": "Password" }] });
        assert_eq!(roles(&auth_v2(Some(&auth), &v2(&[])).unwrap().0), [("sms", AuthRole::Otp), ("pw", AuthRole::Pin)]);
        let auth = json!({ "objects": [{ "type": "Password", "id": "code" }, { "type": "Password", "id": "x", "label": "Codice SMS" }] });
        let (needs, _) = auth_v2(Some(&auth), &v2(&[])).unwrap();
        assert!(!needs.pin);
        assert_eq!(roles(&needs), [("code", AuthRole::Otp), ("x", AuthRole::Otp)]);
    }

    /// Of codes offered as alternatives, one the service sent goes under the object it was
    /// sent for, wherever that comes; one that wasn't, under the first; and the code never
    /// decides whether a confirmation on another device is asked.
    #[test]
    fn a_sent_code_prefers_the_object_it_was_sent_for() {
        for (expression, first) in [("PIN AND (APP XOR SMS)", "APP"), ("PIN AND (SMS XOR APP)", "SMS"), ("(PIN AND APP) OR (PIN AND SMS)", "APP")] {
            let auth = json!({ "expression": expression, "objects": [
                { "type": "Password", "id": "PIN" }, { "type": "ChallengeResponse", "id": "SMS" },
                { "type": "Password", "id": "APP", "generator": "totp" },
            ]});
            let (needs, rule) = auth_v2(Some(&auth), &v2(&[])).unwrap();
            let Rule::V2(rule) = rule else { panic!("a v2 rule") };
            let ids = |sent: bool| {
                let chosen = rule.choose(&needs.objects, &|_| true, &|o| sent && sent_when_asked(o)).unwrap().0;
                chosen.iter().map(|o| o.id.as_str()).filter(|id| *id != "PIN").collect::<Vec<_>>()
            };
            assert_eq!((ids(true), ids(false)), (vec!["SMS"], vec![first]), "{expression}");
        }
        // A confirmation stays the alternative used without a code, the code sent or not.
        let auth = json!({ "expression": "(SMS AND PUSH) XOR APP", "objects": [
            { "type": "ChallengeResponse", "id": "SMS" }, { "type": "ChallengeResponseOOB", "id": "PUSH" },
            { "type": "Password", "id": "APP", "generator": "totp" },
        ]});
        let (needs, rule) = auth_v2(Some(&auth), &v2(&[])).unwrap();
        let Rule::V2(rule) = rule else { panic!("a v2 rule") };
        let chosen = rule.choose(&needs.objects, &|_| true, &sent_when_asked).unwrap();
        assert_eq!((chosen.0.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), chosen.1), (vec!["APP"], false));
    }

    /// When a signature is confirmed on another device, as an interface is told it: always, only
    /// without the code, or never.
    #[test]
    fn whether_a_signature_is_confirmed_elsewhere_follows_the_rule() {
        let confirm = |expression: &str| {
            let auth = json!({ "expression": expression, "objects": [
                { "type": "Password", "id": "PIN" }, { "type": "ChallengeResponseOOB", "id": "PUSH" },
                { "type": "Password", "id": "APP", "generator": "totp" }, { "type": "ChallengeResponse", "id": "SMS" },
            ]});
            auth_v2(Some(&auth), &v2(&[])).unwrap().0.confirm
        };
        assert_eq!(confirm("PIN AND SMS"), Confirmation::Never);
        assert_eq!(confirm("PIN AND PUSH"), Confirmation::Always);
        assert_eq!(confirm("PIN AND APP AND PUSH"), Confirmation::Always);
        assert_eq!(confirm("PIN AND (PUSH XOR APP)"), Confirmation::WithoutCode);
        assert_eq!(confirm("(APP OR PUSH) AND PIN"), Confirmation::WithoutCode);
        assert_eq!(confirm("PIN OR PUSH"), Confirmation::Never, "the PIN is given");
        assert_eq!(confirm("PUSH OR (SMS AND PUSH)"), Confirmation::Always);
        // v1 has no such object.
        assert_eq!(auth_v1(&json!({ "PIN": { "presence": "true" } })).unwrap().0.confirm, Confirmation::Never);
    }

    #[test]
    fn an_exclusive_choice_sends_one_operand_and_a_phone_only_without_a_code() {
        let auth = json!({ "expression": "PIN AND (SMS XOR APP)", "objects": [
            { "type": "Password", "id": "PIN" }, { "type": "ChallengeResponse", "id": "SMS" },
            { "type": "Password", "id": "APP", "generator": "totp" },
        ]});
        assert_eq!(auth_v2(Some(&auth), &v2(&[])).unwrap().0.otp, OtpKind::Sent);
        assert_eq!(sent(&auth, true, true), Some(vec!["PIN".into(), "SMS".into()]));
        assert_eq!(sent(&auth, true, false), None);
        // A confirmation on the phone is the alternative used when no code was given, wherever it is.
        for expression in ["PIN AND (PUSH XOR APP)", "(PUSH OR APP) AND PIN"] {
            let auth = json!({ "expression": expression, "objects": [
                { "type": "Password", "id": "PIN" }, { "type": "ChallengeResponseOOB", "id": "PUSH" },
                { "type": "Password", "id": "APP", "generator": "totp" },
            ]});
            let mut with_code = sent(&auth, true, true).unwrap();
            let mut without = sent(&auth, true, false).unwrap();
            with_code.sort();
            without.sort();
            assert_eq!((with_code, without), (vec!["APP".into(), "PIN".into()], vec!["PIN".into(), "PUSH".into()]), "{expression}");
        }
    }

    #[test]
    fn expressions_are_read_strictly_but_in_any_case() {
        let ids = ["PIN", "OTP"];
        let read = |e: &str| Expr::read(e, &ids);
        let both = Expr::All(vec![Expr::Id("PIN".into()), Expr::Id("OTP".into())]);
        assert_eq!(read(""), Ok(both.clone()));
        assert_eq!(read("PIN and (OTP)"), Ok(both.clone()));
        assert_eq!(read("PIN AND (otp)"), Err("its authorization rule names otp, which isn't one of its authentication objects".into()));
        // AND binds tighter.
        assert_eq!(read("PIN AND OTP OR OTP"), Ok(Expr::One(vec![both, Expr::Id("OTP".into())])));
        for bad in ["PIN AND", "AND PIN", "(PIN", "PIN)", "PIN OTP", "()", "PIN XOR XOR OTP"] {
            assert!(read(bad).unwrap_err().starts_with("its authorization rule can't be read"), "{bad}");
        }
        assert!(read("PIN AND NFC").unwrap_err().contains("names NFC"));
        // Bounded: no deeper than eight, no longer than 64 tokens.
        assert!(read(&format!("{}PIN{}", "(".repeat(8), ")".repeat(8))).is_ok());
        assert!(read(&format!("{}PIN{}", "(".repeat(9), ")".repeat(9))).is_err());
        assert!(read(&vec!["PIN"; 33].join(" OR ")).is_err());
        assert!(read(&"(".repeat(100_000)).is_err());
    }

    #[test]
    fn only_a_token_error_renews_the_sign_in() {
        let r = |status: u16, body: &str| HttpResponse { status, body: body.as_bytes().to_vec() };
        assert!(token_refused(&r(401, r#"{"error":"invalid_token"}"#)));
        assert!(token_refused(&r(400, r#"{"error":"expired_token","error_description":"x"}"#)));
        assert!(token_refused(&r(401, "")));
        assert!(token_refused(&r(401, "<html>Unauthorized</html>")));
        // A wrong PIN some services send as 401 is a wrong PIN, not a sign-in to renew.
        assert!(!token_refused(&r(401, r#"{"error":"invalid_pin"}"#)));
        assert!(!token_refused(&r(400, r#"{"error":"invalid_request"}"#)));
        assert!(!token_refused(&r(200, r#"{"SAD":"x"}"#)));
    }

    #[test]
    fn v1_presence_strings_decide_what_is_asked() {
        let v = json!({ "authMode": "explicit", "PIN": { "presence": "true" }, "OTP": { "presence": "optional", "type": "online" } });
        let (needs, rule) = auth_v1(&v).unwrap();
        assert_eq!((needs.pin, needs.otp), (true, OtpKind::Sent));
        assert!(matches!(rule, Rule::V1 { pin: true, otp: false }));
        let (needs, _) = auth_v1(&json!({ "authMode": "implicit", "PIN": { "presence": "true" } })).unwrap();
        assert!(!needs.pin && needs.objects.is_empty());
        let (needs, rule) = auth_v1(&json!({ "OTP": { "presence": true, "type": "offline" } })).unwrap();
        assert_eq!(needs.otp, OtpKind::Typed);
        assert!(matches!(rule, Rule::V1 { pin: false, otp: true }));
        assert!(auth_v1(&json!({ "authMode": "oauth2code" })).is_err());
    }

    #[test]
    fn sign_algo_is_one_the_key_lists() {
        let algo = |public: &PublicKey, alg: DigestAlg, algos: &[&str]| {
            sign_algo(public, alg, &algos.iter().map(|a| a.to_string()).collect::<Vec<_>>()).map_err(|what| format!("{KEY_CANT_SIGN} {what}"))
        };
        let rsa = PublicKey::Rsa { n: vec![0xC1; 256], e: vec![1, 0, 1] };
        assert_eq!(algo(&rsa, DigestAlg::Sha256, &[]), Ok(oid::RSA));
        assert_eq!(algo(&rsa, DigestAlg::Sha256, &["1.2.840.113549.1.1.11"]), Ok("1.2.840.113549.1.1.11"));
        assert_eq!(algo(&rsa, DigestAlg::Sha384, &[oid::RSA, "1.2.840.113549.1.1.12"]), Ok(oid::RSA));
        assert_eq!(
            algo(&rsa, DigestAlg::Sha256, &[oid::RSA_PSS]),
            Err("the signing service's key can't sign with RSA PKCS #1 v1.5 and SHA-256 (it lists 1.2.840.113549.1.1.10)".into())
        );
        let p384 = PublicKey::P384(Vec::new());
        assert_eq!(algo(&p384, DigestAlg::Sha384, &[]), Ok("1.2.840.10045.4.3.3"));
        assert_eq!(algo(&p384, DigestAlg::Sha384, &["1.2.840.10045.4.3.2", "1.2.840.10045.4.3.3"]), Ok("1.2.840.10045.4.3.3"));
        // Only the key type listed: the hash goes apart, in hashAlgorithmOID.
        assert_eq!(algo(&p384, DigestAlg::Sha384, &[oid::EC]), Ok(oid::EC));
        assert!(algo(&p384, DigestAlg::Sha384, &["1.2.840.10045.4.3.2"]).unwrap_err().contains("ECDSA and SHA-384"));
        assert!(algo(&rsa, DigestAlg::Sha1, &[]).is_err());
    }

    #[test]
    fn refusals_quote_the_service_without_secrets() {
        let r = HttpResponse { status: 400, body: br#"{"error":"invalid_request","error_description":"no hunter2\nhere"}"#.to_vec() };
        let e = refusal(answer(&r).err().unwrap(), &["hunter2"]).to_string();
        assert_eq!(e, "the signing service refused the request (invalid_request): no <redacted> here");
        let r = HttpResponse { status: 503, body: b"<html>".to_vec() };
        assert_eq!(refusal(answer(&r).err().unwrap(), &[]).to_string(), "the signing service refused the request (HTTP 503)");
        let r = HttpResponse { status: 200, body: b"<html>".to_vec() };
        assert_eq!(refusal(answer(&r).err().unwrap(), &[]).to_string(), "the signing service's answer isn't JSON");
        assert!(answer(&HttpResponse { status: 204, body: Vec::new() }).is_ok());
    }
}
