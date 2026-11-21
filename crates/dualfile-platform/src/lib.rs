//! Packaging and host desktop access, shared by the app and its trash implementation.
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Packaging {
    AppImage,
    Flatpak,
    Plain,
}
impl Packaging {
    pub fn detect(
        flatpak_id: Option<&OsStr>,
        flatpak_info: bool,
        appimage: Option<&OsStr>,
    ) -> Self {
        if flatpak_id.is_some() || flatpak_info {
            Self::Flatpak
        } else if appimage.is_some() {
            Self::AppImage
        } else {
            Self::Plain
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppImage => "appImage",
            Self::Flatpak => "flatpak",
            Self::Plain => "plain",
        }
    }
}
pub fn packaging() -> Packaging {
    Packaging::detect(
        std::env::var_os("FLATPAK_ID").as_deref(),
        Path::new("/.flatpak-info").exists(),
        std::env::var_os("APPIMAGE").as_deref(),
    )
}
pub fn desktop_dir_with(
    kind: Packaging,
    home: &Path,
    key: &str,
    fallback: &str,
    env: impl Fn(&str) -> Option<OsString>,
) -> PathBuf {
    let key = if kind == Packaging::Flatpak {
        format!("HOST_{key}")
    } else {
        key.to_owned()
    };
    env(&key)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
}
pub fn desktop_dir(home: &Path, key: &str, fallback: &str) -> PathBuf {
    desktop_dir_with(packaging(), home, key, fallback, |key| {
        std::env::var_os(key)
    })
}
/// The sole host-command constructor. Arguments remain separate argv elements.
pub fn host_command_with(kind: Packaging, program: impl AsRef<OsStr>) -> Command {
    if kind == Packaging::Flatpak {
        let mut command = Command::new("flatpak-spawn");
        command.args(["--host", "--watch-bus"]);
        if Path::new(program.as_ref()).file_name().is_some_and(|name| {
            ["mount", "umount", "gio", "lsblk", "udisksctl", "eject"]
                .iter()
                .any(|helper| name == *helper)
        }) {
            command.args(["--env=LC_ALL=C", "--env=LANGUAGE="]);
        }
        command.arg(program);
        command
    } else {
        Command::new(program)
    }
}
pub fn host_command(program: impl AsRef<OsStr>) -> Command {
    host_command_with(packaging(), program)
}

/// Read control files in the host mount namespace. Bound both output and runtime.
pub fn read_host_file(path: &Path, cap: u64) -> std::io::Result<String> {
    use std::time::{Duration, Instant};
    if packaging() != Packaging::Flatpak {
        return std::fs::read_to_string(path);
    }
    // Share mount snapshots across polling callers; never cache failed reads.
    if matches!(path.to_str(), Some("/proc/mounts" | "/proc/self/mountinfo")) {
        type Cache = std::collections::HashMap<(PathBuf, u64), (Instant, String)>;
        static CACHE: std::sync::OnceLock<std::sync::Mutex<Cache>> = std::sync::OnceLock::new();
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(1));
        let key = (path.to_owned(), cap);
        if let Some((_, text)) = cache.get(&key) {
            return Ok(text.clone());
        }
        let text = read_host_file_uncached(path, cap)?;
        cache.insert(key, (Instant::now(), text.clone()));
        return Ok(text);
    }
    read_host_file_uncached(path, cap)
}

fn read_host_file_uncached(path: &Path, cap: u64) -> std::io::Result<String> {
    use std::{
        io::{self, Read},
        process::Stdio,
        time::{Duration, Instant},
    };
    let mut child = host_command("head")
        .args([
            OsStr::new("-c"),
            OsStr::new(&(cap + 1).to_string()),
            OsStr::new("--"),
            path.as_os_str(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut text = String::new();
        let result = stdout.take(cap + 1).read_to_string(&mut text).map(|_| text);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                let text = rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
                return if text.len() as u64 > cap {
                    Err(io::ErrorKind::InvalidData.into())
                } else {
                    Ok(text)
                };
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(result
                    .err()
                    .unwrap_or_else(|| io::ErrorKind::TimedOut.into()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detection_is_injectable_and_flatpak_takes_precedence() {
        assert_eq!(
            Packaging::detect(Some(OsStr::new("")), false, Some(OsStr::new("app"))),
            Packaging::Flatpak
        );
        assert_eq!(Packaging::detect(None, true, None), Packaging::Flatpak);
        assert_eq!(
            Packaging::detect(None, false, Some(OsStr::new(""))),
            Packaging::AppImage
        );
        assert_eq!(Packaging::detect(None, false, None), Packaging::Plain);
    }
    #[test]
    fn private_xdg_paths_are_ignored_only_in_flatpak() {
        for (key, fallback) in [
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_CONFIG_HOME", ".config"),
        ] {
            let env = |k: &str| (k == key).then(|| OsString::from("/home/u/.var/app/id"));
            assert_eq!(
                desktop_dir_with(Packaging::Flatpak, Path::new("/home/u"), key, fallback, env),
                Path::new("/home/u").join(fallback)
            );
            assert_eq!(
                desktop_dir_with(Packaging::Plain, Path::new("/home/u"), key, fallback, env),
                Path::new("/home/u/.var/app/id")
            );
            assert_eq!(
                desktop_dir_with(
                    Packaging::Flatpak,
                    Path::new("/home/u"),
                    key,
                    fallback,
                    |_| Some("/host/custom".into())
                ),
                Path::new("/host/custom")
            );
        }
    }
    #[test]
    fn host_commands_preserve_arguments() {
        for program in ["lsblk", "udisksctl", "umount", "eject", "gio", "head"] {
            let mut c = host_command_with(Packaging::Flatpak, program);
            c.arg("a b;$x");
            assert_eq!(c.get_program(), "flatpak-spawn");
            assert_eq!(
                c.get_args().take(2).collect::<Vec<_>>(),
                ["--host", "--watch-bus"]
            );
            assert_eq!(c.get_args().last().unwrap(), "a b;$x");
            assert_eq!(
                host_command_with(Packaging::Plain, program).get_program(),
                program
            );
        }
    }
}
