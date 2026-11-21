//! Example plugin for Dualfile (protocol API 1), written without any library so it builds as a
//! small, statically linked program. It serves a fixed in-memory folder tree (read-only), the
//! column values `len` and `upper`, a text/image preview and a tiny archive format `xyz`
//! (lines `path<TAB>content`).
//!
//! Compiled with `--cfg testbin` it is the test double of the host tests: the tree is
//! writable and it carries `debug_misbehave` modes (endless loop, crash, huge or broken
//! answers, standard-error flood, network, write/read outside the plugin folder, shell) and
//! a `mode` file in its own folder that changes the first exchange. None of this is in the
//! normal build.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

// ---------------------------------------------------------------- JSON (just enough)

#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    fn int(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            Json::Int(n) => Some(*n),
            _ => None,
        }
    }
    fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(n) => out.push_str(&n.to_string()),
            Json::Str(s) => write_str(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_str(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }
    fn eat(&mut self, byte: u8) -> Option<()> {
        self.ws();
        (self.bytes.get(self.at) == Some(&byte)).then(|| self.at += 1)
    }
    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.bytes.get(self.at)? {
            b'n' => self.word("null", Json::Null),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'"' => self.string().map(Json::Str),
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Json::Arr(items));
                }
                loop {
                    items.push(self.value()?);
                    if self.eat(b',').is_some() {
                        continue;
                    }
                    self.eat(b']')?;
                    return Some(Json::Arr(items));
                }
            }
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                if self.eat(b'}').is_some() {
                    return Some(Json::Obj(fields));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.eat(b':')?;
                    fields.push((key, self.value()?));
                    if self.eat(b',').is_some() {
                        continue;
                    }
                    self.eat(b'}')?;
                    return Some(Json::Obj(fields));
                }
            }
            _ => self.number(),
        }
    }
    fn word(&mut self, text: &str, value: Json) -> Option<Json> {
        if self.bytes[self.at..].starts_with(text.as_bytes()) {
            self.at += text.len();
            Some(value)
        } else {
            None
        }
    }
    fn number(&mut self) -> Option<Json> {
        let start = self.at;
        while self.at < self.bytes.len()
            && (self.bytes[self.at] == b'-' || self.bytes[self.at].is_ascii_digit())
        {
            self.at += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.at])
            .ok()?
            .parse()
            .ok()
            .map(Json::Int)
    }
    fn string(&mut self) -> Option<String> {
        if self.bytes.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            // Validate only the current character: checking the whole rest per character was
            // quadratic and made 256 KiB write chunks take seconds.
            let first = *self.bytes.get(self.at)?;
            let width = match first {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => return None,
            };
            let ch = std::str::from_utf8(self.bytes.get(self.at..self.at + width)?)
                .ok()?
                .chars()
                .next()?;
            self.at += ch.len_utf8();
            match ch {
                '"' => return Some(out),
                '\\' => {
                    let esc = *self.bytes.get(self.at)?;
                    self.at += 1;
                    match esc {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'/' => out.push('/'),
                        b'\\' => out.push('\\'),
                        b'"' => out.push('"'),
                        b'u' => {
                            let hex =
                                std::str::from_utf8(self.bytes.get(self.at..self.at + 4)?).ok()?;
                            self.at += 4;
                            out.push(
                                char::from_u32(u32::from_str_radix(hex, 16).ok()?)
                                    .unwrap_or('\u{fffd}'),
                            );
                        }
                        _ => return None,
                    }
                }
                c => out.push(c),
            }
        }
    }
}

fn parse(line: &[u8]) -> Option<Json> {
    let mut parser = Parser { bytes: line, at: 0 };
    let value = parser.value()?;
    parser.ws();
    (parser.at == line.len()).then_some(value)
}

