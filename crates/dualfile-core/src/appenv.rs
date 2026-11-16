//! Remove AppImage runtime paths before starting external system programs.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::process::Command;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EnvPlan {
    pub remove: BTreeSet<OsString>,
    pub replace: BTreeMap<OsString, OsString>,
}

impl EnvPlan {
    pub fn apply(&self, command: &mut Command) {
        for key in &self.remove {
            command.env_remove(key);
        }
        command.envs(&self.replace);
    }
}

/// Plan changes without reading or mutating the process environment.
/// Empty path elements are preserved: on Unix they can mean the current directory.
pub fn clean_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    appdir: Option<&OsStr>,
) -> EnvPlan {
    let mut plan = EnvPlan::default();
    let Some(appdir) = appdir else {
        return plan;
    };
    let root = appdir.as_encoded_bytes();
    let root = root.strip_suffix(b"/").unwrap_or(root);
    // After an in-app update the app restarts with the old process environment, so paths of
    // the previous AppImage (still mounted while its runtime lingers) sit next to the current
    // ones. Its bundled libraries would then be loaded by every program started from here.
    // Every AppImage runtime mounts at `<tmp>/.mount_*`, so the previous one shares the parent
    // directory of the current APPDIR.
    let mount_prefix = root
        .iter()
        .rposition(|b| *b == b'/')
        .filter(|_| {
            root.rsplit(|b| *b == b'/')
                .next()
                .is_some_and(|name| name.starts_with(b".mount_"))
        })
        .map(|slash| [&root[..slash], b"/.mount_".as_slice()].concat());
    let in_any_mount = |entry: &[u8]| {
        mount_prefix
            .as_deref()
            .is_some_and(|p| entry.starts_with(p))
    };
    let under_appdir = |entry: &[u8]| {
        (!appdir.is_empty()
            && (entry == root
                || entry
                    .strip_prefix(root)
                    .is_some_and(|tail| tail.starts_with(b"/"))))
            || in_any_mount(entry)
    };
    for (key, value) in vars {
        if key == "GTK_THEME" {
            continue;
        }
        if matches!(
            key.to_str(),
            Some(
                "APPDIR"
                    | "APPIMAGE"
                    | "ARGV0"
                    | "OWD"
                    | "GDK_PIXBUF_MODULE_FILE"
                    | "GTK_EXE_PREFIX"
                    | "GTK_DATA_PREFIX"
                    | "GTK_PATH"
                    | "GTK_IM_MODULE_FILE"
                    | "GIO_MODULE_DIR"
                    | "GIO_EXTRA_MODULES"
                    | "GST_PLUGIN_SYSTEM_PATH"
                    | "GST_PLUGIN_SYSTEM_PATH_1_0"
                    | "GSETTINGS_SCHEMA_DIR"
                    | "PYTHONPATH"
                    | "QT_PLUGIN_PATH"
                    | "PERLLIB"
            )
        ) {
            plan.remove.insert(key);
            continue;
        }
        let entries: Vec<_> = value.as_encoded_bytes().split(|b| *b == b':').collect();
        let kept: Vec<_> = entries
            .iter()
            .copied()
            .filter(|v| !under_appdir(v))
            .collect();
        if entries.len() == kept.len() {
            continue;
        }
        let cleaned = kept.join(&b':');
        if cleaned.is_empty() {
            plan.remove.insert(key);
        } else {
            // SAFETY: splitting at an ASCII colon preserves encoding boundaries, and
            // every remaining byte comes from the same valid OsStr encoding.
            let cleaned = unsafe { OsStr::from_encoded_bytes_unchecked(&cleaned) };
            plan.replace.insert(key, cleaned.to_owned());
        }
    }
    plan
}

/// Choose a working directory outside the AppImage mount, without reading global state.
pub fn external_current_dir(
    appdir: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<std::path::PathBuf> {
    appdir.map(|_| {
        home.map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| "/".into())
    })
}

pub fn apply_external_current_dir(command: &mut Command) {
    if let Some(path) = external_current_dir(
        std::env::var_os("APPDIR").as_deref(),
        std::env::var_os("HOME").as_deref(),
    ) {
        command.current_dir(path);
    }
}

