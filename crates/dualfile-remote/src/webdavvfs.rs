//! WebDAV virtual file system — increment 1: the read side (listing, stat, read) and the PROPFIND
//! response parser, driven through a [`WebdavTransport`] so the protocol logic is testable without a
//! live server. The real HTTP transport (over the crate's rustls) is a thin adapter added with the
//! connector; writing (PUT/MKCOL/MOVE/DELETE) follows in a later increment.
//!
//! Status: standalone and read-only (`is_read_only()` is true, every write returns `Unsupported`),
//! not yet wired into the router — so none of this runs in production until the connector increment.
//! Before that wiring, two items from the review still need attention: (1) bound the staging read of
//! a GET against the advertised size (a general remote-copy hardening shared with SFTP/FTP/SMB), and
//! (2) a malicious server could still make namespace resolution costly with a flood of `xmlns`
//! declarations; the element cap below bounds it, but a stricter `xmlns` limit (or a smaller body
//! cap) should be added before this backend is reachable.
//!
//! Security: the PROPFIND body is attacker-controlled XML from the server. It is read with a hard
//! byte cap (oversize is an error, never a silent partial list), parsed with quick-xml (which does
//! not expand external entities), and a truncated or unterminated `multistatus` is rejected. Paths
//! are handled per URL segment: each server href segment is percent-decoded on its own, so an
//! encoded slash (`%2F`) cannot forge a path separator, and only a response whose decoded path sits
//! exactly one level under the requested collection is accepted. Property values are taken only from
//! `propstat` blocks with a 2xx status, and a resource with a non-2xx response status is dropped.

use dualfile_core::error::{Error, Result};
use dualfile_core::vfs::{Caps, Entry, EntryKind, ReplaceMode, VPath, Vfs, WriteFinish, WriteMode};
use std::io::Read;
use std::sync::Arc;

/// Largest PROPFIND body accepted. A listing of one collection is far smaller; a bigger body is an
/// error (a hostile server must not be able to pass off a truncated list as complete).
const MAX_PROPFIND_BYTES: u64 = 8 << 20;
/// Largest number of responses accepted from one listing; more is treated as an incomplete answer.
const MAX_ENTRIES: usize = 50_000;

/// One HTTP response. `body` is streamed; the caller bounds how much it reads.
pub struct WebdavResponse {
    pub status: u16,
    pub body: Box<dyn Read + Send>,
}

/// The HTTP side of WebDAV, abstracted so the VFS logic is testable with a fake. `path` is the
/// server path (already joined with the connection base); `depth` is the PROPFIND Depth header.
pub trait WebdavTransport: Send + Sync {
    fn request(
        &self,
        method: &str,
        path: &str,
        depth: Option<u8>,
        body: Option<Vec<u8>>,
    ) -> Result<WebdavResponse>;
}

const PROPFIND_BODY: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype/><getcontentlength/></prop></propfind>"#;

pub struct WebdavVfs {
    transport: Arc<dyn WebdavTransport>,
    /// Base path of the connection on the server (e.g. "/dav"), without a trailing slash.
    base: String,
}

impl WebdavVfs {
    pub fn new(transport: Arc<dyn WebdavTransport>, base: &str) -> Self {
        Self {
            transport,
            base: normalize_base(base),
        }
    }

    /// Server path for a VFS path. The VFS path already holds real (decoded) names, so it is not
    /// decoded again here.
    fn server_path(&self, p: &VPath) -> String {
        let rel = p.path().to_string_lossy();
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            format!("{}/", self.base)
        } else {
            format!("{}/{}", self.base, rel)
        }
    }

    /// Reads the body up to the cap. Returns the bytes and whether they stayed within the cap; an
    /// oversize body is reported so the caller can reject it rather than parse a partial list.
    fn read_bounded(resp: WebdavResponse) -> Result<(Vec<u8>, bool)> {
        let mut buf = Vec::new();
        resp.body
            .take(MAX_PROPFIND_BYTES + 1)
            .read_to_end(&mut buf)
            .map_err(|_| remote("PROPFIND-Antwort nicht lesbar"))?;
        let within = buf.len() as u64 <= MAX_PROPFIND_BYTES;
        Ok((buf, within))
    }
}

fn normalize_base(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        format!("/{rest}")
    } else {
        format!("/{trimmed}")
    }
}

fn remote(msg: &str) -> Error {
    Error::Remote(msg.to_owned())
}

