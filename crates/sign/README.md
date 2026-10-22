# pdfcraft-sign

Layer L4: digital signatures (execution plan M9; ISO 32000-2 §12.8; PAdES, ETSI EN 319 142).

```rust
let id = pkcs12::open(&std::fs::read("me.p12")?, "password")?;          // a digital ID
let id = remote::digital_id(transport, &account, &credential, secrets)?; // or a signing service's
let signed = sign(&doc, &id, &SignOptions { page: 0, rect: Some(r), date, ..Default::default() })?;
for s in signatures(&doc, &bytes, &trust) {                                // list + validate
    println!("{}: {} — {:?}", s.field, s.summary(), s.details);
}
```

- **Formats, on RustCrypto primitives:** a small DER reader and writer (`der`), X.509
  certificates (`x509`), CMS SignedData (`cms`), and PKCS #12 digital ID files (`pkcs12`):
  PBES2 (PBKDF2 + AES/3DES), the legacy SHA-1 3DES/RC2 schemes, and MAC checks. Writing uses
  OpenSSL 3's defaults (AES-256-CBC, HMAC-SHA-256).
- **Keys:** RSA, ECDSA P-256 and P-384. Verification uses RustCrypto everywhere. RSA
  private-key operations (signing, key generation) use `aws-lc-rs` on native targets and are
  refused in the browser (ADR-0009: the `rsa` crate's Marvin advisory, RUSTSEC-2023-0071, has
  no fix, so `rsa` is only used to verify). ECDSA signs with deterministic nonces (RFC 6979).
- **Signing:** PAdES B-B (`ETSI.CAdES.detached`, signing-certificate-v2, SHA-256/384, no SHA-1).
  An existing unsigned field or a new one (visible with Acrobat's name-and-details appearance,
  or invisible); certification with DocMDP P=1/2/3. A visible signature can show a `Graphic`
  where the large name would be: drawn strokes, filled outlines (a typed signature) or RGBA
  pixels (an image XObject with a soft mask), checked and size-capped first. The document is written incrementally
  with a zero-filled `/Contents` and fixed-width `/ByteRange`, which are then patched in
  place. Encrypted documents are refused for now.
