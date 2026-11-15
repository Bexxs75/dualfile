//! Real HTTP(S) transport for the WebDAV backend. A thin [`WebdavTransport`] over `ureq`, which runs
//! on the same rustls + ring stack as the FTP/SFTP back ends (no OpenSSL, no native-tls). It adds
//! Basic authentication, percent-encodes the request path, follows no redirects (so a hostile server
//! cannot bounce the client to another host or leak the credentials), and bounds every call with a
//! timeout. For `https`, the agent is built with a rustls `ClientConfig` that carries the shared
//! pinning/trust verifier (`tls::webdav_client_config` — system roots plus a fingerprint pin with
//! user confirmation, with session resumption disabled); plain `http` carries no TLS.
//!
//! Security notes: 4xx/5xx come back as responses (`ureq::Error::Status`), which [`WebdavVfs`] maps
//! itself; `redirects(0)` returns a 3xx as a plain response instead of following it; the response
//! body stays streamed and is bounded by the caller (`read_bounded` for PROPFIND, the shared copy
//! loop for GET), so a lying server cannot flood memory. A refused certificate fails the request;
//! the connect probe recovers the trust reason via the verifier's `take_error`.
//!
//! Known limitation (WDAV-TLS-002): a certificate refused on a *later* request (e.g. after the server
//! rotated its cert and a pooled connection re-handshakes) surfaces only as `Disconnected`, not as a
//! distinct trust error — fail-safe (the bad cert is still refused), but the operation-level reason
//! is coarse. Carrying the trust reason through every operation is a follow-up.

use crate::webdavvfs::{WebdavResponse, WebdavTransport};
use dualfile_core::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const USER_AGENT: &str = "Dualfile-WebDAV/0.1";
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// A WebDAV HTTP transport bound to one origin (`scheme://host[:port]`). Paths handed to
/// [`WebdavTransport::request`] are the server paths already joined with the connection base; this
/// adapter percent-encodes them and appends them to the origin.
pub struct UreqTransport {
    agent: ureq::Agent,
    origin: String,
    /// Pre-built `Basic <base64>` header value, or `None` for an anonymous connection.
    auth: Option<String>,
    /// Set once the connection is closed. Because WebDAV is stateless HTTP there is no socket to
    /// drop, so a disconnect must be enforced here: every later request — including a pending
    /// write's commit, which still holds a clone of this transport — is refused.
    closed: Arc<AtomicBool>,
}

impl UreqTransport {
    /// `origin` is `scheme://host[:port]` (a trailing slash is trimmed). Credentials, when given, are
    /// sent as HTTP Basic auth on every request. Plain transport: `https` uses ureq's default roots.
    pub fn new(origin: &str, user: Option<&str>, pass: Option<&str>) -> Self {
        Self::new_tls(origin, user, pass, None)
    }

    /// Like [`new`](Self::new), but with an explicit rustls `ClientConfig` for TLS — carrying the
    /// pinning/trust verifier (`tls::webdav_client_config`). `None` keeps ureq's default TLS (used for
    /// plain `http`, where it is never consulted).
    pub fn new_tls(
        origin: &str,
        user: Option<&str>,
        pass: Option<&str>,
        tls_config: Option<Arc<rustls::ClientConfig>>,
    ) -> Self {
        let mut builder = ureq::AgentBuilder::new()
            .redirects(0) // never follow: no cross-host bounce, no credential leak
            .timeout(CALL_TIMEOUT)
            .user_agent(USER_AGENT);
        if let Some(config) = tls_config {
            builder = builder.tls_config(config);
        }
        let auth = user.map(|u| {
            let raw = format!("{u}:{}", pass.unwrap_or(""));
            format!("Basic {}", base64_encode(raw.as_bytes()))
        });
        Self {
            agent: builder.build(),
            origin: origin.trim_end_matches('/').to_owned(),
            auth,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl WebdavTransport for UreqTransport {
    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<WebdavResponse> {
        // A disconnect revokes every not-yet-sent request, so a queued/paused job or a pending write
        // commit cannot keep acting on the server after the connection was closed.
        if self.closed.load(Ordering::Relaxed) {
            return Err(Error::Disconnected);
        }
        let uri = format!("{}{}", self.origin, encode_path(path));
        let mut request = self.agent.request(method, &uri);
        if let Some(auth) = &self.auth {
            request = request.set("Authorization", auth);
        }
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("destination") {
                // The `Destination` header (MOVE/COPY) arrives as a decoded server path, exactly like
                // the request path. Encode it the same way and make it an absolute URI on the bound
                // origin, so a target name containing a space, `#`, `?` or a literal `%` cannot be
                // reinterpreted by the server into a different resource than the VFS confirmed.
                let destination = format!("{}{}", self.origin, encode_path(value));
                request = request.set(name, &destination);
            } else {
                request = request.set(name, value);
            }
        }
        let result = match body {
            Some(bytes) => request.send_bytes(&bytes),
            None => request.call(),
        };
        // A 4xx/5xx comes back as `Error::Status` with the response attached; the VFS maps the status
        // itself, so unwrap the response out of it. Any transport-level failure (connect, TLS,
        // timeout, protocol) — and, with redirects(0) disabled, nothing else — is a lost connection.
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(_)) => return Err(Error::Disconnected),
        };
        let status = response.status();
        Ok(WebdavResponse {
            status,
            body: Box::new(response.into_reader()),
        })
    }
}

