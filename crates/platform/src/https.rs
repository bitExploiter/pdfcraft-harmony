//! The network remote signing goes through. `pdfcraft-sign` builds every request and never opens a
//! socket ([`HttpTransport`]); the desktop app and the command-line tool hand it this transport,
//! which is the only one that reaches a signing service.

use std::io::Read as _;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use pdfcraft_sign::SignError;
use pdfcraft_sign::remote::{HttpResponse, HttpTransport, MAX_RESPONSE, check_url};

/// The longest connecting (with the TLS handshake) may take: a service that is up answers that
/// at once, and one that isn't is said to be unreachable without a long wait.
const CONNECT: Duration = Duration::from_secs(15);

/// The longest one exchange may take, from connecting to the end of the answer. A CSC service
/// may hold `credentials/authorize` (or `authorizeCheck`) open until the user has confirmed the
/// signature on a phone, rather than answer 202 and be asked again (S22 §11.8: "a polling or
/// blocking approach"), so the user gets two and a half minutes for that.
const EXCHANGE: Duration = Duration::from_secs(150);

/// HTTPS to a signing service, checked against the certificate authorities the operating system
/// trusts, or plain HTTP to this computer (what [`check_url`] allows, refused otherwise before
/// connecting). Redirects aren't followed and an answer is read to at most [`MAX_RESPONSE`] and
/// one byte more. Cheap: nothing is loaded until the first request.
pub fn transport() -> Arc<dyn HttpTransport> {
    Arc::new(Https::new(CONNECT, EXCHANGE))
}

struct Https {
    connect: Duration,
    exchange: Duration,
    /// The certificate authorities trusted instead of the system's: a test's own.
    roots: Option<Vec<ureq::tls::Certificate<'static>>>,
    /// The agent, and whether any certificate authority is trusted. Loading the system's roots
    /// can take a while (macOS reads its trust settings), so it waits for the first request.
    client: OnceLock<(ureq::Agent, bool)>,
}

impl Https {
    fn new(connect: Duration, exchange: Duration) -> Https {
        Https { connect, exchange, roots: None, client: OnceLock::new() }
    }

    /// Trusting only `roots`, as a test serving HTTPS with its own certificate does.
    #[cfg(test)]
    fn trusting(roots: &[&[u8]]) -> Https {
        let roots = roots.iter().map(|der| ureq::tls::Certificate::from_der(der).to_owned()).collect();
        Https { roots: Some(roots), ..Https::new(CONNECT, EXCHANGE) }
    }

    fn client(&self) -> &(ureq::Agent, bool) {
        self.client.get_or_init(|| {
            let roots = self.roots.clone().unwrap_or_else(os_roots);
            let agent = ureq::Agent::config_builder()
                .timeout_connect(Some(self.connect))
                .timeout_global(Some(self.exchange))
                // A redirect leads to an address `check_url` never saw, and neither protocol uses one.
                .max_redirects(0)
                // The protocols put their errors in the body of a 4xx or 5xx answer.
                .http_status_as_error(false)
                .user_agent(concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
                .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::new_with_certs(&roots)).build())
                .build()
                .new_agent();
            (agent, !roots.is_empty())
        })
    }
}

impl HttpTransport for Https {
    fn post(&self, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, SignError> {
        // The protocols check the address too; this is the last place before the network.
        check_url(url)?;
        let https = url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"));
        let (agent, trusted) = self.client();
        if https && !trusted {
            return Err(SignError::Crypto("no trusted certificate authorities were found on this system".into()));
        }
        let mut request = agent.post(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if !https {
            // Plain HTTP is allowed only because it stays on this computer: a proxy from the
            // environment would carry it, and the password in it, across the network.
            request = request.config().proxy(None).build();
        }
        let mut response = request.send(body).map_err(|e| SignError::Crypto(reason(&e)))?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(SignError::Crypto(format!("the address answers with a redirect (HTTP {status}), which PdfCraft doesn't follow: check it")));
        }
        // One byte past the cap is how the protocols tell an answer too large from one that fits.
        let limit = u64::try_from(MAX_RESPONSE).unwrap_or(u64::MAX).saturating_add(1);
        let mut answer = Vec::new();
        response.body_mut().as_reader().take(limit).read_to_end(&mut answer).map_err(|e| SignError::Crypto(reason(&ureq::Error::from(e))))?;
        Ok(HttpResponse { status, body: answer })
    }
}

/// The certificate authorities the operating system trusts. Stores it can't read are skipped.
fn os_roots() -> Vec<ureq::tls::Certificate<'static>> {
    rustls_native_certs::load_native_certs().certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect()
}