// ---------------------------------------------------------------- base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let bytes: Vec<u8> = text.bytes().filter(|b| *b != b'=').collect();
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, b) in chunk.iter().enumerate() {
            let v = B64.iter().position(|c| c == b)? as u32;
            n |= v << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- the file tree

#[derive(Clone)]
enum Node {
    Dir,
    File(Vec<u8>),
}

struct Tree {
    nodes: BTreeMap<String, Node>,
    writes: BTreeMap<i64, (String, Vec<u8>)>,
    next_handle: i64,
}

const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f,
    0x00, 0x05, 0xfe, 0x02, 0xfe, 0xa7, 0x35, 0x81, 0x84, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

impl Tree {
    fn fixed() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert("/".to_string(), Node::Dir);
        for dir in ["/docs", "/docs/sub", "/evil", "/dup", "/many"] {
            nodes.insert(dir.to_string(), Node::Dir);
        }
        let big: Vec<u8> = (0..700_000u32)
            .map(|i| (i.wrapping_mul(7) % 251) as u8)
            .collect();
        for (path, data) in [
            ("/hello.txt", b"Hallo Welt\n".to_vec()),
            (
                "/readme.md",
                b"# Beispiel\nDies ist ein Beispiel-Plugin.\n".to_vec(),
            ),
            ("/big.bin", big),
            ("/picture.png", TINY_PNG.to_vec()),
            ("/docs/a.txt", b"A".to_vec()),
            ("/docs/b.txt", b"B".to_vec()),
            ("/docs/sub/deep.txt", b"deep".to_vec()),
            ("/evil/ok.txt", b"ok".to_vec()),
            ("/dup/Case.txt", b"1".to_vec()),
            ("/dup/case.txt", b"2".to_vec()),
        ] {
            nodes.insert(path.to_string(), Node::File(data));
        }
        Tree {
            nodes,
            writes: BTreeMap::new(),
            next_handle: 1,
        }
    }

    fn children(&self, dir: &str) -> Vec<(String, &Node)> {
        let prefix = if dir == "/" {
            "/".to_string()
        } else {
            format!("{dir}/")
        };
        self.nodes
            .iter()
            .filter(|(path, _)| {
                path.len() > prefix.len()
                    && path.starts_with(&prefix)
                    && !path[prefix.len()..].contains('/')
            })
            .map(|(path, node)| (path[prefix.len()..].to_string(), node))
            .collect()
    }
}

fn entry_json(name: Option<&str>, node: &Node) -> Json {
    let mut fields = Vec::new();
    if let Some(name) = name {
        fields.push(("name".to_string(), Json::Str(name.to_string())));
    }
    match node {
        Node::Dir => fields.push(("kind".into(), Json::Str("dir".into()))),
        Node::File(data) => {
            fields.push(("kind".into(), Json::Str("file".into())));
            fields.push(("size".into(), Json::Int(data.len() as i64)));
            fields.push(("modified".into(), Json::Int(1_700_000_000)));
        }
    }
    Json::Obj(fields)
}

// ---------------------------------------------------------------- operations

