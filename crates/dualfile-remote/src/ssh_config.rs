//! Bounded, read-only discovery. OpenSSH alone interprets configuration on resolution.
use crate::error::{TResult, TransportError};
use crate::openssh_transport::{configure_external_env, find_ssh_in};
use dualfile_core::connections::valid_ssh_endpoint;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

const MAX_FILE: u64 = 256 * 1024;
const MAX_FILES: usize = 20;
const MAX_HOSTS: usize = 500;
const MAX_OUTPUT: u64 = 256 * 1024;

fn failed() -> TransportError {
    TransportError::Protocol("SSH-Konfiguration konnte nicht gelesen werden".into())
}

/// Reject paths with any symlink resolving outside ~/.ssh, including ancestor links.
fn safe_path(path: &Path, ssh_dir: &Path) -> bool {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        if fs::symlink_metadata(&prefix).is_ok_and(|m| m.file_type().is_symlink())
            && !fs::canonicalize(&prefix).is_ok_and(|p| p.starts_with(ssh_dir))
        {
            return false;
        }
    }
    true
}

/// Anchor every component to an open directory. NOFOLLOW closes the canonicalize/open
/// race; NONBLOCK prevents a file swapped for a FIFO from hanging discovery.
fn open_config(path: &Path) -> TResult<fs::File> {
    use rustix::fs::{open, openat, Mode, OFlags};
    let mut dir = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| failed())?;
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let flags = OFlags::RDONLY
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC
            | if last {
                OFlags::NONBLOCK
            } else {
                OFlags::DIRECTORY
            };
        dir = openat(&dir, *part, flags, Mode::empty()).map_err(|_| failed())?;
    }
    Ok(fs::File::from(dir))
}

fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            _ => result.push(part),
        }
    }
    result
}

// SSH config tokens: quoted paths, escaped spaces, comments, and optional `=`.
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch == '#' {
            break;
        } else if ch.is_whitespace() || (ch == '=' && out.len() <= 1) {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

fn wildcard(pattern: &[u8], value: &[u8]) -> bool {
    // Linear-space glob matching, bounded by config file and filename limits.
    let (mut p, mut v, mut star, mut retry) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(s) = star {
            retry += 1;
            v = retry;
            p = s + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn expand(pattern: &Path, ssh_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::new()];
    for part in pattern.components() {
        let text = part.as_os_str().to_string_lossy();
        let mut next = Vec::new();
        for path in paths {
            if text.contains(['*', '?']) {
                if !safe_path(&path, ssh_dir) {
                    continue;
                }
                if let Ok(entries) = fs::read_dir(&path) {
                    let mut matches: Vec<_> = entries
                        .take(4096)
                        .filter_map(Result::ok)
                        .filter(|e| wildcard(text.as_bytes(), e.file_name().as_encoded_bytes()))
                        .map(|e| e.path())
                        .collect();
                    matches.sort();
                    next.extend(matches.into_iter().take(MAX_FILES));
                }
            } else {
                next.push(path.join(part));
            }
        }
        next.truncate(MAX_FILES);
        paths = next;
    }
    paths
}

pub fn list_hosts() -> TResult<Vec<String>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(failed)?;
    list_hosts_in(&home)
}

fn list_hosts_in(home: &Path) -> TResult<Vec<String>> {
    let ssh_dir = home.join(".ssh");
    let mut files = vec![ssh_dir.join("config")];
    let mut seen = BTreeSet::new();
    let mut hosts = BTreeSet::new();
    let mut index = 0;
    while index < files.len() && index < MAX_FILES {
        let path = files[index].clone();
        let root = index == 0;
        index += 1;
        let Some((canonical, text)) = read_ssh_file(&path, &ssh_dir)? else {
            continue;
        };
        if !seen.insert(canonical) {
            continue;
        }
        for line in text.lines() {
            let words = tokens(line);
            let Some(keyword) = words.first() else {
                continue;
            };
            if keyword.eq_ignore_ascii_case("host") {
                for host in &words[1..] {
                    if valid_ssh_endpoint(host) && !host.contains(['*', '?', '!']) {
                        hosts.insert(host.clone());
                        if hosts.len() > MAX_HOSTS {
                            hosts.pop_last();
                        }
                    }
                }
            } else if root && keyword.eq_ignore_ascii_case("include") {
                for value in &words[1..] {
                    if files.len() == MAX_FILES {
                        break;
                    }
                    let path = if let Some(rest) = value.strip_prefix("~/") {
                        home.join(rest)
                    } else if Path::new(value).is_absolute() {
                        PathBuf::from(value)
                    } else {
                        continue;
                    };
                    for path in expand(&path, &ssh_dir) {
                        if files.len() == MAX_FILES {
                            break;
                        }
                        if !files.contains(&path) {
                            files.push(path);
                        }
                    }
                }
            }
        }
    }
    // Hosts reached without a config entry; an unreadable known_hosts must not hide config hosts.
    if let Ok(Some((_, text))) = read_ssh_file(&ssh_dir.join("known_hosts"), &ssh_dir) {
        for host in known_hosts(&text) {
            if hosts.len() >= MAX_HOSTS {
                break;
            }
            hosts.insert(host);
        }
    }
    Ok(hosts.into_iter().collect())
}

/// Read one file below ~/.ssh with the symlink, size and FIFO guards; `None` if missing.
fn read_ssh_file(path: &Path, ssh_dir: &Path) -> TResult<Option<(PathBuf, String)>> {
    if !safe_path(path, ssh_dir) {
        return Err(failed());
    }
    let canonical = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(failed()),
    };
    if canonical != normalized(path) && !canonical.starts_with(ssh_dir) {
        return Err(failed());
    }
    if !fs::metadata(&canonical).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE) {
        return Err(failed());
    }
    let file = open_config(&canonical)?;
    if !file
        .metadata()
        .is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE)
    {
        return Err(failed());
    }
    let mut text = String::new();
    file.take(MAX_FILE + 1)
        .read_to_string(&mut text)
        .map_err(|_| failed())?;
    if text.len() as u64 > MAX_FILE {
        return Err(failed());
    }
    Ok(Some((canonical, text)))
}