/// Why an exchange failed, in words of our own: nothing sent is quoted, because the headers carry
/// the password (HTTP Basic) or an access token.
fn reason(e: &ureq::Error) -> String {
    use std::io::ErrorKind;
    use ureq::Error as E;
    match e {
        E::Io(io) => match (io.kind(), io.get_ref()) {
            // How rustls reports a certificate it refuses ("invalid peer certificate: UnknownIssuer").
            (ErrorKind::InvalidData, Some(inner)) => inner.to_string(),
            (ErrorKind::UnexpectedEof, _) => "the connection closed before the answer ended".into(),
            (kind, _) => kind.to_string(),
        },
        E::Timeout(_) => "timed out".into(),
        E::HostNotFound => "host not found".into(),
        E::Rustls(tls) => tls.to_string(),
        E::Tls(_) | E::TlsRequired => "the secure connection failed".into(),
        E::Protocol(_) | E::LargeResponseHeader(..) => "its answer isn't valid HTTP".into(),
        E::ConnectProxyFailed(_) | E::InvalidProxyUrl => "the proxy set in the environment (HTTPS_PROXY, ALL_PROXY) failed".into(),
        E::Http(_) => "a request header isn't valid".into(),
        E::BadUri(_) => "the address isn't valid".into(),
        _ => "the connection failed".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use pdfcraft_sign::der::{self, Time};
    use pdfcraft_sign::remote::mock::{self, MockService};
    use pdfcraft_sign::remote::{self, Account, Protocol, Secrets};
    use pdfcraft_sign::{DigestAlg, Name, PrivateKey};

    use super::*;

    fn err(r: Result<HttpResponse, SignError>) -> String {
        r.unwrap_err().to_string()
    }

    /// A self-signed certificate for a TLS server at 127.0.0.1 (its subjectAltName) named `cn`,
    /// and its key as PKCS #8.
    fn server_identity(cn: &str) -> (Vec<u8>, Vec<u8>) {
        let key = PrivateKey::generate_p256().unwrap();
        let name = Name::build(cn, "", "PdfCraft tests", "", "IT");
        let alg = key.preferred_digest();
        let sig_alg = key.signature_algorithm(alg);
        let ext = |oid: &str, value: &[u8]| der::seq(&[&der::oid(oid), &der::octets(value)]);
        // subjectAltName iPAddress ([7]) 127.0.0.1; extKeyUsage serverAuth.
        let extensions = der::seq(&[
            &ext("2.5.29.17", &der::seq(&[&der::tlv(0x87, &[127, 0, 0, 1])])),
            &ext("2.5.29.37", &der::seq(&[&der::oid("1.3.6.1.5.5.7.3.1")])),
        ]);
        let from = Time { year: 2000, month: 1, day: 1, hour: 0, minute: 0, second: 0 };
        let until = Time { year: 2099, month: 12, day: 31, hour: 0, minute: 0, second: 0 };
        let tbs = der::seq(&[
            &der::explicit(0, &der::int(2)),
            &der::uint(&[7]),
            &sig_alg,
            &name.raw,
            &der::seq(&[&from.encode(), &until.encode()]),
            &name.raw,
            &key.public_key().spki(),
            &der::explicit(3, &extensions),
        ]);
        let signature = key.sign(alg, &tbs).unwrap();
        (der::seq(&[&tbs, &sig_alg, &der::bit_string(&signature)]), key.pkcs8().to_vec())
    }

    /// An HTTPS server on 127.0.0.1 with `identity`, passing each request to `mock` (which
    /// answers at `https://rssp.example`): its port, and how many requests it read.
    fn serve_tls(identity: &(Vec<u8>, Vec<u8>), mock: Arc<MockService>) -> (u16, Arc<AtomicUsize>) {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(identity.0.clone())], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.1.clone())))
            .unwrap();
        let config = Arc::new(config);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let read = Arc::new(AtomicUsize::new(0));
        let count = read.clone();
        std::thread::spawn(move || {
            for tcp in listener.incoming().flatten() {
                let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
                let Ok(conn) = rustls::ServerConnection::new(config.clone()) else { continue };
                let mut tls = rustls::StreamOwned::new(conn, tcp);
                // The handshake happens at the first read: a client that refuses the certificate
                // ends it there, before anything of the request is read.
                let mut r = BufReader::new(&mut tls);
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let path = line.split(' ').nth(1).unwrap_or_default().to_string();
                let mut headers = Vec::new();
                loop {
                    line.clear();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim_end().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.trim_end().split_once(':') {
                        headers.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
                let length = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
                let mut body = vec![0; length];
                if r.read_exact(&mut body).is_err() {
                    continue;
                }
                count.fetch_add(1, Ordering::SeqCst);
                let h: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                let a = mock.post(&format!("https://rssp.example{path}"), &h, &body).unwrap();
                let head = format!("HTTP/1.1 {} Answer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", a.status, a.body.len());
                let _ = tls.write_all(head.as_bytes()).and_then(|()| tls.write_all(&a.body));
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        });
        (port, read)
    }

    fn account(port: u16) -> Account {
        Account { protocol: Protocol::Csc, url: format!("https://127.0.0.1:{port}/csc/v2"), user: mock::USER.into(), domain: None, hsm: None }
    }

    #[test]
    fn a_service_whose_certificate_is_trusted_signs_over_https() {
        let identity = server_identity("127.0.0.1");
        let mock = Arc::new(MockService::csc_v2().unwrap());
        let (port, read) = serve_tls(&identity, mock.clone());
        let t = Arc::new(Https::trusting(&[&identity.0]));
        let ids = remote::list(&*t, &account(port), mock::PASSWORD).unwrap();
        assert_eq!(ids.iter().map(|i| i.credential.as_str()).collect::<Vec<_>>(), [mock::RSA, mock::P256]);
        let secrets = Secrets { password: mock::PASSWORD.into(), pin: None, otp: Some(mock.current_otp()), otp_sent: false };
        let id = remote::digital_id(t, &account(port), mock::P256, secrets).unwrap();
        let sig = id.key.sign(DigestAlg::Sha256, b"signed attributes").unwrap();
        let digest = DigestAlg::Sha256.digest(&[b"signed attributes"]);
        assert!(id.certificate.public_key.verify(pdfcraft_sign::keys::Scheme::Ecdsa, DigestAlg::Sha256, &digest, &sig).unwrap());
        assert_eq!(read.load(Ordering::SeqCst), mock.requests().len());
    }

    #[test]
    fn a_service_whose_certificate_isnt_trusted_is_told_nothing() {
        let identity = server_identity("127.0.0.1");
        let mock = Arc::new(MockService::csc_v2().unwrap());
        let (port, read) = serve_tls(&identity, mock.clone());
        let url = format!("https://127.0.0.1:{port}/csc/v2/auth/login");
        let basic = [("Authorization", "Basic bWFyaW8ucm9zc2k6aHVudGVyMg==")];
        // Another certificate authority, and the system's: neither vouches for this one.
        let e = err(Https::trusting(&[&server_identity("another").0]).post(&url, &basic, b"{}"));
        assert!(e.contains("certificate") && !e.contains("bWFy"), "{e}");
        let e = err(transport().post(&url, &basic, b"{}"));
        assert!(e.contains("certificate") || e == "no trusted certificate authorities were found on this system", "{e}");
        let e = remote::list(&Https::trusting(&[&server_identity("another").0]), &account(port), mock::PASSWORD).unwrap_err().to_string();
        assert!(e.starts_with("the signing service is unreachable (") && e.contains("certificate"), "{e}");
        assert_eq!(read.load(Ordering::SeqCst), 0, "not a byte of a request was read");
        assert!(mock.requests().is_empty());
    }

    #[test]
    fn with_no_certificate_authority_https_isnt_tried() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("https://127.0.0.1:{}/csc/v2/info", listener.local_addr().unwrap().port());
        assert_eq!(err(Https::trusting(&[]).post(&url, &[], b"{}")), "no trusted certificate authorities were found on this system");
        assert_eq!(listener.accept().map(|_| ()).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn connecting_has_a_shorter_limit_than_the_exchange() {
        // A blocking authorize gets minutes; a TLS handshake that never comes, seconds.
        assert!(EXCHANGE >= Duration::from_secs(120) && CONNECT <= Duration::from_secs(15));
        // Connections queue in the backlog unaccepted, so the handshake never starts.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("https://127.0.0.1:{}/csc/v2/info", listener.local_addr().unwrap().port());
        let t = Https { connect: Duration::from_millis(300), exchange: Duration::from_secs(20), ..Https::trusting(&[&server_identity("x").0]) };
        let started = Instant::now();
        assert_eq!(err(t.post(&url, &[], b"{}")), "timed out");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    }

    #[test]
    fn only_addresses_check_url_allows_are_contacted() {
        let t = transport();
        // Refused before connecting: a listener on this computer, at an address check_url refuses
        // only for its user name, sees no connection.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let e = err(t.post(&format!("http://user:pw@127.0.0.1:{port}/csc/v2/info"), &[], b"{}"));
        assert_eq!(e, "the signing service address must not contain a user name or password");
        assert_eq!(listener.accept().map(|_| ()).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);

        assert!(check_url("https://example.com").is_ok());
        let refused = check_url("http://example.com").unwrap_err().to_string();
        assert!(refused.contains("must start with https://"), "{refused}");
        assert_eq!(err(t.post("http://example.com/csc/v2/info", &[], b"{}")), refused);
    }

    #[test]
    fn a_service_that_never_answers_times_out() {
        // Connections queue in the backlog without being accepted, so the request is sent and
        // nothing ever comes back.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/csc/v2/info", listener.local_addr().unwrap().port());
        let quick = Duration::from_millis(300);
        assert_eq!(err(Https::new(quick, quick).post(&url, &[], b"{}")), "timed out");
    }

    #[test]
    fn a_header_that_cant_be_sent_isnt_quoted() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/csc/v2/info", listener.local_addr().unwrap().port());
        let quick = Duration::from_millis(300);
        let e = err(Https::new(quick, quick).post(&url, &[("Authorization", "Bearer AT-1\r\nX-Secret: hunter2")], b"{}"));
        assert_eq!(e, "a request header isn't valid");
    }
}
