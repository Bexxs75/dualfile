//! `FtpTransport` on top of `suppaftp` (rustls). Everything the server controls is bounded
//! (reply size by suppaftp, listings here), every wait ends within the profile's time limits,
//! data connections only ever go to the host of the control connection, and nothing that can
//! carry the password reaches an error text.

use crate::error::{sanitize_text, TResult, TransportError};
use crate::passive::resolve_data_addr;
use crate::profile::{ConnectParams, TlsMode};
use crate::secret::Secret;
use crate::tls::{self, PinningServerVerifier};
use crate::transport::{Features, FtpTransport, Limits, StoreMode, Timeouts};
use crate::trust::{CertVerifier, PinKey, Protocol};
use rustls::pki_types::TrustAnchor;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use suppaftp::types::{FileType, Response};
use suppaftp::{
    FtpError, FtpResult, Mode, ReplyTooLarge, RustlsConnector, RustlsFtpStream, Status,
    TlsConnector, TransferStream,
};

type Transfer = TransferStream<<RustlsConnector as TlsConnector>::Stream>;

/// Longest server text kept in an error.
const MAX_ERROR_TEXT: usize = 200;

/// A data connection address that was refused (travels through suppaftp inside an `io::Error`).
#[derive(Debug)]
struct DataAddrRefused(String);

impl std::fmt::Display for DataAddrRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DataAddrRefused {}

/// Refuses an argument that could end the command line or smuggle in another command.
fn check_arg(arg: &str) -> TResult<&str> {
    if arg.chars().any(char::is_control) {
        Err(TransportError::Rejected)
    } else {
        Ok(arg)
    }
}

/// The three digit code of a reply (suppaftp maps codes it does not know to 0).
fn reply_code(resp: &Response) -> u16 {
    let code = resp.status.code();
    if code != 0 {
        return code as u16;
    }
    resp.body
        .get(..3)
        .and_then(|digits| std::str::from_utf8(digits).ok())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0)
}

fn reply_error(resp: &Response) -> TransportError {
    let text = String::from_utf8_lossy(&resp.body);
    let first = text.lines().next().unwrap_or("");
    let first = first
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches(['-', ' ']);
    TransportError::reply(reply_code(resp), first)
}

fn map_io(error: &io::Error) -> TransportError {
    if let Some(inner) = error.get_ref() {
        if inner.is::<ReplyTooLarge>() {
            return TransportError::TooLarge("Antwort des Servers");
        }
        if let Some(refused) = inner.downcast_ref::<DataAddrRefused>() {
            return TransportError::Protocol(refused.0.clone());
        }
    }
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => TransportError::Timeout,
        _ => TransportError::Connection(sanitize_text(&error.to_string(), MAX_ERROR_TEXT)),
    }
}

fn map_ftp(error: FtpError) -> TransportError {
    match error {
        FtpError::ConnectionError(io) => map_io(&io),
        FtpError::SecureError(text) => TransportError::Tls(sanitize_text(&text, MAX_ERROR_TEXT)),
        FtpError::UnexpectedResponse(resp) => reply_error(&resp),
        FtpError::BadResponse => TransportError::Protocol("Ungültige Antwort des Servers".into()),
        FtpError::DataConnectionAlreadyOpen => {
            TransportError::Protocol("Es läuft schon eine Übertragung".into())
        }
        FtpError::InvalidAddress(_) => {
            TransportError::Protocol("Ungültige Adresse vom Server".into())
        }
    }
}

/// The error of a failed operation. If the TLS verifier refused the certificate, that reason
/// replaces rustls' general failure (the handshake runs lazily, so it can surface anywhere).
fn map_with_trust(tls: Option<&PinningServerVerifier>, error: FtpError) -> TransportError {
    if matches!(
        error,
        FtpError::ConnectionError(_) | FtpError::SecureError(_)
    ) {
        if let Some(trust) = tls.and_then(PinningServerVerifier::take_error) {
            return TransportError::Trust(trust);
        }
    }
    map_ftp(error)
}

fn tune(stream: &TcpStream, idle: Duration) -> io::Result<()> {
    stream.set_read_timeout(Some(idle))?;
    stream.set_write_timeout(Some(idle))?;
    stream.set_nodelay(true)
}

/// Puts the socket's time limits in place before the TLS session starts (the handshake runs on
/// the first read, so a server that accepts and then says nothing ends with `TimedOut`).
#[derive(Debug)]
struct TimedConnector {
    inner: RustlsConnector,
    idle: Duration,
}

impl TlsConnector for TimedConnector {
    type Stream = <RustlsConnector as TlsConnector>::Stream;

    fn connect(&self, domain: &str, stream: TcpStream) -> FtpResult<Self::Stream> {
        tune(&stream, self.idle).map_err(FtpError::ConnectionError)?;
        self.inner.connect(domain, stream)
    }
}