/// Plain names on port 22 only: hashed (`|1|`), `[host]:port`, patterns and
/// `@cert-authority`/`@revoked` lines carry no name that `ssh -G` could resolve as is.
fn known_hosts(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|first| !first.starts_with(['#', '@', '|']))
        .flat_map(|names| names.split(','))
        .filter(|h| valid_ssh_endpoint(h) && !h.contains(['*', '?', '!', '[', ']']))
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ResolvedHost {
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identityfile: Vec<String>,
}

pub fn resolve(alias: &str) -> TResult<ResolvedHost> {
    if !valid_ssh_endpoint(alias) {
        return Err(TransportError::Rejected);
    }
    let ssh = find_ssh_in(&std::env::var_os("PATH").unwrap_or_default())?;
    resolve_with(alias, &ssh, Duration::from_secs(5))
}

fn resolve_with(alias: &str, ssh: &Path, timeout: Duration) -> TResult<ResolvedHost> {
    if !valid_ssh_endpoint(alias) {
        return Err(TransportError::Rejected);
    }
    let mut command = Command::new(ssh);
    configure_external_env(&mut command);
    let mut child = command
        .args(["-G", "--", alias])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| failed())?;
    let result = (|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| failed())?;
        let deadline = Instant::now() + timeout;
        let bytes = runtime.block_on(async {
            let reader = tokio::net::unix::pipe::Receiver::from_owned_fd(
                child.stdout.take().ok_or_else(failed)?.into(),
            )
            .map_err(|_| failed())?;
            let mut bytes = Vec::new();
            tokio::time::timeout(timeout, reader.take(MAX_OUTPUT + 1).read_to_end(&mut bytes))
                .await
                .map_err(|_| TransportError::Timeout)?
                .map_err(|_| failed())?;
            if bytes.len() as u64 > MAX_OUTPUT {
                return Err(TransportError::TooLarge("SSH-Konfiguration"));
            }
            Ok(bytes)
        })?;
        loop {
            if let Some(status) = child.try_wait().map_err(|_| failed())? {
                if !status.success() {
                    return Err(failed());
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err(TransportError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| failed())?;
        let mut resolved = ResolvedHost {
            hostname: String::new(),
            user: String::new(),
            port: 0,
            identityfile: Vec::new(),
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once(' ') else {
                continue;
            };
            let value = crate::error::sanitize_text(value, 4096);
            match key {
                "hostname" => resolved.hostname = value,
                "user" => resolved.user = value,
                "port" => resolved.port = value.parse().map_err(|_| failed())?,
                "identityfile" if resolved.identityfile.len() < 32 => {
                    resolved.identityfile.push(value)
                }
                _ => {}
            }
        }
        if resolved.hostname.is_empty() || resolved.user.is_empty() || resolved.port == 0 {
            return Err(failed());
        }
        Ok(resolved)
    })();
    // Also on parse failures, timeout and over-limit output: kill, then reap.
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn home() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".ssh")).unwrap();
        dir
    }
    fn config(home: &Path, text: &str) {
        fs::write(home.join(".ssh/config"), text).unwrap();
    }
    #[test]
    fn hosts_are_sorted_deduplicated_and_patterns_are_excluded() {
        let dir = home();
        config(
            dir.path(),
            "Host z a a *.org !excluded q?\nhOsT b\nHost -oBad a/b\nHost=\"quoted\" # comment\n",
        );
        assert_eq!(
            list_hosts_in(dir.path()).unwrap(),
            ["a", "b", "quoted", "z"]
        );
    }
    #[test]
    fn known_hosts_adds_plain_names_and_skips_hashed_ports_and_markers() {
        let dir = home();
        config(dir.path(), "Host a\n");
        fs::write(
            dir.path().join(".ssh/known_hosts"),
            "eddie,192.168.1.5 ssh-ed25519 AAAA\n|1|abc=|def= ssh-ed25519 AAAA\n[nas]:2222 ssh-rsa AAAA\n@revoked bad ssh-rsa AAAA\n# c\n*.lan ssh-rsa AAAA\na ssh-rsa AAAA\n-oX ssh-rsa AAAA\n",
        )
        .unwrap();
        assert_eq!(
            list_hosts_in(dir.path()).unwrap(),
            ["192.168.1.5", "a", "eddie"]
        );
        fs::remove_file(dir.path().join(".ssh/config")).unwrap();
        assert_eq!(list_hosts_in(dir.path()).unwrap(), ["192.168.1.5", "a", "eddie"]);
    }
    #[test]
    fn includes_are_one_level_absolute_or_tilde_and_at_most_twenty_files() {
        let dir = home();
        let ssh = dir.path().join(".ssh");
        fs::create_dir(ssh.join("parts")).unwrap();
        for i in 0..30 {
            fs::write(
                ssh.join(format!("parts/{i:02}")),
                format!("Host h{i:02}\nInclude ~/.ssh/nested\n"),
            )
            .unwrap();
        }
        fs::write(ssh.join("nested"), "Host forbidden").unwrap();
        fs::write(ssh.join("relative"), "Host relative").unwrap();
        config(
            dir.path(),
            &format!("Include {}/parts/*\nInclude relative\n", ssh.display()),
        );
        let found = list_hosts_in(dir.path()).unwrap();
        assert_eq!(found.len(), 19); // config itself counts toward the twenty-file limit
        assert!(!found.contains(&"forbidden".into()));
        config(dir.path(), "Include ~/.ssh/parts/00");
        assert_eq!(list_hosts_in(dir.path()).unwrap(), ["h00"]);
    }
    #[test]
    fn file_size_symlink_escape_and_host_count_are_bounded() {
        let dir = home();
        config(dir.path(), &"x".repeat(MAX_FILE as usize + 1));
        assert!(list_hosts_in(dir.path()).is_err());
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), "Host secret").unwrap();
        symlink(outside.path(), dir.path().join(".ssh/link")).unwrap();
        config(dir.path(), "Include ~/.ssh/link");
        assert!(list_hosts_in(dir.path()).is_err());
        fs::remove_file(dir.path().join(".ssh/config")).unwrap();
        symlink(outside.path(), dir.path().join(".ssh/config")).unwrap();
        assert!(list_hosts_in(dir.path()).is_err());
        fs::remove_file(dir.path().join(".ssh/config")).unwrap();
        config(
            dir.path(),
            &(0..700)
                .map(|i| format!("Host h{i:03}\n"))
                .collect::<String>(),
        );
        let hosts = list_hosts_in(dir.path()).unwrap();
        assert_eq!(hosts.len(), MAX_HOSTS);
        assert_eq!(hosts.last().unwrap(), "h499");
    }
    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("ssh");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[test]
    fn resolve_uses_exact_arguments_clean_environment_and_display_values() {
        // Mutate only a nested test process's environment, never the parallel test runner.
        if std::env::var_os("DUALFILE_TEST_SECRET").is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ssh_config::tests::resolve_uses_exact_arguments_clean_environment_and_display_values"])
                .env("APPDIR", "/tmp/.mount_x")
                .env("HOME", "/")
                .env("DUALFILE_TEST_SECRET", "must-not-reach-ssh")
                .status().unwrap();
            assert!(status.success());
            return;
        }
        let dir = home();
        let ssh = script(dir.path(), "[ \"$PWD\" = / ] || exit 7\ntest \"$#\" = 3 && test \"$1\" = -G && test \"$2\" = -- && test \"$3\" = nas || exit 9\n[ -z \"$DUALFILE_TEST_SECRET\" ] || exit 8\nprintf 'hostname example.org\\nuser demo\\nport 2222\\nidentityfile ~/.ssh/key\\nproxycommand ignored\\n'");
        let result = resolve_with("nas", &ssh, Duration::from_secs(1)).unwrap();
        assert_eq!(
            result,
            ResolvedHost {
                hostname: "example.org".into(),
                user: "demo".into(),
                port: 2222,
                identityfile: vec!["~/.ssh/key".into()]
            }
        );
        for alias in ["", "-oProxyCommand=x", "x y", "x\n", "a/b"] {
            assert!(resolve_with(alias, &ssh, Duration::from_secs(1)).is_err());
        }
    }
    #[test]
    fn resolve_times_out_and_bounds_output() {
        let dir = home();
        let ssh = script(dir.path(), "exec sleep 10");
        let start = Instant::now();
        assert!(matches!(
            resolve_with("host", &ssh, Duration::from_millis(40)),
            Err(TransportError::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        let ssh = script(dir.path(), "head -c 300000 /dev/zero");
        assert!(matches!(
            resolve_with("host", &ssh, Duration::from_secs(1)),
            Err(TransportError::TooLarge(_))
        ));
    }
}