- **Timestamps (PAdES B-T):** the `timestamp` module builds and parses RFC 3161 requests,
  responses and TSTInfo tokens (size-capped, imprint- and signature-checked). `sign_with_timestamp`
  attaches the TSA's token as an unsigned attribute over the signature value; the transport is
  the caller's (`TimestampAuthority` — this crate never opens a socket). Validation verifies
  embedded tokens; when the TSA chains to the trust store it reports the trusted time in
  `SignatureInfo::timestamp_time`, which then anchors certificate-validity and revocation
  checks. An untrusted TSA's time is reported as an unverified timestamp and never used as the
  validation time (the signer's claimed time is). `timestamp::respond` is the TSA-side signer behind the
  deterministic test authority.
- **Document timestamps and LTV:** `timestamp_document` appends a standalone RFC 3161
  document timestamp (`/ETSI.RFC3161`) covering the whole file; validation discovers and
  verifies these dictionaries (imprint over the signed bytes, token signature, TSA cert
  validity). `dss::embed` merges revocation evidence into the catalog's `/DSS` with `/VRI`
  entries keyed per signature (uppercase-hex SHA-1 of `/Contents`), deduplicating
  byte-identical blobs — sign → DSS → timestamp makes a B-LTA file, and the change classifier
  treats the store as a permitted change: only objects reached through `/Certs`, `/CRLs`,
  `/OCSPs` and `/VRI` with the shape of their role, and new or only grown since the signature,
  count (a `/DSS` entry naming a page's contents, or a `/Type /DSS` label, does not).
  `revocation` parses and verifies RFC 5280 CRLs and
  RFC 6960 OCSP responses (responder identity, OCSP-signing EKU for delegated responders,
  CertID hash matching, validity windows); validation checks embedded evidence against the
  signer's chain and a verified revocation invalidates the signature.

Not yet: revocation fetching (AIA/CRLDP extraction and a fetcher), timestamp-server
configuration, FieldMDP locks, certificate security, direct PKCS #11 access to tokens.
macOS Keychain and Windows Current User Personal (My) store identities sign through
`ExternalKey` without exporting private keys. Windows CNG supports RSA PKCS #1 v1.5 and
ECDSA P-256/P-384; the store integration is tested with software-backed keys. A smart card
or token whose driver registers a CNG key storage provider is such an identity too: the key
is opened for signing without the silent flag, so the provider shows its own PIN dialog
(checked by hand with a SafeNet token; CI has no hardware). A store identity signs with the
issuers Windows chains its certificate to, without the root. Store certificates that can't
sign (no private key, another key type, a key CNG can't open) are returned by
`windows::list` as `unusable` with the reason and the Windows error code, shown greyed in
the Sign dialog and by `sign_windows_ids`, instead of being left out silently; a key that
needs a prompt to open is listed and asks at signing (issue #179).
- **Remote signing (`remote`, "firma remota"):** a digital ID whose key a signing service holds,
  each signature authorized with a one-time password (OTP), as Italian and EU qualified
  signatures are made. Two protocols, from their public specifications: the Cloud Signature
  Consortium API v2 (2.0–2.2), with the v1 (1.0.4) field names tolerated (`info`, HTTP Basic
  `auth/login`, `credentials/list` and `credentials/info`, the OTP through `credentials/sendOTP`
  or `credentials/getChallenge`, `credentials/authorize`, `signatures/signHash`, `auth/revoke`),
  and Aruba Remote Sign Server (SOAP 1.1 from its WSDL: `listCert`, `sendCredential` by SMS,
  `signhash`). `remote::list` signs in and lists the credentials PdfCraft can sign with (an RSA,
  P-256 or P-384 key whose certificate allows signing documents) and what a signature asks for: a
  PIN, an OTP typed from an app or token, or one the service sends (`remote::send_otp`).
  `remote::digital_id` gives a `DigitalId` that `sign` uses like any other; a `remote:` reference
  names the service, user and credential, and no secret. The PDF, its byte range and the CMS are
  built here, and the service is sent only the digest of the CMS signed attributes, besides the
  sign-in, the credential's name, the PIN and the OTP: the document never leaves the machine.
  The service password, PIN, OTP and the token and SAD a service hands back are never stored or
  logged, `Debug` prints none of them, and a service's error text is quoted cut to 200
  characters with them taken out. The network is the caller's, as for timestamps: this crate
  hands each exchange to a `remote::HttpTransport`. The desktop app and `pdfcraft-cli run` pass
  `pdfcraft-platform`'s `https` transport (the system's certificate authorities, no redirects,
  15 s to connect and 150 s an exchange, since a CSC service may hold an authorization open
  until the user confirms it on a phone), and `pdfcraft-cli mcp` only with `--remote-signing`;
  the web build has none, so remote IDs are greyed there. Addresses must be `https://` (plain `http://`
  only to this computer), and an Aruba account needs its domain (`typeOtpAuth`, the "dominio"),
  without which Aruba answers "Invalid Parameters": it is refused before anything is sent. The
  Sign dialog's Remote signature (OTP) option and the `sign_remote_ids`, `sign_remote_send_otp`
  and `sign_document` tools sit on top. Their errors begin with sentences written once in
  `remote` (`WORDINGS` lists the ones an interface translates), and `remote::fault_of` tells
  from that sentence whether the caller has to change the call or the service failed, so the
  automation tools report the two apart without a word list of their own.
- **Where the protocols come from.** Clean room (AGENTS.md §2): the CSC API specifications
  v2.2.0.0, v2.0.0.2 and v1.0.4.0 and the CSC Data Model v1.0.0, as published by the Cloud
  Signature Consortium; Aruba's public ARSS WSDL, with Aruba PEC's certification practice
  statement MGA_A_66 v1.13 and its Firma Remota manual v0.3 for what the messages mean; and,
  since Aruba publishes no API manual, the answers its public endpoint gave unauthenticated calls
  with made-up values on 2026-10-21 (no account): the refusals the tests use, and that the domain
  is required. No other CSC or ARSS client was read, nor code samples that turned up in search
  results. `remote`'s module documentation lists the addresses and the sections followed.
- **How a CSC credential is authorized.** Its authentication objects are told apart by type: a
  `Password` with a generator, or whose id or label names it a code ("OTP", "one-time", "SMS",
  or "code", "codice", "token" without "PIN"), is a code even with no PIN beside it, as is a
  `ChallengeResponse`; of the other Passwords the one named PIN (else the first) is the PIN and
  the rest are codes, so a PIN and a second value never get the PIN twice; `PasswordOOB` and
  `ChallengeResponseOOB` are confirmed on another device. `auth/expression` is read (`AND` before
  `OR` and `XOR`, nested eight deep at most) and decides what `credentials/authorize` sends: all
  of an `AND`, and of an alternative one operand only, the first the user gave a value for, so an
  `XOR` never gets both; a confirmation on the phone (and its `getChallenge`) only when no code
  that would do was given, which `AuthNeeds::confirm` tells an interface. One PIN and one code
  are asked for, so of two codes offered as alternatives (an SMS or an app) Send code
  (`remote::send_otp`) asks for the one a signature fills with a code the service sent
  (`Secrets::otp_sent`): the SMS, whichever comes first. A code nobody had sent goes under the
  first. The user name and the Aruba domain go trimmed, as they are checked. A credential with an
  object of another type, an expression that can't be read or names no object of it, or a key
  whose `key/algo` lists nothing PdfCraft signs with (RSA PKCS #1 v1.5, ECDSA with SHA-256, or
  SHA-384 for P-384) can't sign: it is left out of the list (which names the reason when nothing
  else can sign), and asking for its code or signing with it fails with that reason before a
  code is sent or anything is authorized.
- **A signing service is checked, not trusted.** Every signature it returns is verified against
  the certificate before it is used: RSA is brought to the modulus' length (leading zeros taken
  off or put back, anything else refused), raw ECDSA `r ‖ s` is turned into DER, and a signature
  that doesn't verify fails the signing instead of making a broken file, saying so when the
  service signed the bare digest without its DigestInfo. One OTP authorizes one signature: the
  ID refuses a second. A CSC sign-in is renewed once, and only when the service says the token
  is no longer good (`invalid_token`, `expired_token`, or a 401 with no error code); a 401 naming
  another error is that error. `credentials/authorize`, `sendOTP` and `getChallenge` are never
  sent twice, so a retry can't count as one more wrong PIN or void the code the user just got. An
  authorization confirmed on another device (HTTP 202) is polled for at most a minute, and an
  answer over 1 MiB is refused. Not done: OAuth sign-in (a service or credential that needs it
  is reported as such), showing a challenge while signing (a `ChallengeResponseOOB` credential
  whose service answers with one is refused with that reason), Aruba sessions (several
  signatures on one OTP), timestamps from the service itself, and revoking the CSC sign-in of a
  remote ID opened but never signed with (it expires on the service), and the issuers of an
  Aruba certificate: `listCert` gives the signer's alone, so the file carries no chain and a
  validator that doesn't fetch issuers needs Aruba's CA trusted, not only its root. Tested only
  against the in-process services of `remote::mock` (feature `mock`: CSC v2, CSC v1, ARSS, a CA
  chain, authentication expressions, and their failure modes) and, for the transport, loopback
  HTTP and HTTPS servers: no real provider has been tried yet.
- **Validation:** `/ByteRange` and the CMS are read from the file's own bytes; the digest,
  the signature value and the signer's chain (against a `TrustStore`) are checked. Later
  revisions are diffed against the signed one, and the changes are classified (signing, form
  fill, comments, metadata, page content, document structure) under the DocMDP permissions.
  The verdict follows Acrobat: valid, unknown (intact but the identity isn't trusted) or invalid.
- **Chains are checked, not just linked.** `x509::build_chain` only takes an issuer whose key
  verifies the certificate below it *and* that may issue: `basicConstraints` CA:TRUE (an old v1
  self-signed root without constraints counts), `keyCertSign` when it has a key usage, a
  `pathLenConstraint` that allows the CAs below it, and, given the signing time, validity then.
  Certificates embedded in a document are in the pool too, so an ordinary subscriber certificate
  can never vouch for another one; when an issuer is refused, the signature's details say why.
- **The signer's certificate must allow signing documents** (`Certificate::signing_problem`): a
  key usage needs digitalSignature or nonRepudiation, an extended key usage needs a
  document-signing purpose (any, emailProtection, codeSigning, RFC 9336 documentSigning, Adobe
  Authentic Documents, Microsoft document signing), and no critical extension may be one PdfCraft
  doesn't process. Otherwise the verdict is unknown, never valid, and the details say why.
- **BER as well as DER.** The CMS is read with `der::Tlv::parse_ber`, as Windows CryptoAPI, Adobe
  PPKMS, DocuSign, Documenso and `openssl cms -stream` write it: indefinite lengths and an
  OCTET STRING split into segments. Only the structure is read that way; certificates and signed
  attributes are still verified over their own exact bytes (signed attributes written with an
  indefinite length are re-encoded as DER first, as RFC 5652 §5.4 has them signed). Nesting is
  bounded, and the details say that the signature was BER. High tag numbers are not read.
- **One content type, one message digest.** A CMS whose signed attributes repeat the content type
  or the message digest, or give either of them more or fewer than one value, is not read (RFC
  5652 §11.1, §11.2). Neither is one whose content-type attribute differs from `eContentType`:
  the signature covers the attribute but not `eContentType`. Both content types must be object
  identifiers, and a PDF signature's must be data. A signature with signed attributes but no
  content-type attribute is read anyway, with a note, although §11.1 requires one: then
  `eContentType` alone says that the content is data.
- **Later changes are classified narrowly.** Only the catalog's own `/Metadata` stream is the
  document's XMP, and only objects reached through the `/DSS` keys above are the security store. A
  `/Type /Metadata` or `/Type /DSS` *label* on any other dictionary or stream (a page's Form
  XObject, say) makes nothing permitted, at every DocMDP level.
- **Strict in what it checks, tolerant where signers differ.** RSA PKCS #1 signatures are checked
  against the one encoding the digest should have, built here and compared byte for byte; the
  `DigestInfo` may omit its NULL parameter, as older signers write it (RFC 8017 App. B.1), but a
  signature over the bare digest, which doesn't say which hash it uses, is refused. A signature
  value must be below the modulus (RFC 8017 §5.2.2). RSA-PSS uses the salt length it declares.
  ECDSA is read as DER, non-minimal DER or raw `r ‖ s`. `adbe.x509.rsa_sha1` is validated. What the
  check tolerated is listed in the signature's details: a DigestInfo without its NULL, an RSA value
  that isn't modulus-length, ECDSA that isn't canonical DER, a SignerInfo whose digest algorithm
  differs from the one the signature algorithm names. What PdfCraft can't check (an unknown
  algorithm or curve) is *unknown*, never *invalid*.
- **Weak algorithms still validate, and say so.** SHA-1 and RIPEMD-160 signatures are common in
  documents signed years ago, so they validate (new signatures never use SHA-1), but the signature's
  details say that the algorithm is weak and shouldn't be relied on.
- **Validation algorithms:** digests SHA-1, SHA-224/256/384/512, SHA-512/224, SHA-512/256,
  SHA3-224/256/384/512 and RIPEMD-160. RSA PKCS #1 v1.5 and RSASSA-PSS (the hash, MGF1 hash and
  salt length the signature declares), ECDSA on P-256, P-384, P-521, brainpoolP256r1, P384r1 and
  P512r1 (also BSI "plain" ECDSA), and Ed25519 (RFC 8419, over the signed attributes).
  Signing: RSA, P-256, P-384 with SHA-256/384/512. The RSA padding checks and brainpoolP512r1 are
  written here (`rsa_pad`, `ec512`) on `crypto-bigint`, because the typed APIs of the `rsa` crate
  and RustCrypto don't cover them.

Oracles: poppler's `pdfsig` reports our signatures valid; OpenSSL reads our `.p12` files and
verifies our CMS; `tests/data/openssl-signed.pdf` is a signature OpenSSL made, which we validate.
