//! Register of external helper programs (no UI yet).
//!
//! One place that knows which helper programs Dualfile can use, what they are for and where
//! they are looked up: first on the system, then the copy shipped with the package, then a
//! second system lookup. Detection only looks at files; nothing is ever started here.

use dualfile_platform::Packaging;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// File name of the helper shipped with AppImage and Flatpak (7-Zip, `7zzs` renamed to `7zz`).
pub const BUNDLED_7ZZ: &str = "7zz";
/// Location of the shipped helper inside a Flatpak.
pub const FLATPAK_BIN_DIR: &str = "/app/bin";

/// How a helper program is used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelperKind {
    Unrar,
    SevenZip,
}

/// Where a found helper comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    /// Found in `PATH` (or the configured search path), before the shipped copy.
    System,
    /// Shipped with the AppImage or Flatpak.
    Bundled,
    /// Found in `PATH` after the shipped copy (`7zz`, `7z`).
    SystemFallback,
}

/// A helper that was found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub kind: HelperKind,
    pub program: PathBuf,
    pub origin: Origin,
}

/// One entry of the register.
#[derive(Clone, Copy, Debug)]
pub struct HelperSpec {
    /// Stable id for later UI and messages.
    pub id: &'static str,
    /// What it is needed for.
    pub purpose: &'static str,
    /// Program names tried in a system search, in order, before the shipped copy.
    pub first: &'static [(&'static str, HelperKind)],
    /// Whether a copy shipped with the package is tried after `first`.
    pub bundled: Option<&'static str>,
    /// Program names tried in a system search after the shipped copy.
    pub last: &'static [(&'static str, HelperKind)],
}

pub const REGISTER: &[HelperSpec] = &[HelperSpec {
    id: "rar",
    purpose: "Reading RAR archives",
    first: &[("unrar", HelperKind::Unrar)],
    bundled: Some(BUNDLED_7ZZ),
    last: &[("7zz", HelperKind::SevenZip), ("7z", HelperKind::SevenZip)],
}];

pub fn spec(id: &str) -> Option<&'static HelperSpec> {
    REGISTER.iter().find(|s| s.id == id)
}

/// What the lookup of shipped helpers depends on.
#[derive(Clone, Debug)]
pub struct HelperEnv {
    pub packaging: Packaging,
    pub appdir: Option<OsString>,
    /// Directory of the running executable (AppImage fallback when `APPDIR` is missing).
    pub exe_dir: Option<PathBuf>,
    /// `/app/bin` in a real Flatpak; replaceable for tests.
    pub flatpak_bin: PathBuf,
}

impl HelperEnv {
    pub fn current() -> Self {
        Self {
            packaging: dualfile_platform::packaging(),
            appdir: std::env::var_os("APPDIR"),
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
            flatpak_bin: PathBuf::from(FLATPAK_BIN_DIR),
        }
    }

