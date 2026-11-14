//! Delegate authentication and mounting to the desktop. Capture only bounded, sanitized helper diagnostics.
use dualfile_core::system_places::{mounted_path, parse_url, DiscoveryPaths};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq, Eq)]
pub enum MountError {
    Unavailable,
    Failed(String),
    Timeout(String),
    Invalid,
}

pub fn run_gio(
    program: &Path,
    url: &str,
    unmount: bool,
    timeout: Duration,
) -> Result<(), MountError> {
    let place = parse_url(url, "", "").ok_or(MountError::Invalid)?;
    if place.url != url {
        return Err(MountError::Invalid);
    }
    let mut command = Command::new(program);
    command.arg("mount");
    if unmount {
        command.arg("-u");
    }
    command
        .arg("--")
        .arg(&place.url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    dualfile_core::appenv::apply_clean_env(&mut command);
    dualfile_core::appenv::apply_external_current_dir(&mut command);
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            MountError::Unavailable
        } else {
            MountError::Failed(e.kind().to_string())
        }
    })?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    // Nonblocking reads keep the deadline effective even if a descendant holds the pipe.
    if rustix::fs::fcntl_getfl(&stderr)
        .and_then(|flags| rustix::fs::fcntl_setfl(&stderr, flags | rustix::fs::OFlags::NONBLOCK))
        .is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(MountError::Failed(String::new()));
    }
    let mut output = Vec::new();
    let start = Instant::now();
    let result = loop {
        drain_diagnostics(&mut stderr, &mut output);
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err(MountError::Failed(sanitize_diagnostics(&output)))
                }
            }
            Err(_) => break Err(MountError::Failed(sanitize_diagnostics(&output))),
            _ => {}
        }
        if start.elapsed() >= timeout {
            break Err(MountError::Timeout(sanitize_diagnostics(&output)));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let _ = child.kill();
    let _ = child.wait();
    drain_diagnostics(&mut stderr, &mut output);
    match result {
        Err(MountError::Failed(_)) => Err(MountError::Failed(sanitize_diagnostics(&output))),
        other => other,
    }
}

fn drain_diagnostics(reader: &mut impl Read, output: &mut Vec<u8>) {
    let mut buffer = [0; 4096];
    for _ in 0..16 {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => output.extend_from_slice(&buffer[..n.min(8192 - output.len())]),
        }
    }
}