pub fn apply_clean_env(command: &mut Command) {
    let appdir = std::env::var_os("APPDIR");
    clean_env(std::env::vars_os(), appdir.as_deref()).apply(command);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(vars: &[(&str, &str)]) -> EnvPlan {
        clean_env(
            vars.iter().map(|(k, v)| ((*k).into(), (*v).into())),
            Some(OsStr::new("/tmp/.mount_x")),
        )
    }

    #[test]
    fn paths_of_a_previous_appimage_mount_are_removed_too() {
        // Environment of the process restarted by an in-app update.
        let result = plan(&[
            ("APPDIR", "/tmp/.mount_x"),
            (
                "LD_LIBRARY_PATH",
                "/tmp/.mount_x/usr/lib/:/tmp/.mount_old.AbC/usr/lib/:/opt/lib",
            ),
            (
                "XDG_DATA_DIRS",
                "/tmp/.mount_x/usr/share:/usr/share:/tmp/.mount_old.AbC/usr/share:/usr/local/share",
            ),
            (
                "PATH",
                "/tmp/.mount_x/usr/bin/:/tmp/.mount_old.AbC/usr/bin/:/usr/bin",
            ),
            ("HOME_VALUE", "/home/me/.mount_notes/file"),
        ]);
        assert_eq!(
            result.replace.get(OsStr::new("LD_LIBRARY_PATH")).unwrap(),
            "/opt/lib"
        );
        assert_eq!(
            result.replace.get(OsStr::new("XDG_DATA_DIRS")).unwrap(),
            "/usr/share:/usr/local/share"
        );
        assert_eq!(result.replace.get(OsStr::new("PATH")).unwrap(), "/usr/bin");
        assert!(!result.replace.contains_key(OsStr::new("HOME_VALUE")));
        assert!(!result.remove.contains(OsStr::new("HOME_VALUE")));
    }

    #[test]
    fn cleans_runtime_paths_and_preserves_user_values() {
        let result = plan(&[
            ("APPDIR", "/tmp/.mount_x"),
            ("APPIMAGE", "/home/me/Dualfile.AppImage"),
            ("OWD", "/home/me"),
            ("GTK_PATH", "/usr/lib/gtk"),
            ("PATH", "/tmp/.mount_x/usr/bin:/usr/bin:/bin"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_x/lib"),
            (
                "XDG_DATA_DIRS",
                "/usr/share:/tmp/.mount_x/usr/share:/usr/local/share",
            ),
            ("UNKNOWN", "/tmp/.mount_x/other"),
            ("ROOT", "/tmp/.mount_x"),
            ("PREFIX", "/tmp/.mountx/lib"),
            ("GDK_BACKEND", "wayland"),
            ("GTK_THEME", "/tmp/.mount_x/theme"),
            ("USER_VALUE", "hello"),
        ]);
        assert_eq!(
            result.remove,
            [
                "APPDIR",
                "APPIMAGE",
                "OWD",
                "GTK_PATH",
                "LD_LIBRARY_PATH",
                "UNKNOWN",
                "ROOT"
            ]
            .map(OsString::from)
            .into()
        );
        assert_eq!(
            result.replace,
            [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (
                    OsString::from("XDG_DATA_DIRS"),
                    OsString::from("/usr/share:/usr/local/share")
                )
            ]
            .into()
        );
    }

    #[test]
    fn handles_unknown_lists_empty_elements_and_path_boundaries() {
        let result = plan(&[
            ("CUSTOM", ":/tmp/.mount_x/lib::/tmp/.mountx/lib:"),
            ("LD_PRELOAD", "/tmp/.mount_x/a:/usr/lib/b"),
            ("XDG_DATA_DIRS", "/tmp/.mount_x/share:/tmp/.mount_x"),
            ("GDK_BACKEND", "/tmp/.mount_x/backend"),
            ("EMPTY", "::"),
        ]);
        assert_eq!(result.replace[OsStr::new("CUSTOM")], "::/tmp/.mountx/lib:");
        assert_eq!(result.replace[OsStr::new("LD_PRELOAD")], "/usr/lib/b");
        assert_eq!(
            result.remove,
            ["XDG_DATA_DIRS", "GDK_BACKEND"].map(OsString::from).into()
        );
        assert!(!result.replace.contains_key(OsStr::new("EMPTY")));
    }

    #[test]
    fn no_appdir_means_no_changes() {
        assert_eq!(
            clean_env(
                [
                    ("PATH".into(), "/tmp/.mount_x/bin:/bin".into()),
                    ("APPIMAGE".into(), "image".into())
                ],
                None
            ),
            EnvPlan::default()
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;
        let result = clean_env(
            [(
                "PATH".into(),
                OsString::from_vec(b"/tmp/.mount_x/bin:/system/\xff".to_vec()),
            )],
            Some(OsStr::new("/tmp/.mount_x")),
        );
        assert_eq!(
            result.replace[OsStr::new("PATH")],
            OsString::from_vec(b"/system/\xff".to_vec())
        );
    }
}

#[cfg(test)]
mod cwd_tests {
    use super::*;
    #[test]
    fn appimage_external_current_dir_uses_home_or_root() {
        for (appdir, home, expected) in [
            (
                Some("/tmp/.mount_x"),
                Some("/home/demo"),
                Some("/home/demo"),
            ),
            (Some("/tmp/.mount_x"), None, Some("/")),
            (Some("/tmp/.mount_x"), Some(""), Some("/")),
            (Some("/tmp/.mount_x"), Some("relative"), Some("/")),
            (None, Some("/home/demo"), None),
        ] {
            let mut command = Command::new("xdg-open");
            if let Some(path) = external_current_dir(appdir.map(OsStr::new), home.map(OsStr::new)) {
                command.current_dir(path);
            }
            assert_eq!(
                command.get_current_dir(),
                expected.map(std::path::Path::new)
            );
        }
    }
}
