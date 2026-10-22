//! Remote signing against the in-process signing services of `remote::mock`: listing, sending
//! the OTP and signing a PDF that then validates, for CSC v2 and v1 and ARSS; every way a service
//! can refuse or misbehave; references; and what goes over the wire.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use pdfcraft_cos::Document;
use pdfcraft_sign::remote::mock::{self, Fault, MockService};
use pdfcraft_sign::remote::{
    self, ARSS_ENDPOINT, Account, AuthRole, HttpResponse, HttpTransport, OtpKind, OtpSent, Protocol, Secrets, check_url, parse_reference,
};
use pdfcraft_sign::{Certificate, SignError, SignOptions, Status, Time, TrustStore, signatures};
use serde_json::{Value, json};

/// The fixture's image: bytes no compression shrinks (xorshift), over 32 KiB of them, so a
/// request carrying the document in any encoding would be far larger than one carrying a digest.
fn noise() -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..128 * 86 * 3)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// One page with text and an image: what is signed, and what must never reach the service.
fn fixture() -> Vec<u8> {
    let content = b"q 128 0 0 86 150 20 cm /Im1 Do Q BT /F1 14 Tf 20 250 Td (Confidential contract) Tj ET";
    let stream = |dict: &str, data: &[u8]| [format!("<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes(), data, b"\nendstream"].concat();
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> >> >>"
            .to_vec(),
        stream("", content),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 128 /Height 86 /ColorSpace /DeviceRGB /BitsPerComponent 8", &noise()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

fn opts() -> SignOptions {
    SignOptions {
        page: 0,
        rect: Some([20.0, 100.0, 220.0, 150.0]),
        reason: Some("I approve this contract".into()),
        date: "D:20261021120000+02'00'".into(),
        ..SignOptions::default()
    }
}

fn secrets(pin: Option<&str>, otp: Option<String>) -> Secrets {
    Secrets { password: mock::PASSWORD.into(), pin: pin.map(str::to_string), otp, otp_sent: false }
}

/// [`secrets`] with an OTP that `remote::send_otp` had the service send (Send code).
fn sent(pin: Option<&str>, otp: String) -> Secrets {
    Secrets { otp_sent: true, ..secrets(pin, Some(otp)) }
}

/// Sign the fixture with `credential` of `mock`.
fn sign_with(mock: &Arc<MockService>, credential: &str, secrets: Secrets) -> Result<Vec<u8>, SignError> {
    let id = remote::digital_id(mock.clone(), &mock.account(), credential, secrets)?;
    pdfcraft_sign::sign(&open(&fixture()), &id, &opts())
}

/// The file's one signature is intact: its digest and signature value check out, it is "unknown"
/// only because nobody trusts the self-signed certificate, and trusting it makes it valid.
fn assert_intact(signed: &[u8], cert: &Certificate) {
    assert!(signed.starts_with(&fixture()), "an incremental update");
    let doc = open(signed);
    let sigs = signatures(&doc, signed, &TrustStore::default());
    let s = sigs.iter().find(|s| s.signed).expect("a signature");
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    let said: Vec<&str> = s.details.iter().map(|d| d.message.as_str()).collect();
    assert!(said.iter().any(|d| d.contains("identity is unknown")), "{said:?}");
    assert!(
        !said
            .iter()
            .any(|d| d.contains("altered") || d.contains("corrupt") || d.contains("raw r and s") || d.contains("length of the key's modulus")),
        "{said:?}"
    );
    assert_eq!(s.certificate.as_ref().map(|c| &c.raw), Some(&cert.raw));
    let trusted = signatures(&doc, signed, &TrustStore { certs: vec![cert.clone()] });
    let s = trusted.iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
}

/// The CSC methods called, in order.
fn methods(mock: &MockService) -> Vec<String> {
    let method = |url: &str| {
        url.strip_prefix(mock::CSC_V2_URL).or_else(|| url.strip_prefix(mock::CSC_V1_URL)).unwrap_or("").trim_start_matches('/').to_string()
    };
    mock.requests().iter().map(|r| method(&r.url)).collect()
}

fn err(r: Result<impl Sized, SignError>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn csc_v2_rsa_lists_sends_the_otp_signs_and_validates() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    let ids = remote::list(&*mock, &mock.account(), mock::PASSWORD).unwrap();
    // The disabled key isn't offered.
    assert_eq!(ids.iter().map(|i| i.credential.as_str()).collect::<Vec<_>>(), [mock::RSA, mock::P256]);
    let rsa = &ids[0];
    assert!(rsa.auth.pin);
    assert_eq!(rsa.auth.otp, OtpKind::Sent);
    assert_eq!(rsa.auth.objects.iter().map(|o| (o.id.as_str(), o.role)).collect::<Vec<_>>(), [("PIN", AuthRole::Pin), ("OTP", AuthRole::Otp)]);
    assert_eq!(rsa.auth.objects[1].label.as_deref(), Some("SMS code"));
    assert_eq!(rsa.certificate.subject.common_name(), Some("Mario Rossi"));
    assert!(rsa.reference().starts_with("remote:csc?url=https%3A%2F%2Frssp.example%2Fcsc%2Fv2&user=mario.rossi&credential=rsa-2048"));

    let before = mock.current_otp();
    assert_eq!(remote::send_otp(&*mock, &mock.account(), mock::RSA, mock::PASSWORD).unwrap(), OtpSent::Sent);
    assert_ne!(mock.current_otp(), before, "a new code was sent");
    let signed = sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
    let m = methods(&mock);
    let tail = &m[m.len() - 6..];
    assert_eq!(tail, ["info", "auth/login", "credentials/info", "credentials/authorize", "signatures/signHash", "auth/revoke"], "{m:?}");
    // Every session is signed out: listing, sending and signing.
    assert_eq!(m.iter().filter(|m| *m == "auth/login").count(), m.iter().filter(|m| *m == "auth/revoke").count());
}

#[test]
fn csc_v2_p256_signs_with_a_code_from_an_app() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    let ids = remote::list(&*mock, &mock.account(), mock::PASSWORD).unwrap();
    let p256 = ids.iter().find(|i| i.credential == mock::P256).unwrap();
    assert!(!p256.auth.pin);
    assert_eq!(p256.auth.otp, OtpKind::Typed);
    let signed = sign_with(&mock, mock::P256, secrets(None, Some(mock.current_otp()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::P256).unwrap());
    // An app's code isn't sent by the service.
    assert!(err(remote::send_otp(&*mock, &mock.account(), mock::P256, mock::PASSWORD)).contains("authenticator app"));
}

#[test]
fn csc_v1_dialect_signs_with_its_own_field_names() {
    let mock = Arc::new(MockService::csc_v1().unwrap());
    let ids = remote::list(&*mock, &mock.account(), mock::PASSWORD).unwrap();
    assert_eq!(ids.len(), 2);
    let rsa = ids.iter().find(|i| i.credential == mock::RSA).unwrap();
    assert_eq!((rsa.auth.pin, rsa.auth.otp), (true, OtpKind::Sent));
    let p256 = ids.iter().find(|i| i.credential == mock::P256).unwrap();
    assert_eq!((p256.auth.pin, p256.auth.otp), (false, OtpKind::Typed));

    assert_eq!(remote::send_otp(&*mock, &mock.account(), mock::RSA, mock::PASSWORD).unwrap(), OtpSent::Sent);
    assert!(methods(&mock).contains(&"credentials/sendOTP".to_string()));
    let signed = sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
    let signed = sign_with(&mock, mock::P256, secrets(None, Some(mock.current_otp()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::P256).unwrap());

    let requests = mock.requests();
    let body = |m: &str| -> serde_json::Value { serde_json::from_slice(&requests.iter().rev().find(|r| r.url.ends_with(m)).unwrap().body).unwrap() };
    let authorize = body("credentials/authorize");
    assert!(
        authorize.get("hash").is_some() && authorize.get("OTP").is_some() && authorize.get("hashes").is_none() && authorize.get("authData").is_none()
    );
    let sign_hash = body("signatures/signHash");
    assert!(sign_hash.get("hash").is_some() && sign_hash.get("hashAlgo").is_some() && sign_hash.get("hashAlgorithmOID").is_none());
}

#[test]
fn arss_lists_sends_the_sms_and_signs() {
    let mock = Arc::new(MockService::arss().unwrap());
    let ids = remote::list(&*mock, &mock.account(), mock::PASSWORD).unwrap();
    assert_eq!(ids.iter().map(|i| i.credential.as_str()).collect::<Vec<_>>(), [mock::RSA, mock::P256]);
    assert_eq!((ids[0].auth.pin, ids[0].auth.otp), (false, OtpKind::Sent));
    let reference = ids[0].reference();
    assert_eq!(
        reference,
        "remote:arss?user=mario.rossi&credential=rsa-2048&domain=frtest&url=https%3A%2F%2Farss.example%2FArubaSignService%2FArubaSignService"
    );
    assert_eq!(parse_reference(&reference).unwrap(), (mock.account(), mock::RSA.to_string()));

    let before = mock.current_otp();
    assert_eq!(remote::send_otp(&*mock, &mock.account(), mock::RSA, mock::PASSWORD).unwrap(), OtpSent::Sent);
    assert_ne!(mock.current_otp(), before);
    let signed = sign_with(&mock, mock::RSA, secrets(None, Some(mock.current_otp()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());

    for r in mock.requests() {
        assert_eq!((r.header("SOAPAction"), r.header("Content-Type")), (Some("\"\""), Some("text/xml; charset=utf-8")));
        // Only the operation is qualified; no default namespace leaks onto its children.
        assert!(!r.body_text().contains("xmlns=\""), "{}", r.body_text());
    }
    let last = mock.requests().pop().unwrap().body_text();
    assert!(last.contains("<arss:signhash><SignHashRequest><certID>rsa-2048</certID><hash>"), "{last}");
    assert!(last.contains("<hashtype>SHA256</hashtype><identity><otpPwd>"), "{last}");
    assert!(last.contains("<requirecert>false</requirecert></SignHashRequest></arss:signhash>"), "{last}");
}

#[test]
fn a_wrong_password_is_refused_at_sign_in() {
    for mock in [MockService::csc_v2().unwrap(), MockService::csc_v1().unwrap()] {
        let e = err(remote::list(&mock, &mock.account(), "wrong"));
        assert_eq!(e, "the signing service rejected the user name or password");
    }
    let mock = MockService::arss().unwrap();
    assert_eq!(err(remote::list(&mock, &mock.account(), "wrong")), "the signing service refused to list the certificates: Invalid Credentials");
    assert_eq!(
        err(remote::send_otp(&mock, &mock.account(), mock::RSA, "wrong")),
        "the signing service refused to send the OTP: Credentials Error (code 0003)"
    );
}

#[test]
fn a_wrong_or_expired_otp_is_refused() {
    let refusals = [
        (MockService::csc_v2 as fn() -> _, "the signing service rejected the PIN or the OTP (the OTP may have expired)"),
        (MockService::csc_v1, "the signing service rejected the OTP (it may have expired): The OTP is invalid or has expired"),
        (MockService::arss, "the signing service refused to sign: Invalid OTP (code 0001)"),
    ];
    for (make, said) in refusals {
        let mock = Arc::new(make().unwrap());
        let pin = (mock.account().protocol == Protocol::Csc).then_some(mock::PIN);
        let before = mock.current_otp();
        let wrong = format!("{:06}", (before.parse::<u32>().unwrap() + 1) % 1_000_000);
        assert_eq!(err(sign_with(&mock, mock::RSA, secrets(pin, Some(wrong)))), said);
        // The service spent the code anyway: the right one from before no longer works either.
        assert_eq!(err(sign_with(&mock, mock::RSA, secrets(pin, Some(before)))), said);
        mock.fail(Fault::OtpExpired);
        assert_eq!(err(sign_with(&mock, mock::RSA, secrets(pin, Some(mock.current_otp())))), said);
    }
    let mock = Arc::new(MockService::csc_v1().unwrap());
    let e = err(sign_with(&mock, mock::RSA, secrets(Some("0000"), Some(mock.current_otp()))));
    assert_eq!(e, "the signing service rejected the PIN: Invalid PIN");
}

#[test]
fn missing_secrets_are_asked_for_before_anything_is_signed() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    assert_eq!(err(sign_with(&mock, mock::RSA, secrets(None, Some(mock.current_otp())))), "this certificate needs its PIN to sign: enter it");
    assert_eq!(
        err(sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(String::new())))),
        "this certificate needs a one-time password (OTP) to sign: enter it"
    );
    assert!(!methods(&mock).iter().any(|m| m == "credentials/authorize"));
    let mock = Arc::new(MockService::arss().unwrap());
    assert!(err(sign_with(&mock, mock::RSA, secrets(None, None))).contains("one-time password"));
    assert!(mock.requests().is_empty());
}

#[test]
fn an_oauth_only_service_is_reported() {
    let mock = MockService::csc_v2().unwrap();
    mock.fail(Fault::OauthOnly);
    let e = err(remote::list(&mock, &mock.account(), mock::PASSWORD));
    assert!(e.contains("OAuth") && e.contains("doesn't support yet"), "{e}");
    assert_eq!(mock.requests().len(), 1, "nothing after info");
}

/// How many times `method` was called.
fn calls(mock: &MockService, method: &str) -> usize {
    methods(mock).iter().filter(|m| *m == method).count()
}

#[test]
fn an_authorization_confirmed_later_is_polled() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::AuthorizePending(2));
    let signed = sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
    assert_eq!(mock.waits(), 3);
    assert_eq!(calls(&mock, "credentials/authorizeCheck"), 3);
}

#[test]
fn an_authorization_never_confirmed_stops_being_waited_for() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::AuthorizePending(31));
    let e = err(sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))));
    assert_eq!(e, "the signature wasn't confirmed within a minute");
    assert_eq!((mock.waits(), calls(&mock, "credentials/authorizeCheck")), (30, 30));
    assert_eq!(calls(&mock, "signatures/signHash"), 0);
}