/// Decodes `%XX` escapes in a single path segment (invalid escapes are kept literally). Returns
/// `None` if the decoded bytes are not valid UTF-8, so two distinct hrefs can never collapse to the
/// same lossily-replaced name.
fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// The path portion of an href (an absolute URL's scheme/host is stripped).
fn href_path_str(href: &str) -> &str {
    match href.find("://") {
        Some(pos) => {
            let after = &href[pos + 3..];
            after.find('/').map(|s| &after[s..]).unwrap_or("")
        }
        None => href,
    }
}

/// Splits a raw (still percent-encoded) href path into decoded segments. Each segment is decoded on
/// its own, so `%2F` stays inside one segment instead of forging a separator. Returns `None` if any
/// segment is not valid UTF-8 after decoding.
fn decoded_segments(href: &str) -> Option<Vec<String>> {
    href_path_str(href)
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect()
}

/// Splits an already-decoded server path (not an href) into segments without decoding again.
fn raw_segments(path: &str) -> Vec<String> {
    path.trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Extracts the numeric code from a DAV `<status>` line such as "HTTP/1.1 200 OK".
fn parse_http_status(line: &str) -> Option<u16> {
    line.split_whitespace().find_map(|tok| {
        let n: u16 = tok.parse().ok()?;
        (100..600).contains(&n).then_some(n)
    })
}

/// Maps a namespace-stripped local element name to a small fixed set; everything else is ignored.
fn local_name(name: &[u8]) -> &'static str {
    let local = name.rsplit(|&b| b == b':').next().unwrap_or(name);
    match local {
        b"multistatus" => "multistatus",
        b"response" => "response",
        b"prop" => "prop",
        b"href" => "href",
        b"status" => "status",
        b"propstat" => "propstat",
        b"resourcetype" => "resourcetype",
        b"collection" => "collection",
        b"getcontentlength" => "getcontentlength",
        _ => "",
    }
}

#[derive(Default)]
struct DavResponse {
    href: String,
    is_collection: bool,
    size: u64,
    /// A 2xx `propstat` provided the `resourcetype`, so file-vs-folder is actually known.
    resourcetype_known: bool,
    /// A 2xx `propstat` provided a parseable `getcontentlength` (needed for a file's size).
    size_known: bool,
    /// Response-level `<status>` (used for error responses without a propstat).
    response_status: Option<u16>,
}

/// Resolves one XML general/character reference (`amp`, `#45`, `#x2d`, …) to its text. An unknown
/// named entity or a bad numeric reference yields `None`, which the caller treats as an error.
fn resolve_ref(name: &[u8]) -> Option<char> {
    match name {
        b"amp" => Some('&'),
        b"lt" => Some('<'),
        b"gt" => Some('>'),
        b"quot" => Some('"'),
        b"apos" => Some('\''),
        _ => {
            let rest = name.strip_prefix(b"#")?;
            let code =
                if let Some(hex) = rest.strip_prefix(b"x").or_else(|| rest.strip_prefix(b"X")) {
                    u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?
                } else {
                    std::str::from_utf8(rest).ok()?.parse().ok()?
                };
            char::from_u32(code)
        }
    }
}

/// Reads the character content of the element whose start tag was just consumed, up to its matching
/// end tag (`end` = the start tag's raw name). Text is decoded, entity/character references are
/// resolved (unknown ones are rejected), CDATA is taken verbatim and comments are ignored. An
/// unexpected child element or EOF is an error — so a value is never confused with markup.
fn read_element_text(reader: &mut quick_xml::NsReader<&[u8]>, end: &[u8]) -> Result<String> {
    use quick_xml::events::Event;
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_resolved_event_into(&mut buf).map(|(_, ev)| ev) {
            Err(_) => return Err(remote("Textinhalt nicht lesbar")),
            Ok(Event::Eof) => return Err(remote("unerwartetes Dateiende im Textfeld")),
            Ok(Event::Text(t)) => {
                out.push_str(&t.decode().map_err(|_| remote("Textkodierung ungültig"))?)
            }
            Ok(Event::CData(c)) => out.push_str(
                std::str::from_utf8(&c).map_err(|_| remote("CDATA ist kein gültiges UTF-8"))?,
            ),
            Ok(Event::GeneralRef(r)) => {
                let ch = resolve_ref(r.as_ref())
                    .ok_or_else(|| remote("unbekannte oder ungültige XML-Referenz"))?;
                out.push(ch);
            }
            Ok(Event::Comment(_)) => {}
            Ok(Event::End(e)) if e.name().as_ref() == end => return Ok(out),
            Ok(Event::Start(_)) | Ok(Event::End(_)) => {
                return Err(remote("unerwartetes Element im Textfeld"))
            }
            _ => {}
        }
    }
}

