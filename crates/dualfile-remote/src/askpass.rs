//! Private, bounded IPC for one OpenSSH connection's askpass helpers.
use crate::secret::Secret;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File};
use std::io::{self, IsTerminal, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

pub const SOCKET_ENV: &str = "DUALFILE_ASKPASS_SOCKET";
pub const TOKEN_ENV: &str = "DUALFILE_ASKPASS_TOKEN";
const LIMIT: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptKind {
    Password,
    Passphrase,
    HostKey,
    Other,
}

#[derive(Clone, Debug)]
pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    pub secret: bool,
}
impl Prompt {
    pub fn new(text: &str) -> Self {
        let lower = text.to_ascii_lowercase();
        let kind = if lower.contains("are you sure you want to continue connecting") {
            PromptKind::HostKey
        } else if lower.contains("passphrase for key") || lower.contains("enter passphrase") {
            PromptKind::Passphrase
        } else if lower.contains("password:") {
            PromptKind::Password
        } else {
            PromptKind::Other
        };
        // Line breaks and tabs become one space so sentences stay apart; every other control
        // character is dropped.
        let mut clean = String::new();
        let mut space = true;
        for ch in text
            .chars()
            .map(|c| {
                if matches!(c, '\n' | '\r' | '\t') {
                    ' '
                } else {
                    c
                }
            })
            .filter(|c| !c.is_control())
        {
            if ch == ' ' {
                if space {
                    continue;
                }
                space = true;
            } else {
                space = false;
            }
            if clean.len() + ch.len_utf8() > LIMIT {
                break;
            }
            clean.push(ch);
        }
        let clean = clean.trim_end().to_string();
        Self {
            kind,
            text: clean,
            secret: kind != PromptKind::HostKey,
        }
    }
}

#[derive(Debug)]
pub enum PromptAnswer {
    Text(Secret),
    Cancel,
}
pub trait PromptHandler: Send + Sync + 'static {
    /// Must return promptly on cancellation and within 120 seconds. Rust cannot forcibly
    /// interrupt an arbitrary synchronous handler; the IPC deadline still closes its helper.
    fn prompt(&self, p: Prompt) -> PromptAnswer;
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("socket"));
        let _ = fs::remove_dir(&self.0);
    }
}