#[test]
fn a_wrong_pin_answered_with_401_is_not_sent_again() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::WrongPinIs401);
    let e = err(sign_with(&mock, mock::RSA, secrets(Some("0000"), Some(mock.current_otp()))));
    assert_eq!(e, "the signing service rejected the PIN: Invalid PIN");
    // Taken for an expired sign-in, it would have been one more wrong try towards a locked PIN.
    assert_eq!((calls(&mock, "credentials/authorize"), calls(&mock, "auth/login")), (1, 1));
}

#[test]
fn a_sign_in_that_expires_at_the_authorization_is_not_tried_again() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::TokenExpiresAtAuthorize);
    let e = err(sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))));
    assert_eq!(e, "the signing service's sign-in expired before the request went through: try again");
    assert_eq!((calls(&mock, "credentials/authorize"), calls(&mock, "auth/login")), (1, 1));
}

#[test]
fn credentials_the_list_leaves_undescribed_are_read_one_by_one() {
    let mock = MockService::csc_v2().unwrap();
    mock.fail(Fault::NoCredentialInfos);
    let ids = remote::list(&mock, &mock.account(), mock::PASSWORD).unwrap();
    assert_eq!(ids.iter().map(|i| i.credential.as_str()).collect::<Vec<_>>(), [mock::RSA, mock::P256]);
    assert_eq!(calls(&mock, "credentials/info"), 3);
}

