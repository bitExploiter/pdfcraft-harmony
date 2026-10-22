//! The remote signing transport over real HTTP: a server thread on 127.0.0.1 (plain HTTP is what
//! `check_url` allows to this computer) passes each request to the in-process signing service of
//! `pdfcraft_sign::remote::mock`, so listing and signing go through sockets, ureq and back. Also
//! what the transport refuses: redirects, answers over the cap, a closed port. HTTPS itself is
//! tested in `src/https.rs`, which can trust a test's own certificate.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pdfcraft_platform::https::transport;
use pdfcraft_sign::keys::Scheme;
use pdfcraft_sign::remote::mock::{self, MockService};
use pdfcraft_sign::remote::{self, Account, HttpTransport, MAX_RESPONSE, OtpSent, Protocol, Secrets};
use pdfcraft_sign::{DigestAlg, SignError};

/// A request as the server read it.
#[derive(Clone, Debug)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// What the server sends back: the status, more header lines, the body, and whether to give its
/// length (otherwise the body ends when the connection closes).
struct Answer {
    status: u16,
    headers: Vec<String>,
    body: Vec<u8>,
    length: bool,
}

impl Answer {
    fn new(status: u16, body: Vec<u8>) -> Answer {
        Answer { status, headers: Vec::new(), body, length: true }
    }
}

/// A plain HTTP/1.1 server on 127.0.0.1 answering every request with `answer`, one request per
/// connection (`Connection: close`). Returns its port and the requests it read.
fn serve(answer: impl Fn(&Seen) -> Answer + Send + 'static) -> (u16, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
            let Some(request) = read_request(&stream) else { continue };
            let a = answer(&request);
            log.lock().unwrap().push(request);
            // The client may hang up mid-answer (one over the cap): that's its business.
            let _ = write_answer(stream, &a);
        }
    });
    (port, seen)
}

fn read_request(stream: &TcpStream) -> Option<Seen> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let path = line.split(' ').nth(1)?.to_string();
    let mut headers = Vec::new();
    loop {
        line.clear();
        r.read_line(&mut line).ok()?;
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let length: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0; length];
    r.read_exact(&mut body).ok()?;
    Some(Seen { path, headers, body })
}

fn write_answer(mut stream: TcpStream, a: &Answer) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {} Answer\r\nConnection: close\r\n", a.status);
    // A 204 has no body, and says nothing of its length.
    if a.length && a.status != 204 {
        head.push_str(&format!("Content-Length: {}\r\n", a.body.len()));
    }
    for h in &a.headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&a.body)?;
    stream.flush()
}

/// A server in front of `mock`, which answers at `origin` (its own address's scheme and host).
fn serve_mock(mock: Arc<MockService>, origin: &'static str) -> (u16, Arc<Mutex<Vec<Seen>>>) {
    serve(move |r| {
        let headers: Vec<(&str, &str)> = r.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        match mock.post(&format!("{origin}{}", r.path), &headers, &r.body) {
            Ok(a) => Answer::new(a.status, a.body),
            Err(e) => Answer::new(500, e.to_string().into_bytes()),
        }
    })
}

/// `mock`'s account, at the server on `port` instead.
fn on_loopback(mock: &MockService, port: u16, path: &str) -> Account {
    Account { url: format!("http://127.0.0.1:{port}{path}"), ..mock.account() }
}

/// A CSC account at `url`, for a server that isn't a signing service.
fn csc(url: String) -> Account {
    Account { protocol: Protocol::Csc, url, user: mock::USER.into(), domain: None, hsm: None }
}