/// Parses a `multistatus` body. Returns the responses and whether the document was structurally
/// complete: exactly one `multistatus` root that opened and closed, no entry-count cap hit. Property
/// values come only from a `propstat` whose own `<status>` is 2xx, and a response without a known
/// `resourcetype` is not treated as a plain file.
fn parse_multistatus(xml: &[u8]) -> Result<(Vec<DavResponse>, bool)> {
    use quick_xml::events::Event;
    use quick_xml::name::{Namespace, ResolveResult};
    // Namespace-aware: only elements in the DAV: namespace are treated as protocol elements, so a
    // foreign-namespace element with a colliding local name is inert.
    const DAV: ResolveResult = ResolveResult::Bound(Namespace(b"DAV:"));
    // Exact DAV document paths a state transition is allowed at (the open-element stack equals one
    // of these). Matching the full path, not just a parent, keeps foreign-wrapped DAV elements inert.
    const MS: &[&str] = &["multistatus"];
    const R: &[&str] = &["multistatus", "response"];
    const PS: &[&str] = &["multistatus", "response", "propstat"];
    const PROP: &[&str] = &["multistatus", "response", "propstat", "prop"];
    const RT: &[&str] = &[
        "multistatus",
        "response",
        "propstat",
        "prop",
        "resourcetype",
    ];
    let mut reader = quick_xml::NsReader::from_reader(xml);
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_end_names = true;

    let mut responses: Vec<DavResponse> = Vec::new();
    let mut root_opened = false;
    let mut root_closed = false;
    // The open structural elements (local names), e.g. ["multistatus", "response", "propstat"].
    // A field is only acted on in its correct DAV nesting, so an extension element with the same
    // local name (after namespace stripping) cannot overwrite a resource's href/status/props.
    let mut stack: Vec<&'static str> = Vec::new();
    let mut cur: Option<DavResponse> = None;
    let mut ps_status: Option<u16> = None;
    let mut ps_collection = false;
    let mut ps_size: Option<u64> = None;
    let mut ps_has_resourcetype = false;
    // Bounds against an adversarial document structure (deep nesting or a flood of elements, the
    // latter also the lever behind a namespace-declaration blow-up during resolution).
    const MAX_DEPTH: usize = 128;
    const MAX_ELEMENTS: usize = 200_000;
    let mut elements = 0usize;

    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (ns, event) = match reader.read_resolved_event_into(&mut buf) {
            Err(_) => return Err(remote("PROPFIND-Antwort ist kein gültiges XML")),
            Ok(pair) => pair,
        };
        let is_dav = ns == DAV;
        match event {
            Event::Eof => break,
            Event::Start(e) => {
                elements += 1;
                if elements > MAX_ELEMENTS {
                    return Err(remote("PROPFIND-Antwort hat zu viele Elemente"));
                }
                let qn = e.name().as_ref().to_vec();
                // A non-DAV element is structural filler only (pushed on the stack, no field acted on).
                let name = if is_dav { local_name(&qn) } else { "" };
                if matches!(name, "href" | "status" | "getcontentlength") {
                    // Self-contained text leaf, consumed whole by the helper (no stack change).
                    if stack.is_empty() {
                        return Err(remote("Element außerhalb der multistatus-Wurzel"));
                    }
                    let text = read_element_text(&mut reader, &qn)?;
                    // The exact DAV document path (not just the parent) is required, so a DAV element
                    // nested inside a foreign wrapper cannot be mistaken for a real field.
                    let at = stack.as_slice();
                    match name {
                        "href" if at == R => {
                            if let Some(c) = cur.as_mut() {
                                c.href = text.trim().to_string();
                            }
                        }
                        "status" if at == R => {
                            if let Some(c) = cur.as_mut() {
                                c.response_status = parse_http_status(&text);
                            }
                        }
                        "status" if at == PS => ps_status = parse_http_status(&text),
                        "getcontentlength" if at == PROP => ps_size = text.trim().parse().ok(),
                        _ => {} // a field in an unexpected place is ignored
                    }
                    continue;
                }
                // Structural element: a single multistatus root, nothing outside it.
                if name == "multistatus" {
                    if !stack.is_empty() {
                        return Err(remote("multistatus ist nicht die Wurzel"));
                    }
                    if root_opened {
                        return Err(remote("mehrere multistatus-Wurzeln"));
                    }
                    root_opened = true;
                } else if stack.is_empty() {
                    return Err(remote("Element außerhalb der multistatus-Wurzel"));
                }
                let at = stack.as_slice();
                match name {
                    "response" if at == MS => cur = Some(DavResponse::default()),
                    "propstat" if at == R => {
                        ps_status = None;
                        ps_collection = false;
                        ps_size = None;
                        ps_has_resourcetype = false;
                    }
                    "resourcetype" if at == PROP => ps_has_resourcetype = true,
                    "collection" if at == RT => ps_collection = true,
                    _ => {}
                }
                if stack.len() >= MAX_DEPTH {
                    return Err(remote("PROPFIND-Antwort ist zu tief verschachtelt"));
                }
                stack.push(name);
            }
            Event::End(e) => {
                let name = if is_dav {
                    local_name(e.name().as_ref())
                } else {
                    ""
                };
                // The closing element's own full path (stack still including it) decides finalization.
                let at = stack.as_slice();
                let finalize_propstat = name == "propstat" && at == PS;
                let finalize_response = name == "response" && at == R;
                let close_root = name == "multistatus" && at == MS;
                if stack.pop().is_none() {
                    return Err(remote("unausgeglichene Tags"));
                }
                if finalize_propstat {
                    // Only a successful propstat contributes properties.
                    if ps_status.is_some_and(|s| (200..300).contains(&s)) {
                        if let Some(c) = cur.as_mut() {
                            if ps_has_resourcetype {
                                c.resourcetype_known = true;
                                c.is_collection = ps_collection;
                            }
                            if let Some(sz) = ps_size {
                                c.size = sz;
                                c.size_known = true;
                            }
                        }
                    }
                } else if finalize_response {
                    if let Some(c) = cur.take() {
                        responses.push(c);
                        if responses.len() >= MAX_ENTRIES {
                            return Ok((responses, false));
                        }
                    }
                } else if close_root {
                    root_closed = true;
                }
            }
            // Character data or a reference outside the root is malformed (wrapper text, trailing
            // garbage, a stray entity after the root).
            Event::Text(t) => {
                if stack.is_empty() && !t.decode().map(|s| s.trim().is_empty()).unwrap_or(false) {
                    return Err(remote("Zeichendaten außerhalb der multistatus-Wurzel"));
                }
            }
            Event::CData(_) if stack.is_empty() => {
                return Err(remote("CDATA außerhalb der multistatus-Wurzel"))
            }
            Event::GeneralRef(_) if stack.is_empty() => {
                return Err(remote("Referenz außerhalb der multistatus-Wurzel"))
            }
            _ => {}
        }
    }
    Ok((responses, root_opened && root_closed && stack.is_empty()))
}