#[test]
fn v1_lists_are_read_page_by_page_up_to_a_cap() {
    let mock = MockService::csc_v1().unwrap();
    mock.fail(Fault::Paged);
    let ids = remote::list(&mock, &mock.account(), mock::PASSWORD).unwrap();
    assert_eq!(ids.iter().map(|i| i.credential.as_str()).collect::<Vec<_>>(), [mock::RSA, mock::P256]);
    assert_eq!(calls(&mock, "credentials/list"), 3);
    // A hundred credentials that can't be read: 64 are tried, then the list says why.
    let mock = MockService::csc_v1().unwrap();
    mock.fail(Fault::ManyCredentials);
    let e = err(remote::list(&mock, &mock.account(), mock::PASSWORD));
    assert!(e.starts_with("the signing service has no certificate PdfCraft can sign with (ghost-0: the signing service refused"), "{e}");
    assert_eq!(calls(&mock, "credentials/info"), 64);
    // And no more than eight pages.
    mock.fail(Fault::Paged);
    let before = calls(&mock, "credentials/list");
    let _ = remote::list(&mock, &mock.account(), mock::PASSWORD);
    assert_eq!(calls(&mock, "credentials/list") - before, 8);
}

#[test]
fn nil_and_repeated_arss_entries_are_passed_over() {
    let mock = MockService::arss().unwrap();
    mock.fail(Fault::NilAndRepeated);
    let ids = remote::list(&mock, &mock.account(), mock::PASSWORD).unwrap();
    assert_eq!(ids.iter().map(|i| i.credential.as_str()).collect::<Vec<_>>(), [mock::RSA, mock::P256]);
}

#[test]
fn a_v1_pin_goes_only_to_a_credential_that_takes_one() {
    let mock = Arc::new(MockService::csc_v1().unwrap());
    // The P-256 credential's PIN `presence` is "false": a PIN given anyway isn't sent.
    let signed = sign_with(&mock, mock::P256, secrets(Some(mock::PIN), Some(mock.current_otp()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::P256).unwrap());
    let authorize = mock.requests().into_iter().rev().find(|r| r.url.ends_with("/credentials/authorize")).unwrap();
    let body: Value = serde_json::from_slice(&authorize.body).unwrap();
    assert!(body.get("PIN").is_none() && body.get("OTP").is_some(), "{body}");
}

#[test]
fn a_sign_byte_before_an_rsa_signature_is_taken_off() {
    for make in [MockService::csc_v2, MockService::arss] {
        let mock = Arc::new(make().unwrap());
        mock.fail(Fault::ExtraLeadingZero);
        let pin = (mock.account().protocol == Protocol::Csc).then_some(mock::PIN);
        let signed = sign_with(&mock, mock::RSA, secrets(pin, Some(mock.current_otp()))).unwrap();
        // `assert_intact` also checks validation took no note of the length: it is the modulus'.
        assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
    }
}

#[test]
fn refusals_after_the_secrets_are_sent_never_quote_them() {
    let cases = [
        (MockService::csc_v2 as fn() -> _, Fault::EchoSecretsOnAuthorize),
        (MockService::csc_v2, Fault::EchoSecretsOnSign),
        (MockService::csc_v1, Fault::EchoSecretsOnAuthorize),
        (MockService::csc_v1, Fault::EchoSecretsOnSign),
        (MockService::arss, Fault::EchoSecretsOnSign),
    ];
    for (make, fault) in cases {
        let mock = Arc::new(make().unwrap());
        mock.fail(fault);
        let pin = (mock.account().protocol == Protocol::Csc).then_some(mock::PIN);
        let otp = mock.current_otp();
        let e = err(sign_with(&mock, mock::RSA, secrets(pin, Some(otp.clone()))));
        assert!(e.contains("refused") && e.contains("<redacted>"), "{fault:?}: {e}");
        // The mock's tokens are AT-n, its SADs SAD-n.
        for secret in [mock::PASSWORD, mock::PIN, &otp, "AT-", "SAD-"] {
            assert!(!e.contains(secret), "{fault:?}: {e}");
        }
    }
}

#[test]
fn a_digest_the_sad_does_not_cover_is_reported() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::HashNotAuthorized);
    let e = err(sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))));
    assert_eq!(
        e,
        "the signing service refused the request (invalid_request): Hash is not authorized by neither SAD nor an access token with scope 'credential'"
    );
}

#[test]
fn a_signature_over_another_digest_is_never_used() {
    for (make, credential) in [(MockService::csc_v2 as fn() -> _, mock::RSA), (MockService::csc_v2, mock::P256), (MockService::arss, mock::RSA)] {
        let mock = Arc::new(make().unwrap());
        mock.fail(Fault::WrongDigest);
        let pin = (mock.account().protocol == Protocol::Csc && credential == mock::RSA).then_some(mock::PIN);
        let e = err(sign_with(&mock, credential, secrets(pin, Some(mock.current_otp()))));
        assert!(e.contains("does not match the certificate"), "{e}");
    }
}

#[test]
fn raw_ecdsa_is_turned_into_der() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::RawEcdsa);
    let signed = sign_with(&mock, mock::P256, secrets(None, Some(mock.current_otp()))).unwrap();
    // `assert_intact` also checks validation took no note of a raw signature: it was converted.
    assert_intact(&signed, &mock.certificate(mock::P256).unwrap());
}

#[test]
fn a_bare_digest_rsa_signature_is_diagnosed() {
    for make in [MockService::csc_v2, MockService::arss] {
        let mock = Arc::new(make().unwrap());
        mock.fail(Fault::BareDigest);
        let pin = (mock.account().protocol == Protocol::Csc).then_some(mock::PIN);
        let e = err(sign_with(&mock, mock::RSA, secrets(pin, Some(mock.current_otp()))));
        assert!(e.contains("signed the bare digest"), "{e}");
    }
}

#[test]
fn oversized_answers_are_refused() {
    for make in [MockService::csc_v2, MockService::arss] {
        let mock = make().unwrap();
        mock.fail(Fault::Oversized);
        assert_eq!(err(remote::list(&mock, &mock.account(), mock::PASSWORD)), "the signing service's answer is larger than 1 MiB, so it wasn't read");
    }
}