fn err(r: Result<impl Sized, SignError>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

/// Sign a message with `credential` and check the signature against its certificate.
fn sign_and_check(account: &Account, credential: &str, secrets: Secrets, scheme: Scheme) {
    let id = remote::digital_id(transport(), account, credential, secrets).unwrap();
    let msg: &[u8] = b"the CMS signed attributes";
    let sig = id.key.sign(DigestAlg::Sha256, msg).unwrap();
    let digest = DigestAlg::Sha256.digest(&[msg]);
    assert!(id.certificate.public_key.verify(scheme, DigestAlg::Sha256, &digest, &sig).unwrap());
}

#[test]
fn csc_lists_sends_the_otp_and_signs_over_http() {
    let mock = Arc::new(MockService::csc_v2().unwrap());
    let (port, seen) = serve_mock(mock.clone(), "https://rssp.example");
    let account = on_loopback(&mock, port, "/csc/v2");
    let t = transport();

    let ids = remote::list(&*t, &account, mock::PASSWORD).unwrap();
    assert_eq!(ids.iter().map(|i| i.credential.as_str()).collect::<Vec<_>>(), [mock::RSA, mock::P256]);
    assert_eq!(ids[0].certificate.raw, mock.certificate(mock::RSA).unwrap().raw);
    // An empty 204 answer (getChallenge) comes through too.
    assert_eq!(remote::send_otp(&*t, &account, mock::RSA, mock::PASSWORD).unwrap(), OtpSent::Sent);
    let secrets = Secrets { password: mock::PASSWORD.into(), pin: Some(mock::PIN.into()), otp: Some(mock.current_otp()), otp_sent: true };
    sign_and_check(&account, mock::RSA, secrets, Scheme::RsaPkcs1);
    let secrets = Secrets { password: mock::PASSWORD.into(), pin: None, otp: Some(mock.current_otp()), otp_sent: false };
    sign_and_check(&account, mock::P256, secrets, Scheme::Ecdsa);

    let seen = seen.lock().unwrap().clone();
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    let methods =
        ["info", "auth/login", "credentials/list", "credentials/getChallenge", "credentials/authorize", "signatures/signHash", "auth/revoke"];
    for method in methods {
        assert!(paths.contains(&format!("/csc/v2/{method}").as_str()), "{method} in {paths:?}");
    }
    let agent = format!("PdfCraft/{}", env!("CARGO_PKG_VERSION"));
    assert!(seen.iter().all(|s| s.header("User-Agent") == Some(agent.as_str())), "{seen:?}");
    assert_eq!(mock.requests().len(), seen.len(), "every request reached the service once");
}

#[test]
fn arss_soap_signs_over_http() {
    let mock = Arc::new(MockService::arss().unwrap());
    let (port, seen) = serve_mock(mock.clone(), "https://arss.example");
    let account = on_loopback(&mock, port, "/ArubaSignService/ArubaSignService");
    let t = transport();
    let ids = remote::list(&*t, &account, mock::PASSWORD).unwrap();
    assert_eq!(ids.iter().map(|i| i.credential.as_str()).collect::<Vec<_>>(), [mock::RSA, mock::P256]);
    assert_eq!(remote::send_otp(&*t, &account, mock::RSA, mock::PASSWORD).unwrap(), OtpSent::Sent);
    let secrets = Secrets { password: mock::PASSWORD.into(), pin: None, otp: Some(mock.current_otp()), otp_sent: true };
    sign_and_check(&account, mock::RSA, secrets, Scheme::RsaPkcs1);
    // `SOAPAction: ""` arrives as the two quotes, which the service insists on.
    assert!(seen.lock().unwrap().iter().all(|s| s.header("SOAPAction") == Some("\"\"")));
}

#[test]
fn redirects_are_not_followed() {
    // Where every redirect points: it must never be asked anything.
    let (elsewhere, reached) = serve(|_| Answer::new(200, b"{}".to_vec()));
    let (port, seen) = serve(move |r| {
        let status = r.path.trim_start_matches('/').parse().unwrap_or(302);
        Answer { headers: vec![format!("Location: http://127.0.0.1:{elsewhere}/csc/v2/info")], ..Answer::new(status, Vec::new()) }
    });
    let t = transport();
    for status in [301, 302, 303, 307, 308] {
        let e = err(t.post(&format!("http://127.0.0.1:{port}/{status}"), &[("Authorization", "Bearer AT-1")], b"{}"));
        assert_eq!(e, format!("the address answers with a redirect (HTTP {status}), which PdfCraft doesn't follow: check it"));
    }
    assert_eq!(seen.lock().unwrap().len(), 5);
    assert_eq!(
        err(remote::list(&*t, &csc(format!("http://127.0.0.1:{port}/302")), mock::PASSWORD)),
        "the signing service is unreachable (the address answers with a redirect (HTTP 302), which PdfCraft doesn't follow: check it)"
    );
    assert!(reached.lock().unwrap().is_empty(), "a redirect was followed");
}

#[test]
fn answers_are_read_up_to_one_byte_over_the_cap() {
    // The size is the path's first part; `…/close` gives no length, and the answer ends when the
    // connection does.
    let (port, _) = serve(|r| {
        let size = r.path.trim_start_matches('/').split('/').next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Answer { length: !r.path.ends_with("/close"), ..Answer::new(200, vec![b' '; size]) }
    });
    let t = transport();
    let read = |path: String| t.post(&format!("http://127.0.0.1:{port}/{path}"), &[], b"").map(|r| r.body.len()).unwrap();
    assert_eq!(read(MAX_RESPONSE.to_string()), MAX_RESPONSE, "an answer that fits is read whole");
    for over in [MAX_RESPONSE + 1, 3 * MAX_RESPONSE] {
        assert_eq!(read(over.to_string()), MAX_RESPONSE + 1, "{over} bytes");
        assert_eq!(read(format!("{over}/close")), MAX_RESPONSE + 1, "{over} bytes until the connection closes");
    }
    // And the signing code refuses it.
    let account = csc(format!("http://127.0.0.1:{port}/{}", 3 * MAX_RESPONSE));
    assert_eq!(err(remote::list(&*t, &account, mock::PASSWORD)), "the signing service's answer is larger than 1 MiB, so it wasn't read");
}

#[test]
fn a_closed_port_is_unreachable() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let account = csc(format!("http://127.0.0.1:{port}/csc/v2"));
    assert_eq!(err(remote::list(&*transport(), &account, mock::PASSWORD)), "the signing service is unreachable (connection refused)");
}

#[test]
fn a_proxy_from_the_environment_isnt_used_on_this_computer() {
    // A proxy where nothing listens: a request sent through it fails. The environment is the
    // process's, so the signing test runs again in a child process with these set.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", "csc_lists_sends_the_otp_and_signs_over_http"]);
    for var in ["ALL_PROXY", "HTTPS_PROXY", "HTTP_PROXY", "all_proxy", "https_proxy", "http_proxy"] {
        child.env(var, format!("http://127.0.0.1:{dead}"));
    }
    let out = child.env_remove("NO_PROXY").env_remove("no_proxy").output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && said.contains("1 passed"), "{said}");
}
