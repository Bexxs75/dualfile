//! Real HTTP(S) transport for the WebDAV backend — increment 3a. A thin [`WebdavTransport`] over
//! `ureq`, which runs on the same rustls + ring stack as the FTP/SFTP back ends (no OpenSSL, no
//! native-tls). It adds Basic authentication, percent-encodes the request path, follows no redirects
//! (so a hostile server cannot bounce the client to another host or leak the credentials), and bounds
//! every call with a timeout. TLS trust uses the bundled Mozilla root set; native-root and
//! self-signed (fingerprint) trust follow in increment 3b, and wiring into the connector/router
//! (profile, DTO, secret store, frontend) follows after that. Nothing here runs in production yet.
//!
//! Security notes: `http_status_as_error(false)` hands 4xx/5xx back to [`WebdavVfs`], which maps them
//! itself; `max_redirects(0)` with `max_redirects_will_error(false)` returns a 3xx as a plain
//! response instead of following it; the response body stays streamed and is bounded by the caller
//! (`read_bounded` for PROPFIND, the shared copy loop for GET), so a lying server cannot flood memory.

use crate::webdavvfs::{WebdavResponse, WebdavTransport};
use dualfile_core::error::{Error, Result};
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
}

impl UreqTransport {
    /// `origin` is `scheme://host[:port]` (a trailing slash is trimmed). Credentials, when given, are
    /// sent as HTTP Basic auth on every request.
    pub fn new(origin: &str, user: Option<&str>, pass: Option<&str>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .redirects(0) // never follow: no cross-host bounce, no credential leak
            .timeout(CALL_TIMEOUT)
            .user_agent(USER_AGENT)
            .build();
        let auth = user.map(|u| {
            let raw = format!("{u}:{}", pass.unwrap_or(""));
            format!("Basic {}", base64_encode(raw.as_bytes()))
        });
        Self {
            agent,
            origin: origin.trim_end_matches('/').to_owned(),
            auth,
        }
    }
}

impl WebdavTransport for UreqTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<WebdavResponse> {
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
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
    }
}