#[test]
fn an_expired_token_signs_in_again_once() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::TokenExpired);
    let signed = sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
    assert_eq!(methods(&mock)[..4], ["info", "auth/login", "credentials/info", "auth/login"]);
}

#[test]
fn the_otp_is_spent_by_one_signature() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    let id = remote::digital_id(mock.clone(), &mock.account(), mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))).unwrap();
    assert!(id.key.is_external());
    assert_eq!(id.friendly_name.as_deref(), Some("Mario Rossi"));
    let first = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    assert_intact(&first, &id.certificate);
    let sent = mock.requests().len();
    let e = err(pdfcraft_sign::sign(&open(&fixture()), &id, &opts()));
    assert!(e.contains("already used its one-time password"), "{e}");
    assert_eq!(mock.requests().len(), sent, "nothing more was sent");
}

#[test]
fn only_the_digest_leaves_the_machine_and_secrets_only_where_they_belong() {
    let pdf = fixture();
    assert!(pdf.len() > 32 * 1024);
    let windows: HashSet<&[u8]> = pdf.windows(64).collect();
    // Base64 of the document, from any offset that is a multiple of three, holds these.
    let chunks: Vec<String> = pdf.as_chunks::<48>().0.iter().map(|c| base64(c)).collect();
    for make in [MockService::csc_v2, MockService::csc_v1, MockService::arss] {
        let mock = Arc::new(make().unwrap());
        let csc = mock.account().protocol == Protocol::Csc;
        remote::list(&*mock, &mock.account(), mock::PASSWORD).unwrap();
        remote::send_otp(&*mock, &mock.account(), mock::RSA, mock::PASSWORD).unwrap();
        let otp = mock.current_otp();
        sign_with(&mock, mock::RSA, secrets(csc.then_some(mock::PIN), Some(otp.clone()))).unwrap();
        let basic = format!("Basic {}", base64(format!("{}:{}", mock::USER, mock::PASSWORD).as_bytes()));
        for r in mock.requests() {
            let mut text = r.body_text();
            // Far smaller than the document, which no encoding makes small, and none of it.
            assert!(r.body.len() < 4096, "{}: {} bytes", r.url, r.body.len());
            assert!(!text.contains("Confidential contract") && !r.body.windows(64).any(|w| windows.contains(w)), "{text}");
            assert!(!chunks.iter().any(|c| text.contains(c.as_str())), "{text}");
            if csc {
                // The password only in the Basic header of a sign-in.
                for (k, v) in &r.headers {
                    let login = r.url.ends_with("/auth/login") && k == "Authorization";
                    assert!(if login { *v == basic } else { !v.contains(mock::PASSWORD) && !v.starts_with("Basic") }, "{k}: {v}");
                }
                assert!(!text.contains(mock::PASSWORD));
                // The PIN and OTP only in the authorization, and only one digest, of SHA-256's length.
                if !r.url.ends_with("/credentials/authorize") {
                    assert!(!text.contains(mock::PIN) && !text.contains(&otp), "{}: {text}", r.url);
                }
                let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                if let Some(hashes) = v.get("hashes").or_else(|| v.get("hash")).and_then(|h| h.as_array()) {
                    assert_eq!(hashes.len(), 1);
                    assert_eq!(remote_base64(hashes[0].as_str().unwrap()).len(), 32);
                }
            } else {
                // The one digest signed, of SHA-256's length.
                if let Some((_, rest)) = text.split_once("<hash>") {
                    let hash = rest.split_once("</hash>").unwrap().0;
                    assert_eq!(remote_base64(hash).len(), 32, "{text}");
                }
                assert_eq!(text.matches("<hash>").count(), usize::from(text.contains("<arss:signhash>")), "{text}");
                // The password only in the identity, and the OTP only in the signature's.
                let user_pwd = format!("<userPWD>{}</userPWD>", mock::PASSWORD);
                assert!(text.contains(&user_pwd), "{text}");
                text = text.replace(&user_pwd, "");
                assert!(!text.contains(mock::PASSWORD), "{text}");
                let otp_pwd = format!("<otpPwd>{otp}</otpPwd>");
                assert_eq!(text.contains(&otp_pwd), text.contains("<arss:signhash>"), "{text}");
                assert!(!text.replace(&otp_pwd, "").contains(&otp), "{text}");
            }
        }
    }
}