/// Implicit FTPS. suppaftp opens the socket itself (`TcpStream::connect`, no time limit) and
/// has no entry point for a ready socket with TLS, so it runs in a helper thread that is
/// waited for at most `connect + idle`. The handshake and the greeting are bounded by the
/// socket timeouts set in `TimedConnector`; only a blackholed SYN keeps the helper thread
/// until the operating system gives up (it ends by itself, its result is dropped).
fn connect_implicit(
    addr: SocketAddr,
    config: RustlsConnector,
    host: &str,
    timeouts: Timeouts,
) -> FtpResult<RustlsFtpStream> {
    let connector = TimedConnector {
        inner: config,
        idle: timeouts.idle,
    };
    let host = host.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(RustlsFtpStream::connect_secure_implicit(
            addr, connector, &host,
        ));
    });
    match rx.recv_timeout(timeouts.connect.saturating_add(timeouts.idle)) {
        Ok(result) => result,
        Err(_) => Err(FtpError::ConnectionError(io::Error::from(
            io::ErrorKind::TimedOut,
        ))),
    }
}

fn set_control_timeouts(ftp: &RustlsFtpStream, idle: Duration) -> TResult<()> {
    let socket = ftp.get_ref();
    socket
        .set_read_timeout(Some(idle))
        .and_then(|()| socket.set_write_timeout(Some(idle)))
        .map_err(|e| map_io(&e))
}

fn resolve(host: &str, port: u16) -> TResult<SocketAddr> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| TransportError::Connection(sanitize_text(&e.to_string(), MAX_ERROR_TEXT)))?
        .collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .ok_or_else(|| TransportError::Connection("Servername nicht gefunden".into()))
}

pub struct SuppaTransport {
    ftp: RustlsFtpStream,
    host: String,
    port: u16,
    features: Features,
    tls: Option<Arc<PinningServerVerifier>>,
    transfer: Option<Transfer>,
    /// `EPSV` is still used for data connections (it is given up once after a 500/501/502).
    extended_passive: bool,
}

impl std::fmt::Debug for SuppaTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SuppaTransport")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl SuppaTransport {
    /// Connects, secures (per profile) and logs in. `roots` default to the system's.
    pub fn connect(
        params: &ConnectParams,
        password: &Secret,
        verifier: Arc<dyn CertVerifier>,
        roots: Option<Vec<TrustAnchor<'static>>>,
    ) -> TResult<SuppaTransport> {
        Self::connect_inner(params, password, verifier, roots).map_err(|e| e.scrubbed(password))
    }

    fn connect_inner(
        params: &ConnectParams,
        password: &Secret,
        verifier: Arc<dyn CertVerifier>,
        roots: Option<Vec<TrustAnchor<'static>>>,
    ) -> TResult<SuppaTransport> {
        let profile = &params.profile;
        profile.validate().map_err(TransportError::Unsupported)?;
        let addr = resolve(&profile.host, profile.port)?;
        let tls = (profile.tls != TlsMode::None).then(|| {
            let key = PinKey::new(Protocol::Ftps, &profile.host, profile.port);
            PinningServerVerifier::new(verifier, key, roots.unwrap_or_else(tls::system_roots))
        });
        Self::open(
            addr,
            &profile.host,
            profile.tls,
            tls,
            &profile.user,
            password,
            params.timeouts,
        )
    }

    /// The connection steps after name resolution (also used by the tests with `TlsMode::None`).
    fn open(
        addr: SocketAddr,
        host: &str,
        mode: TlsMode,
        tls: Option<Arc<PinningServerVerifier>>,
        user: &str,
        password: &Secret,
        timeouts: Timeouts,
    ) -> TResult<SuppaTransport> {
        let user = check_arg(user)?;
        let pass = check_arg(password.expose())?;
        let trust = |e| map_with_trust(tls.as_deref(), e);
        let connect_tcp = || -> TResult<TcpStream> {
            let tcp =
                TcpStream::connect_timeout(&addr, timeouts.connect).map_err(|e| map_io(&e))?;
            tune(&tcp, timeouts.idle).map_err(|e| map_io(&e))?;
            Ok(tcp)
        };
        let config = || {
            let verifier = tls.clone().expect("TLS modes have a verifier");
            RustlsConnector::from(tls::client_config(verifier))
        };

        let control_ip = addr.ip();
        let data_builder = move |announced: SocketAddr| -> FtpResult<TcpStream> {
            let target = resolve_data_addr(control_ip, announced).map_err(|e| {
                FtpError::ConnectionError(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    DataAddrRefused(e.to_string()),
                ))
            })?;
            let stream = TcpStream::connect_timeout(&target, timeouts.connect)
                .map_err(FtpError::ConnectionError)?;
            tune(&stream, timeouts.idle).map_err(FtpError::ConnectionError)?;
            Ok(stream)
        };

        let mut ftp = match mode {
            TlsMode::None => RustlsFtpStream::connect_with_stream(connect_tcp()?).map_err(trust)?,
            TlsMode::Explicit => RustlsFtpStream::connect_with_stream(connect_tcp()?)
                .map_err(trust)?
                .passive_stream_builder(data_builder)
                .into_secure(config(), host)
                .map_err(trust)?,
            TlsMode::Implicit => connect_implicit(addr, config(), host, timeouts).map_err(trust)?,
        }
        .passive_stream_builder(data_builder);
        set_control_timeouts(&ftp, timeouts.idle)?;
        ftp.set_mode(Mode::ExtendedPassive);

        ftp.login(user, pass).map_err(|e| match e {
            // The server's text may echo the password: not kept.
            FtpError::UnexpectedResponse(resp) if reply_code(&resp) != 421 => TransportError::Auth,
            other => trust(other),
        })?;
        ftp.transfer_type(FileType::Binary).map_err(trust)?;
        let _ = ftp.opts("UTF8", Some("ON"));
        let features = match ftp.feat() {
            Ok(list) => {
                let has = |name: &str| list.keys().any(|k| k.eq_ignore_ascii_case(name));
                Features {
                    mlsd: has("MLST"),
                    mlst: has("MLST"),
                    size: has("SIZE"),
                    mdtm: has("MDTM"),
                    rest_stream: list.iter().any(|(k, v)| {
                        k.eq_ignore_ascii_case("REST")
                            && v.as_deref()
                                .is_none_or(|v| v.to_ascii_uppercase().contains("STREAM"))
                    }),
                }
            }
            // Without FEAT: assume the basics, no machine-readable listings.
            Err(_) => Features {
                mlsd: false,
                mlst: false,
                size: true,
                mdtm: true,
                rest_stream: true,
            },
        };
        Ok(SuppaTransport {
            ftp,
            host: host.to_owned(),
            port: addr.port(),
            features,
            tls,
            transfer: None,
            extended_passive: true,
        })
    }

    fn err(&self, error: FtpError) -> TransportError {
        map_with_trust(self.tls.as_deref(), error)
    }

    /// Runs a command that needs a data connection. If the server does not know `EPSV`
    /// (500/501/502) the connection falls back to `PASV` once and the command is repeated.
    fn data_command<R>(
        &mut self,
        command: impl Fn(&mut RustlsFtpStream) -> FtpResult<R>,
    ) -> TResult<R> {
        let mut result = command(&mut self.ftp);
        if let Err(FtpError::UnexpectedResponse(resp)) = &result {
            if self.extended_passive && matches!(reply_code(resp), 500..=502) {
                self.extended_passive = false;
                self.ftp.set_mode(Mode::Passive);
                self.ftp.set_passive_nat_workaround(true);
                result = command(&mut self.ftp);
            }
        }
        result.map_err(|e| self.err(e))
    }

    fn begin(&self) -> TResult<()> {
        if self.transfer.is_some() {
            return Err(TransportError::Protocol(
                "Es läuft schon eine Übertragung".into(),
            ));
        }
        Ok(())
    }

    fn current(&mut self) -> TResult<&mut Transfer> {
        self.transfer
            .as_mut()
            .ok_or_else(|| TransportError::Protocol("Keine Übertragung gestartet".into()))
    }

    fn take_transfer(&mut self) -> TResult<Transfer> {
        self.transfer
            .take()
            .ok_or_else(|| TransportError::Protocol("Keine Übertragung gestartet".into()))
    }

    fn io_error(&self, error: io::Error) -> TransportError {
        self.err(FtpError::ConnectionError(error))
    }
}