fn entry_from(name: String, is_collection: bool, size: u64) -> Entry {
    Entry {
        name,
        kind: if is_collection {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        size,
        modified: None,
        mode: None,
        hidden: false,
        link_target: None,
        broken_link: false,
        error: None,
    }
}

fn unsupported() -> Error {
    Error::Unsupported("Dieser WebDAV-Vorgang wird noch nicht unterstützt")
}

impl WebdavVfs {
    fn propfind(&self, path: &str, depth: u8, missing: &VPath) -> Result<Vec<DavResponse>> {
        let resp =
            self.transport
                .request("PROPFIND", path, Some(depth), Some(PROPFIND_BODY.to_vec()))?;
        if resp.status == 404 {
            return Err(Error::NotFound(missing.path().to_path_buf()));
        }
        // PROPFIND must answer 207 Multi-Status; a 200/206/other "success" (e.g. a 206 partial body
        // that parses as a well-formed subset) must not pass as a complete listing.
        if resp.status != 207 {
            return Err(remote(&format!(
                "WebDAV-Server antwortete auf PROPFIND mit {} statt 207",
                resp.status
            )));
        }
        let (body, within) = WebdavVfs::read_bounded(resp)?;
        if !within {
            return Err(remote("PROPFIND-Antwort ist zu groß"));
        }
        let (responses, complete) = parse_multistatus(&body)?;
        if !complete {
            return Err(remote("PROPFIND-Antwort ist unvollständig"));
        }
        Ok(responses)
    }
}

impl Vfs for WebdavVfs {
    fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
        let path = self.server_path(p);
        let responses = self.propfind(&path, 1, p)?;
        let requested = raw_segments(&path);
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for r in responses {
            // Drop a resource the server marked as errored, one without a usable resourcetype (its
            // kind is unknown), or a file without a usable size (a remote copy needs the size).
            if r.response_status.is_some_and(|s| !(200..300).contains(&s))
                || !r.resourcetype_known
                || (!r.is_collection && !r.size_known)
            {
                continue;
            }
            // A non-UTF-8 href is skipped, never lossily substituted into a colliding name.
            let Some(child) = decoded_segments(&r.href) else {
                continue;
            };
            // Accept only an entry exactly one level under the requested collection.
            if child.len() != requested.len() + 1 || child[..requested.len()] != requested[..] {
                continue;
            }
            let name = child.last().cloned().unwrap_or_default();
            // Apply the shared server-name policy (separators, control chars, length, dots).
            if crate::names::name_problem(&name).is_some() {
                continue;
            }
            // A correct server never lists the same name twice; drop any later collision.
            if !seen.insert(name.clone()) {
                continue;
            }
            entries.push(entry_from(name, r.is_collection, r.size));
        }
        Ok(entries)
    }

    fn stat(&self, p: &VPath) -> Result<Entry> {
        let path = self.server_path(p);
        let mut responses = self.propfind(&path, 0, p)?;
        let r = responses
            .drain(..)
            .next()
            .ok_or_else(|| remote("WebDAV-Server lieferte keine Eigenschaften"))?;
        if r.response_status.is_some_and(|s| !(200..300).contains(&s)) {
            return Err(Error::NotFound(p.path().to_path_buf()));
        }
        if !r.resourcetype_known {
            return Err(remote("WebDAV-Server lieferte keinen Ressourcentyp"));
        }
        if !r.is_collection && !r.size_known {
            return Err(remote("WebDAV-Server lieferte keine Dateigröße"));
        }
        // The name is always the requested path's own last segment, never a server-supplied one:
        // a hostile server must not control a name that later becomes a local target.
        let name = raw_segments(&path).last().cloned().unwrap_or_default();
        Ok(entry_from(name, r.is_collection, r.size))
    }

    fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
        let path = self.server_path(p);
        let resp = self.transport.request("GET", &path, None, None)?;
        if resp.status == 404 {
            return Err(Error::NotFound(p.path().to_path_buf()));
        }
        // An unconditional full-file GET must be a plain 200: a 206 (partial) or 204 (no content)
        // would hand back a fragment or nothing as if it were the whole file.
        if resp.status != 200 {
            return Err(remote(&format!(
                "WebDAV-Server antwortete mit {} statt 200",
                resp.status
            )));
        }
        Ok(resp.body)
    }

    fn create_write(&self, _p: &VPath, _mode: WriteMode) -> Result<Box<dyn WriteFinish>> {
        Err(unsupported())
    }
    fn mkdir(&self, _p: &VPath) -> Result<()> {
        Err(unsupported())
    }
    fn rename(&self, _from: &VPath, _to: &VPath, _mode: ReplaceMode) -> Result<()> {
        Err(unsupported())
    }
    fn read_link(&self, _p: &VPath) -> Result<String> {
        Err(unsupported())
    }
    fn create_symlink(&self, _target: &str, _at: &VPath) -> Result<()> {
        Err(unsupported())
    }
    fn trash(&self, _p: &VPath) -> Result<()> {
        Err(unsupported())
    }
    fn remove_permanent(&self, _p: &VPath) -> Result<()> {
        Err(unsupported())
    }
    fn capabilities(&self) -> Caps {
        // Read-only for now; write capabilities arrive with the write increment.
        Caps::default()
    }
    fn is_read_only(&self) -> bool {
        // Every mutation is refused in this increment, so `ops` must reject a move up front rather
        // than copy to the destination and only then fail to delete the source.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeTransport {
        responses: Mutex<Vec<(u16, Vec<u8>)>>,
    }
    impl FakeTransport {
        fn new(responses: Vec<(u16, Vec<u8>)>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses),
            })
        }
    }
    impl WebdavTransport for FakeTransport {
        fn request(
            &self,
            _method: &str,
            _path: &str,
            _depth: Option<u8>,
            _body: Option<Vec<u8>>,
        ) -> Result<WebdavResponse> {
            let (status, body) = self.responses.lock().unwrap().remove(0);
            Ok(WebdavResponse {
                status,
                body: Box::new(std::io::Cursor::new(body)),
            })
        }
    }

    fn vpath(raw: &str) -> VPath {
        // The VFS logic only looks at the path portion; any remote scheme works for the test, and
        // the dedicated `Scheme::Webdav` is added with the connector increment.
        VPath::sftp("conn", raw).unwrap()
    }
    fn vfs(responses: Vec<(u16, Vec<u8>)>) -> WebdavVfs {
        WebdavVfs::new(FakeTransport::new(responses), "/dav")
    }
    fn sorted_names(mut entries: Vec<Entry>) -> Vec<String> {
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries.into_iter().map(|e| e.name).collect()
    }

    const LISTING: &[u8] = br#"<?xml version="1.0"?>
      <d:multistatus xmlns:d="DAV:">
        <d:response><d:href>/dav/ordner/</d:href>
          <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        <d:response><d:href>/dav/ordner/unter/</d:href>
          <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        <d:response><d:href>/dav/ordner/bild%20a.jpg</d:href>
          <d:propstat><d:prop><d:resourcetype/><d:getcontentlength>1234</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        <d:response><d:href>/dav/anderswo/fremd.txt</d:href>
          <d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
      </d:multistatus>"#;

    #[test]
    fn listing_returns_children_only_with_kinds_and_sizes() {
        let v = vfs(vec![(207, LISTING.to_vec())]);
        let entries = v.list(&vpath("/ordner")).unwrap();
        let names = sorted_names(entries.clone());
        // The collection itself and the out-of-folder "fremd.txt" are excluded; the space is decoded.
        assert_eq!(names, vec!["bild a.jpg", "unter"]);
        let file = entries.iter().find(|e| e.name == "bild a.jpg").unwrap();
        assert_eq!(file.kind, EntryKind::File);
        assert_eq!(file.size, 1234);
        assert_eq!(
            entries.iter().find(|e| e.name == "unter").unwrap().kind,
            EntryKind::Dir
        );
    }

    #[test]
    fn missing_collection_maps_to_not_found() {
        let v = vfs(vec![(404, b"nope".to_vec())]);
        assert!(matches!(
            v.list(&vpath("/weg")).unwrap_err(),
            Error::NotFound(_)
        ));
    }

    #[test]
    fn malformed_xml_is_a_clean_error_not_a_panic() {
        let v = vfs(vec![(207, b"<d:multistatus><d:response".to_vec())]);
        assert!(v.list(&vpath("/ordner")).is_err());
    }

    #[test]
    fn read_streams_the_body() {
        let v = vfs(vec![(200, b"dateiinhalt".to_vec())]);
        let mut out = String::new();
        v.open_read(&vpath("/ordner/datei.txt"))
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out, "dateiinhalt");
    }

    #[test]
    fn write_operations_are_not_supported_yet() {
        let v = vfs(vec![]);
        assert!(matches!(
            v.mkdir(&vpath("/x")).unwrap_err(),
            Error::Unsupported(_)
        ));
    }

    // --- Hostile-server cases from the Codex review (WD-01..WD-05) ---

    #[test]
    fn encoded_slash_in_name_cannot_forge_a_separator_or_escape() {
        // WD-01: a child whose name decodes to contain "/" is rejected (per-segment decode).
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/%2e%2e%2fescape</d:href><d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert!(v.list(&vpath("/ordner")).unwrap().is_empty());
    }

    #[test]
    fn stat_name_comes_from_the_request_never_the_server() {
        // WD-01 (HIGH): even on the root, a malicious self-href cannot set the entry name.
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/%2ftmp%2fescaped</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>1</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        // The name comes from the trusted base/request ("dav"), never the server's "/tmp/escaped".
        let entry = v.stat(&vpath("/")).unwrap();
        assert_eq!(entry.name, "dav");
        assert!(!entry.name.contains('/'));
        let v2 = vfs(vec![(207, body.to_vec())]);
        assert_eq!(v2.stat(&vpath("/datei.txt")).unwrap().name, "datei.txt");
    }

    #[test]
    fn entity_encoded_href_keeps_the_real_name() {
        // WD-02: "&amp;" / numeric refs must not split the href into the wrong name.
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/a&amp;b&#45;c.txt</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>3</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert_eq!(
            sorted_names(v.list(&vpath("/ordner")).unwrap()),
            vec!["a&b-c.txt"]
        );
    }

    #[test]
    fn failed_propstat_properties_are_ignored() {
        // WD-03/WD-R1: a 404 propstat's props are ignored; a 404 response drops the entry; and a
        // resource whose resourcetype is only in a failed propstat is skipped (kind unknown).
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/weg.txt</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
          <d:response><d:href>/dav/ordner/nurgroesse.txt</d:href>
            <d:propstat><d:prop><d:getcontentlength>9</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
            <d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/da.txt</d:href>
            <d:propstat><d:prop><d:resourcetype/><d:getcontentlength>7</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        let entries = v.list(&vpath("/ordner")).unwrap();
        // "weg.txt" (404 response) and "nurgroesse.txt" (no usable resourcetype) are both dropped.
        assert_eq!(sorted_names(entries.clone()), vec!["da.txt"]);
        assert_eq!(entries[0].size, 7);
    }

    #[test]
    fn response_without_a_usable_resourcetype_is_skipped() {
        // WD-R1: an all-failed response must not become a size-0 file.
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/kaputt</d:href><d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 403 Forbidden</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert!(v.list(&vpath("/ordner")).unwrap().is_empty());
    }

    #[test]
    fn cdata_and_comments_in_href_are_handled() {
        // WD-R2: read_text would include CDATA markers; the event reader must not.
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/<![CDATA[datei.txt]]></d:href><!-- c --><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>9</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert_eq!(
            sorted_names(v.list(&vpath("/ordner")).unwrap()),
            vec!["datei.txt"]
        );
    }

    #[test]
    fn a_nested_or_second_multistatus_is_rejected() {
        // WD-R3: "<multistatus><multistatus/>" must not count as a complete document.
        let v = vfs(vec![(
            207,
            br#"<d:multistatus xmlns:d="DAV:"><d:multistatus/>"#.to_vec(),
        )]);
        assert!(v.list(&vpath("/ordner")).is_err());
    }

    #[test]
    fn propfind_requires_207_multi_status() {
        // WD-R5-01: a 200/206 "success" must not pass as a complete listing.
        assert!(vfs(vec![(200, LISTING.to_vec())])
            .list(&vpath("/ordner"))
            .is_err());
        assert!(vfs(vec![(206, LISTING.to_vec())])
            .list(&vpath("/ordner"))
            .is_err());
    }

    #[test]
    fn an_extension_element_cannot_overwrite_the_resource_href() {
        // WD-R5-02: a nested <x:href> inside an extension must not replace the real child href.
        let body = br#"<d:multistatus xmlns:d="DAV:" xmlns:x="urn:x">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/echt.txt</d:href>
            <x:metadata><x:href>/dav/woanders/falsch.txt</x:href></x:metadata>
            <d:propstat><d:prop><d:resourcetype/><d:getcontentlength>4</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert_eq!(
            sorted_names(v.list(&vpath("/ordner")).unwrap()),
            vec!["echt.txt"]
        );
    }

    #[test]
    fn a_dav_element_nested_in_a_foreign_wrapper_is_inert() {
        // WD-R7-01: even a correctly-namespaced <d:response>/<d:href> inside a foreign wrapper must
        // not finalize or overwrite the real response (exact DAV document path is required).
        let body = br#"<d:multistatus xmlns:d="DAV:" xmlns:x="urn:x">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/echt.txt</d:href>
            <x:metadata><d:response><d:href>/dav/woanders/forged.txt</d:href></d:response></x:metadata>
            <d:propstat><d:prop><d:resourcetype/><d:getcontentlength>4</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert_eq!(
            sorted_names(v.list(&vpath("/ordner")).unwrap()),
            vec!["echt.txt"]
        );
    }

    #[test]
    fn names_violating_the_server_name_policy_are_dropped() {
        // WD-R7-02: a decoded NUL/backslash/overlong name must be rejected (shared name policy).
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/a%00b</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>1</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/a%5Cb</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>1</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert!(v.list(&vpath("/ordner")).unwrap().is_empty());
    }

    #[test]
    fn foreign_namespace_elements_are_not_treated_as_dav() {
        // WD-R6: an <x:response>/<x:href>/<x:collection> in another namespace must be inert, even
        // when nested so a name-only check would accept it.
        let body = br#"<d:multistatus xmlns:d="DAV:" xmlns:x="urn:x">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/echt.txt</d:href>
            <x:metadata><x:response><x:href>/dav/woanders/forged.txt</x:href></x:response></x:metadata>
            <d:propstat><d:prop><d:resourcetype><x:collection/></d:resourcetype><d:getcontentlength>4</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        let entries = v.list(&vpath("/ordner")).unwrap();
        // Real href kept (forged x:href ignored); x:collection does not mark the file a directory.
        assert_eq!(sorted_names(entries.clone()), vec!["echt.txt"]);
        assert_eq!(entries[0].kind, EntryKind::File);
    }

    #[test]
    fn multistatus_must_be_the_outermost_element_and_nothing_may_trail() {
        // WD-R3-01: a wrapped root or trailing content is not a complete empty listing.
        assert!(vfs(vec![(
            207,
            br#"<wrapper><d:multistatus xmlns:d="DAV:"/></wrapper>"#.to_vec()
        )])
        .list(&vpath("/ordner"))
        .is_err());
        assert!(vfs(vec![(
            207,
            br#"<d:multistatus xmlns:d="DAV:"/>garbage"#.to_vec()
        )])
        .list(&vpath("/ordner"))
        .is_err());
    }

    #[test]
    fn a_file_without_a_known_size_is_skipped() {
        // WD-R3-03: resourcetype present but no content length → size is unknown → drop the file.
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/ordner/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/ordner/ohnegroesse.txt</d:href><d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert!(v.list(&vpath("/ordner")).unwrap().is_empty());
    }

    #[test]
    fn invalid_cdata_bytes_are_rejected() {
        // WD-R4-01: CDATA must be strict UTF-8, not lossily substituted.
        let mut body =
            br#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/ordner/<![CDATA[x"#.to_vec();
        body.push(0xFF);
        body.extend_from_slice(br#"]]></d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>1</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#);
        assert!(vfs(vec![(207, body)]).list(&vpath("/ordner")).is_err());
    }

    #[test]
    fn a_trailing_reference_outside_the_root_is_rejected() {
        // WD-R4-02: a GeneralRef after the root must not pass as a complete empty listing.
        let v = vfs(vec![(
            207,
            br#"<d:multistatus xmlns:d="DAV:"/>&#65;"#.to_vec(),
        )]);
        assert!(v.list(&vpath("/ordner")).is_err());
    }

    #[test]
    fn the_backend_reports_itself_read_only() {
        // WD-R4-03: so `ops` rejects a move before creating the destination.
        assert!(vfs(vec![]).is_read_only());
    }

    #[test]
    fn excessively_deep_nesting_is_rejected() {
        // WD-R8-02: a pathologically deep document is refused instead of overflowing the parser.
        let mut body = br#"<d:multistatus xmlns:d="DAV:">"#.to_vec();
        for _ in 0..200 {
            body.extend_from_slice(b"<d:x>");
        }
        body.extend_from_slice(b"</d:multistatus>");
        assert!(vfs(vec![(207, body)]).list(&vpath("/ordner")).is_err());
    }

    #[test]
    fn a_partial_or_empty_get_is_an_error() {
        // WD-R4: a 206/204 GET must not pass as a full-file read.
        assert!(vfs(vec![(206, b"frag".to_vec())])
            .open_read(&vpath("/ordner/d.txt"))
            .is_err());
        assert!(vfs(vec![(204, Vec::new())])
            .open_read(&vpath("/ordner/d.txt"))
            .is_err());
    }

    #[test]
    fn truncated_or_unterminated_responses_are_errors_not_empty_lists() {
        // WD-04: an unterminated multistatus is incomplete, not a successful empty list.
        let v = vfs(vec![(207, br#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/ordner/x</d:href><d:propstat><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#.to_vec())]);
        assert!(v.list(&vpath("/ordner")).is_err());
        // An empty 204-style body is also not a valid listing.
        let v2 = vfs(vec![(207, Vec::new())]);
        assert!(v2.list(&vpath("/ordner")).is_err());
    }

    #[test]
    fn literal_percent_in_requested_name_still_lists_children() {
        // WD-05: the requested path is not decoded again; a folder literally named "a%20b" works.
        let body = br#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/dav/a%2520b/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/dav/a%2520b/datei.txt</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>5</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
        </d:multistatus>"#;
        let v = vfs(vec![(207, body.to_vec())]);
        assert_eq!(
            sorted_names(v.list(&vpath("/a%20b")).unwrap()),
            vec!["datei.txt"]
        );
    }

    #[test]
    fn decoded_segments_split_before_decoding() {
        assert_eq!(
            decoded_segments("/dav/a/bild%20a.jpg").unwrap(),
            vec!["dav", "a", "bild a.jpg"]
        );
        assert_eq!(
            decoded_segments("https://host/dav/ordner/").unwrap(),
            vec!["dav", "ordner"]
        );
        // An encoded slash stays within one segment.
        assert_eq!(decoded_segments("/dav/a%2Fb").unwrap(), vec!["dav", "a/b"]);
        // Invalid UTF-8 after decoding is rejected (not lossily substituted).
        assert!(decoded_segments("/dav/%FF").is_none());
    }

    #[test]
    fn http_status_is_extracted() {
        assert_eq!(parse_http_status("HTTP/1.1 207 Multi-Status"), Some(207));
        assert_eq!(parse_http_status("HTTP/1.1 404 Not Found"), Some(404));
        assert_eq!(parse_http_status("garbage"), None);
    }
}