fn base64(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn remote_base64(s: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

/// A transport that answers with fixed text, to see what reaches an error message.
struct Echo(&'static str, u16);

impl HttpTransport for Echo {
    fn post(&self, url: &str, _headers: &[(&str, &str)], _body: &[u8]) -> Result<HttpResponse, SignError> {
        let body =
            if url.ends_with("/info") { r#"{"specs":"2.0.0.0","authType":["basic"]}"#.as_bytes().to_vec() } else { self.0.as_bytes().to_vec() };
        Ok(HttpResponse { status: if url.ends_with("/info") { 200 } else { self.1 }, body })
    }
}

struct Down;

impl HttpTransport for Down {
    fn post(&self, _url: &str, _headers: &[(&str, &str)], _body: &[u8]) -> Result<HttpResponse, SignError> {
        Err(SignError::Crypto("connection refused".into()))
    }
}

fn csc_account() -> Account {
    Account { protocol: Protocol::Csc, url: "https://rssp.example/csc/v2/".into(), user: "u".into(), domain: None, hsm: None }
}

#[test]
fn errors_never_quote_a_secret() {
    let echo = Echo(r#"{"error":"invalid_request","error_description":"bad credentials u:s3cret-pass\u0007"}"#, 400);
    let e = err(remote::list(&echo, &csc_account(), "s3cret-pass"));
    assert_eq!(e, "the signing service refused the request (invalid_request): bad credentials u:<redacted>");
    let e = err(remote::list(&Down, &csc_account(), "pw"));
    assert_eq!(e, "the signing service is unreachable (connection refused)");
    let echo = Echo("<html>maintenance</html>", 503);
    assert_eq!(err(remote::list(&echo, &csc_account(), "pw")), "the signing service refused the request (HTTP 503)");
    let s = Secrets { password: "s3cret-pass".into(), pin: Some("1234".into()), otp: Some("654321".into()), otp_sent: true };
    assert_eq!(format!("{s:?}"), "Secrets(<redacted>)");
}

#[test]
fn references_round_trip() {
    let csc =
        Account { protocol: Protocol::Csc, url: "https://rssp.example:8443/csc/v2".into(), user: "mario rossi&co=1".into(), domain: None, hsm: None };
    let r = remote::reference(&csc, "cred/1 è");
    assert_eq!(r, "remote:csc?url=https%3A%2F%2Frssp.example%3A8443%2Fcsc%2Fv2&user=mario%20rossi%26co%3D1&credential=cred%2F1%20%C3%A8");
    assert_eq!(parse_reference(&r).unwrap(), (csc, "cred/1 è".to_string()));
    let aruba =
        Account { protocol: Protocol::Arss, url: ARSS_ENDPOINT.into(), user: "u1".into(), domain: Some("dom".into()), hsm: Some("hsm1".into()) };
    let r = remote::reference(&aruba, "AS0001");
    assert_eq!(r, "remote:arss?user=u1&credential=AS0001&domain=dom&hsm=hsm1");
    assert_eq!(parse_reference(&r).unwrap(), (aruba, "AS0001".to_string()));
    // Fields a later version adds are skipped.
    assert!(parse_reference("remote:arss?user=u&credential=c&color=blue").is_ok());
    for bad in [
        "keychain:abc",
        "remote:pkcs11?user=u&credential=c",
        "remote:csc?user=u&credential=c",
        "remote:csc?url=http%3A%2F%2Frssp.example&user=u&credential=c",
        "remote:arss?credential=c",
        "remote:arss?user=u",
        "remote:arss?user=u&user=v&credential=c",
        "remote:arss?user=%ZZ&credential=c",
        "remote:arss?user&credential=c",
    ] {
        assert!(parse_reference(bad).is_err(), "{bad}");
    }
}

#[test]
fn addresses_must_be_https_but_for_this_computer() {
    assert!(check_url("https://arss.arubapec.it/ArubaSignService/ArubaSignService").is_ok());
    assert!(check_url("http://127.0.0.1:8080/csc/v2").is_ok());
    assert_eq!(
        err(check_url("http://rssp.example/csc/v2")),
        "the signing service address must start with https:// (plain http:// is allowed only to this computer)"
    );
    let plain = Account { url: "http://rssp.example/csc/v2".into(), ..csc_account() };
    assert!(err(remote::list(&Down, &plain, "pw")).contains("https://"));
}

/// How [`Scripted`] answers a method other than `info`, `auth/login` and `auth/revoke`.
type Answer = Box<dyn Fn(&str) -> Option<HttpResponse> + Send + Sync>;

/// A CSC v2 service written out by hand, for credentials the mock doesn't have: `info` lists
/// `methods`, sign-in always works, and `answer` gives every other method's answer (`None`:
/// the transport fails). It records the methods called.
struct Scripted {
    methods: &'static [&'static str],
    answer: Answer,
    called: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(methods: &'static [&'static str], answer: impl Fn(&str) -> Option<HttpResponse> + Send + Sync + 'static) -> Arc<Scripted> {
        Arc::new(Scripted { methods, answer: Box::new(answer), called: Mutex::new(Vec::new()) })
    }

    fn called(&self, method: &str) -> usize {
        self.called.lock().unwrap().iter().filter(|m| *m == method).count()
    }
}

impl HttpTransport for Scripted {
    fn post(&self, url: &str, _headers: &[(&str, &str)], _body: &[u8]) -> Result<HttpResponse, SignError> {
        let method = url.strip_prefix("https://rssp.example/csc/v2/").unwrap_or(url);
        self.called.lock().unwrap().push(method.to_string());
        match method {
            "info" => Ok(ok(&json!({ "specs": "2.2.0.0", "authType": ["basic"], "methods": self.methods }))),
            "auth/login" => Ok(ok(&json!({ "access_token": "scripted-token" }))),
            "auth/revoke" => Ok(HttpResponse { status: 204, body: Vec::new() }),
            m => (self.answer)(m).ok_or_else(|| SignError::Crypto("timed out".into())),
        }
    }
}

fn ok(v: &Value) -> HttpResponse {
    HttpResponse { status: 200, body: v.to_string().into_bytes() }
}

/// A `credentialInfo` for the mock's P-256 certificate, authorized with `auth`.
fn p256_info(auth: Value) -> Value {
    let cert = MockService::csc_v2().unwrap().certificate(mock::P256).unwrap();
    json!({ "key": { "status": "enabled", "algo": ["1.2.840.10045.4.3.2"], "len": 256 },
            "cert": { "certificates": [base64(&cert.raw)] }, "auth": auth, "SCAL": "2", "multisign": 1 })
}

#[test]
fn the_specs_totp_code_received_by_sms_is_sent_for() {
    // The credential of the spec's own credentials/list example (S22 p.61), on a service whose
    // info example lists sendOTP: a "totp" Password whose description says it comes by SMS.
    let info = p256_info(json!({ "mode": "explicit", "expression": "PIN AND OTP", "objects": [
        { "type": "Password", "id": "PIN", "format": "N", "label": "PIN", "description": "Please enter the signature PIN" },
        { "type": "Password", "id": "OTP", "format": "N", "generator": "totp", "label": "Mobile OTP",
          "description": "Please enter the 6 digit code you received by SMS" },
    ]}));
    let methods = &["auth/login", "credentials/list", "credentials/info", "credentials/sendOTP", "credentials/authorize", "signatures/signHash"];
    let t = Scripted::new(methods, move |m| match m {
        "credentials/info" => Some(ok(&info)),
        "credentials/sendOTP" => Some(HttpResponse { status: 204, body: Vec::new() }),
        _ => None,
    });
    assert_eq!(remote::send_otp(&*t, &csc_account(), "C1", "pw").unwrap(), OtpSent::Sent);
    assert_eq!(t.called("credentials/sendOTP"), 1);
}

#[test]
fn a_challenge_to_confirm_on_a_phone_is_not_waited_for_blind() {
    let info =
        p256_info(json!({ "mode": "explicit", "objects": [{ "type": "ChallengeResponseOOB", "id": "PUSH", "label": "Confirm on your phone" }] }));
    let t = Scripted::new(&[], move |m| match m {
        "credentials/info" => Some(ok(&info)),
        // A challenge for PdfCraft to show (200), not one sent to the phone (201, 204).
        "credentials/getChallenge" => Some(ok(&json!({ "challenge": "4711" }))),
        "credentials/authorize" | "credentials/authorizeCheck" => Some(HttpResponse { status: 202, body: br#"{"handle":"h"}"#.to_vec() }),
        _ => None,
    });
    assert_eq!(
        err(remote::send_otp(&*t, &csc_account(), "C1", "pw")),
        "this certificate's signatures are confirmed on your other device while signing: there is no code to send"
    );
    assert_eq!(t.called("credentials/getChallenge"), 0, "a challenge asked for now would be replaced by the signature's");
    let id = remote::digital_id(t.clone(), &csc_account(), "C1", Secrets { password: "pw".into(), pin: None, otp: None, otp_sent: false }).unwrap();
    let e = err(pdfcraft_sign::sign(&open(&fixture()), &id, &opts()));
    assert_eq!(e, "the signing service shows a challenge to confirm this signature with, which PdfCraft can't display while signing yet");
    assert_eq!((t.called("credentials/getChallenge"), t.called("credentials/authorize")), (1, 0));
}

#[test]
fn a_service_that_stops_answering_ends_the_list() {
    let t = Scripted::new(&[], |m| match m {
        "credentials/list" => Some(ok(&json!({ "credentialIDs": ["C1", "C2", "C3"] }))),
        _ => None,
    });
    assert_eq!(err(remote::list(&*t, &csc_account(), "pw")), "the signing service is unreachable (timed out)");
    assert_eq!(t.called("credentials/info"), 1, "one timeout, not one per credential");
}

/// The `authorize` requests' `authData`, as (id, value) pairs, in order.
fn auth_data(mock: &MockService) -> Vec<Vec<(String, Option<String>)>> {
    let pairs = |d: &Value| -> Vec<(String, Option<String>)> {
        d.as_array()
            .into_iter()
            .flatten()
            .map(|o| (o["id"].as_str().unwrap().to_string(), o.get("value").and_then(Value::as_str).map(str::to_string)))
            .collect()
    };
    let authorizations = mock.requests().into_iter().filter(|r| r.url.ends_with("/credentials/authorize"));
    authorizations.map(|r| pairs(&serde_json::from_slice::<Value>(&r.body).unwrap()["authData"])).collect()
}

/// Only the ID of a `getChallenge` request's object.
fn challenged(mock: &MockService) -> Vec<String> {
    let challenges = mock.requests().into_iter().filter(|r| r.url.ends_with("/credentials/getChallenge"));
    challenges.map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["authObjectID"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_code_after_the_pin_is_sent_as_the_code_not_as_a_second_pin() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::OffersSendOtp);
    mock.set_auth(
        mock::RSA,
        "PIN AND CODICE",
        json!([{ "type": "Password", "id": "PIN", "format": "N", "label": "PIN" }, { "type": "Password", "id": "CODICE", "label": "Codice SMS" }]),
    );
    let ids = remote::list(&*mock, &mock.account(), mock::PASSWORD).unwrap();
    let rsa = ids.iter().find(|i| i.credential == mock::RSA).unwrap();
    assert_eq!((rsa.auth.pin, rsa.auth.otp), (true, OtpKind::Sent), "a field for the code, and Send code");
    assert_eq!(rsa.auth.objects.iter().map(|o| o.role).collect::<Vec<_>>(), [AuthRole::Pin, AuthRole::Otp]);
    assert_eq!(remote::send_otp(&*mock, &mock.account(), mock::RSA, mock::PASSWORD).unwrap(), OtpSent::Sent);
    assert_eq!(calls(&mock, "credentials/sendOTP"), 1);
    let otp = mock.current_otp();
    let signed = sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(otp.clone()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
    assert_eq!(auth_data(&mock), [vec![("PIN".to_string(), Some(mock::PIN.to_string())), ("CODICE".to_string(), Some(otp))]]);
}

/// A PIN, and a code by SMS or from an app: one or the other.
fn sms_or_app(mock: &MockService, expression: &str) {
    mock.set_auth(
        mock::RSA,
        expression,
        json!([{ "type": "Password", "id": "PIN", "label": "PIN" }, { "type": "ChallengeResponse", "id": "SMS", "label": "SMS code" },
               { "type": "Password", "id": "APP", "generator": "totp", "label": "App code" }]),
    );
}

#[test]
fn an_exclusive_choice_of_codes_sends_one() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    sms_or_app(&mock, "PIN AND (SMS XOR APP)");
    let ids = remote::list(&*mock, &mock.account(), mock::PASSWORD).unwrap();
    let rsa = ids.iter().find(|i| i.credential == mock::RSA).unwrap();
    assert_eq!((rsa.auth.pin, rsa.auth.otp), (true, OtpKind::Sent));
    assert_eq!(
        err(sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), None))),
        "this certificate needs a one-time password (OTP) to sign: enter it"
    );
    // A code nobody had sent (an app's) goes under the first of the two.
    let otp = mock.current_otp();
    let signed = sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(otp.clone()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
    // The mock refuses both sides of the XOR: one code, under one of them.
    assert_eq!(auth_data(&mock), [vec![("PIN".to_string(), Some(mock::PIN.to_string())), ("SMS".to_string(), Some(otp))]]);
}

/// Send code and the signature after it pick the same object, in either order of the two: the
/// SMS is asked for, and its code goes under SMS, never under the app's object (which the
/// service would refuse it under, and the SMS be wasted). A code from the app goes under the
/// first, as before.
#[test]
fn a_code_sent_by_sms_goes_under_the_sms_whatever_the_order() {
    for (expression, first) in [("PIN AND (APP XOR SMS)", "APP"), ("PIN AND (SMS XOR APP)", "SMS")] {
        let mock = Arc::new(MockService::csc_v2().unwrap());
        sms_or_app(&mock, expression);
        assert_eq!(remote::send_otp(&*mock, &mock.account(), mock::RSA, mock::PASSWORD).unwrap(), OtpSent::Sent, "{expression}");
        assert_eq!(challenged(&mock), ["SMS"], "{expression}");
        let otp = mock.current_otp();
        let signed = sign_with(&mock, mock::RSA, sent(Some(mock::PIN), otp.clone())).unwrap();
        assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
        let pin = ("PIN".to_string(), Some(mock::PIN.to_string()));
        assert_eq!(auth_data(&mock), [vec![pin.clone(), ("SMS".to_string(), Some(otp))]], "{expression}");
        // An app's code, nothing sent: the first of the two takes it.
        let otp = mock.current_otp();
        sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(otp.clone()))).unwrap();
        assert_eq!(auth_data(&mock).pop().unwrap(), [pin, (first.to_string(), Some(otp))], "{expression}");
    }
    // The mock holds a code to the object it was sent for: the SMS code under the app's object
    // is refused, so a client that put it there would be caught.
    let mock = Arc::new(MockService::csc_v2().unwrap());
    sms_or_app(&mock, "PIN AND (APP XOR SMS)");
    remote::send_otp(&*mock, &mock.account(), mock::RSA, mock::PASSWORD).unwrap();
    let e = err(sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))));
    assert_eq!(e, "the signing service rejected the PIN or the OTP (the OTP may have expired)");
    assert_eq!(auth_data(&mock).pop().unwrap().get(1).map(|(id, _)| id.as_str()), Some("APP"));
    // Of two codes the service sends, Send code asks for the one signing fills: the first in the
    // expression, not in the list of objects.
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.set_auth(
        mock::RSA,
        "PIN AND (CALL XOR SMS)",
        json!([{ "type": "Password", "id": "PIN" }, { "type": "ChallengeResponse", "id": "SMS" }, { "type": "ChallengeResponse", "id": "CALL" }]),
    );
    remote::send_otp(&*mock, &mock.account(), mock::RSA, mock::PASSWORD).unwrap();
    assert_eq!(challenged(&mock), ["CALL"]);
    let otp = mock.current_otp();
    sign_with(&mock, mock::RSA, sent(Some(mock::PIN), otp.clone())).unwrap();
    assert_eq!(auth_data(&mock).pop().unwrap().pop(), Some(("CALL".to_string(), Some(otp))));
}

