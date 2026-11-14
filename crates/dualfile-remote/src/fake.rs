//! An FTP server kept in memory, for tests of the layers above the wire (no sockets).
//! It follows the protocol closely enough to catch mistakes: transfers are a start /
//! data / finish sequence, `STOR` truncates, `APPE` appends, `RNTO` replaces by default
//! (like many real servers), a lost connection keeps the bytes that had arrived, and
//! everything the client sends is logged (passwords as `***`).

use crate::error::{TResult, TransportError};
use crate::passive::{parse_pasv, resolve_data_addr};
use crate::profile::{ConnectParams, TlsMode};
use crate::secret::Secret;
use crate::transport::{Features, FtpTransport, Limits, StoreMode};
use crate::trust::{CertObservation, CertVerifier, PinKey, Protocol};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub enum Node {
    Dir,
    File { data: Vec<u8>, mtime: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListStyle {
    Mlsd,
    UnixList,
}

#[derive(Debug)]
pub struct ServerState {
    pub nodes: BTreeMap<String, Node>,
    pub user: String,
    pub password: String,
    pub features: Features,
    /// Answer `LIST` in Unix format (and refuse `MLSD`) if `UnixList`.
    pub style: ListStyle,
    pub log: Vec<String>,
    /// Bump to cut every open connection.
    pub generation: u64,
    pub connects: u32,
    pub refuse_connects_after: Option<u32>,
    /// The failed-login reply repeats the password it was given (a hostile or sloppy server).
    pub echo_password: bool,
    /// The next command never gets an answer.
    pub hang_next: bool,
    /// Uploads lose the connection after this many bytes (per upload).
    pub cut_upload_after: Option<usize>,
    /// Raw lines answered for every listing, instead of the real content.
    pub raw_listing: Option<Vec<String>>,
    /// The text of the `PASV` reply.
    pub pasv_reply: Option<String>,
    pub control_ip: IpAddr,
    /// Addresses the client opened data connections to.
    pub data_connects: Vec<SocketAddr>,
    /// `RNTO` onto an existing file replaces it (true) or fails with 550 (false).
    pub rename_replaces: bool,
    /// Every error reply repeats this text (to look for a leaking password).
    pub echo_in_errors: Option<String>,
    /// The certificate the server presents (FTPS modes).
    pub cert: Option<CertObservation>,
    /// Entries created by "another client" right before the next rename.
    pub inject_before_rename: Option<(String, Vec<u8>)>,
    /// The next command fails with this reply.
    pub fail_next: Option<(u16, String)>,
    /// Bytes received by uploads so far.
    pub uploaded: usize,
}

#[derive(Clone)]
pub struct FakeServer(pub Arc<Mutex<ServerState>>);

impl FakeServer {
    pub fn new() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert("/".to_owned(), Node::Dir);
        Self(Arc::new(Mutex::new(ServerState {
            nodes,
            user: "user".into(),
            password: "secret".into(),
            features: Features {
                mlsd: true,
                mlst: true,
                size: true,
                mdtm: true,
                rest_stream: true,
            },
            style: ListStyle::Mlsd,
            log: Vec::new(),
            generation: 0,
            connects: 0,
            refuse_connects_after: None,
            echo_password: false,
            hang_next: false,
            cut_upload_after: None,
            raw_listing: None,
            pasv_reply: None,
            control_ip: "203.0.113.7".parse().unwrap(),
            data_connects: Vec::new(),
            rename_replaces: true,
            echo_in_errors: None,
            cert: None,
            inject_before_rename: None,
            fail_next: None,
            uploaded: 0,
        })))
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, ServerState> {
        self.0.lock().unwrap()
    }

    pub fn put(&self, path: &str, data: &[u8]) {
        let mut state = self.state();
        // parents
        let mut at = String::new();
        for part in path
            .trim_start_matches('/')
            .split('/')
            .collect::<Vec<_>>()
            .split_last()
            .map(|(_, rest)| rest.to_vec())
            .unwrap_or_default()
        {
            at.push('/');
            at.push_str(part);
            state.nodes.entry(at.clone()).or_insert(Node::Dir);
        }
        state.nodes.insert(
            path.to_owned(),
            Node::File {
                data: data.to_vec(),
                mtime: 1_700_000_000,
            },
        );
    }

    pub fn mkdirs(&self, path: &str) {
        let mut state = self.state();
        let mut at = String::new();
        for part in path
            .trim_start_matches('/')
            .split('/')
            .filter(|p| !p.is_empty())
        {
            at.push('/');
            at.push_str(part);
            state.nodes.entry(at.clone()).or_insert(Node::Dir);
        }
    }

    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        match self.state().nodes.get(path) {
            Some(Node::File { data, .. }) => Some(data.clone()),
            _ => None,
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.state().nodes.contains_key(path)
    }

    pub fn names_in(&self, dir: &str) -> Vec<String> {
        let prefix = if dir == "/" {
            "/".to_owned()
        } else {
            format!("{dir}/")
        };
        self.state()
            .nodes
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter(|rest| !rest.is_empty() && !rest.contains('/'))
            .map(str::to_owned)
            .collect()
    }

    pub fn log(&self) -> Vec<String> {
        self.state().log.clone()
    }

    pub fn disconnect_everybody(&self) {
        self.state().generation += 1;
    }

    pub fn connect(&self, user: &str, password: &Secret) -> TResult<FakeTransport> {
        let mut state = self.state();
        state.connects += 1;
        if let Some(limit) = state.refuse_connects_after {
            if state.connects > limit {
                return Err(TransportError::Connection("Verbindung abgelehnt".into()));
            }
        }
        state.log.push(format!("USER {user}"));
        state.log.push("PASS ***".into());
        if user != state.user || password.expose() != state.password {
            let text = if state.echo_password {
                format!("Login incorrect for {user} with {}", password.expose())
            } else {
                "Login incorrect".to_owned()
            };
            return Err(TransportError::reply(530, &text));
        }
        let generation = state.generation;
        drop(state);
        Ok(FakeTransport {
            server: self.clone(),
            generation,
            dead: false,
            transfer: Transfer::None,
            quit: false,
        })
    }
}

impl Default for FakeServer {
    fn default() -> Self {
        Self::new()
    }
}

enum Transfer {
    None,
    Download { data: Vec<u8>, pos: usize },
    Upload { path: String, count: usize },
}

pub struct FakeTransport {
    server: FakeServer,
    generation: u64,
    dead: bool,
    transfer: Transfer,
    quit: bool,
}

impl std::fmt::Debug for FakeTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeTransport").finish_non_exhaustive()
    }
}

