//! The wire protocol, API version 1: one JSON object per line (UTF-8, `\n`), host and plugin
//! alternate strictly (one request, one answer). Every answer is checked against the schema
//! here: unknown fields are refused, types and sizes are bounded, names are validated. A
//! plugin never gets to say anything the host would interpret as a path, a command or markup.
//!
//! Request: `{"id": N, "op": "<op>", ...fields}`.
//! Answer: `{"id": N, "ok": true, "result": {...}}` or
//! `{"id": N, "ok": false, "error": {"code": "<code>", "message": "<text>"}}` (the `id`
//! must repeat the request's; `message` is optional diagnostic text, shown as plain text only).
//! Error codes: `not_found`, `exists`, `permission_denied`, `not_a_directory`,
//! `is_a_directory`, `not_empty`, `unsupported`, `invalid`, `too_large`, `other`.
//!
//! First exchange: `hello` `{"api": 1}` -> `{"api": 1, "kinds": ["filesystem", ...]}`; a
//! different `api` or kinds the manifest does not declare end the session.
//!
//! Operations (paths are the plugin's own, absolute, `/`-separated, already normalized by the
//! host; byte data is standard base64):
//! - `fs.capabilities` -> `{"write": bool, "rename_no_replace": bool}`
//! - `fs.list` `{path}` -> `{"entries": [{"name", "kind": "file"|"dir"|"symlink", "size"?,
//!   "modified"? (seconds since 1970), "mode"? (permission bits)}]}`
//! - `fs.stat` `{path}` -> `{"kind", "size"?, "modified"?, "mode"?}`
//! - `fs.read` `{path, offset, len}` -> `{"data"}` (at most `len` bytes, `len` <= 256 KiB; empty
//!   at or behind the end)
//! - with the `write` right only: `fs.mkdir` `{path}`, `fs.rename` `{from, to, replace}`,
//!   `fs.remove` `{path}` (a file or an empty folder), `fs.write_open` `{path, replace}` ->
//!   `{"handle": N}`, `fs.write_chunk` `{handle, data}` (<= 256 KiB, appended), `fs.write_close`
//!   `{handle, commit}` (`commit: false` discards the data).
//! - `packer.list` `{name, data}` -> `{"entries": [{"path", "kind": "file"|"dir", "size"?}]}`,
//!   `packer.read` `{name, data, path}` -> `{"data"}`; the host passes the archive bytes
//!   (<= 4 MiB), the plugin has no file access of its own.
//! - `column.values` `{column, items: [{"name", "size"?, "modified"?}]}` (<= 500 items) ->
//!   `{"values": [string|null, ...]}` (same length, each <= 256 bytes).
//! - `preview.render` `{name, data, max_chars}` (data <= 1 MiB) -> `{"kind": "text", "text"}`
//!   or `{"kind": "image", "mime": "image/png"|"image/jpeg"|"image/gif"|"image/webp",
//!   "data"}` (the bytes must start with that format's signature). Never markup.

use crate::error::{Diagnostic, ErrorCode, PluginError, PluginResult};
use crate::manifest::Kind;
use base64ct::{Base64, Encoding};
use dualfile_core::vfs::EntryKind;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_READ_CHUNK: usize = 256 * 1024;
pub const MAX_WRITE_CHUNK: usize = 256 * 1024;
pub const MAX_COLUMN_ITEMS: usize = 500;
pub const MAX_VALUE_BYTES: usize = 256;
pub const MAX_PREVIEW_INPUT: usize = 1024 * 1024;
pub const MAX_PREVIEW_TEXT: usize = 256 * 1024;
pub const MAX_PREVIEW_IMAGE: usize = 1024 * 1024;
pub const MAX_PACKER_INPUT: usize = 4 * 1024 * 1024;
pub const MAX_PACKER_READ: usize = 4 * 1024 * 1024;
pub const MAX_PACKER_ENTRIES: usize = 20_000;

/// Seconds since 1970 that are accepted as a modification time (about +-34 000 years).
const MAX_TIME_SECS: i64 = 1 << 40;