enum Line {
    End,
    Kept,
    /// Longer than allowed: skipped without keeping it.
    Dropped,
}

/// Reads one listing line of at most `max_line_len` bytes into `buf`. A longer line is skipped
/// to its end without keeping it (but only while `total`, the bytes read so far, stays within
/// `max_total`; the caller then sees the exceeded limit).
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max_line_len: usize,
    total: &mut usize,
    max_total: usize,
) -> io::Result<Line> {
    buf.clear();
    // room for the line end (\r\n) on top of the allowed length
    let room = max_line_len as u64 + 2;
    let n = reader.by_ref().take(room).read_until(b'\n', buf)?;
    *total += n;
    if n == 0 {
        return Ok(Line::End);
    }
    if buf.ends_with(b"\n") || (n as u64) < room {
        return Ok(Line::Kept);
    }
    let mut skipped = Vec::new();
    while *total <= max_total {
        skipped.clear();
        let n = reader.by_ref().take(4096).read_until(b'\n', &mut skipped)?;
        *total += n;
        if n == 0 || skipped.ends_with(b"\n") {
            break;
        }
    }
    Ok(Line::Dropped)
}

impl FtpTransport for SuppaTransport {
    fn features(&self) -> Features {
        self.features
    }

    fn list(&mut self, path: &str, mlsd: bool, limits: &Limits) -> TResult<Vec<String>> {
        self.begin()?;
        let command = format!(
            "{} {}",
            if mlsd { "MLSD" } else { "LIST" },
            check_arg(path)?
        );
        let stream = self
            .data_command(|ftp| {
                ftp.custom_data_command(&command, &[Status::AboutToSend, Status::AlreadyOpen])
            })?
            .1;
        let mut reader = BufReader::new(stream);
        let mut lines = Vec::new();
        let mut total = 0usize;
        let mut count = 0usize;
        let mut buf = Vec::new();
        let outcome: TResult<()> = loop {
            let read = read_bounded_line(
                &mut reader,
                &mut buf,
                limits.max_line_len,
                &mut total,
                limits.max_list_bytes,
            );
            match read {
                Err(e) => break Err(self.io_error(e)),
                Ok(Line::End) => break Ok(()),
                Ok(kind) => {
                    count += 1;
                    if total > limits.max_list_bytes {
                        break Err(TransportError::TooLarge("Verzeichnisliste (Bytes)"));
                    }
                    if count > limits.max_list_lines {
                        break Err(TransportError::TooLarge("Verzeichnisliste (Zeilen)"));
                    }
                    if matches!(kind, Line::Kept) {
                        let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
                        let line = line.strip_suffix(b"\r").unwrap_or(line);
                        if !line.is_empty() {
                            lines.push(String::from_utf8_lossy(line).into_owned());
                        }
                    }
                }
            }
        };
        let stream = reader.into_inner();
        match outcome {
            Ok(()) => {
                stream.finish().map_err(|e| self.err(e))?;
                Ok(lines)
            }
            Err(error) => {
                let _ = self.ftp.abort(stream);
                Err(error)
            }
        }
    }