    /// `(root, dir)` pairs that may hold a shipped helper; a helper must stay inside its `root`.
    fn bundled_dirs(&self) -> Vec<(PathBuf, PathBuf)> {
        match self.packaging {
            Packaging::Flatpak => {
                let root = self.flatpak_bin.parent().unwrap_or(&self.flatpak_bin);
                vec![(root.to_path_buf(), self.flatpak_bin.clone())]
            }
            Packaging::AppImage => {
                let mut dirs = Vec::new();
                if let Some(appdir) = self.appdir.as_deref().map(PathBuf::from) {
                    if appdir.is_absolute() {
                        dirs.push((appdir.clone(), appdir.join("usr/bin")));
                    }
                } else if let Some(exe) = &self.exe_dir {
                    dirs.push((exe.clone(), exe.clone()));
                }
                dirs
            }
            Packaging::Plain => Vec::new(),
        }
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// First executable `name` in `dirs`.
pub fn find_in_dirs(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(name))
        .find(|c| is_executable_file(c))
}

/// A shipped helper is only accepted when its real path (links resolved) stays inside the
/// real `root` it is supposed to live in.
fn bundled_in(root: &Path, dir: &Path, name: &str) -> Option<PathBuf> {
    let candidate = dir.join(name);
    if !is_executable_file(&candidate) {
        return None;
    }
    let real_root = std::fs::canonicalize(root).ok()?;
    let real = std::fs::canonicalize(&candidate).ok()?;
    (real.starts_with(&real_root) && is_executable_file(&real)).then_some(candidate)
}

/// Finds the shipped copy of `name` for this package, if any.
pub fn find_bundled(name: &str, env: &HelperEnv) -> Option<PathBuf> {
    env.bundled_dirs()
        .iter()
        .find_map(|(root, dir)| bundled_in(root, dir, name))
}

/// Looks `spec` up: system names first, then the shipped copy, then the second system list.
/// `search` is the directory list for system lookups (`PATH` when `None`).
pub fn locate(spec: &HelperSpec, search: Option<&OsStr>, env: &HelperEnv) -> Option<Found> {
    let path = search
        .map(OsStr::to_owned)
        .or_else(|| std::env::var_os("PATH"));
    let dirs: Vec<PathBuf> = path
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    let system = |list: &[(&str, HelperKind)], origin| {
        list.iter().find_map(|(name, kind)| {
            find_in_dirs(name, &dirs).map(|program| Found {
                kind: *kind,
                program,
                origin,
            })
        })
    };
    system(spec.first, Origin::System)
        .or_else(|| {
            let name = spec.bundled?;
            find_bundled(name, env).map(|program| Found {
                kind: HelperKind::SevenZip,
                program,
                origin: Origin::Bundled,
            })
        })
        .or_else(|| system(spec.last, Origin::SystemFallback))
}

/// Status of one register entry, for a later system check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelperStatus {
    pub id: &'static str,
    pub purpose: &'static str,
    pub found: Option<Found>,
}