fn plain(path: &str) -> TResult<&str> {
    if path.chars().any(char::is_control) {
        Err(TransportError::Rejected)
    } else {
        Ok(path)
    }
}

fn parent_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".to_owned(),
        Some((parent, _)) => parent.to_owned(),
    }
}

fn mdtm_text(secs: u64) -> String {
    // 1_700_000_000 = 2023-11-14 22:13:20 UTC; computed from the civil date for any value
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn month_name(m: u32) -> &'static str {
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(m - 1) as usize]
}

impl FakeTransport {
    fn begin(&mut self, command: String) -> TResult<std::sync::MutexGuard<'_, ServerState>> {
        if self.dead || self.quit {
            return Err(TransportError::Connection("Verbindung getrennt".into()));
        }
        let mut state = self.server.0.lock().unwrap();
        if state.generation != self.generation {
            self.dead = true;
            return Err(TransportError::Connection("Verbindung getrennt".into()));
        }
        if state.hang_next {
            state.hang_next = false;
            self.dead = true;
            return Err(TransportError::Timeout);
        }
        state.log.push(command);
        if let Some((code, text)) = state.fail_next.take() {
            return Err(Self::fail(&state, code, &text));
        }
        Ok(state)
    }

    fn fail(state: &ServerState, code: u16, text: &str) -> TransportError {
        match &state.echo_in_errors {
            Some(echo) => TransportError::reply(code, &format!("{text} [{echo}]")),
            None => TransportError::reply(code, text),
        }
    }

    fn open_data(&mut self) -> TResult<()> {
        let mut state = self.server.0.lock().unwrap();
        let control = state.control_ip;
        let reply = state.pasv_reply.clone().unwrap_or_else(|| {
            let octets = match control {
                IpAddr::V4(ip) => ip.octets(),
                IpAddr::V6(_) => [127, 0, 0, 1],
            };
            format!(
                "227 Entering Passive Mode ({},{},{},{},195,80)",
                octets[0], octets[1], octets[2], octets[3]
            )
        });
        let (ip, port) = parse_pasv(&reply)?;
        let target = resolve_data_addr(control, SocketAddr::new(IpAddr::V4(ip), port))?;
        state.data_connects.push(target);
        Ok(())
    }

    fn list_lines(state: &ServerState, dir: &str, mlsd: bool) -> TResult<Vec<String>> {
        match state.nodes.get(dir) {
            Some(Node::Dir) => {}
            Some(_) => return Err(Self::fail(state, 550, "Not a directory")),
            None => return Err(Self::fail(state, 550, "No such file or directory")),
        }
        let prefix = if dir == "/" {
            "/".to_owned()
        } else {
            format!("{dir}/")
        };
        let mut lines = Vec::new();
        for (path, node) in &state.nodes {
            let Some(rest) = path.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() || rest.contains('/') {
                continue;
            }
            lines.push(match (mlsd, node) {
                (true, Node::Dir) => format!("type=dir;modify=20231114221320; {rest}"),
                (true, Node::File { data, mtime }) => {
                    format!(
                        "type=file;size={};modify={}; {rest}",
                        data.len(),
                        mdtm_text(*mtime)
                    )
                }
                (false, Node::Dir) => format!("drwxr-xr-x 2 ftp ftp 4096 Nov 14  2023 {rest}"),
                (false, Node::File { data, mtime }) => {
                    let date = mdtm_text(*mtime);
                    format!(
                        "-rw-r--r-- 1 ftp ftp {} {} {:>2}  {} {rest}",
                        data.len(),
                        month_name(date[4..6].parse().unwrap()),
                        date[6..8].parse::<u32>().unwrap(),
                        &date[0..4]
                    )
                }
            });
        }
        Ok(lines)
    }
}