fn sanitize_diagnostics(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut safe = Vec::new();
    for line in text.lines() {
        // Do not retain arbitrary password values (including quoted values with spaces).
        let lower = line.to_lowercase();
        if ["password", "passwd", "passwort", "pwd="]
            .iter()
            .any(|key| lower.contains(key))
        {
            safe.push("[redacted]".to_owned());
            continue;
        }
        for word in line.split_whitespace() {
            let mut word = word.to_owned();
            if let Some(start) = word.find("://") {
                let authority = start + 3;
                let end = word[authority..]
                    .find('/')
                    .map_or(word.len(), |i| authority + i);
                if let Some(at) = word[authority..end].rfind('@') {
                    word.replace_range(authority..authority + at, "[redacted]");
                } else if word[authority..end].contains(':') {
                    // The bounded capture may end in the middle of user:password@host.
                    word.truncate(authority);
                    word.push_str("[redacted]");
                }
            }
            safe.push(word.chars().filter(|c| !c.is_control()).collect());
        }
    }
    safe.join(" ").chars().take(200).collect()
}
pub fn connect(url: &str, paths: &DiscoveryPaths) -> Result<PathBuf, MountError> {
    let place = parse_url(url, "", "")
        .filter(|p| p.url == url)
        .ok_or(MountError::Invalid)?;
    if let Some(path) = mounted_path(&place, paths) {
        return Ok(path);
    }
    run_gio(Path::new("gio"), url, false, Duration::from_secs(30))?;
    // Give the FUSE daemon a bounded opportunity to publish the mount.
    for _ in 0..20 {
        if let Some(path) = mounted_path(&place, paths) {
            return Ok(path);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(MountError::Failed(String::new()))
}
pub fn unmount(url: &str) -> Result<(), MountError> {
    run_gio(Path::new("gio"), url, true, Duration::from_secs(30))
}

#[cfg(test)]
mod tests {
    use super::{connect, run_gio, MountError};
    use dualfile_core::system_places::DiscoveryPaths;
    use std::{fs, os::unix::fs::PermissionsExt};
    use std::{process::Command, time::Duration};
    #[test]
    fn gio_cleans_appimage_environment_and_discards_output() {
        if std::env::var_os("DUALFILE_GIO_ENV_TEST").is_none() {
            let result = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "system_mount::tests::gio_cleans_appimage_environment_and_discards_output",
                ])
                .env("DUALFILE_GIO_ENV_TEST", "1")
                .env("APPDIR", "/tmp/.mount_test")
                .env("GIO_MODULE_DIR", "/tmp/.mount_test/gio")
                .env("HOME", "/")
                .status()
                .unwrap();
            assert!(result.success());
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let gio = tmp.path().join("gio");
        fs::write(&gio, "#!/bin/sh\n[ -z \"$APPDIR\" ] && [ -z \"$GIO_MODULE_DIR\" ] && [ \"$PWD\" = / ] || exit 9\nhead -c 300000 /dev/zero\nhead -c 300000 /dev/zero >&2\n").unwrap();
        fs::set_permissions(&gio, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            run_gio(&gio, "smb://host/share", false, Duration::from_secs(2)),
            Ok(())
        );
        assert_eq!(
            run_gio(
                &tmp.path().join("missing"),
                "smb://host/share",
                false,
                Duration::from_secs(1)
            ),
            Err(MountError::Unavailable)
        );
    }
    #[test]
    fn helper_arguments_secrets_failure_timeout_and_existing_mount() {
        let tmp = tempfile::tempdir().unwrap();
        let gio = tmp.path().join("gio");
        fs::write(&gio, "#!/bin/sh\ntest \"$1\" = mount && test \"$2\" = -u && test \"$3\" = -- && test \"$4\" = smb://host/share\n").unwrap();
        fs::set_permissions(&gio, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            run_gio(&gio, "smb://host/share", true, Duration::from_secs(1)),
            Ok(())
        );
        assert_eq!(
            run_gio(
                &gio,
                "smb://u:secret@host/share",
                true,
                Duration::from_secs(1)
            ),
            Err(MountError::Invalid)
        );
        assert_eq!(
            run_gio(&gio, "smb://host/share", false, Duration::from_secs(1)),
            Err(MountError::Failed(String::new()))
        );
        fs::write(&gio, "#!/bin/sh\nexec sleep 10\n").unwrap();
        assert_eq!(
            run_gio(&gio, "smb://host/share", false, Duration::from_millis(40)),
            Err(MountError::Timeout(String::new()))
        );
        let paths = DiscoveryPaths {
            home: tmp.path().into(),
            data: tmp.path().into(),
            config: tmp.path().into(),
            runtime: tmp.path().into(),
        };
        let mounted = tmp.path().join("gvfs/smb-share:server=host,share=share");
        fs::create_dir_all(&mounted).unwrap();
        assert_eq!(connect("smb://host/share", &paths), Ok(mounted));
    }
    #[test]
    fn failed_gio_preserves_bounded_sanitized_diagnostics() {
        let tmp = tempfile::tempdir().unwrap();
        let gio = tmp.path().join("gio");
        fs::write(&gio, "#!/bin/sh\nprintf 'gio: smb://user:secret@host/share: Host unreachable\nPassword=another secret\n' >&2\nexit 1\n").unwrap();
        fs::set_permissions(&gio, fs::Permissions::from_mode(0o700)).unwrap();
        let Err(MountError::Failed(detail)) =
            run_gio(&gio, "smb://host/share", false, Duration::from_secs(1))
        else {
            panic!("mount failure must not report missing GVfs");
        };
        assert!(detail.contains("Host unreachable"));
        assert!(!detail.contains("secret"));
        assert!(!detail.contains("user:"));
        fs::write(
            &gio,
            format!("#!/bin/sh\nprintf '%s' '{}' >&2\nexit 1\n", "ä".repeat(500)),
        )
        .unwrap();
        let Err(MountError::Failed(detail)) =
            run_gio(&gio, "smb://host/share", false, Duration::from_secs(1))
        else {
            panic!("expected failure");
        };
        assert_eq!(detail.chars().count(), 200);
        fs::write(
            &gio,
            "#!/bin/sh\necho 'Authentication rejected' >&2\nexec sleep 10\n",
        )
        .unwrap();
        let Err(MountError::Timeout(detail)) =
            run_gio(&gio, "smb://host/share", false, Duration::from_millis(80))
        else {
            panic!("expected timeout");
        };
        assert!(detail.contains("Authentication rejected"));
    }
}