#[test]
fn a_confirmation_on_the_phone_is_asked_for_only_without_a_code() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.set_auth(
        mock::P256,
        "PUSH XOR APP",
        json!([{ "type": "ChallengeResponseOOB", "id": "PUSH", "label": "Confirm on your phone" },
               { "type": "Password", "id": "APP", "generator": "totp", "label": "App code" }]),
    );
    let otp = mock.current_otp();
    let signed = sign_with(&mock, mock::P256, secrets(None, Some(otp.clone()))).unwrap();
    assert_intact(&signed, &mock.certificate(mock::P256).unwrap());
    assert!(challenged(&mock).is_empty(), "nothing is pushed to the phone when a code was given");
    assert_eq!(auth_data(&mock), [vec![("APP".to_string(), Some(otp))]]);
    let signed = sign_with(&mock, mock::P256, secrets(None, None)).unwrap();
    assert_intact(&signed, &mock.certificate(mock::P256).unwrap());
    assert_eq!(challenged(&mock), ["PUSH"]);
    assert_eq!(auth_data(&mock).pop().unwrap(), [("PUSH".to_string(), None)]);
}

#[test]
fn the_clock_bounds_the_wait_for_a_confirmation() {
    // Ten seconds an authorizeCheck, plus the two waited before each: a minute is up after five,
    // long before the thirty polls the count allows.
    let mock = Arc::new(MockService::csc_v2().unwrap());
    mock.fail(Fault::AuthorizePending(31));
    mock.fail(Fault::SlowAuthorizeCheck(10));
    let e = err(sign_with(&mock, mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))));
    assert_eq!(e, "the signature wasn't confirmed within a minute");
    assert_eq!((mock.waits(), calls(&mock, "credentials/authorizeCheck")), (5, 5));
    assert_eq!(calls(&mock, "signatures/signHash"), 0);
}