impl FtpTransport for FakeTransport {
    fn features(&self) -> Features {
        self.server.0.lock().unwrap().features
    }

    fn list(&mut self, path: &str, mlsd: bool, limits: &Limits) -> TResult<Vec<String>> {
        plain(path)?;
        let state = self.begin(format!("{} {path}", if mlsd { "MLSD" } else { "LIST" }))?;
        if mlsd && (!state.features.mlsd || state.style == ListStyle::UnixList) {
            return Err(Self::fail(&state, 500, "MLSD not understood"));
        }
        let lines = match &state.raw_listing {
            Some(raw) => raw.clone(),
            None => Self::list_lines(&state, path, mlsd)?,
        };
        drop(state);
        self.open_data()?;
        // the same bounds the real transport applies while reading
        let mut bytes = 0usize;
        let mut kept = Vec::new();
        for (index, line) in lines.into_iter().enumerate() {
            if index >= limits.max_list_lines {
                return Err(TransportError::TooLarge("Verzeichnisliste"));
            }
            bytes += line.len() + 2;
            if bytes > limits.max_list_bytes {
                return Err(TransportError::TooLarge("Verzeichnisliste"));
            }
            if line.len() <= limits.max_line_len {
                kept.push(line);
            }
        }
        Ok(kept)
    }

    fn mlst(&mut self, path: &str) -> TResult<String> {
        plain(path)?;
        let state = self.begin(format!("MLST {path}"))?;
        if !state.features.mlst {
            return Err(Self::fail(&state, 500, "MLST not understood"));
        }
        match state.nodes.get(path) {
            Some(Node::Dir) => Ok(format!(" type=dir;modify=20231114221320; {path}")),
            Some(Node::File { data, mtime }) => Ok(format!(
                " type=file;size={};modify={}; {path}",
                data.len(),
                mdtm_text(*mtime)
            )),
            None => Err(Self::fail(&state, 550, "No such file or directory")),
        }
    }

    fn size(&mut self, path: &str) -> TResult<u64> {
        plain(path)?;
        let state = self.begin(format!("SIZE {path}"))?;
        match state.nodes.get(path) {
            Some(Node::File { data, .. }) => Ok(data.len() as u64),
            _ => Err(Self::fail(&state, 550, "No such file")),
        }
    }