fn proto(why: &'static str) -> PluginError {
    PluginError::Protocol(why)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: u64,
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorBody {
    code: String,
    #[serde(default)]
    message: Option<String>,
}

/// Parses one answer line. The outer error is a protocol violation (the process must end);
/// the inner error is the plugin's regular error answer (the session stays usable).
pub fn parse_response(line: &[u8], expected_id: u64) -> PluginResult<Result<Value, PluginError>> {
    let text = std::str::from_utf8(line).map_err(|_| proto("Die Antwort ist kein UTF-8"))?;
    let envelope: Envelope =
        serde_json::from_str(text).map_err(|_| proto("Die Antwort ist kein gültiges JSON"))?;
    if envelope.id != expected_id {
        return Err(proto("Die Antwort gehört zu keiner Anfrage"));
    }
    match (envelope.ok, envelope.result, envelope.error) {
        (true, Some(result @ Value::Object(_)), None) => Ok(Ok(result)),
        (false, None, Some(error)) => {
            let code =
                ErrorCode::parse(&error.code).ok_or_else(|| proto("Unbekannter Fehlercode"))?;
            Ok(Err(PluginError::Reported {
                code,
                diagnostic: Diagnostic::new(error.message.as_deref().unwrap_or("")),
            }))
        }
        _ => Err(proto("Die Antwort passt nicht zum Schema")),
    }
}

fn typed<T: for<'de> Deserialize<'de>>(value: &Value) -> PluginResult<T> {
    T::deserialize(value).map_err(|_| proto("Die Antwort passt nicht zum Schema"))
}

pub fn encode_bytes(data: &[u8]) -> String {
    Base64::encode_string(data)
}

pub fn decode_bytes(text: &str, max: usize) -> PluginResult<Vec<u8>> {
    if text.len() > max / 3 * 4 + 8 {
        return Err(proto("Die Daten sind zu groß"));
    }
    let bytes =
        Base64::decode_vec(text).map_err(|_| proto("Die Daten sind nicht gültig kodiert"))?;
    if bytes.len() > max {
        return Err(proto("Die Daten sind zu groß"));
    }
    Ok(bytes)
}

// ---------------------------------------------------------------- hello / capabilities

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloBody {
    api: u32,
    kinds: Vec<Kind>,
}

#[derive(Debug)]
pub struct Hello {
    pub api: u32,
    pub kinds: Vec<Kind>,
}

pub fn parse_hello(result: &Value) -> PluginResult<Hello> {
    let body: HelloBody = typed(result)?;
    if body.kinds.len() > 4 {
        return Err(proto("Zu viele Arten"));
    }
    Ok(Hello {
        api: body.api,
        kinds: body.kinds,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapsBody {
    write: bool,
    rename_no_replace: bool,
}

/// What the plugin says about its file system.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FsCapabilities {
    pub write: bool,
    pub rename_no_replace: bool,
}

pub fn parse_capabilities(result: &Value) -> PluginResult<FsCapabilities> {
    let body: CapsBody = typed(result)?;
    Ok(FsCapabilities {
        write: body.write,
        rename_no_replace: body.rename_no_replace,
    })
}

// ---------------------------------------------------------------- entries

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryBody {
    name: String,
    kind: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    modified: Option<i64>,
    #[serde(default)]
    mode: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatBody {
    kind: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    modified: Option<i64>,
    #[serde(default)]
    mode: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListBody {
    entries: Vec<EntryBody>,
}

/// An entry as the plugin reported it; the name is not yet checked (`names::problems`).
#[derive(Clone, Debug, PartialEq)]
pub struct PluginEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: Option<u32>,
}

fn kind_of(text: &str) -> PluginResult<EntryKind> {
    match text {
        "file" => Ok(EntryKind::File),
        "dir" => Ok(EntryKind::Dir),
        "symlink" => Ok(EntryKind::Symlink),
        _ => Err(proto("Unbekannte Eintragsart")),
    }
}

fn time_of(secs: Option<i64>) -> PluginResult<Option<SystemTime>> {
    match secs {
        None => Ok(None),
        Some(secs) if secs.abs() > MAX_TIME_SECS => Err(proto("Ungültige Zeitangabe")),
        Some(secs) if secs >= 0 => Ok(Some(UNIX_EPOCH + Duration::from_secs(secs as u64))),
        Some(secs) => Ok(Some(UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs()))),
    }
}

fn entry_of(
    name: String,
    kind: &str,
    size: Option<u64>,
    modified: Option<i64>,
    mode: Option<u32>,
) -> PluginResult<PluginEntry> {
    if mode.is_some_and(|mode| mode > 0o7777) {
        return Err(proto("Ungültige Rechte"));
    }
    Ok(PluginEntry {
        name,
        kind: kind_of(kind)?,
        size: size.unwrap_or(0),
        modified: time_of(modified)?,
        mode,
    })
}

pub fn parse_list(result: &Value) -> PluginResult<Vec<PluginEntry>> {
    let body: ListBody = typed(result)?;
    if body.entries.len() > MAX_ENTRIES {
        return Err(proto("Zu viele Einträge"));
    }
    body.entries
        .into_iter()
        .map(|e| entry_of(e.name, &e.kind, e.size, e.modified, e.mode))
        .collect()
}

/// The answer to `fs.stat`; the entry carries `name` as given.
pub fn parse_stat(result: &Value, name: &str) -> PluginResult<PluginEntry> {
    let body: StatBody = typed(result)?;
    entry_of(
        name.to_owned(),
        &body.kind,
        body.size,
        body.modified,
        body.mode,
    )
}

// ---------------------------------------------------------------- data

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DataBody {
    data: String,
}

/// The bytes of a `fs.read`/`packer.read` answer, at most `max`.
pub fn parse_data(result: &Value, max: usize) -> PluginResult<Vec<u8>> {
    let body: DataBody = typed(result)?;
    decode_bytes(&body.data, max)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HandleBody {
    handle: u32,
}

pub fn parse_handle(result: &Value) -> PluginResult<u32> {
    Ok(typed::<HandleBody>(result)?.handle)
}

/// An answer that must be an empty object (`mkdir`, `rename`, ...).
pub fn parse_empty(result: &Value) -> PluginResult<()> {
    match result.as_object() {
        Some(map) if map.is_empty() => Ok(()),
        _ => Err(proto("Die Antwort passt nicht zum Schema")),
    }
}

// ---------------------------------------------------------------- column

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValuesBody {
    values: Vec<Option<String>>,
}

/// The column values, one per requested item; control characters are replaced.
pub fn parse_values(result: &Value, expected: usize) -> PluginResult<Vec<Option<String>>> {
    let body: ValuesBody = typed(result)?;
    if body.values.len() != expected {
        return Err(proto("Falsche Anzahl Werte"));
    }
    body.values
        .into_iter()
        .map(|value| match value {
            None => Ok(None),
            Some(text) if text.len() > MAX_VALUE_BYTES => Err(proto("Ein Wert ist zu lang")),
            Some(text) => Ok(Some(
                text.chars()
                    .map(|c| if c.is_control() { '\u{fffd}' } else { c })
                    .collect(),
            )),
        })
        .collect()
}

// ---------------------------------------------------------------- preview

/// What a preview plugin returns: plain text or image bytes, never markup.
#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    Text(String),
    Image { mime: &'static str, bytes: Vec<u8> },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewBody {
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

fn image_mime(mime: &str, bytes: &[u8]) -> Option<&'static str> {
    match mime {
        "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
        "image/jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => Some("image/jpeg"),
        "image/gif" if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") => {
            Some("image/gif")
        }
        "image/webp" if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" => {
            Some("image/webp")
        }
        _ => None,
    }
}

pub fn parse_preview(result: &Value) -> PluginResult<Preview> {
    let body: PreviewBody = typed(result)?;
    match body.kind.as_str() {
        "text" => {
            let text = body.text.ok_or_else(|| proto("Vorschautext fehlt"))?;
            if body.mime.is_some() || body.data.is_some() || text.len() > MAX_PREVIEW_TEXT {
                return Err(proto("Die Vorschau passt nicht zum Schema"));
            }
            Ok(Preview::Text(
                text.chars()
                    .map(|c| {
                        if c.is_control() && c != '\n' && c != '\t' {
                            '\u{fffd}'
                        } else {
                            c
                        }
                    })
                    .collect(),
            ))
        }
        "image" => {
            let (mime, data) = match (body.mime, body.data, body.text) {
                (Some(mime), Some(data), None) => (mime, data),
                _ => return Err(proto("Die Vorschau passt nicht zum Schema")),
            };
            let bytes = decode_bytes(&data, MAX_PREVIEW_IMAGE)?;
            let mime =
                image_mime(&mime, &bytes).ok_or_else(|| proto("Kein erlaubtes Bildformat"))?;
            Ok(Preview::Image { mime, bytes })
        }
        _ => Err(proto("Unbekannte Vorschauart")),
    }
}

// ---------------------------------------------------------------- packer

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackerEntryBody {
    path: String,
    kind: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackerListBody {
    entries: Vec<PackerEntryBody>,
}

/// An archive member a packer plugin reports; `problem` marks an unusable path.
#[derive(Clone, Debug, PartialEq)]
pub struct PackerEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub problem: Option<&'static str>,
}

pub fn parse_packer_list(result: &Value) -> PluginResult<Vec<PackerEntry>> {
    let body: PackerListBody = typed(result)?;
    if body.entries.len() > MAX_PACKER_ENTRIES {
        return Err(proto("Zu viele Einträge"));
    }
    let mut entries = Vec::with_capacity(body.entries.len());
    for raw in body.entries {
        let is_dir = match raw.kind.as_str() {
            "file" => false,
            "dir" => true,
            _ => return Err(proto("Unbekannte Eintragsart")),
        };
        let problem = crate::names::archive_member_path(&raw.path)
            .is_none()
            .then_some("Ungültiger Pfad im Archiv");
        entries.push(PackerEntry {
            path: raw.path,
            is_dir,
            size: raw.size.unwrap_or(0),
            problem,
        });
    }
    // the same path twice (or differing only by case) marks all members
    let mut counts = std::collections::HashMap::new();
    for entry in &entries {
        *counts.entry(entry.path.to_lowercase()).or_insert(0usize) += 1;
    }
    for entry in entries.iter_mut() {
        if entry.problem.is_none() && counts[&entry.path.to_lowercase()] > 1 {
            entry.problem = Some("Der Pfad kollidiert mit einem anderen Eintrag");
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(value: Value) -> Vec<u8> {
        value.to_string().into_bytes()
    }

    #[test]
    fn answers_are_checked_strictly() {
        let ok = parse_response(&line(json!({"id": 3, "ok": true, "result": {}})), 3).unwrap();
        assert!(ok.is_ok());
        // wrong id, missing/extra parts, unknown fields, non-object result
        for bad in [
            json!({"id": 4, "ok": true, "result": {}}),
            json!({"id": 3, "ok": true}),
            json!({"id": 3, "ok": true, "result": []}),
            json!({"id": 3, "ok": true, "result": {}, "extra": 1}),
            json!({"id": 3, "ok": true, "result": {}, "error": {"code": "other"}}),
            json!({"id": 3, "ok": false, "result": {}}),
            json!({"id": 3, "ok": false}),
            json!({"id": 3, "ok": false, "error": {"code": "weird"}}),
            json!({"id": 3, "ok": false, "error": {"code": "other", "x": 1}}),
            json!({"id": "3", "ok": true, "result": {}}),
            json!({"ok": true, "result": {}}),
            json!([1]),
        ] {
            assert!(
                matches!(
                    parse_response(&line(bad.clone()), 3),
                    Err(PluginError::Protocol(_))
                ),
                "{bad}"
            );
        }
        assert!(parse_response(b"not json", 3).is_err());
        assert!(parse_response(&[0xff, 0xfe], 3).is_err());
        assert!(parse_response(b"", 3).is_err());
    }

    #[test]
    fn a_plugin_error_answer_keeps_the_text_out_of_the_message() {
        let secret = "SECRET-PLUGIN-TEXT\n\u{1b}[31m";
        let answer = parse_response(
            &line(json!({"id": 1, "ok": false, "error": {"code": "not_found", "message": secret}})),
            1,
        )
        .unwrap()
        .unwrap_err();
        let shown = answer.to_string();
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(matches!(
            answer,
            PluginError::Reported {
                code: ErrorCode::NotFound,
                ..
            }
        ));
        let diagnostic = answer.diagnostic().unwrap();
        assert!(diagnostic.contains("SECRET") && !diagnostic.chars().any(char::is_control));
        let long = parse_response(
            &line(json!({"id": 1, "ok": false, "error": {"code": "other", "message": "x".repeat(10_000)}})),
            1,
        )
        .unwrap()
        .unwrap_err();
        assert!(long.diagnostic().unwrap().len() <= crate::error::MAX_DIAGNOSTIC_BYTES);
    }

    #[test]
    fn listings_are_bounded_and_typed() {
        let good = json!({"entries": [
            {"name": "a", "kind": "file", "size": 3, "modified": 100, "mode": 420},
            {"name": "d", "kind": "dir"}
        ]});
        let entries = parse_list(&good).unwrap();
        assert_eq!(entries[0].size, 3);
        assert_eq!(
            entries[0].modified,
            Some(UNIX_EPOCH + Duration::from_secs(100))
        );
        assert_eq!(entries[1].kind, EntryKind::Dir);
        for bad in [
            json!({"entries": [{"name": "a", "kind": "fifo"}]}),
            json!({"entries": [{"name": "a", "kind": "file", "owner": "x"}]}),
            json!({"entries": [{"name": "a", "kind": "file", "size": -1}]}),
            json!({"entries": [{"name": "a", "kind": "file", "size": "3"}]}),
            json!({"entries": [{"name": "a", "kind": "file", "mode": 99999}]}),
            json!({"entries": [{"name": "a", "kind": "file", "modified": i64::MAX}]}),
            json!({"entries": [{"kind": "file"}]}),
            json!({"entries": 5}),
            json!({"entries": [], "more": true}),
            json!({}),
        ] {
            assert!(parse_list(&bad).is_err(), "{bad}");
        }
        let many: Vec<Value> = (0..=MAX_ENTRIES)
            .map(|i| json!({"name": format!("n{i}"), "kind": "file"}))
            .collect();
        assert!(parse_list(&json!({"entries": many})).is_err());
    }

    #[test]
    fn data_is_decoded_strictly_and_bounded() {
        let encoded = encode_bytes(b"hello");
        assert_eq!(parse_data(&json!({"data": encoded}), 10).unwrap(), b"hello");
        assert!(parse_data(&json!({"data": encoded}), 4).is_err());
        assert!(parse_data(&json!({"data": "!!!"}), 10).is_err());
        assert!(
            parse_data(&json!({"data": "aGVsbG8"}), 10).is_err(),
            "padding is required"
        );
        assert!(parse_data(&json!({"data": encoded, "extra": 1}), 10).is_err());
        assert!(parse_data(&json!({"data": 5}), 10).is_err());
        assert!(parse_data(&json!({"data": "A".repeat(1000)}), 10).is_err());
    }

    #[test]
    fn column_values_must_match_the_request() {
        let ok = parse_values(&json!({"values": ["a", null, "b\nc"]}), 3).unwrap();
        assert_eq!(ok[2].as_deref(), Some("b\u{fffd}c"));
        assert!(parse_values(&json!({"values": ["a"]}), 2).is_err());
        assert!(parse_values(&json!({"values": [1]}), 1).is_err());
        assert!(parse_values(&json!({"values": ["x".repeat(MAX_VALUE_BYTES + 1)]}), 1).is_err());
    }

    #[test]
    fn previews_are_text_or_known_images_never_markup() {
        let png = encode_bytes(b"\x89PNG\r\n\x1a\nrest");
        assert_eq!(
            parse_preview(&json!({"kind": "text", "text": "a\u{1b}b\nc"})).unwrap(),
            Preview::Text("a\u{fffd}b\nc".into())
        );
        assert!(matches!(
            parse_preview(&json!({"kind": "image", "mime": "image/png", "data": png})).unwrap(),
            Preview::Image {
                mime: "image/png",
                ..
            }
        ));
        for bad in [
            json!({"kind": "html", "text": "<b>x</b>"}),
            json!({"kind": "image", "mime": "image/svg+xml", "data": encode_bytes(b"<svg/>")}),
            json!({"kind": "image", "mime": "image/png", "data": encode_bytes(b"not a png")}),
            json!({"kind": "image", "mime": "text/html", "data": png}),
            json!({"kind": "text"}),
            json!({"kind": "text", "text": "x", "data": png}),
            json!({"kind": "image", "mime": "image/png", "data": png, "text": "x"}),
        ] {
            assert!(parse_preview(&bad).is_err(), "{bad}");
        }
        assert!(
            parse_preview(&json!({"kind": "text", "text": "x".repeat(MAX_PREVIEW_TEXT + 1)}))
                .is_err()
        );
    }

    #[test]
    fn packer_entries_mark_bad_and_colliding_paths() {
        let list = parse_packer_list(&json!({"entries": [
            {"path": "ok/a.txt", "kind": "file", "size": 1},
            {"path": "../evil", "kind": "file"},
            {"path": "/abs", "kind": "file"},
            {"path": "D", "kind": "dir"},
            {"path": "d", "kind": "dir"},
        ]}))
        .unwrap();
        assert!(list[0].problem.is_none());
        assert!(list[1].problem.is_some() && list[2].problem.is_some());
        assert!(list[3].problem.is_some() && list[4].problem.is_some());
        assert!(parse_packer_list(&json!({"entries": [{"path": "a", "kind": "link"}]})).is_err());
    }

    #[test]
    fn hello_and_capabilities_are_strict() {
        let hello = parse_hello(&json!({"api": 1, "kinds": ["filesystem", "column"]})).unwrap();
        assert_eq!(hello.api, 1);
        assert!(parse_hello(&json!({"api": 1, "kinds": ["exec"]})).is_err());
        assert!(parse_hello(&json!({"api": 1})).is_err());
        assert!(parse_hello(&json!({"api": 1, "kinds": [], "x": 1})).is_err());
        assert!(
            parse_capabilities(&json!({"write": true, "rename_no_replace": false}))
                .unwrap()
                .write
        );
        assert!(parse_capabilities(&json!({"write": true})).is_err());
        assert!(parse_handle(&json!({"handle": 7})).is_ok());
        assert!(parse_handle(&json!({"handle": -1})).is_err());
        assert!(parse_empty(&json!({})).is_ok() && parse_empty(&json!({"a": 1})).is_err());
    }
}