    fn mlst(&mut self, path: &str) -> TResult<String> {
        let path = check_arg(path)?;
        self.ftp.mlst(Some(path)).map_err(|e| self.err(e))
    }

    fn size(&mut self, path: &str) -> TResult<u64> {
        let path = check_arg(path)?;
        self.ftp
            .size(path)
            .map(|n| n as u64)
            .map_err(|e| self.err(e))
    }

    fn mdtm(&mut self, path: &str) -> TResult<SystemTime> {
        let path = check_arg(path)?;
        let time = self.ftp.mdtm(path).map_err(|e| self.err(e))?;
        let secs = time.and_utc().timestamp();
        let delta = Duration::from_secs(secs.unsigned_abs());
        Ok(if secs >= 0 {
            UNIX_EPOCH + delta
        } else {
            UNIX_EPOCH - delta
        })
    }

    fn mkdir(&mut self, path: &str) -> TResult<()> {
        let path = check_arg(path)?;
        self.ftp.mkdir(path).map_err(|e| self.err(e))
    }

    fn rmdir(&mut self, path: &str) -> TResult<()> {
        let path = check_arg(path)?;
        self.ftp.rmdir(path).map_err(|e| self.err(e))
    }

    fn delete(&mut self, path: &str) -> TResult<()> {
        let path = check_arg(path)?;
        self.ftp.rm(path).map_err(|e| self.err(e))
    }

    fn rename(&mut self, from: &str, to: &str) -> TResult<()> {
        let (from, to) = (check_arg(from)?, check_arg(to)?);
        self.ftp.rename(from, to).map_err(|e| self.err(e))
    }

    fn noop(&mut self) -> TResult<()> {
        self.ftp.noop().map_err(|e| self.err(e))
    }

    fn start_download(&mut self, path: &str, offset: u64) -> TResult<()> {
        self.begin()?;
        let path = check_arg(path)?;
        let offset = usize::try_from(offset).map_err(|_| TransportError::Rejected)?;
        let stream = self.data_command(|ftp| {
            ftp.resume_transfer(offset)?;
            ftp.retr_as_stream(path)
        })?;
        self.transfer = Some(stream);
        Ok(())
    }

    fn read_data(&mut self, buf: &mut [u8]) -> TResult<usize> {
        let read = self.current()?.read(buf);
        read.map_err(|e| self.io_error(e))
    }

    fn finish_download(&mut self) -> TResult<()> {
        let stream = self.take_transfer()?;
        stream.finish().map_err(|e| self.err(e))
    }

    fn start_upload(&mut self, path: &str, mode: StoreMode) -> TResult<()> {
        self.begin()?;
        let path = check_arg(path)?;
        let stream = self.data_command(|ftp| {
            ftp.resume_transfer(0)?;
            match mode {
                StoreMode::Create => ftp.put_with_stream(path),
                StoreMode::Append => ftp.append_with_stream(path),
            }
        })?;
        self.transfer = Some(stream);
        Ok(())
    }

    fn write_data(&mut self, buf: &[u8]) -> TResult<()> {
        let written = self.current()?.write_all(buf);
        written.map_err(|e| self.io_error(e))
    }

    fn finish_upload(&mut self) -> TResult<()> {
        let mut stream = self.take_transfer()?;
        if let Err(e) = stream.flush() {
            let _ = self.ftp.abort(stream);
            return Err(self.io_error(e));
        }
        stream.finish().map_err(|e| self.err(e))
    }

    fn abort_transfer(&mut self) {
        if let Some(stream) = self.transfer.take() {
            let _ = self.ftp.abort(stream);
        }
    }

    fn quit(&mut self) {
        self.abort_transfer();
        let _ = self.ftp.quit();
    }
}

impl Drop for SuppaTransport {
    fn drop(&mut self) {
        // an abandoned transfer would otherwise wait for the server's completion reply
        self.abort_transfer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Instant;

    const PASSWORD: &str = "marker-secret-9";

    #[derive(Default)]
    struct Cfg {
        /// `EPSV` is answered with 500.
        no_epsv: bool,
        /// Address announced by `PASV` (default 10.9.9.9).
        pasv_ip: Option<[u8; 4]>,
        pasv_port: Option<u16>,
        /// This command is read but never answered.
        silent_on: Option<&'static str>,
        /// Sent instead of the real listing.
        listing: Option<Vec<u8>>,
    }

    struct Server {
        cfg: Cfg,
        files: Mutex<HashMap<String, Vec<u8>>>,
        seen: Mutex<Vec<String>>,
    }

    impl Server {
        fn saw(&self, prefix: &str) -> bool {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.starts_with(prefix))
        }
        fn file(&self, path: &str) -> Option<Vec<u8>> {
            self.files.lock().unwrap().get(path).cloned()
        }
    }