    fn mdtm(&mut self, path: &str) -> TResult<SystemTime> {
        plain(path)?;
        let state = self.begin(format!("MDTM {path}"))?;
        match state.nodes.get(path) {
            Some(Node::File { mtime, .. }) => Ok(UNIX_EPOCH + Duration::from_secs(*mtime)),
            _ => Err(Self::fail(&state, 550, "No such file")),
        }
    }

    fn mkdir(&mut self, path: &str) -> TResult<()> {
        plain(path)?;
        let mut state = self.begin(format!("MKD {path}"))?;
        if state.nodes.contains_key(path) {
            return Err(Self::fail(&state, 550, "File exists"));
        }
        if !matches!(state.nodes.get(&parent_of(path)), Some(Node::Dir)) {
            return Err(Self::fail(&state, 550, "No such file or directory"));
        }
        state.nodes.insert(path.to_owned(), Node::Dir);
        Ok(())
    }

    fn rmdir(&mut self, path: &str) -> TResult<()> {
        plain(path)?;
        let mut state = self.begin(format!("RMD {path}"))?;
        match state.nodes.get(path) {
            Some(Node::Dir) => {}
            _ => return Err(Self::fail(&state, 550, "No such directory")),
        }
        let prefix = format!("{path}/");
        if state.nodes.keys().any(|k| k.starts_with(&prefix)) {
            return Err(Self::fail(&state, 550, "Directory not empty"));
        }
        state.nodes.remove(path);
        Ok(())
    }

