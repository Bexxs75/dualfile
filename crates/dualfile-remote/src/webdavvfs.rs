//! WebDAV virtual file system — increment 1: the read side (listing, stat, read) and the PROPFIND
//! response parser, driven through a [`WebdavTransport`] so the protocol logic is testable without a
//! live server. The real HTTP transport (over the crate's rustls) is a thin adapter added with the
//! connector; writing (PUT/MKCOL/MOVE/DELETE) follows in a later increment.
//!
//! Status: standalone and read-only (`is_read_only()` is true, every write returns `Unsupported`),
//! not yet wired into the router — so none of this runs in production until the connector increment.
//! The GET body is bounded against the advertised size by the shared copy loop (`ops.rs`), and the
//! number of namespace declarations is capped here before parsing; the remaining work for the
//! connector increment is the write side (PUT/MKCOL/MOVE/DELETE) and the real HTTP/TLS transport.
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
/// server path (already joined with the connection base). `headers` are extra request headers
/// (e.g. `Depth` for PROPFIND, `Destination`/`Overwrite` for MOVE, `If-None-Match` for a
/// non-replacing PUT); the real transport percent-encodes the path and sets host/auth itself.
pub trait WebdavTransport: Send + Sync {
    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
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
    // Bound the number of namespace declarations before parsing: resolving each element scans the
    // active bindings, so a flood of `xmlns:` declarations would make resolution quadratic. A real
    // DAV listing has a handful; reject far more rather than do the work. (Cheap single pass.)
    const MAX_NS_DECLS: usize = 64;
    if xml.windows(5).filter(|w| w == b"xmlns").count() > MAX_NS_DECLS {
        return Err(remote(
            "PROPFIND-Antwort hat zu viele Namespace-Deklarationen",
        ));
    }
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

/// Largest file written through PUT. Writes are buffered in memory in this increment (a streaming
/// PUT is a later refinement), so a very large write is refused rather than exhausting memory.
const MAX_PUT_BYTES: usize = 256 << 20;

/// Maps a WebDAV failure status to the core error model for a mutation at `at`.
fn status_to_err(status: u16, at: &VPath) -> Error {
    match status {
        404 => Error::NotFound(at.path().to_path_buf()),
        401 | 403 => Error::PermissionDenied(at.path().to_path_buf()),
        405 => Error::Unsupported("WebDAV: Methode nicht erlaubt"),
        409 => Error::NotFound(at.path().to_path_buf()), // Conflict: a parent is missing
        412 => Error::Exists(at.path().to_path_buf()), // Precondition failed (If-None-Match/Overwrite:F)
        423 => remote("WebDAV: Ressource ist gesperrt"),
        207 => remote("WebDAV: Vorgang teilweise fehlgeschlagen"),
        s => remote(&format!("WebDAV-Server antwortete mit {s}")),
    }
}

#[derive(PartialEq)]
enum CommitState {
    Pending,
    Done,
    Failed,
}

/// A PUT buffered in memory until `commit`. `WriteMode::NoReplace` sends `If-None-Match: *` so the
/// server refuses to overwrite an existing resource. `commit` runs the PUT at most once: a second
/// commit after success is a no-op (it must not send an empty body and truncate the file), and after
/// a failure it refuses rather than resend the already-consumed (empty) buffer.
struct WebdavWrite {
    transport: Arc<dyn WebdavTransport>,
    path: String,
    vpath: VPath,
    mode: WriteMode,
    buf: Vec<u8>,
    state: CommitState,
}
impl std::io::Write for WebdavWrite {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > MAX_PUT_BYTES {
            return Err(std::io::Error::other(
                "WebDAV-PUT: Datei ist zu groß zum Puffern",
            ));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl WriteFinish for WebdavWrite {
    fn commit(&mut self) -> Result<()> {
        match self.state {
            CommitState::Done => return Ok(()),
            CommitState::Failed => return Err(remote("WebDAV-PUT ist bereits fehlgeschlagen")),
            CommitState::Pending => {}
        }
        let headers: &[(&str, &str)] = if self.mode == WriteMode::NoReplace {
            &[("If-None-Match", "*")]
        } else {
            &[]
        };
        // Consume the buffer and mark failed up front: a transport error or bad status then leaves
        // the write in the Failed state, so a retry cannot resend an empty body.
        self.state = CommitState::Failed;
        let body = std::mem::take(&mut self.buf);
        let resp = self
            .transport
            .request("PUT", &self.path, headers, Some(body))?;
        match resp.status {
            200 | 201 | 204 => {
                self.state = CommitState::Done;
                Ok(())
            }
            s => Err(status_to_err(s, &self.vpath)),
        }
    }
}

impl WebdavVfs {
    fn propfind(&self, path: &str, depth: u8, missing: &VPath) -> Result<Vec<DavResponse>> {
        let depth = if depth == 0 { "0" } else { "1" };
        let resp = self.transport.request(
            "PROPFIND",
            path,
            &[("Depth", depth)],
            Some(PROPFIND_BODY.to_vec()),
        )?;
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
        let resp = self.transport.request("GET", &path, &[], None)?;
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

    fn create_write(&self, p: &VPath, mode: WriteMode) -> Result<Box<dyn WriteFinish>> {
        Ok(Box::new(WebdavWrite {
            transport: self.transport.clone(),
            path: self.server_path(p),
            vpath: p.clone(),
            mode,
            buf: Vec::new(),
            state: CommitState::Pending,
        }))
    }
    fn mkdir(&self, p: &VPath) -> Result<()> {
        let path = self.server_path(p);
        let resp = self.transport.request("MKCOL", &path, &[], None)?;
        match resp.status {
            201 => Ok(()),
            // MKCOL on an existing resource answers 405; report it as "already exists".
            405 => Err(Error::Exists(p.path().to_path_buf())),
            s => Err(status_to_err(s, p)),
        }
    }
    fn rename(&self, from: &VPath, to: &VPath, mode: ReplaceMode) -> Result<()> {
        let src = self.server_path(from);
        let dst = self.server_path(to);
        let mut headers: Vec<(&str, &str)> = vec![("Destination", dst.as_str())];
        if mode == ReplaceMode::NoReplace {
            headers.push(("Overwrite", "F"));
        }
        let resp = self.transport.request("MOVE", &src, &headers, None)?;
        match resp.status {
            201 | 204 => Ok(()),
            s => Err(status_to_err(s, to)),
        }
    }
    fn read_link(&self, _p: &VPath) -> Result<String> {
        Err(unsupported())
    }
    fn create_symlink(&self, _target: &str, _at: &VPath) -> Result<()> {
        Err(unsupported())
    }
    fn trash(&self, _p: &VPath) -> Result<()> {
        // WebDAV has no trash; the file layer falls back to permanent deletion with confirmation.
        Err(Error::Unsupported("Papierkorb auf WebDAV nicht verfügbar"))
    }
    fn remove_permanent(&self, p: &VPath) -> Result<()> {
        // The core deletes one planned entry at a time with rmdir semantics (folders are emptied
        // first, then removed, and anything that appeared after the plan must make the folder fail
        // as "not empty"). A plain WebDAV DELETE on a collection removes it recursively, which would
        // also take such unconfirmed children. So refuse a non-empty collection here; `list` returns
        // only child entries (empty for a plain file or an empty collection), so this stays a no-op
        // for files. Residual: a child appearing between this check and the DELETE is still inherent
        // to plain WebDAV (no atomic conditional delete), matching the core's own lstat/unlink window.
        if !self.list(p)?.is_empty() {
            return Err(Error::DirectoryNotEmpty(p.path().to_path_buf()));
        }
        let path = self.server_path(p);
        let resp = self.transport.request("DELETE", &path, &[], None)?;
        match resp.status {
            200 | 204 => Ok(()),
            s => Err(status_to_err(s, p)),
        }
    }
    fn capabilities(&self) -> Caps {
        Caps {
            can_trash: false,
            can_rename_in_place: true,
        }
    }
    fn is_read_only(&self) -> bool {
        // Writing (PUT/MKCOL/MOVE/DELETE) is implemented now.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Call {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    }
    struct FakeTransport {
        responses: Mutex<Vec<(u16, Vec<u8>)>>,
        calls: Mutex<Vec<Call>>,
    }
    impl FakeTransport {
        fn new(responses: Vec<(u16, Vec<u8>)>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses),
                calls: Mutex::new(Vec::new()),
            })
        }
    }
    impl WebdavTransport for FakeTransport {
        fn request(
            &self,
            method: &str,
            path: &str,
            headers: &[(&str, &str)],
            body: Option<Vec<u8>>,
        ) -> Result<WebdavResponse> {
            self.calls.lock().unwrap().push(Call {
                method: method.to_owned(),
                path: path.to_owned(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                body,
            });
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
    /// Like `vfs`, but also returns the transport so a test can inspect the requests made.
    fn vfs_t(responses: Vec<(u16, Vec<u8>)>) -> (WebdavVfs, Arc<FakeTransport>) {
        let t = FakeTransport::new(responses);
        (WebdavVfs::new(t.clone(), "/dav"), t)
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
    fn mkdir_sends_mkcol_and_maps_existing() {
        let (v, t) = vfs_t(vec![(201, vec![])]);
        v.mkdir(&vpath("/neu")).unwrap();
        let call = t.calls.lock().unwrap()[0].clone();
        assert_eq!(call.method, "MKCOL");
        assert_eq!(call.path, "/dav/neu");
        // 405 means the collection already exists.
        assert!(matches!(
            vfs(vec![(405, vec![])]).mkdir(&vpath("/da")).unwrap_err(),
            Error::Exists(_)
        ));
    }

    #[test]
    fn put_sends_the_buffered_body_and_honours_no_replace() {
        let (v, t) = vfs_t(vec![(201, vec![])]);
        let mut w = v
            .create_write(&vpath("/ordner/datei.txt"), WriteMode::NoReplace)
            .unwrap();
        use std::io::Write;
        w.write_all(b"hallo").unwrap();
        w.commit().unwrap();
        let call = t.calls.lock().unwrap()[0].clone();
        assert_eq!(call.method, "PUT");
        assert_eq!(call.path, "/dav/ordner/datei.txt");
        assert_eq!(call.body.as_deref(), Some(&b"hallo"[..]));
        assert!(call
            .headers
            .iter()
            .any(|(k, val)| k == "If-None-Match" && val == "*"));
        // A 412 Precondition Failed (target exists) maps to Exists.
        let mut w2 = vfs(vec![(412, vec![])])
            .create_write(&vpath("/da.txt"), WriteMode::NoReplace)
            .unwrap();
        w2.write_all(b"x").unwrap();
        assert!(matches!(w2.commit().unwrap_err(), Error::Exists(_)));
    }

    #[test]
    fn put_refuses_a_body_larger_than_the_buffer_cap() {
        let mut w = vfs(vec![])
            .create_write(&vpath("/gross.bin"), WriteMode::Replace)
            .unwrap();
        use std::io::Write;
        let chunk = vec![0u8; 8 << 20];
        let mut total = 0usize;
        let mut hit = false;
        while total <= MAX_PUT_BYTES {
            if w.write_all(&chunk).is_err() {
                hit = true;
                break;
            }
            total += chunk.len();
        }
        assert!(hit, "an oversized write must be refused");
    }

    #[test]
    fn rename_sends_move_with_destination_and_overwrite() {
        let (v, t) = vfs_t(vec![(201, vec![])]);
        v.rename(&vpath("/a.txt"), &vpath("/b.txt"), ReplaceMode::NoReplace)
            .unwrap();
        let call = t.calls.lock().unwrap()[0].clone();
        assert_eq!(call.method, "MOVE");
        assert_eq!(call.path, "/dav/a.txt");
        assert!(call
            .headers
            .iter()
            .any(|(k, val)| k == "Destination" && val == "/dav/b.txt"));
        assert!(call
            .headers
            .iter()
            .any(|(k, val)| k == "Overwrite" && val == "F"));
    }

    // A PROPFIND body with only the requested path itself (no children): `list` filters out the
    // self entry, so this stands for a plain file or an empty collection.
    const SELF_ONLY: &[u8] = br#"<d:multistatus xmlns:d="DAV:">
      <d:response><d:href>/dav/weg.txt</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>5</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
    </d:multistatus>"#;

    #[test]
    fn delete_checks_emptiness_then_sends_delete_and_maps_not_found() {
        // A plain file: the PROPFIND lists no children, so the DELETE goes out.
        let (v, t) = vfs_t(vec![(207, SELF_ONLY.to_vec()), (204, vec![])]);
        v.remove_permanent(&vpath("/weg.txt")).unwrap();
        let calls = t.calls.lock().unwrap();
        assert_eq!(calls[0].method, "PROPFIND");
        assert_eq!(calls[1].method, "DELETE");
        // A missing target: the PROPFIND 404 already maps to NotFound (no DELETE is attempted).
        assert!(matches!(
            vfs(vec![(404, vec![])])
                .remove_permanent(&vpath("/fehlt"))
                .unwrap_err(),
            Error::NotFound(_)
        ));
    }

    #[test]
    fn delete_refuses_a_non_empty_collection_and_sends_no_delete() {
        // WD-W-01: a WebDAV DELETE on a collection recurses; the core expects rmdir semantics, so a
        // non-empty folder must be refused here and NO DELETE may be sent.
        let (v, t) = vfs_t(vec![(207, LISTING.to_vec())]);
        assert!(matches!(
            v.remove_permanent(&vpath("/ordner")).unwrap_err(),
            Error::DirectoryNotEmpty(_)
        ));
        let calls = t.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "PROPFIND");
    }

    #[test]
    fn commit_is_a_no_op_after_success() {
        // WD-W-02: a second commit must not send an empty PUT and truncate the just-written file.
        let (v, t) = vfs_t(vec![(201, vec![])]);
        let mut w = v
            .create_write(&vpath("/datei.txt"), WriteMode::Replace)
            .unwrap();
        use std::io::Write;
        w.write_all(b"inhalt").unwrap();
        w.commit().unwrap();
        w.commit().unwrap();
        let calls = t.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "the second commit must send no request");
        assert_eq!(calls[0].body.as_deref(), Some(&b"inhalt"[..]));
    }

    #[test]
    fn commit_refuses_to_resend_after_a_failure() {
        // WD-W-02: after a failed PUT the buffer is already consumed; a retry must error, not resend
        // an empty body.
        let (v, t) = vfs_t(vec![(500, vec![])]);
        let mut w = v
            .create_write(&vpath("/datei.txt"), WriteMode::Replace)
            .unwrap();
        use std::io::Write;
        w.write_all(b"inhalt").unwrap();
        assert!(w.commit().is_err());
        assert!(w.commit().is_err());
        assert_eq!(
            t.calls.lock().unwrap().len(),
            1,
            "the retry must send no request"
        );
    }

    #[test]
    fn trash_is_unsupported_on_webdav() {
        assert!(matches!(
            vfs(vec![]).trash(&vpath("/x")).unwrap_err(),
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
    fn the_backend_is_writable() {
        // Writing is implemented (increment 2), so ops treats it as a normal read-write backend.
        assert!(!vfs(vec![]).is_read_only());
    }

    #[test]
    fn a_flood_of_namespace_declarations_is_rejected() {
        // WD-R8-01: many xmlns declarations would make namespace resolution quadratic.
        let mut body = br#"<d:multistatus xmlns:d="DAV:" "#.to_vec();
        for i in 0..100 {
            body.extend_from_slice(format!("xmlns:p{i}=\"urn:{i}\" ").as_bytes());
        }
        body.extend_from_slice(b"/>");
        assert!(vfs(vec![(207, body)]).list(&vpath("/ordner")).is_err());
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