/// Percent-encodes a server path, keeping `/` so the path structure survives. The VFS path already
/// holds decoded names, so this is the single place encoding happens; the unreserved set (RFC 3986)
/// plus `/` passes through, everything else becomes `%XX`.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        match b {
            b'/' | b'-' | b'.' | b'_' | b'~' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push(hex(b >> 4));
                out.push(hex(b & 0x0f));
            }
        }
    }
    out
}

fn hex(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + (nibble - 10)) as char,
    }
}

/// Standard base64 (RFC 4648) for the Basic-auth header. Small and dependency-free; the input is a
/// short `user:pass` string.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    /// What the mock server saw on the one request it handled.
    struct Captured {
        method: String,
        target: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    /// Spawns a one-shot HTTP/1.1 server on loopback that records the request and replies with
    /// `response_bytes`. Returns the origin (`http://127.0.0.1:<port>`) and a receiver for the
    /// captured request. Real sockets, so the actual `ureq` wire behaviour is exercised.
    fn mock_server(response_bytes: &'static [u8]) -> (String, mpsc::Receiver<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let captured = handle(stream, response_bytes);
            let _ = tx.send(captured);
        });
        (origin, rx)
    }

    fn handle(mut stream: TcpStream, response_bytes: &[u8]) -> Captured {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_owned();
        let target = parts.next().unwrap_or("").to_owned();
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim().to_owned();
                if k == "content-length" {
                    content_length = v.parse().unwrap_or(0);
                }
                headers.push((k, v));
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body).unwrap();
        }
        stream.write_all(response_bytes).unwrap();
        stream.flush().unwrap();
        Captured {
            method,
            target,
            headers,
            body,
        }
    }

    const OK_HALLO: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhallo";

    #[test]
    fn get_returns_status_and_body() {
        let (origin, rx) = mock_server(OK_HALLO);
        let t = UreqTransport::new(&origin, None, None);
        let resp = t.request("GET", "/dav/datei.txt", &[], None).unwrap();
        assert_eq!(resp.status, 200);
        let mut out = String::new();
        let mut body = resp.body;
        body.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hallo");
        let cap = rx.recv().unwrap();
        assert_eq!(cap.method, "GET");
        assert_eq!(cap.target, "/dav/datei.txt");
    }

    #[test]
    fn authorization_header_is_sent_as_basic() {
        let (origin, rx) = mock_server(OK_HALLO);
        let t = UreqTransport::new(&origin, Some("user"), Some("pass"));
        t.request("GET", "/dav/", &[], None).unwrap();
        let cap = rx.recv().unwrap();
        // base64("user:pass") == "dXNlcjpwYXNz"
        assert!(cap
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Basic dXNlcjpwYXNz"));
    }

    #[test]
    fn custom_method_headers_and_body_reach_the_server() {
        let (origin, rx) = mock_server(
            b"HTTP/1.1 207 Multi-Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let t = UreqTransport::new(&origin, None, None);
        let resp = t
            .request(
                "PROPFIND",
                "/dav/ordner/",
                &[("depth", "1")],
                Some(b"<propfind/>".to_vec()),
            )
            .unwrap();
        assert_eq!(resp.status, 207);
        let cap = rx.recv().unwrap();
        assert_eq!(cap.method, "PROPFIND");
        assert_eq!(cap.target, "/dav/ordner/");
        assert!(cap.headers.iter().any(|(k, v)| k == "depth" && v == "1"));
        assert_eq!(cap.body, b"<propfind/>");
    }

    #[test]
    fn move_destination_header_is_encoded_and_origin_absolute() {
        // WD-3A-01: the Destination for MOVE must be encoded like the request path (a space becomes
        // %20, a literal percent becomes %25) and be an absolute URI on the bound origin, so it
        // cannot address a different resource than the VFS confirmed.
        let (origin, rx) =
            mock_server(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let t = UreqTransport::new(&origin, None, None);
        t.request(
            "MOVE",
            "/dav/quelle.txt",
            &[("Destination", "/dav/ziel a%20b.txt"), ("Overwrite", "F")],
            None,
        )
        .unwrap();
        let cap = rx.recv().unwrap();
        assert_eq!(cap.method, "MOVE");
        let dest = cap
            .headers
            .iter()
            .find(|(k, _)| k == "destination")
            .map(|(_, v)| v.as_str())
            .unwrap();
        // Space -> %20, the literal "%" in the filename -> %25 (so "a%20b" stays "a%2520b").
        assert_eq!(dest, format!("{origin}/dav/ziel%20a%2520b.txt"));
        assert!(cap
            .headers
            .iter()
            .any(|(k, v)| k == "overwrite" && v == "F"));
    }

    #[test]
    fn path_with_spaces_and_unicode_is_percent_encoded() {
        let (origin, rx) = mock_server(OK_HALLO);
        let t = UreqTransport::new(&origin, None, None);
        t.request("GET", "/dav/a b/\u{00e4}.txt", &[], None)
            .unwrap();
        let cap = rx.recv().unwrap();
        // Space -> %20, "ä" (U+00E4 -> UTF-8 C3 A4) -> %C3%A4; slashes kept.
        assert_eq!(cap.target, "/dav/a%20b/%C3%A4.txt");
    }

    #[test]
    fn a_redirect_is_returned_not_followed() {
        // If ureq followed this, it would try to connect to the (dead) Location host and the result
        // would not be a clean 302. max_redirects(0) must hand the 302 straight back.
        let (origin, _rx) = mock_server(
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let t = UreqTransport::new(&origin, None, None);
        let resp = t.request("GET", "/dav/datei.txt", &[], None).unwrap();
        assert_eq!(resp.status, 302);
    }

    #[test]
    fn a_dead_port_maps_to_disconnected() {
        // Bind then drop, so the port is almost certainly closed.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let t = UreqTransport::new(&origin, None, None);
        match t.request("GET", "/dav/x", &[], None) {
            Err(Error::Disconnected) => {}
            other => panic!("expected Disconnected, got {:?}", other.map(|_| "ok")),
        }
    }

    #[test]
    fn webdavvfs_lists_over_the_real_transport() {
        // End-to-end: the real ureq transport feeds a genuine PROPFIND 207 into WebdavVfs's parser
        // over a loopback socket, proving the two halves fit (headers, body read, status, parsing).
        use crate::webdavvfs::WebdavVfs;
        use dualfile_core::vfs::{VPath, Vfs};
        use std::sync::Arc;

        let xml = concat!(
            r#"<d:multistatus xmlns:d="DAV:">"#,
            r#"<d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#,
            r#"<d:response><d:href>/dav/ordner/datei.txt</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>7</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#,
            r#"</d:multistatus>"#,
        );
        let response = format!(
            "HTTP/1.1 207 Multi-Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            xml.len(),
            xml
        );
        let (origin, _rx) = mock_server(Box::leak(response.into_boxed_str()).as_bytes());
        let vfs = WebdavVfs::new(Arc::new(UreqTransport::new(&origin, None, None)), "/dav");
        let entries = vfs.list(&VPath::sftp("conn", "/ordner").unwrap()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "datei.txt");
        assert_eq!(entries[0].size, 7);
    }

    #[test]
    fn close_revokes_every_later_request() {
        // WD-4A-02: after close() no request may reach the server, even though WebDAV keeps no
        // socket. No mock is needed — a revoked transport must fail before touching the network.
        let t = UreqTransport::new("http://127.0.0.1:1", None, None);
        t.close();
        match t.request("GET", "/dav/x", &[], None) {
            Err(Error::Disconnected) => {}
            other => panic!(
                "expected Disconnected after close, got {:?}",
                other.map(|_| "ok")
            ),
        }
        // A PUT (write commit) is refused the same way.
        match t.request("PUT", "/dav/x", &[], Some(b"data".to_vec())) {
            Err(Error::Disconnected) => {}
            other => panic!(
                "expected Disconnected after close, got {:?}",
                other.map(|_| "ok")
            ),
        }
    }

    #[test]
    fn https_with_a_pinned_self_signed_certificate_connects_over_real_tls() {
        // End-to-end TLS: a self-signed cert is pinned, and the real ureq -> rustls -> pinning
        // verifier -> fingerprint path must complete a genuine TLS handshake and return the body.
        // This is the one piece the verifier's own unit tests (tls.rs) cannot cover: the wiring of
        // tls_config into ureq and the pin being honoured across a real handshake.
        use crate::tls;
        use crate::trust::{
            hex, CertVerifier, ConfirmTrust, MemoryPins, Pin, PinKey, PinStore, PolicyVerifier,
            Protocol, TrustDecision, TrustRequest,
        };
        use sha2::{Digest, Sha256};

        let issued = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert_der = issued.cert.der().clone();
        let fingerprint = hex(&Sha256::digest(cert_der.as_ref()));
        let key_der = issued.key_pair.serialize_der();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_config = std::sync::Arc::new(
            rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert_der],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.into()),
            )
            .unwrap(),
        );
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut conn = rustls::ServerConnection::new(server_config).unwrap();
            let mut tls = rustls::Stream::new(&mut conn, &mut sock);
            // Read the WHOLE request (headers until CRLFCRLF, then the Content-Length body) before
            // replying. Otherwise the thread could close the connection while ureq is still writing
            // its body, racing `Connection: close` and flaking the test.
            let mut data = Vec::new();
            let mut chunk = [0u8; 512];
            let mut header_end: Option<usize> = None;
            let mut content_len = 0usize;
            loop {
                match tls.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => data.extend_from_slice(&chunk[..n]),
                }
                if header_end.is_none() {
                    if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = Some(pos + 4);
                        for line in String::from_utf8_lossy(&data[..pos]).lines() {
                            if let Some(v) =
                                line.to_ascii_lowercase().strip_prefix("content-length:")
                            {
                                content_len = v.trim().parse().unwrap_or(0);
                            }
                        }
                    }
                }
                if header_end.is_some_and(|he| data.len() >= he + content_len) {
                    break;
                }
            }
            let xml = concat!(
                r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/</d:href>"#,
                r#"<d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop>"#,
                r#"<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#,
            );
            let resp = format!(
                "HTTP/1.1 207 Multi-Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                xml.len(),
                xml
            );
            let _ = tls.write_all(resp.as_bytes());
            let _ = tls.flush();
            // Graceful TLS shutdown so the client reliably sees the full response before the socket
            // drops.
            conn.send_close_notify();
            let _ = conn.complete_io(&mut sock);
        });

        struct NeverConfirm;
        impl ConfirmTrust for NeverConfirm {
            fn confirm(&self, _: &TrustRequest) -> TrustDecision {
                TrustDecision::Reject
            }
        }
        // A matching pin trusts the fingerprint on its own, so the confirmation is never consulted.
        let pins: std::sync::Arc<dyn PinStore> = std::sync::Arc::new(MemoryPins::new());
        pins.set(Pin {
            protocol: Protocol::Webdav,
            host: "localhost".to_owned(),
            port,
            sha256: fingerprint,
            key_type: None,
        });
        let verifier: Arc<dyn CertVerifier> =
            Arc::new(PolicyVerifier::new(pins, Arc::new(NeverConfirm)));
        let pinning = tls::PinningServerVerifier::new(
            verifier,
            PinKey::new(Protocol::Webdav, "localhost", port),
            tls::system_roots(),
        );
        let config = tls::webdav_client_config(pinning.clone());

        let t = UreqTransport::new_tls(
            &format!("https://localhost:{port}"),
            None,
            None,
            Some(config),
        );
        let resp = t
            .request(
                "PROPFIND",
                "/",
                &[("Depth", "0")],
                Some(b"<propfind/>".to_vec()),
            )
            .unwrap();
        assert_eq!(resp.status, 207);
        assert!(
            pinning.take_error().is_none(),
            "a matching pin must not record a trust error"
        );
    }

    #[test]
    fn trust_once_survives_a_fresh_tls_transport_until_reset_or_mismatch() {
        use crate::trust::*;
        use std::sync::atomic::AtomicUsize;

        struct Once(AtomicUsize);
        impl ConfirmTrust for Once {
            fn confirm(&self, _: &TrustRequest) -> TrustDecision {
                self.0.fetch_add(1, Ordering::SeqCst);
                TrustDecision::TrustOnce
            }
        }

        for reset in [false, true] {
            let issued = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            listener.set_nonblocking(true).unwrap();
            let config = Arc::new(
                rustls::ServerConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![issued.cert.der().clone()],
                    rustls::pki_types::PrivateKeyDer::Pkcs8(issued.key_pair.serialize_der().into()),
                )
                .unwrap(),
            );
            let server = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let mut accepted = 0;
                while accepted < 3 && std::time::Instant::now() < deadline {
                    let (mut sock, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(e) => panic!("{e}"),
                    };
                    accepted += 1;
                    sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    sock.set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut conn = rustls::ServerConnection::new(config.clone()).unwrap();
                    let mut stream = rustls::Stream::new(&mut conn, &mut sock);
                    let mut request = Vec::new();
                    let mut byte = [0];
                    while request.len() < 8192 && stream.read_exact(&mut byte).is_ok() {
                        request.push(byte[0]);
                        if request.ends_with(b"\r\n\r\n") {
                            stream
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                                )
                                .unwrap();
                            stream.flush().unwrap();
                            break;
                        }
                    }
                    conn.send_close_notify();
                    let _ = conn.complete_io(&mut sock);
                }
                accepted
            });
            let pins = Arc::new(MemoryPins::new());
            let confirm = Arc::new(Once(AtomicUsize::new(0)));
            let verifier = Arc::new(AttemptVerifier::new(
                Arc::new(MemoryPins::new()),
                pins.clone(),
                confirm.clone(),
            ));
            let key = PinKey::new(Protocol::Webdav, "localhost", port);
            let pinning = crate::tls::PinningServerVerifier::new(
                verifier.clone(),
                key.clone(),
                crate::tls::system_roots(),
            );
            // Each request creates both a fresh HTTP agent and TLS config (no session resumption).
            let request = || {
                UreqTransport::new_tls(
                    &format!("https://localhost:{port}"),
                    None,
                    None,
                    Some(crate::tls::webdav_client_config(pinning.clone())),
                )
                .request("GET", "/", &[], None)
            };
            assert_eq!(request().unwrap().status, 200);
            verifier.finish();
            assert_eq!(request().unwrap().status, 200);
            assert!(pins.get(&key).is_none());
            if reset {
                pins.remove(&key);
            } else {
                pins.set(Pin {
                    protocol: key.protocol,
                    host: key.host.clone(),
                    port,
                    sha256: "00".repeat(32),
                    key_type: None,
                });
            }
            assert!(request().is_err());
            let error = pinning.take_error().unwrap();
            if reset {
                assert!(matches!(error, TrustError::NotConfirmed(_)));
            } else {
                assert!(matches!(error, TrustError::PinMismatch { .. }));
            }
            assert_eq!(confirm.0.load(Ordering::SeqCst), 1);
            assert_eq!(server.join().unwrap(), 3);
        }
    }

    #[test]
    fn retained_webdav_config_reverifies_after_ticket_and_trust_reset() {
        use crate::trust::*;
        use std::sync::atomic::AtomicUsize;

        struct Once;
        impl ConfirmTrust for Once {
            fn confirm(&self, _: &TrustRequest) -> TrustDecision {
                TrustDecision::TrustOnce
            }
        }

        // Count actual ticket issuance, rather than merely enabling it in the server config.
        #[derive(Debug)]
        struct Tickets {
            inner: Arc<dyn rustls::server::ProducesTickets>,
            issued: AtomicUsize,
        }
        impl rustls::server::ProducesTickets for Tickets {
            fn enabled(&self) -> bool {
                self.inner.enabled()
            }
            fn lifetime(&self) -> u32 {
                self.inner.lifetime()
            }
            fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>> {
                let ticket = self.inner.encrypt(plain)?;
                self.issued.fetch_add(1, Ordering::SeqCst);
                Some(ticket)
            }
            fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
                self.inner.decrypt(cipher)
            }
        }
        let tickets = Arc::new(Tickets {
            inner: rustls::crypto::ring::Ticketer::new().unwrap(),
            issued: AtomicUsize::new(0),
        });
        let issued = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let mut server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![issued.cert.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(issued.key_pair.serialize_der().into()),
        )
        .unwrap();
        server_config.ticketer = tickets.clone();
        server_config.send_tls13_tickets = 2;
        let server_config = Arc::new(server_config);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let (tx, rx) = mpsc::channel();
        let server =
            thread::spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let mut accepted = 0;
                while accepted < 2 && std::time::Instant::now() < deadline {
                    let (mut socket, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(e) => panic!("{e}"),
                    };
                    accepted += 1;
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut conn = rustls::ServerConnection::new(server_config.clone()).unwrap();
                    let handshake = conn.complete_io(&mut socket);
                    let kind = conn.handshake_kind();
                    if handshake.is_ok() {
                        let mut stream = rustls::Stream::new(&mut conn, &mut socket);
                        let mut request = Vec::new();
                        let mut byte = [0];
                        while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                            stream.read_exact(&mut byte).unwrap();
                            request.push(byte[0]);
                        }
                        stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    ).unwrap();
                        stream.flush().unwrap();
                        conn.send_close_notify();
                        let _ = conn.complete_io(&mut socket);
                    }
                    // Signal only after the socket is closed, so reset cannot race that close.
                    drop(socket);
                    tx.send((handshake.is_ok(), kind)).unwrap();
                }
                accepted
            });
        let pins = Arc::new(MemoryPins::new());
        let verifier = Arc::new(AttemptVerifier::new(
            Arc::new(MemoryPins::new()),
            pins.clone(),
            Arc::new(Once),
        ));
        let key = PinKey::new(Protocol::Webdav, "localhost", port);
        let pinning =
            crate::tls::PinningServerVerifier::new(verifier.clone(), key.clone(), Vec::new());
        let config = crate::tls::webdav_client_config(pinning.clone());
        // Keep exactly the same production config AND agent across both TCP connections.
        let transport = UreqTransport::new_tls(
            &format!("https://localhost:{port}"),
            None,
            None,
            Some(config),
        );
        let mut response = transport.request("GET", "/", &[], None).unwrap();
        assert_eq!(response.status, 200);
        let mut body = String::new();
        response.body.read_to_string(&mut body).unwrap();
        assert_eq!(body, "ok");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (true, Some(rustls::HandshakeKind::Full)),
        );
        assert!(tickets.issued.load(Ordering::SeqCst) > 0);
        verifier.finish();
        assert!(pins.get(&key).is_none(), "TrustOnce must not persist a pin");
        pins.remove(&key);
        assert!(pinning.take_error().is_none());
        let result = transport.request("GET", "/", &[], None);
        let (succeeded, _) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(server.join().unwrap(), 2);
        assert!(matches!(result, Err(Error::Disconnected)));
        // This error can only be recorded by a fresh verify_server_cert call; resumption skips it.
        assert!(matches!(
            pinning.take_error(),
            Some(TrustError::NotConfirmed(_))
        ));
        assert!(!succeeded, "revoked trust must fail the new TLS handshake");
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
    }
}