#[test]
fn the_chain_the_service_gives_is_embedded_and_validates_to_its_root() {
    let only = |c: Certificate| TrustStore { certs: vec![c] };
    for make in [MockService::csc_v2, MockService::csc_v1] {
        let mock = Arc::new(make().unwrap());
        let (leaf, intermediate, root) = (mock.certificate(mock::RSA).unwrap(), mock.intermediate(), mock.root());
        assert_ne!(leaf.issuer.raw, leaf.subject.raw, "issued by the mock CA");
        // Valid far past any machine's clock, which some callers sign with.
        let day = |year, month, day| Time { year, month, day, hour: 12, minute: 0, second: 0 };
        for c in [&leaf, &intermediate, &root] {
            assert!(c.valid_at(day(2000, 1, 2)) && c.valid_at(day(2099, 12, 30)), "{}", c.display_name());
        }
        let listed = remote::list(&*mock, &mock.account(), mock::PASSWORD).unwrap();
        let rsa = listed.iter().find(|i| i.credential == mock::RSA).unwrap();
        assert_eq!(rsa.chain.iter().map(|c| &c.raw).collect::<Vec<_>>(), [&intermediate.raw, &root.raw], "end-entity first, without it");
        let id = remote::digital_id(mock.clone(), &mock.account(), mock::RSA, secrets(Some(mock::PIN), Some(mock.current_otp()))).unwrap();
        assert!(!id.chain.iter().any(|c| c.raw == leaf.raw) && id.chain.iter().any(|c| c.raw == intermediate.raw));
        let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
        let doc = open(&signed);
        let s = signatures(&doc, &signed, &only(root.clone())).into_iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Valid, "{:?}", s.details);
        assert_eq!(s.chain.iter().map(|c| &c.raw).collect::<Vec<_>>(), [&leaf.raw, &intermediate.raw, &root.raw]);
        let s = signatures(&doc, &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Unknown);
    }
    // Aruba lists the signer's certificate alone, so the file carries no issuer: its root
    // vouches for it only to a validator that has the intermediate too.
    let mock = Arc::new(MockService::arss().unwrap());
    let id = remote::digital_id(mock.clone(), &mock.account(), mock::RSA, secrets(None, Some(mock.current_otp()))).unwrap();
    assert!(id.chain.is_empty());
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let doc = open(&signed);
    let status = |trust: &TrustStore| signatures(&doc, &signed, trust).into_iter().find(|s| s.signed).unwrap().status;
    assert_eq!((status(&only(mock.intermediate())), status(&only(mock.root()))), (Status::Valid, Status::Unknown));
}

#[test]
fn a_key_that_signs_with_nothing_pdfcraft_asks_for_is_not_offered() {
    // A P-256 key that lists only ECDSA with SHA-384, while PdfCraft signs P-256 with SHA-256.
    let mut info = p256_info(json!({ "mode": "explicit", "objects": [{ "type": "ChallengeResponse", "id": "OTP" }] }));
    info["key"]["algo"] = json!(["1.2.840.10045.4.3.3"]);
    let t = Scripted::new(&[], move |m| match m {
        "credentials/list" => Some(ok(&json!({ "credentialIDs": ["C1"] }))),
        "credentials/info" => Some(ok(&info)),
        "credentials/getChallenge" | "credentials/sendOTP" => Some(HttpResponse { status: 204, body: Vec::new() }),
        _ => None,
    });
    let why = "its key can't sign with ECDSA and SHA-256 (it lists 1.2.840.10045.4.3.3)";
    assert_eq!(err(remote::list(&*t, &csc_account(), "pw")), format!("the signing service has no certificate PdfCraft can sign with (C1: {why})"));
    // No code is sent for it, and no signature started.
    assert_eq!(err(remote::send_otp(&*t, &csc_account(), "C1", "pw")), format!("the signing service's certificate C1 can't sign: {why}"));
    assert_eq!(t.called("credentials/getChallenge") + t.called("credentials/sendOTP"), 0);
    let e = err(remote::digital_id(
        t.clone(),
        &csc_account(),
        "C1",
        Secrets { password: "pw".into(), pin: None, otp: Some("1".into()), otp_sent: false },
    ));
    assert_eq!(e, format!("the signing service's certificate C1 can't sign: {why}"));
    assert_eq!(t.called("credentials/authorize"), 0);
}

/// `service`, with the Aruba domain taken out of every request.
struct NoDomain(Arc<MockService>);