pub struct AskpassServer {
    directory: Directory,
    token: Secret,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl AskpassServer {
    pub fn start(handler: Arc<dyn PromptHandler>) -> io::Result<Self> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self::start_in(handler, &base, TIMEOUT)
    }
    fn start_in(
        handler: Arc<dyn PromptHandler>,
        base: &Path,
        timeout: Duration,
    ) -> io::Result<Self> {
        let mut random = Zeroizing::new([0u8; 48]);
        File::open("/dev/urandom")?.read_exact(&mut *random)?;
        let token = Secret::new(
            random[..32]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        );
        let path = base.join(format!(
            "dualfile-askpass-{}",
            random[32..]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ));
        DirBuilder::new().mode(0o700).create(&path)?;
        let directory = Directory(path);
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700))?;
        let uid = fs::metadata(&directory.0)?.uid();
        let socket = directory.0.join("socket");
        let listener = std::os::unix::net::UnixListener::bind(&socket)?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()?;
        let (stop, stopped) = oneshot::channel();
        let expected = token.clone();
        let worker = std::thread::Builder::new()
            .name("dualfile-askpass".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::UnixListener::from_std(listener) else {
                        return;
                    };
                    let serve = async {
                        let mut count = 0;
                        loop {
                            let Ok((mut stream, _)) = listener.accept().await else {
                                break;
                            };
                            // Tokio uses SO_PEERCRED on Linux, getpeereid on BSD/macOS.
                            if !stream.peer_cred().is_ok_and(|cred| cred.uid() == uid) {
                                continue;
                            }
                            let _ = tokio::time::timeout(timeout, async {
                                let mut supplied = Zeroizing::new([0u8; 64]);
                                stream.read_exact(&mut *supplied).await?;
                                if supplied
                                    .iter()
                                    .zip(expected.expose().bytes())
                                    .fold(0u8, |a, (x, y)| a | (x ^ y))
                                    != 0
                                {
                                    return Ok::<(), io::Error>(());
                                }
                                if count >= 8 {
                                    return Ok(());
                                }
                                count += 1;
                                let len = stream.read_u32().await? as usize;
                                if len > LIMIT {
                                    return Ok(());
                                }
                                let mut bytes = vec![0; len];
                                stream.read_exact(&mut bytes).await?;
                                let Ok(text) = std::str::from_utf8(&bytes) else {
                                    return Ok(());
                                };
                                let prompt = Prompt::new(text);
                                let kind = prompt.kind;
                                let handler = handler.clone();
                                let (send, receive) = oneshot::channel();
                                // A broken embedding handler must not hold the server's Drop hostage.
                                std::thread::Builder::new()
                                    .name("dualfile-prompt".into())
                                    .spawn(move || {
                                        let _ = send.send(handler.prompt(prompt));
                                    })?;
                                if let Ok(PromptAnswer::Text(answer)) = receive.await {
                                    let text = answer.expose();
                                    if text.len() <= LIMIT
                                        && !text.contains(['\n', '\r', '\0'])
                                        && (kind != PromptKind::HostKey
                                            || matches!(text, "yes" | "no"))
                                    {
                                        stream.write_u32(text.len() as u32).await?;
                                        stream.write_all(text.as_bytes()).await?;
                                    }
                                }
                                Ok(())
                            })
                            .await;
                        }
                    };
                    tokio::select! { _ = stopped => {}, _ = serve => {} }
                });
            })?;
        Ok(Self {
            directory,
            token,
            stop: Some(stop),
            worker: Some(worker),
        })
    }
    pub fn socket_path(&self) -> PathBuf {
        self.directory.0.join("socket")
    }
    pub(crate) fn configure(&self, command: &mut std::process::Command) -> io::Result<()> {
        command
            .env("SSH_ASKPASS", std::env::current_exe()?)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env(SOCKET_ENV, self.socket_path())
            .env(TOKEN_ENV, self.token.expose());
        Ok(())
    }
}
impl Drop for AskpassServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn ask(socket: &Path, token: &str, text: &str) -> io::Result<Secret> {
    if token.len() != 64 || text.len() > LIMIT {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(token.as_bytes())?;
    stream.write_all(&(text.len() as u32).to_be_bytes())?;
    stream.write_all(text.as_bytes())?;
    let mut len = [0; 4];
    stream.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > LIMIT {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Zeroizing::new(vec![0; len]);
    stream.read_exact(&mut bytes)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| io::ErrorKind::InvalidData)?;
    if text.contains(['\n', '\r', '\0']) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(Secret::new(text))
}

/// Pure startup guard, also usable by embedding executables before starting their UI.
pub fn is_askpass_invocation(
    args: &[OsString],
    socket_present: bool,
    stdin_is_terminal: bool,
) -> bool {
    socket_present && !stdin_is_terminal && args.len() == 2 && args[1].to_str().is_some()
}
pub fn helper_requested() -> bool {
    is_askpass_invocation(
        &std::env::args_os().collect::<Vec<_>>(),
        std::env::var_os(SOCKET_ENV).is_some(),
        io::stdin().is_terminal(),
    )
}

/// No diagnostic or partial answer is emitted on protocol errors or cancellation.
pub fn run_askpass_helper(
    args: impl IntoIterator<Item = OsString>,
    env: impl IntoIterator<Item = (OsString, OsString)>,
) -> i32 {
    helper(args, env, &mut io::stdout().lock())
}
fn helper(
    args: impl IntoIterator<Item = OsString>,
    env: impl IntoIterator<Item = (OsString, OsString)>,
    out: &mut impl Write,
) -> i32 {
    let args: Vec<_> = args.into_iter().collect();
    let mut socket = None;
    let mut token = None;
    for (key, value) in env {
        if key == OsStr::new(SOCKET_ENV) {
            socket = Some(PathBuf::from(value));
        } else if key == OsStr::new(TOKEN_ENV) {
            token = value.into_string().ok().map(Secret::new);
        }
    }
    if args.len() != 2 {
        return 1;
    }
    let (Some(socket), Some(token), Some(text)) = (socket, token, args[1].to_str()) else {
        return 1;
    };
    match ask(&socket, token.expose(), text) {
        Ok(answer)
            if writeln!(out, "{}", answer.expose())
                .and_then(|_| out.flush())
                .is_ok() =>
        {
            0
        }
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Answer(&'static str);
    impl PromptHandler for Answer {
        fn prompt(&self, _: Prompt) -> PromptAnswer {
            PromptAnswer::Text(Secret::new(self.0))
        }
    }
    fn server(answer: &'static str) -> (tempfile::TempDir, AskpassServer) {
        let dir = tempfile::tempdir().unwrap();
        let server =
            AskpassServer::start_in(Arc::new(Answer(answer)), dir.path(), Duration::from_secs(1))
                .unwrap();
        (dir, server)
    }
    fn exchange(server: &AskpassServer, text: &str, token: &str) -> (i32, Vec<u8>) {
        let mut out = Vec::new();
        let code = helper(
            [OsString::from("dualfile"), text.into()],
            [
                (SOCKET_ENV.into(), server.socket_path().into()),
                (TOKEN_ENV.into(), token.into()),
            ],
            &mut out,
        );
        (code, out)
    }
    #[test]
    fn round_trip_permissions_and_cleanup() {
        let (_dir, server) = server("answer-marker");
        let socket = server.socket_path();
        let directory = socket.parent().unwrap().to_path_buf();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            exchange(&server, "Password:", server.token.expose()),
            (0, b"answer-marker\n".to_vec())
        );
        drop(server);
        assert!(!socket.exists());
        assert!(!directory.exists());
    }
    #[test]
    fn wrong_token_and_ninth_prompt_are_rejected() {
        let (_dir, server) = server("ok");
        assert_eq!(exchange(&server, "Password:", &"0".repeat(64)), (1, vec![]));
        for _ in 0..8 {
            assert_eq!(exchange(&server, "Password:", server.token.expose()).0, 0);
        }
        assert_eq!(
            exchange(&server, "Password:", server.token.expose()),
            (1, vec![])
        );
    }
    #[test]
    fn oversize_wire_request_is_rejected() {
        let (_dir, server) = server("ok");
        let mut stream = UnixStream::connect(server.socket_path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        stream.write_all(server.token.expose().as_bytes()).unwrap();
        stream.write_all(&4097u32.to_be_bytes()).unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        assert_eq!(
            exchange(&server, &"x".repeat(4097), server.token.expose()),
            (1, vec![])
        );
    }
    #[test]
    fn kinds_sanitization_and_safe_default() {
        for (text, kind) in [
            ("user Password:", PromptKind::Password),
            ("passphrase for key x", PromptKind::Passphrase),
            ("Enter passphrase", PromptKind::Passphrase),
            (
                "Are you sure you want to continue connecting",
                PromptKind::HostKey,
            ),
            ("OTP:", PromptKind::Other),
        ] {
            let p = Prompt::new(text);
            assert_eq!(p.kind, kind);
            assert_eq!(p.secret, kind != PromptKind::HostKey);
        }
        let p = Prompt::new(&format!("\0\n\r\x1b{}", "ü".repeat(4096)));
        assert_eq!(p.text.len(), 4096);
        assert!(!p.text.chars().any(char::is_control));
        assert!(!format!("{:?}", PromptAnswer::Text(Secret::new("marker"))).contains("marker"));
    }
    #[test]
    fn line_breaks_become_single_spaces_so_sentences_stay_apart() {
        let p = Prompt::new(
            "The authenticity of host 'h' can't be established.\nED25519 key fingerprint is SHA256:abc.\r\n\tThis key is not known.\n\nAre you sure?  ",
        );
        assert_eq!(
            p.text,
            "The authenticity of host 'h' can't be established. ED25519 key fingerprint is SHA256:abc. This key is not known. Are you sure?"
        );
        assert!(!p.text.chars().any(char::is_control));
    }
    #[test]
    fn host_key_only_accepts_yes_or_no_and_answers_are_single_line() {
        for answer in ["yes", "no", "fingerprint", "yes\n", "bad\0answer"] {
            let (_dir, server) = server(answer);
            let result = exchange(
                &server,
                "Are you sure you want to continue connecting",
                server.token.expose(),
            );
            assert_eq!(result.0, if matches!(answer, "yes" | "no") { 0 } else { 1 });
            if result.0 != 0 {
                assert!(result.1.is_empty());
            }
        }
    }
    struct Slow;
    impl PromptHandler for Slow {
        fn prompt(&self, _: Prompt) -> PromptAnswer {
            std::thread::sleep(Duration::from_millis(150));
            PromptAnswer::Cancel
        }
    }
    #[test]
    fn deadline_cancel_and_drop_close_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let server =
            AskpassServer::start_in(Arc::new(Slow), dir.path(), Duration::from_millis(20)).unwrap();
        let start = std::time::Instant::now();
        assert_eq!(
            exchange(&server, "Password:", server.token.expose()),
            (1, vec![])
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        let mut stalled = UnixStream::connect(server.socket_path()).unwrap();
        stalled
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        drop(server);
        assert!(match stalled.read(&mut [0]) {
            Ok(0) => true,
            Err(e) => e.kind() == io::ErrorKind::ConnectionReset,
            _ => false,
        });
        let server =
            AskpassServer::start_in(Arc::new(Slow), dir.path(), Duration::from_secs(1)).unwrap();
        assert_eq!(
            exchange(&server, "Password:", server.token.expose()),
            (1, vec![])
        );
    }
    #[test]
    fn normal_start_is_not_helper_mode() {
        let args = vec!["dualfile".into(), "Password:".into()];
        assert!(!is_askpass_invocation(&args, false, false));
        assert!(!is_askpass_invocation(&args[..1], true, false));
        assert!(!is_askpass_invocation(&args, true, true));
        assert!(is_askpass_invocation(&args, true, false));
    }
    #[test]
    fn fake_ssh_invokes_real_helper() {
        let server = AskpassServer::start(Arc::new(Answer("process-marker"))).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("ssh");
        std::fs::write(
            &script,
            "#!/bin/sh\n\"$SSH_ASKPASS\" 'Password:' > \"$1\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        // The askpass_helper example (built by `cargo build --examples`) lives at
        // <target>/<profile>/examples/. The test binary's location varies, so search upward
        // from it for the first ancestor that holds examples/askpass_helper.
        let exe = std::env::current_exe().unwrap();
        let helper = exe
            .ancestors()
            .map(|dir| dir.join("examples/askpass_helper"))
            .find(|p| p.is_file())
            .expect("build the askpass_helper example first: cargo build --examples");
        let output = dir.path().join("answer");
        let mut command = std::process::Command::new(&script);
        server.configure(&mut command).unwrap();
        let status = command
            .env("SSH_ASKPASS", helper)
            .arg(&output)
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read(&output).unwrap(), b"process-marker\n");
    }
}