    fn start(cfg: Cfg, files: &[(&str, &[u8])]) -> (SocketAddr, Arc<Server>) {
        let server = Arc::new(Server {
            cfg,
            files: Mutex::new(
                files
                    .iter()
                    .map(|(p, d)| (p.to_string(), d.to_vec()))
                    .collect(),
            ),
            seen: Mutex::new(Vec::new()),
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = server.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let server = shared.clone();
                thread::spawn(move || serve(&server, stream));
            }
        });
        (addr, server)
    }

    fn send(out: &mut TcpStream, text: &str) {
        let _ = out.write_all(text.as_bytes());
    }

    fn serve(server: &Server, stream: TcpStream) {
        let mut out = stream.try_clone().unwrap();
        let mut input = BufReader::new(stream);
        send(&mut out, "220 fake ready\r\n");
        let mut data: Option<TcpListener> = None;
        let mut rest = 0usize;
        let mut line = String::new();
        loop {
            line.clear();
            if input.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let line = line.trim_end().to_owned();
            server.seen.lock().unwrap().push(line.clone());
            let (cmd, arg) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            let cmd = cmd.to_ascii_uppercase();
            if server.cfg.silent_on == Some(cmd.as_str()) {
                continue;
            }
            match cmd.as_str() {
                "USER" => send(&mut out, "331 Password required\r\n"),
                "PASS" if arg == PASSWORD => send(&mut out, "230 Logged in\r\n"),
                "PASS" => send(&mut out, &format!("530 Login incorrect {arg}\r\n")),
                "FEAT" => send(
                    &mut out,
                    "211-Features:\r\n MLST type*;size*;modify*;\r\n SIZE\r\n MDTM\r\n REST STREAM\r\n UTF8\r\n211 End\r\n",
                ),
                "TYPE" | "OPTS" | "NOOP" => send(&mut out, "200 OK\r\n"),
                "EPSV" if server.cfg.no_epsv => send(&mut out, "500 EPSV not understood\r\n"),
                "EPSV" | "PASV" => {
                    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                    let port = server.cfg.pasv_port.unwrap_or(listener.local_addr().unwrap().port());
                    data = Some(listener);
                    if cmd == "EPSV" {
                        send(&mut out, &format!("229 Entering Extended Passive Mode (|||{port}|)\r\n"));
                    } else {
                        let [a, b, c, d] = server.cfg.pasv_ip.unwrap_or([10, 9, 9, 9]);
                        send(
                            &mut out,
                            &format!("227 Entering Passive Mode ({a},{b},{c},{d},{},{})\r\n", port >> 8, port & 255),
                        );
                    }
                }
                "REST" => {
                    rest = arg.parse().unwrap_or(0);
                    send(&mut out, "350 Restarting\r\n");
                }
                "MLSD" | "LIST" | "RETR" | "STOR" | "APPE" => {
                    let Some(listener) = data.take() else {
                        send(&mut out, "425 No data connection\r\n");
                        continue;
                    };
                    let offset = std::mem::take(&mut rest);
                    let files = server.files.lock().unwrap().clone();
                    if cmd == "RETR" && !files.contains_key(arg) {
                        send(&mut out, "550 No such file\r\n");
                        continue;
                    }
                    send(&mut out, "150 Opening data connection\r\n");
                    let Ok((mut conn, _)) = listener.accept() else { continue };
                    match cmd.as_str() {
                        "MLSD" | "LIST" => {
                            let payload = server.cfg.listing.clone().unwrap_or_else(|| {
                                let mut text = String::new();
                                for (path, bytes) in &files {
                                    let name = path.rsplit('/').next().unwrap();
                                    if cmd == "MLSD" {
                                        text += &format!("type=file;size={}; {name}\r\n", bytes.len());
                                    } else {
                                        text += &format!("-rw-r--r-- 1 u g {} Jan  1  2026 {name}\r\n", bytes.len());
                                    }
                                }
                                text.into_bytes()
                            });
                            let _ = conn.write_all(&payload);
                        }
                        "RETR" => {
                            let _ = conn.write_all(&files[arg][offset.min(files[arg].len())..]);
                        }
                        _ => {
                            let mut body = Vec::new();
                            let _ = conn.read_to_end(&mut body);
                            let mut files = server.files.lock().unwrap();
                            let entry = files.entry(arg.to_owned()).or_default();
                            if cmd == "STOR" {
                                entry.clear();
                            }
                            entry.extend_from_slice(&body);
                        }
                    }
                    drop(conn);
                    send(&mut out, "226 Transfer complete\r\n");
                }
                "SIZE" => match server.file(arg) {
                    Some(bytes) => send(&mut out, &format!("213 {}\r\n", bytes.len())),
                    None => send(&mut out, "550 No such file\r\n"),
                },
                "MDTM" => send(&mut out, "213 20260102030405\r\n"),
                "MLST" => send(&mut out, &format!("250-Listing {arg}\r\n type=file;size=3; {arg}\r\n250 End\r\n")),
                "MKD" => send(&mut out, &format!("257 \"{arg}\" created\r\n")),
                "RMD" => send(&mut out, "250 Removed\r\n"),
                "DELE" => {
                    if server.files.lock().unwrap().remove(arg).is_some() {
                        send(&mut out, "250 Deleted\r\n");
                    } else {
                        send(&mut out, "550 No such file\r\n");
                    }
                }
                "RNFR" => send(&mut out, "350 Ready for RNTO\r\n"),
                "RNTO" => send(&mut out, "250 Renamed\r\n"),
                "ABOR" => send(&mut out, "226 Aborted\r\n"),
                "QUIT" => {
                    send(&mut out, "221 Bye\r\n");
                    return;
                }
                _ => send(&mut out, "502 Not implemented\r\n"),
            }
        }
    }

    fn timeouts() -> Timeouts {
        Timeouts {
            connect: Duration::from_secs(5),
            idle: Duration::from_secs(5),
        }
    }

    fn open_with(addr: SocketAddr, password: &str, timeouts: Timeouts) -> TResult<SuppaTransport> {
        SuppaTransport::open(
            addr,
            "127.0.0.1",
            TlsMode::None,
            None,
            "u",
            &Secret::new(password),
            timeouts,
        )
    }

    fn open(addr: SocketAddr) -> SuppaTransport {
        open_with(addr, PASSWORD, timeouts()).unwrap()
    }

    fn download(t: &mut SuppaTransport, path: &str, offset: u64) -> TResult<Vec<u8>> {
        t.start_download(path, offset)?;
        let mut all = Vec::new();
        let mut buf = [0u8; 100];
        loop {
            match t.read_data(&mut buf)? {
                0 => break,
                n => all.extend_from_slice(&buf[..n]),
            }
        }
        t.finish_download()?;
        Ok(all)
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn logs_in_and_reads_the_features() {
        let (addr, server) = start(Cfg::default(), &[]);
        let t = open(addr);
        let f = t.features();
        assert!(f.mlsd && f.mlst && f.size && f.mdtm && f.rest_stream);
        assert!(server.saw("TYPE I"));
        assert!(server.saw("OPTS UTF8 ON"));
        assert!(format!("{t:?}").contains("127.0.0.1"));
    }

    #[test]
    fn a_wrong_password_is_an_auth_error_that_never_shows_the_password() {
        let (addr, _server) = start(Cfg::default(), &[]);
        // the fake server answers `530 Login incorrect <the password it was sent>`
        let secret = "x-marker-secret-9";
        let error = open_with(addr, secret, timeouts()).unwrap_err();
        assert!(matches!(error, TransportError::Auth), "{error:?}");
        for text in [format!("{error}"), format!("{error:?}")] {
            assert!(!text.contains("marker-secret-9"), "{text}");
        }
    }

    #[test]
    fn server_text_that_echoes_the_password_is_scrubbed_in_other_errors() {
        let secret = Secret::new(PASSWORD);
        let error = TransportError::reply(550, "denied for marker-secret-9").scrubbed(&secret);
        assert!(!format!("{error}").contains("marker-secret-9"));
    }

    #[test]
    fn listings_are_read_as_lines() {
        let (addr, _server) = start(Cfg::default(), &[("/a.txt", b"abc")]);
        let mut t = open(addr);
        let limits = Limits::default();
        assert_eq!(
            t.list("/", true, &limits).unwrap(),
            vec!["type=file;size=3; a.txt"]
        );
        assert_eq!(
            t.list("/", false, &limits).unwrap(),
            vec!["-rw-r--r-- 1 u g 3 Jan  1  2026 a.txt"]
        );
        // the connection is still usable
        t.noop().unwrap();
    }

    #[test]
    fn too_many_lines_or_bytes_end_with_too_large() {
        let many: Vec<u8> = (0..10_000)
            .flat_map(|i| format!("type=file;size=1; f{i}\r\n").into_bytes())
            .collect();
        let (addr, _s) = start(
            Cfg {
                listing: Some(many.clone()),
                ..Cfg::default()
            },
            &[],
        );
        let mut t = open(addr);
        let by_lines = Limits {
            max_list_lines: 100,
            ..Limits::default()
        };
        assert!(matches!(
            t.list("/", true, &by_lines),
            Err(TransportError::TooLarge(_))
        ));
        drop(t);

        let (addr, _s) = start(
            Cfg {
                listing: Some(many),
                ..Cfg::default()
            },
            &[],
        );
        let mut t = open(addr);
        let by_bytes = Limits {
            max_list_bytes: 1000,
            ..Limits::default()
        };
        assert!(matches!(
            t.list("/", true, &by_bytes),
            Err(TransportError::TooLarge(_))
        ));
    }

    #[test]
    fn over_long_lines_are_dropped_without_being_kept() {
        let mut payload = b"type=file; ok1\r\n".to_vec();
        payload.extend(std::iter::repeat_n(b'a', 1 << 20));
        payload.extend_from_slice(b"\r\ntype=file; ok2\n\n");
        let (addr, _s) = start(
            Cfg {
                listing: Some(payload),
                ..Cfg::default()
            },
            &[],
        );
        let mut t = open(addr);
        let limits = Limits {
            max_line_len: 100,
            ..Limits::default()
        };
        assert_eq!(
            t.list("/", true, &limits).unwrap(),
            vec!["type=file; ok1", "type=file; ok2"]
        );
    }

    #[test]
    fn the_line_reader_is_bounded() {
        let data = [vec![b'x'; 10_000], b"\nshort\n".to_vec()].concat();
        let mut reader = io::Cursor::new(data);
        let (mut buf, mut total) = (Vec::new(), 0);
        let mut next = |buf: &mut Vec<u8>, total: &mut usize| {
            read_bounded_line(&mut reader, buf, 50, total, 1 << 20).unwrap()
        };
        assert!(matches!(next(&mut buf, &mut total), Line::Dropped));
        assert!(buf.len() <= 52, "only a bounded prefix was kept");
        assert!(matches!(next(&mut buf, &mut total), Line::Kept));
        assert_eq!(buf, b"short\n");
        assert!(matches!(next(&mut buf, &mut total), Line::End));
        assert_eq!(total, 10_007);
        // an endless line stops at the total limit
        let mut endless = io::repeat(b'y');
        let mut reader = BufReader::new(&mut endless);
        let mut total = 0;
        assert!(matches!(
            read_bounded_line(&mut reader, &mut buf, 50, &mut total, 100_000).unwrap(),
            Line::Dropped
        ));
        assert!(total > 100_000 && total < 200_000);
    }

    #[test]
    fn downloads_whole_and_from_an_offset() {
        let content = pattern(1000);
        let (addr, _s) = start(Cfg::default(), &[("/f.bin", &content)]);
        let mut t = open(addr);
        assert_eq!(download(&mut t, "/f.bin", 0).unwrap(), content);
        assert_eq!(download(&mut t, "/f.bin", 400).unwrap(), content[400..]);
        assert!(matches!(
            t.start_download("/missing", 0),
            Err(TransportError::Reply { code: 550, .. })
        ));
        // only one transfer at a time
        t.start_download("/f.bin", 0).unwrap();
        assert!(matches!(
            t.start_download("/f.bin", 0),
            Err(TransportError::Protocol(_))
        ));
        t.abort_transfer();
    }

    #[test]
    fn uploads_create_and_append() {
        let (addr, server) = start(Cfg::default(), &[]);
        let mut t = open(addr);
        t.start_upload("/n.txt", StoreMode::Create).unwrap();
        t.write_data(b"hello ").unwrap();
        t.finish_upload().unwrap();
        assert_eq!(server.file("/n.txt").unwrap(), b"hello ");
        t.start_upload("/n.txt", StoreMode::Append).unwrap();
        t.write_data(b"world").unwrap();
        t.finish_upload().unwrap();
        assert_eq!(server.file("/n.txt").unwrap(), b"hello world");
        t.start_upload("/n.txt", StoreMode::Create).unwrap();
        t.write_data(b"new").unwrap();
        t.finish_upload().unwrap();
        assert_eq!(server.file("/n.txt").unwrap(), b"new");
    }

    #[test]
    fn simple_commands_work() {
        let (addr, server) = start(Cfg::default(), &[("/a", b"12345"), ("/gone", b"")]);
        let mut t = open(addr);
        assert_eq!(t.size("/a").unwrap(), 5);
        // 2026-01-02 03:04:05 UTC
        assert_eq!(
            t.mdtm("/a").unwrap(),
            UNIX_EPOCH + Duration::from_secs(1_767_323_045)
        );
        assert_eq!(t.mlst("/a").unwrap(), "type=file;size=3; /a");
        t.mkdir("/d").unwrap();
        t.rmdir("/d").unwrap();
        t.delete("/gone").unwrap();
        assert!(matches!(
            t.delete("/gone"),
            Err(TransportError::Reply { code: 550, .. })
        ));
        t.rename("/a", "/b").unwrap();
        assert!(server.saw("RNFR /a") && server.saw("RNTO /b"));
        t.noop().unwrap();
        t.quit();
        assert!(server.saw("QUIT"));
    }

    #[test]
    fn control_characters_in_arguments_are_refused_and_nothing_is_sent() {
        assert_eq!(check_arg("/ok/ä b").unwrap(), "/ok/ä b");
        for bad in ["a\rb", "a\nb", "a\0b", "a\tb", "\x1b[0m", "/x\r\nDELE y"] {
            assert!(
                matches!(check_arg(bad), Err(TransportError::Rejected)),
                "{bad:?}"
            );
        }
        let (addr, server) = start(Cfg::default(), &[("/x", b"1")]);
        let mut t = open(addr);
        let evil = "/x\r\nDELE x";
        let limits = Limits::default();
        assert!(matches!(t.delete(evil), Err(TransportError::Rejected)));
        assert!(matches!(
            t.list(evil, true, &limits),
            Err(TransportError::Rejected)
        ));
        assert!(matches!(t.mlst(evil), Err(TransportError::Rejected)));
        assert!(matches!(t.size(evil), Err(TransportError::Rejected)));
        assert!(matches!(t.mdtm(evil), Err(TransportError::Rejected)));
        assert!(matches!(t.mkdir(evil), Err(TransportError::Rejected)));
        assert!(matches!(t.rmdir(evil), Err(TransportError::Rejected)));
        assert!(matches!(
            t.rename(evil, "/y"),
            Err(TransportError::Rejected)
        ));
        assert!(matches!(
            t.rename("/y", evil),
            Err(TransportError::Rejected)
        ));
        assert!(matches!(
            t.start_download(evil, 0),
            Err(TransportError::Rejected)
        ));
        assert!(matches!(
            t.start_upload(evil, StoreMode::Create),
            Err(TransportError::Rejected)
        ));
        t.noop().unwrap();
        let seen = server.seen.lock().unwrap().clone();
        assert!(
            seen.iter()
                .all(|l| !l.starts_with("DELE") && !l.starts_with("MLSD") && !l.contains('\r')),
            "{seen:?}"
        );
        assert!(server.file("/x").is_some());
        // credentials are checked the same way
        let bad_user = SuppaTransport::open(
            addr,
            "127.0.0.1",
            TlsMode::None,
            None,
            "u\r\nDELE x",
            &Secret::new(PASSWORD),
            timeouts(),
        );
        assert!(matches!(bad_user, Err(TransportError::Rejected)));
    }

    #[test]
    fn epsv_is_replaced_by_pasv_when_the_server_does_not_know_it() {
        let content = pattern(300);
        let (addr, server) = start(
            Cfg {
                no_epsv: true,
                ..Cfg::default()
            },
            &[("/f", &content)],
        );
        let mut t = open(addr);
        assert_eq!(download(&mut t, "/f", 0).unwrap(), content);
        assert!(server.saw("EPSV") && server.saw("PASV"));
        // and stays on PASV
        assert_eq!(download(&mut t, "/f", 100).unwrap(), content[100..]);
        let epsv = server
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|l| *l == "EPSV")
            .count();
        assert_eq!(epsv, 1);
    }

    #[test]
    fn a_foreign_pasv_address_is_replaced_by_the_control_host() {
        let content = pattern(300);
        let (addr, _s) = start(
            Cfg {
                no_epsv: true,
                ..Cfg::default()
            },
            &[("/f", &content)],
        );
        let mut t = open(addr);
        // without suppaftp's own NAT workaround the announced 10.9.9.9 reaches our own check
        t.extended_passive = false;
        t.ftp.set_mode(Mode::Passive);
        t.ftp.set_passive_nat_workaround(false);
        assert_eq!(download(&mut t, "/f", 0).unwrap(), content);
        // and with the workaround (the normal fallback) it works as well
        t.ftp.set_passive_nat_workaround(true);
        assert_eq!(download(&mut t, "/f", 0).unwrap(), content);
    }

    #[test]
    fn a_pasv_port_of_another_service_is_refused() {
        let (addr, _s) = start(
            Cfg {
                no_epsv: true,
                pasv_port: Some(25),
                ..Cfg::default()
            },
            &[("/f", b"x")],
        );
        let mut t = open(addr);
        assert!(matches!(
            t.start_download("/f", 0),
            Err(TransportError::Protocol(_))
        ));
        t.ftp.set_passive_nat_workaround(false);
        assert!(matches!(
            t.start_download("/f", 0),
            Err(TransportError::Protocol(_))
        ));
        // EPSV with such a port as well
        let (addr, _s) = start(
            Cfg {
                pasv_port: Some(25),
                ..Cfg::default()
            },
            &[("/f", b"x")],
        );
        let mut t = open(addr);
        assert!(matches!(
            t.list("/", true, &Limits::default()),
            Err(TransportError::Protocol(_))
        ));
    }

    #[test]
    fn a_silent_server_ends_with_a_timeout_within_the_idle_time() {
        let (addr, _s) = start(
            Cfg {
                silent_on: Some("NOOP"),
                ..Cfg::default()
            },
            &[],
        );
        let short = Timeouts {
            connect: Duration::from_secs(5),
            idle: Duration::from_millis(300),
        };
        let mut t = open_with(addr, PASSWORD, short).unwrap();
        let started = Instant::now();
        let error = t.noop().unwrap_err();
        assert!(matches!(error, TransportError::Timeout), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(error.is_connection_lost());
    }

    #[test]
    fn implicit_ftps_to_a_server_that_never_answers_times_out_in_time() {
        use crate::trust::{CertObservation, TrustError, TrustOutcome};
        struct Never;
        impl CertVerifier for Never {
            fn verify(&self, _: &PinKey, _: &CertObservation) -> Result<TrustOutcome, TrustError> {
                Err(TrustError::Expired)
            }
        }
        // accepts the connection, sends nothing, keeps the socket open
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (hold_tx, hold_rx) = std::sync::mpsc::channel::<()>();
        let holder = thread::spawn(move || {
            let _conn = listener.accept();
            let _ = hold_rx.recv();
        });
        let key = PinKey::new(Protocol::Ftps, "127.0.0.1", addr.port());
        let verifier = PinningServerVerifier::new(Arc::new(Never), key, Vec::new());
        let short = Timeouts {
            connect: Duration::from_millis(300),
            idle: Duration::from_millis(300),
        };
        let started = Instant::now();
        let result = SuppaTransport::open(
            addr,
            "127.0.0.1",
            TlsMode::Implicit,
            Some(verifier),
            "u",
            &Secret::new(PASSWORD),
            short,
        );
        let Err(error) = result else {
            panic!("must fail")
        };
        assert!(matches!(error, TransportError::Timeout), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(hold_tx);
        let _ = holder.join();
    }
}
