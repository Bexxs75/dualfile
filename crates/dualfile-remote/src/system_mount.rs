//! Delegate authentication and mounting to the desktop. Capture only bounded, sanitized helper diagnostics.
use dualfile_core::system_places::{mounted_path_bounded, parse_url, DiscoveryPaths};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
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
    dualfile_core::appenv::run_isolated(command, timeout).map_err(|e| match e {
        dualfile_core::appenv::HelperError::Unavailable => MountError::Unavailable,
        dualfile_core::appenv::HelperError::Failed(message) => MountError::Failed(message),
        dualfile_core::appenv::HelperError::Timeout => MountError::Timeout(String::new()),
    })
}
fn bounded_mounted_path(
    place: &dualfile_core::system_places::SystemPlace,
    paths: &DiscoveryPaths,
) -> Option<PathBuf> {
    mounted_path_bounded(place, paths)
}
pub fn connect(url: &str, paths: &DiscoveryPaths) -> Result<PathBuf, MountError> {
    connect_with(url, paths, || {
        run_gio(Path::new("gio"), url, false, Duration::from_secs(30))
    })
}
fn connect_with(
    url: &str,
    paths: &DiscoveryPaths,
    mount: impl FnOnce() -> Result<(), MountError>,
) -> Result<PathBuf, MountError> {
    let place = parse_url(url, "", "")
        .filter(|p| p.url == url)
        .ok_or(MountError::Invalid)?;
    if let Some(path) = bounded_mounted_path(&place, paths) {
        return Ok(path);
    }
    let result = mount();
    // A timeout does not imply failure: the daemon may have completed the mount.
    if result.is_err() {
        if let Some(path) = bounded_mounted_path(&place, paths) {
            return Ok(path);
        }
        result?;
    }
    // Give the FUSE daemon a bounded opportunity to publish the mount.
    for _ in 0..20 {
        if let Some(path) = bounded_mounted_path(&place, paths) {
            return Ok(path);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(MountError::Failed(String::new()))
}
pub fn unmount(url: &str) -> Result<(), MountError> {
    let result = run_gio(Path::new("gio"), url, true, Duration::from_secs(30));
    if matches!(result, Err(MountError::Timeout(_))) {
        let (mounts, complete) = dualfile_core::system_places::probe_mounts(
            &DiscoveryPaths::system(),
            dualfile_core::places_pool::mount_checks(),
            Duration::from_millis(750),
        );
        if complete && mounts.iter().all(|place| place.url != url) {
            return Ok(());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{connect, run_gio, MountError};
    use dualfile_core::system_places::DiscoveryPaths;
    use std::{fs, os::unix::fs::PermissionsExt};
    use std::{process::Command, time::Duration};
    #[test]
    fn connect_timeout_rechecks_mounts_even_when_sidebar_probes_are_stuck() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DiscoveryPaths {
            home: tmp.path().into(),
            data: tmp.path().into(),
            config: tmp.path().into(),
            runtime: tmp.path().join("run"),
        };
        fs::create_dir_all(paths.runtime.join("gvfs")).unwrap();
        let mut releases = Vec::new();
        for i in 0..4 {
            let (release, blocked) = std::sync::mpsc::channel();
            releases.push(release);
            dualfile_core::places_pool::paths().run_keyed(
                format!("test-hang-{i}"),
                Duration::from_millis(20),
                move || blocked.recv().unwrap(),
            );
        }
        let mounted = paths.runtime.join("gvfs/smb-share:server=host,share=team");
        let result = super::connect_with("smb://host/team", &paths, || {
            fs::create_dir_all(&mounted).unwrap();
            Err(MountError::Timeout(String::new()))
        });
        for release in releases {
            let _ = release.send(());
        }
        assert_eq!(result, Ok(mounted));
    }
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
    fn failed_gio_only_returns_allowlisted_diagnostics() {
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
        assert!(detail.is_empty());
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
        assert!(detail.is_empty());
    }
}