    fn delete(&mut self, path: &str) -> TResult<()> {
        plain(path)?;
        let mut state = self.begin(format!("DELE {path}"))?;
        match state.nodes.get(path) {
            Some(Node::File { .. }) => {
                state.nodes.remove(path);
                Ok(())
            }
            _ => Err(Self::fail(&state, 550, "No such file")),
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> TResult<()> {
        plain(from)?;
        plain(to)?;
        let mut state = self.begin(format!("RNFR {from}"))?;
        state.log.push(format!("RNTO {to}"));
        if let Some((path, data)) = state.inject_before_rename.take() {
            state.nodes.insert(path, Node::File { data, mtime: 1 });
        }
        if !state.nodes.contains_key(from) {
            return Err(Self::fail(&state, 550, "No such file"));
        }
        if state.nodes.contains_key(to) && !state.rename_replaces {
            return Err(Self::fail(&state, 550, "File exists"));
        }
        if !matches!(state.nodes.get(&parent_of(to)), Some(Node::Dir)) {
            return Err(Self::fail(&state, 550, "No such directory"));
        }
        let moved: Vec<(String, Node)> = state
            .nodes
            .iter()
            .filter(|(k, _)| *k == from || k.starts_with(&format!("{from}/")))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, node) in moved {
            state.nodes.remove(&key);
            state
                .nodes
                .insert(format!("{to}{}", &key[from.len()..]), node);
        }
        Ok(())
    }

    fn noop(&mut self) -> TResult<()> {
        self.begin("NOOP".into()).map(|_| ())
    }

    fn start_download(&mut self, path: &str, offset: u64) -> TResult<()> {
        plain(path)?;
        if !matches!(self.transfer, Transfer::None) {
            return Err(TransportError::Protocol(
                "Es läuft schon eine Übertragung".into(),
            ));
        }
        let state = self.begin(format!("RETR {path} (REST {offset})"))?;
        if offset > 0 && !state.features.rest_stream {
            return Err(Self::fail(&state, 502, "REST not supported"));
        }
        let data = match state.nodes.get(path) {
            Some(Node::File { data, .. }) => data.clone(),
            _ => return Err(Self::fail(&state, 550, "No such file")),
        };
        if offset as usize > data.len() {
            return Err(Self::fail(&state, 554, "Invalid REST parameter"));
        }
        drop(state);
        self.open_data()?;
        self.transfer = Transfer::Download {
            data,
            pos: offset as usize,
        };
        Ok(())
    }

    fn read_data(&mut self, buf: &mut [u8]) -> TResult<usize> {
        if self.dead {
            return Err(TransportError::Connection("Verbindung getrennt".into()));
        }
        match &mut self.transfer {
            Transfer::Download { data, pos } => {
                let n = buf.len().min(data.len() - *pos).min(64 * 1024);
                buf[..n].copy_from_slice(&data[*pos..*pos + n]);
                *pos += n;
                Ok(n)
            }
            _ => Err(TransportError::Protocol("Keine Übertragung".into())),
        }
    }

    fn finish_download(&mut self) -> TResult<()> {
        match std::mem::replace(&mut self.transfer, Transfer::None) {
            Transfer::Download { .. } => Ok(()),
            _ => Err(TransportError::Protocol("Keine Übertragung".into())),
        }
    }

    fn start_upload(&mut self, path: &str, mode: StoreMode) -> TResult<()> {
        plain(path)?;
        if !matches!(self.transfer, Transfer::None) {
            return Err(TransportError::Protocol(
                "Es läuft schon eine Übertragung".into(),
            ));
        }
        let verb = if mode == StoreMode::Create {
            "STOR"
        } else {
            "APPE"
        };
        let mut state = self.begin(format!("{verb} {path}"))?;
        if !matches!(state.nodes.get(&parent_of(path)), Some(Node::Dir)) {
            return Err(Self::fail(&state, 550, "No such directory"));
        }
        match (mode, state.nodes.get(path)) {
            (_, Some(Node::Dir)) => return Err(Self::fail(&state, 550, "Is a directory")),
            (StoreMode::Create, _) => {
                state.nodes.insert(
                    path.to_owned(),
                    Node::File {
                        data: Vec::new(),
                        mtime: 1_700_000_100,
                    },
                );
            }
            (StoreMode::Append, None) => {
                state.nodes.insert(
                    path.to_owned(),
                    Node::File {
                        data: Vec::new(),
                        mtime: 1_700_000_100,
                    },
                );
            }
            (StoreMode::Append, Some(_)) => {}
        }
        drop(state);
        self.open_data()?;
        self.transfer = Transfer::Upload {
            path: path.to_owned(),
            count: 0,
        };
        Ok(())
    }

    fn write_data(&mut self, buf: &[u8]) -> TResult<()> {
        if self.dead {
            return Err(TransportError::Connection("Verbindung getrennt".into()));
        }
        let Transfer::Upload { path, count } = &mut self.transfer else {
            return Err(TransportError::Protocol("Keine Übertragung".into()));
        };
        let mut state = self.server.0.lock().unwrap();
        if state.generation != self.generation {
            self.dead = true;
            return Err(TransportError::Connection("Verbindung getrennt".into()));
        }
        let (allowed, cut) = match state.cut_upload_after {
            Some(limit) if *count + buf.len() > limit => (limit.saturating_sub(*count), true),
            _ => (buf.len(), false),
        };
        if let Some(Node::File { data, .. }) = state.nodes.get_mut(path.as_str()) {
            data.extend_from_slice(&buf[..allowed]);
        }
        state.uploaded += allowed;
        *count += allowed;
        if cut {
            self.dead = true;
            return Err(TransportError::Connection(
                "Connection reset by peer".into(),
            ));
        }
        Ok(())
    }

    fn finish_upload(&mut self) -> TResult<()> {
        if self.dead {
            return Err(TransportError::Connection("Verbindung getrennt".into()));
        }
        match std::mem::replace(&mut self.transfer, Transfer::None) {
            Transfer::Upload { .. } => Ok(()),
            _ => Err(TransportError::Protocol("Keine Übertragung".into())),
        }
    }

    fn abort_transfer(&mut self) {
        self.transfer = Transfer::None;
    }

    fn quit(&mut self) {
        self.quit = true;
    }
}

/// Opens connections to a `FakeServer` and, in FTPS modes, lets the policy judge the
/// server's (fake) certificate first, like the real TLS handshake does.
pub struct FakeConnector {
    pub server: FakeServer,
}

impl crate::manager::Connector for FakeConnector {
    type Transport = FakeTransport;

    fn connect(
        &self,
        params: &ConnectParams,
        password: &Secret,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<FakeTransport> {
        if params.profile.tls != TlsMode::None {
            let cert = self.server.state().cert.clone();
            if let Some(observation) = cert {
                let key = PinKey::new(Protocol::Ftps, &params.profile.host, params.profile.port);
                verifier.verify(&key, &observation)?;
            }
        }
        self.server.connect(&params.profile.user, password)
    }
}