type Reply = Result<Json, (&'static str, String)>;

fn obj(fields: Vec<(&str, Json)>) -> Json {
    Json::Obj(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn err(code: &'static str, message: &str) -> Reply {
    Err((code, message.to_string()))
}

fn path_of(request: &Json, key: &str) -> Result<String, (&'static str, String)> {
    match request.str(key) {
        Some(path) if path.starts_with('/') => Ok(path.to_string()),
        _ => Err(("invalid", "path".into())),
    }
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => path[..i].to_string(),
    }
}

fn handle(tree: &mut Tree, request: &Json) -> Reply {
    let op = request.str("op").unwrap_or("");
    match op {
        "hello" => Ok(obj(vec![
            ("api", Json::Int(api_version())),
            (
                "kinds",
                Json::Arr(
                    ["filesystem", "packer", "column", "preview"]
                        .iter()
                        .map(|k| Json::Str((*k).into()))
                        .collect(),
                ),
            ),
        ])),
        "fs.capabilities" => Ok(obj(vec![
            ("write", Json::Bool(cfg!(testbin))),
            ("rename_no_replace", Json::Bool(cfg!(testbin))),
        ])),
        "fs.list" => {
            let path = path_of(request, "path")?;
            match tree.nodes.get(&path) {
                None => err("not_found", "no such folder"),
                Some(Node::File(_)) => err("not_a_directory", "is a file"),
                Some(Node::Dir) => {
                    let mut entries: Vec<Json> = tree
                        .children(&path)
                        .iter()
                        .map(|(name, node)| entry_json(Some(name), node))
                        .collect();
                    if path == "/evil" {
                        // hostile names a plugin must not be able to smuggle in
                        for name in ["..", "a/b", "back\\slash", "ctl\u{1}x", ".", ""] {
                            entries.push(entry_json(Some(name), &Node::File(b"x".to_vec())));
                        }
                    }
                    if path == "/many" {
                        for i in 0..1000 {
                            entries.push(entry_json(
                                Some(&format!("f{i:04}.txt")),
                                &Node::File(Vec::new()),
                            ));
                        }
                    }
                    Ok(obj(vec![("entries", Json::Arr(entries))]))
                }
            }
        }
        "fs.stat" => {
            let path = path_of(request, "path")?;
            match tree.nodes.get(&path) {
                None => err("not_found", "no such entry"),
                Some(node) => Ok(entry_json(None, node)),
            }
        }
        "fs.read" => {
            let path = path_of(request, "path")?;
            let offset = request.int("offset").unwrap_or(0).max(0) as usize;
            let len = request.int("len").unwrap_or(0).max(0) as usize;
            match tree.nodes.get(&path) {
                None => err("not_found", "no such file"),
                Some(Node::Dir) => err("is_a_directory", "is a folder"),
                Some(Node::File(data)) => {
                    let start = offset.min(data.len());
                    let end = (start + len).min(data.len());
                    Ok(obj(vec![(
                        "data",
                        Json::Str(b64_encode(&data[start..end])),
                    )]))
                }
            }
        }
        "fs.mkdir" | "fs.rename" | "fs.remove" | "fs.write_open" | "fs.write_chunk"
        | "fs.write_close" => write_op(tree, op, request),
        "column.values" => {
            let column = request.str("column").unwrap_or("");
            let Some(Json::Arr(items)) = request.get("items") else {
                return err("invalid", "items");
            };
            let values = items
                .iter()
                .map(|item| {
                    let name = item.str("name").unwrap_or("");
                    match column {
                        "len" => Json::Str(name.chars().count().to_string()),
                        "upper" => Json::Str(name.to_uppercase()),
                        _ => Json::Null,
                    }
                })
                .collect();
            Ok(obj(vec![("values", Json::Arr(values))]))
        }
        "preview.render" => {
            let name = request.str("name").unwrap_or("");
            let data = b64_decode(request.str("data").unwrap_or(""))
                .ok_or(("invalid", "data".to_string()))?;
            #[cfg(testbin)]
            match startup_mode().as_str() {
                // answers a real preview request with markup / a vector image: both must be refused
                "html_preview_always" => {
                    return Ok(obj(vec![
                        ("kind", Json::Str("html".into())),
                        ("text", Json::Str("<script>1</script>".into())),
                    ]))
                }
                "svg_preview_always" => {
                    return Ok(obj(vec![
                        ("kind", Json::Str("image".into())),
                        ("mime", Json::Str("image/svg+xml".into())),
                        ("data", Json::Str(b64_encode(b"<svg xmlns='x'/>"))),
                    ]))
                }
                _ => {}
            }
            if name.ends_with(".png") {
                return Ok(obj(vec![
                    ("kind", Json::Str("image".into())),
                    ("mime", Json::Str("image/png".into())),
                    ("data", Json::Str(b64_encode(TINY_PNG))),
                ]));
            }
            let max = request.int("max_chars").unwrap_or(200).clamp(1, 10_000) as usize;
            let text = String::from_utf8_lossy(&data);
            let first: String = text
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(max)
                .collect();
            Ok(obj(vec![
                ("kind", Json::Str("text".into())),
                ("text", Json::Str(format!("Vorschau: {first}"))),
            ]))
        }
        "packer.list" | "packer.read" => {
            let data = b64_decode(request.str("data").unwrap_or(""))
                .ok_or(("invalid", "data".to_string()))?;
            let text = String::from_utf8_lossy(&data).into_owned();
            let members: Vec<(&str, &str)> = text
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .collect();
            if op == "packer.list" {
                let entries = members
                    .iter()
                    .map(|(path, content)| {
                        obj(vec![
                            ("path", Json::Str((*path).into())),
                            ("kind", Json::Str("file".into())),
                            ("size", Json::Int(content.len() as i64)),
                        ])
                    })
                    .collect();
                Ok(obj(vec![("entries", Json::Arr(entries))]))
            } else {
                let wanted = request.str("path").unwrap_or("");
                match members.iter().find(|(path, _)| *path == wanted) {
                    Some((_, content)) => Ok(obj(vec![(
                        "data",
                        Json::Str(b64_encode(content.as_bytes())),
                    )])),
                    None => err("not_found", "no such member"),
                }
            }
        }
        _ => misbehave_or_unknown(op, request),
    }
}

fn write_op(tree: &mut Tree, op: &str, request: &Json) -> Reply {
    if !cfg!(testbin) {
        return err("unsupported", "read-only tree");
    }
    match op {
        "fs.mkdir" => {
            let path = path_of(request, "path")?;
            if tree.nodes.contains_key(&path) {
                return err("exists", "exists");
            }
            if !matches!(tree.nodes.get(&parent_of(&path)), Some(Node::Dir)) {
                return err("not_found", "parent");
            }
            tree.nodes.insert(path, Node::Dir);
            Ok(obj(vec![]))
        }
        "fs.rename" => {
            let (from, to) = (path_of(request, "from")?, path_of(request, "to")?);
            let replace = request.bool("replace").unwrap_or(false);
            if !tree.nodes.contains_key(&from) {
                return err("not_found", "source");
            }
            if tree.nodes.contains_key(&to) && !replace {
                return err("exists", "target");
            }
            if !matches!(tree.nodes.get(&parent_of(&to)), Some(Node::Dir)) {
                return err("not_found", "parent");
            }
            let moved: Vec<(String, Node)> = tree
                .nodes
                .iter()
                .filter(|(path, _)| **path == from || path.starts_with(&format!("{from}/")))
                .map(|(path, node)| (path.clone(), node.clone()))
                .collect();
            for (path, node) in moved {
                tree.nodes.remove(&path);
                tree.nodes
                    .insert(format!("{to}{}", &path[from.len()..]), node);
            }
            Ok(obj(vec![]))
        }
        "fs.remove" => {
            let path = path_of(request, "path")?;
            match tree.nodes.get(&path) {
                None => err("not_found", "no such entry"),
                Some(Node::Dir) if !tree.children(&path).is_empty() => {
                    err("not_empty", "not empty")
                }
                Some(_) => {
                    tree.nodes.remove(&path);
                    Ok(obj(vec![]))
                }
            }
        }
        "fs.write_open" => {
            let path = path_of(request, "path")?;
            if tree.nodes.contains_key(&path) && !request.bool("replace").unwrap_or(false) {
                return err("exists", "exists");
            }
            if !matches!(tree.nodes.get(&parent_of(&path)), Some(Node::Dir)) {
                return err("not_found", "parent");
            }
            let handle = tree.next_handle;
            tree.next_handle += 1;
            tree.nodes.insert(path.clone(), Node::File(Vec::new()));
            tree.writes.insert(handle, (path, Vec::new()));
            Ok(obj(vec![("handle", Json::Int(handle))]))
        }
        "fs.write_chunk" => {
            let handle = request.int("handle").unwrap_or(-1);
            let data = b64_decode(request.str("data").unwrap_or(""))
                .ok_or(("invalid", "data".to_string()))?;
            match tree.writes.get_mut(&handle) {
                Some((_, buffer)) => buffer.extend_from_slice(&data),
                None => return err("invalid", "handle"),
            }
            Ok(obj(vec![]))
        }
        _ => {
            let handle = request.int("handle").unwrap_or(-1);
            let Some((path, buffer)) = tree.writes.remove(&handle) else {
                return err("invalid", "handle");
            };
            if request.bool("commit").unwrap_or(false) {
                tree.nodes.insert(path, Node::File(buffer));
            } else {
                tree.nodes.remove(&path);
            }
            Ok(obj(vec![]))
        }
    }
}

#[cfg(not(testbin))]
fn api_version() -> i64 {
    1
}
#[cfg(not(testbin))]
fn misbehave_or_unknown(_op: &str, _request: &Json) -> Reply {
    err("unsupported", "unknown operation")
}

// ---------------------------------------------------------------- test double

#[cfg(testbin)]
fn own_dir() -> std::path::PathBuf {
    std::env::args_os()
        .next()
        .map(std::path::PathBuf::from)
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default()
}

/// The `mode` file in the plugin folder (the environment is empty, so this is how a test
/// changes the plugin's behaviour before the first exchange).
#[cfg(testbin)]
fn startup_mode() -> String {
    std::fs::read_to_string(own_dir().join("mode"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(testbin)]
fn api_version() -> i64 {
    if startup_mode() == "api2" {
        2
    } else {
        1
    }
}

#[cfg(testbin)]
fn outcome<T, E: std::fmt::Display>(result: Result<T, E>) -> Json {
    match result {
        Ok(_) => obj(vec![
            ("outcome", Json::Str("allowed".into())),
            ("detail", Json::Str(String::new())),
        ]),
        Err(e) => obj(vec![
            ("outcome", Json::Str("denied".into())),
            ("detail", Json::Str(e.to_string())),
        ]),
    }
}

#[cfg(testbin)]
extern "C" {
    fn syscall(number: i64, ...) -> i64;
}

#[cfg(testbin)]
fn raw_syscall(name: &str, number: i64, a: i64, b: i64) -> (String, Json) {
    // SAFETY: deliberately probing forbidden system calls with harmless arguments.
    let ret = unsafe { syscall(number, a, b, 0i64, 0i64, 0i64, 0i64) };
    let errno = if ret == -1 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    } else {
        0
    };
    (
        name.to_string(),
        Json::Int(if ret == -1 { -(errno as i64) } else { ret }),
    )
}

#[cfg(testbin)]
fn misbehave_or_unknown(op: &str, request: &Json) -> Reply {
    use std::io::Write as _;
    if op != "debug_misbehave" {
        return err("unsupported", "unknown operation");
    }
    let mode = request.str("mode").unwrap_or("").to_string();
    match mode.as_str() {
        "loop" => {
            let mut x = 0u64;
            loop {
                x = std::hint::black_box(x.wrapping_add(1));
            }
        }
        "sleep" => {
            std::thread::sleep(std::time::Duration::from_secs(3600));
            err("other", "woke up")
        }
        "crash" => std::process::abort(),
        "exit" => std::process::exit(3),
        "huge" => {
            let chunk = vec![b'a'; 1 << 20];
            let mut out = std::io::stdout();
            for _ in 0..64 {
                if out.write_all(&chunk).is_err() {
                    break;
                }
            }
            let _ = out.flush();
            std::thread::sleep(std::time::Duration::from_secs(30));
            err("other", "")
        }
        "huge_line" => Ok(obj(vec![("data", Json::Str("A".repeat(6 << 20)))])),
        "badjson" => {
            println!("{{not json");
            std::process::exit(0)
        }
        "stderr_flood" => {
            let junk = vec![b'e'; 64 * 1024];
            let mut err_out = std::io::stderr();
            for _ in 0..80 {
                let _ = err_out.write_all(&junk);
            }
            let _ = err_out.write_all(b"\x1b[31mflood-end\n");
            Ok(obj(vec![]))
        }
        "env" => {
            let vars = std::env::vars_os()
                .map(|(k, v)| {
                    Json::Arr(vec![
                        Json::Str(k.to_string_lossy().into_owned()),
                        Json::Str(v.to_string_lossy().into_owned()),
                    ])
                })
                .collect();
            let args = std::env::args_os()
                .map(|a| Json::Str(a.to_string_lossy().into_owned()))
                .collect();
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(obj(vec![
                ("vars", Json::Arr(vars)),
                ("args", Json::Arr(args)),
                ("cwd", Json::Str(cwd)),
            ]))
        }
        "net" => Ok(outcome(std::net::TcpStream::connect_timeout(
            &"127.0.0.1:9".parse().unwrap(),
            std::time::Duration::from_millis(500),
        ))),
        "udp" => Ok(outcome(std::net::UdpSocket::bind("127.0.0.1:0"))),
        "unix_connect" => Ok(outcome(std::os::unix::net::UnixStream::connect(
            "/run/dbus/system_bus_socket",
        ))),
        "write_tmp" => Ok(outcome(std::fs::write(
            format!("/tmp/dualfile-plugin-escape-{}", std::process::id()),
            b"escaped",
        ))),
        "write_plugin_dir" => Ok(outcome(std::fs::write(
            own_dir().join("escape.txt"),
            b"escaped",
        ))),
        "mkdir_plugin_dir" => Ok(outcome(std::fs::create_dir(own_dir().join("escape-dir")))),
        "read_outside" => Ok(outcome(std::fs::read("/etc/passwd"))),
        "list_outside" => Ok(outcome(std::fs::read_dir("/etc").map(|d| d.count()))),
        "read_own" => Ok(outcome(std::fs::read(own_dir().join("plugin.json")))),
        "shell" => Ok(outcome(
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("echo escaped")
                .status(),
        )),
        "shell_own_dir" => Ok(outcome(
            std::process::Command::new(own_dir().join("example-plugin-testbin")).status(),
        )),
        "thread" => Ok(outcome(
            std::thread::Builder::new().spawn(|| ()).map(|h| h.join()),
        )),
        "syscalls" => {
            let calls = vec![
                raw_syscall("ptrace", 101, 0, 0),
                raw_syscall("mount", 165, 0, 0),
                raw_syscall("unshare", 272, 0x1000_0000, 0),
                raw_syscall("setns", 308, -1, 0),
                raw_syscall("bpf", 321, 0, 0),
                raw_syscall("kexec_load", 246, 0, 0),
                raw_syscall("init_module", 175, 0, 0),
                raw_syscall("socket_inet", 41, 2, 1),
                raw_syscall("socket_netlink", 41, 16, 3),
                raw_syscall("socket_packet", 41, 17, 3),
                raw_syscall("socket_unix", 41, 1, 1),
                raw_syscall("execve", 59, 0, 0),
                raw_syscall("fork", 57, 0, 0),
                raw_syscall("kill_init", 62, 1, 0),
                raw_syscall("memfd_create", 319, 0, 0),
                raw_syscall("io_uring_setup", 425, 1, 0),
                raw_syscall("getpid", 39, 0, 0),
            ];
            Ok(Json::Obj(calls))
        }
        "unsolicited" => {
            println!("{{\"id\":9999,\"ok\":true,\"result\":{{}}}}");
            Ok(obj(vec![]))
        }
        "wrong_id" | "bad_schema" | "html_preview" | "no_newline" => {
            Ok(obj(vec![("mode", Json::Str(mode))]))
        }
        _ => err("invalid", "unknown mode"),
    }
}

// ---------------------------------------------------------------- main loop

fn reply_line(id: i64, reply: &Reply) -> String {
    let mut out = String::new();
    let body = match reply {
        Ok(result) => obj(vec![
            ("id", Json::Int(id)),
            ("ok", Json::Bool(true)),
            ("result", result.clone()),
        ]),
        Err((code, message)) => obj(vec![
            ("id", Json::Int(id)),
            ("ok", Json::Bool(false)),
            (
                "error",
                obj(vec![
                    ("code", Json::Str((*code).into())),
                    ("message", Json::Str(message.clone())),
                ]),
            ),
        ]),
    };
    body.write(&mut out);
    out
}

fn main() {
    #[cfg(testbin)]
    {
        match startup_mode().as_str() {
            "silent" => std::thread::sleep(std::time::Duration::from_secs(3600)),
            "exit_at_start" => std::process::exit(7),
            "garbage" => {
                println!("hello? this is not json");
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
            _ => {}
        }
    }
    let mut tree = Tree::fixed();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = Vec::new();
    let mut input = stdin.lock();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line).into_owned();
        let Some(request) = parse(text.trim_end().as_bytes()) else {
            return;
        };
        let id = request.int("id").unwrap_or(0);
        let reply = handle(&mut tree, &request);
        #[cfg(testbin)]
        {
            let mode = if request.str("op") == Some("debug_misbehave") {
                request.str("mode").unwrap_or("")
            } else {
                ""
            };
            match mode {
                "wrong_id" => {
                    let _ = writeln!(stdout, "{}", reply_line(id + 1, &reply));
                    continue;
                }
                "bad_schema" => {
                    let _ = writeln!(
                        stdout,
                        "{{\"id\":{id},\"ok\":true,\"result\":{{}},\"extra\":1}}"
                    );
                    continue;
                }
                "html_preview" => {
                    let _ = writeln!(stdout, "{{\"id\":{id},\"ok\":true,\"result\":{{\"kind\":\"html\",\"text\":\"<script>1</script>\"}}}}");
                    continue;
                }
                "no_newline" => {
                    let _ = write!(stdout, "{}", reply_line(id, &reply));
                    let _ = stdout.flush();
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    continue;
                }
                _ => {}
            }
        }
        if writeln!(stdout, "{}", reply_line(id, &reply))
            .and_then(|()| stdout.flush())
            .is_err()
        {
            return;
        }
    }
}