impl HttpTransport for NoDomain {
    fn post(&self, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, SignError> {
        let text = String::from_utf8(body.to_vec()).unwrap().replace(&format!("<typeOtpAuth>{}</typeOtpAuth>", mock::DOMAIN), "");
        self.0.post(url, headers, text.as_bytes())
    }
}

#[test]
fn an_aruba_account_needs_its_domain() {
    let mock = Arc::new(MockService::arss().unwrap());
    let needs = "an Aruba account needs its domain (the \"dominio\" Aruba gives with the account): enter it";
    for domain in [None, Some(" ".to_string())] {
        let account = Account { domain, ..mock.account() };
        assert_eq!(err(remote::list(&*mock, &account, mock::PASSWORD)), needs);
        assert_eq!(err(remote::send_otp(&*mock, &account, mock::RSA, mock::PASSWORD)), needs);
        assert_eq!(err(remote::digital_id(mock.clone(), &account, mock::RSA, secrets(None, Some(mock.current_otp())))), needs);
    }
    assert!(mock.requests().is_empty(), "refused before asking Aruba");
    // What Aruba answers without one, and with a domain that isn't the account's.
    let e = err(remote::list(&NoDomain(mock.clone()), &mock.account(), mock::PASSWORD));
    assert_eq!(e, "the signing service refused to list the certificates: Invalid Parameters");
    let account = Account { domain: Some("other".into()), ..mock.account() };
    assert_eq!(
        err(remote::list(&*mock, &account, mock::PASSWORD)),
        "the signing service refused to list the certificates: Error 0003 - Invalid User Credentials"
    );
}

/// The user name and the Aruba domain go as the account check took them, trimmed: " frtest "
/// reaches Aruba as "frtest", and a CSC service signs in " mario.rossi " as "mario.rossi".
#[test]
fn the_user_name_and_the_domain_go_trimmed() {
    let mock = Arc::new(MockService::arss().unwrap());
    let account = Account { user: format!(" {} ", mock::USER), domain: Some(format!(" {} ", mock::DOMAIN)), ..mock.account() };
    assert_eq!(remote::list(&*mock, &account, mock::PASSWORD).unwrap().len(), 2);
    assert_eq!(remote::send_otp(&*mock, &account, mock::RSA, mock::PASSWORD).unwrap(), OtpSent::Sent);
    let id = remote::digital_id(mock.clone(), &account, mock::RSA, secrets(None, Some(mock.current_otp()))).unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    assert_intact(&signed, &mock.certificate(mock::RSA).unwrap());
    let requests = mock.requests();
    assert_eq!(requests.len(), 4, "listCert, sendCredential, listCert, signhash");
    for r in requests {
        let body = r.body_text();
        assert!(body.contains(&format!("<typeOtpAuth>{}</typeOtpAuth>", mock::DOMAIN)), "{body}");
        assert!(body.contains(&format!("<user>{}</user>", mock::USER)), "{body}");
    }
    let mock = MockService::csc_v2().unwrap();
    let account = Account { user: format!(" {} ", mock::USER), ..mock.account() };
    assert_eq!(remote::list(&mock, &account, mock::PASSWORD).unwrap().len(), 2);
}

#[test]
fn every_error_says_who_has_to_act() {
    use remote::RemoteFault::{Caller, Service};
    let csc = || Arc::new(MockService::csc_v2().unwrap());
    let faulty = |f: Fault| {
        let m = csc();
        m.fail(f);
        m
    };
    let sign_rsa = |m: &Arc<MockService>, pin: &str| sign_with(m, mock::RSA, secrets(Some(pin), Some(m.current_otp())));
    let v1 = Arc::new(MockService::csc_v1().unwrap());
    let arss = Arc::new(MockService::arss().unwrap());
    let used = {
        let m = csc();
        let id = remote::digital_id(m.clone(), &m.account(), mock::RSA, secrets(Some(mock::PIN), Some(m.current_otp()))).unwrap();
        pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
        pdfcraft_sign::sign(&open(&fixture()), &id, &opts())
    };
    let no_otp = Scripted::new(&[], |m| match m {
        "credentials/info" => Some(ok(&p256_info(json!({ "mode": "explicit", "objects": [{ "type": "Password", "id": "PIN" }] })))),
        _ => None,
    });
    let cases: Vec<(Result<(), SignError>, &str, remote::RemoteFault)> = vec![
        (remote::list(&*csc(), &csc().account(), "wrong").map(drop), "the signing service rejected the user name or password", Caller),
        (sign_rsa(&csc(), "0000").map(drop), "the signing service rejected the PIN or the OTP", Caller),
        (sign_with(&v1, mock::RSA, secrets(Some("0000"), Some(v1.current_otp()))).map(drop), "the signing service rejected the PIN", Caller),
        (sign_with(&v1, mock::RSA, secrets(Some(mock::PIN), Some("1".into()))).map(drop), "the signing service rejected the OTP", Caller),
        (remote::list(&*arss, &arss.account(), "wrong").map(drop), "the signing service refused to list the certificates", Caller),
        (remote::send_otp(&*arss, &arss.account(), mock::RSA, "wrong").map(drop), "the signing service refused to send the OTP", Caller),
        (sign_with(&arss, mock::RSA, secrets(None, Some("1".into()))).map(drop), "the signing service refused to sign", Caller),
        (
            remote::send_otp(&*csc(), &csc().account(), "nope", mock::PASSWORD).map(drop),
            "the signing service refused to describe the certificate nope",
            Caller,
        ),
        (
            sign_with(&csc(), "nope", secrets(None, Some("1".into()))).map(drop),
            "the signing service refused to describe the certificate nope",
            Caller,
        ),
        (
            sign_with(&csc(), mock::DISABLED, secrets(None, Some("1".into()))).map(drop),
            "the signing service's certificate disabled can't sign",
            Caller,
        ),
        (sign_with(&arss, "nope", secrets(None, Some("1".into()))).map(drop), "the signing service has no certificate nope", Caller),
        (remote::send_otp(&*csc(), &csc().account(), mock::P256, mock::PASSWORD).map(drop), "the signing service doesn't send", Caller),
        (remote::send_otp(&*no_otp, &csc_account(), "C1", "pw").map(drop), "this certificate signs without a one-time password", Caller),
        (sign_with(&csc(), mock::RSA, secrets(None, Some("1".into()))).map(drop), "this certificate needs its PIN", Caller),
        (remote::list(&*csc(), &Account { user: " ".into(), ..csc().account() }, "pw").map(drop), "enter the signing service user name", Caller),
        (remote::list(&*csc(), &Account { user: "a:b".into(), ..csc().account() }, "pw").map(drop), "the signing service user name can't", Caller),
        (remote::list(&*arss, &Account { domain: None, ..arss.account() }, "pw").map(drop), "an Aruba account needs its domain", Caller),
        (check_url("http://rssp.example"), "the signing service address", Caller),
        (parse_reference("remote:csc?user=u").map(drop), "malformed data: remote signing reference", Caller),
        (used.map(drop), "this remote signature has already used its one-time password", Caller),
        (remote::list(&Down, &csc_account(), "pw").map(drop), "the signing service is unreachable (connection refused)", Service),
        (remote::list(&*faulty(Fault::Oversized), &csc().account(), mock::PASSWORD).map(drop), "the signing service's answer is larger", Service),
        (remote::list(&*faulty(Fault::OauthOnly), &csc().account(), mock::PASSWORD).map(drop), "this signing service can't be used", Service),
        (remote::list(&Echo("<html>", 200), &csc_account(), "pw").map(drop), "the signing service's answer isn't JSON", Service),
        (remote::list(&Echo("<html>", 503), &csc_account(), "pw").map(drop), "the signing service refused the request (HTTP 503)", Service),
        (sign_rsa(&faulty(Fault::WrongDigest), mock::PIN).map(drop), "the signing service's signature does not match", Service),
        (sign_rsa(&faulty(Fault::BareDigest), mock::PIN).map(drop), "the signing service signed the bare digest", Service),
        (sign_rsa(&faulty(Fault::HashNotAuthorized), mock::PIN).map(drop), "the signing service refused the request (invalid_request)", Service),
        (sign_rsa(&faulty(Fault::TokenExpiresAtAuthorize), mock::PIN).map(drop), "the signing service's sign-in expired", Service),
        (sign_rsa(&faulty(Fault::AuthorizePending(31)), mock::PIN).map(drop), "the signature wasn't confirmed within a minute", Service),
    ];
    for (r, begins, fault) in cases {
        let e = r.expect_err(begins);
        assert!(e.to_string().starts_with(begins), "{e} / {begins}");
        assert_eq!(remote::fault_of(&e), fault, "{e}");
        // The same through a layer that passes the text on.
        assert_eq!(remote::fault_of(e.to_string()), fault, "{e}");
    }
    // The ones an interface translates are all a caller's but OAuth.
    for w in remote::WORDINGS {
        assert_eq!(remote::fault_of(w), if *w == remote::NEEDS_OAUTH { Service } else { Caller }, "{w}");
    }
    assert_eq!(remote::fault_of("not supported: SHA-1 for new signatures"), Service);
}