/// Reports every register entry; only looks at files, never starts a program.
pub fn report(search: Option<&OsStr>, env: &HelperEnv) -> Vec<HelperStatus> {
    REGISTER
        .iter()
        .map(|s| HelperStatus {
            id: s.id,
            purpose: s.purpose,
            found: locate(s, search, env),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn exe(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn env(packaging: Packaging, appdir: Option<&Path>, flatpak: &Path) -> HelperEnv {
        HelperEnv {
            packaging,
            appdir: appdir.map(|p| p.as_os_str().to_owned()),
            exe_dir: None,
            flatpak_bin: flatpak.to_path_buf(),
        }
    }

    fn rar() -> &'static HelperSpec {
        spec("rar").unwrap()
    }

    #[test]
    fn register_lists_the_rar_helpers() {
        let s = rar();
        assert_eq!(s.first[0].0, "unrar");
        assert_eq!(s.bundled, Some("7zz"));
        assert_eq!(
            s.last.iter().map(|l| l.0).collect::<Vec<_>>(),
            ["7zz", "7z"]
        );
        assert!(spec("nope").is_none());
    }

    #[test]
    fn order_is_system_unrar_then_bundled_then_path_7zz() {
        let sys = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        exe(&app.path().join("usr/bin"), "7zz");
        let e = env(
            Packaging::AppImage,
            Some(app.path()),
            Path::new("/nonexistent"),
        );
        let search = sys.path().as_os_str();

        // only the shipped copy
        let f = locate(rar(), Some(search), &e).unwrap();
        assert_eq!(f.origin, Origin::Bundled);
        assert_eq!(f.kind, HelperKind::SevenZip);
        // a system 7zz does not beat the shipped copy
        exe(sys.path(), "7zz");
        assert_eq!(
            locate(rar(), Some(search), &e).unwrap().origin,
            Origin::Bundled
        );
        // a system unrar beats everything
        exe(sys.path(), "unrar");
        let f = locate(rar(), Some(search), &e).unwrap();
        assert_eq!((f.kind, f.origin), (HelperKind::Unrar, Origin::System));
    }

    #[test]
    fn falls_back_to_path_7zz_then_7z_and_then_nothing() {
        let sys = tempfile::tempdir().unwrap();
        let e = env(Packaging::Plain, None, Path::new("/nonexistent"));
        let search = sys.path().as_os_str();
        assert!(locate(rar(), Some(search), &e).is_none());
        exe(sys.path(), "7z");
        let f = locate(rar(), Some(search), &e).unwrap();
        assert!(f.program.ends_with("7z"));
        assert_eq!(f.origin, Origin::SystemFallback);
        exe(sys.path(), "7zz");
        assert!(locate(rar(), Some(search), &e)
            .unwrap()
            .program
            .ends_with("7zz"));
    }

    #[test]
    fn plain_build_ignores_a_stray_appdir() {
        let sys = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        exe(&app.path().join("usr/bin"), "7zz");
        let e = env(Packaging::Plain, Some(app.path()), app.path());
        assert!(locate(rar(), Some(sys.path().as_os_str()), &e).is_none());
    }

    #[test]
    fn flatpak_uses_its_own_bin_dir() {
        let sys = tempfile::tempdir().unwrap();
        let flat = tempfile::tempdir().unwrap();
        exe(flat.path(), "7zz");
        let e = env(Packaging::Flatpak, None, flat.path());
        let f = locate(rar(), Some(sys.path().as_os_str()), &e).unwrap();
        assert_eq!(f.origin, Origin::Bundled);
        assert_eq!(f.program, flat.path().join("7zz"));
    }

    #[test]
    fn appdir_must_be_absolute_and_links_may_not_leave_it() {
        let sys = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        exe(outside.path(), "7zz");
        let app = tempfile::tempdir().unwrap();
        fs::create_dir_all(app.path().join("usr/bin")).unwrap();
        symlink(outside.path().join("7zz"), app.path().join("usr/bin/7zz")).unwrap();
        let e = env(
            Packaging::AppImage,
            Some(app.path()),
            Path::new("/nonexistent"),
        );
        assert!(locate(rar(), Some(sys.path().as_os_str()), &e).is_none());

        let rel = env(
            Packaging::AppImage,
            Some(Path::new("relative/dir")),
            Path::new("/x"),
        );
        assert!(rel.bundled_dirs().is_empty());
        // a link that stays inside is fine
        let app2 = tempfile::tempdir().unwrap();
        exe(&app2.path().join("usr/lib"), "7zz-real");
        fs::create_dir_all(app2.path().join("usr/bin")).unwrap();
        symlink("../lib/7zz-real", app2.path().join("usr/bin/7zz")).unwrap();
        let e = env(
            Packaging::AppImage,
            Some(app2.path()),
            Path::new("/nonexistent"),
        );
        assert!(locate(rar(), Some(sys.path().as_os_str()), &e).is_some());
    }

    #[test]
    fn appimage_without_appdir_uses_the_exe_directory() {
        let sys = tempfile::tempdir().unwrap();
        let exe_dir = tempfile::tempdir().unwrap();
        exe(exe_dir.path(), "7zz");
        let mut e = env(Packaging::AppImage, None, Path::new("/nonexistent"));
        e.exe_dir = Some(exe_dir.path().to_path_buf());
        assert_eq!(
            locate(rar(), Some(sys.path().as_os_str()), &e)
                .unwrap()
                .origin,
            Origin::Bundled
        );
    }

    #[test]
    fn a_file_without_the_execute_bit_does_not_count() {
        let app = tempfile::tempdir().unwrap();
        fs::create_dir_all(app.path().join("usr/bin")).unwrap();
        fs::write(app.path().join("usr/bin/7zz"), "x").unwrap();
        let sys = tempfile::tempdir().unwrap();
        let e = env(
            Packaging::AppImage,
            Some(app.path()),
            Path::new("/nonexistent"),
        );
        assert!(locate(rar(), Some(sys.path().as_os_str()), &e).is_none());
    }

    #[test]
    fn report_has_one_entry_per_register_item() {
        let sys = tempfile::tempdir().unwrap();
        let e = env(Packaging::Plain, None, Path::new("/nonexistent"));
        let r = report(Some(sys.path().as_os_str()), &e);
        assert_eq!(r.len(), REGISTER.len());
        assert_eq!(r[0].id, "rar");
        assert!(r[0].found.is_none());
    }
}
