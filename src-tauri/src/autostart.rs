//! XDG autostart entry (`$XDG_CONFIG_HOME/autostart`). Never replace or remove an unrelated entry.
use crate::desktop_entry::{self, invalid, ID, MARKER};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

/// Command-line flag of the autostart entry: start without showing the main window.
pub const MINIMIZED_FLAG: &str = "--minimized";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Off,
    Minimized,
    Shown,
}

pub fn minimized_requested<S: AsRef<OsStr>>(args: impl IntoIterator<Item = S>) -> bool {
    args.into_iter()
        .skip(1)
        .any(|arg| arg.as_ref() == MINIMIZED_FLAG)
}

/// A relative `XDG_CONFIG_HOME` is invalid by specification and ignored.
pub fn config_home(xdg_config_home: Option<&OsStr>, home: &Path) -> PathBuf {
    xdg_config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
}

fn exec_line(exe: &Path, minimized: bool) -> io::Result<String> {
    let exec = desktop_entry::quote_exec(exe)?;
    Ok(if minimized {
        format!("Exec={exec} {MINIMIZED_FLAG}")
    } else {
        format!("Exec={exec}")
    })
}

pub fn render(exe: &Path, minimized: bool) -> io::Result<String> {
    let exec = exec_line(exe, minimized)?;
    Ok(format!("[Desktop Entry]\nType=Application\nName=Dualfile\nComment=Two-pane file manager\nComment[de]=Zwei-Fenster-Dateimanager\n{exec}\nIcon={ID}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n{MARKER}\n"))
}

pub struct Autostart {
    entry: PathBuf,
}
impl Autostart {
    pub fn new(config_home: &Path) -> Self {
        Self {
            entry: config_home.join(format!("autostart/{ID}.desktop")),
        }
    }
    pub fn from_env() -> io::Result<Self> {
        Ok(Self::new(&config_home(
            std::env::var_os("XDG_CONFIG_HOME").as_deref(),
            &desktop_entry::home()?,
        )))
    }
    // A symlinked entry is someone else's arrangement: neither read through nor replaced.
    fn contents(&self) -> io::Result<Option<String>> {
        match fs::symlink_metadata(&self.entry) {
            Ok(meta) if meta.file_type().is_symlink() => {
                Err(invalid("Autostart entry is a symlink"))
            }
            Ok(_) => fs::read_to_string(&self.entry).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn owned(s: &str) -> bool {
        s.lines().any(|line| line == MARKER)
    }
    // An entry switched off with desktop tools (Hidden, GNOME's enabled key) counts as off.
    fn mode_of(s: &str) -> Mode {
        let disabled = s
            .lines()
            .any(|line| line == "Hidden=true" || line == "X-GNOME-Autostart-enabled=false");
        match s.lines().find(|line| line.starts_with("Exec=")) {
            Some(_) if disabled || !Self::owned(s) => Mode::Off,
            Some(exec) if exec.ends_with(&format!(" {MINIMIZED_FLAG}")) => Mode::Minimized,
            Some(_) => Mode::Shown,
            None => Mode::Off,
        }
    }
    /// What the desktop will really do at login, read from the entry itself.
    pub fn mode(&self) -> io::Result<Mode> {
        Ok(self.contents()?.map_or(Mode::Off, |s| Self::mode_of(&s)))
    }
    pub fn set(&self, mode: Mode, exe: &Path) -> io::Result<()> {
        let existing = self.contents()?;
        if existing.as_deref().is_some_and(|s| !Self::owned(s)) {
            return Err(invalid("Autostart entry belongs to another application"));
        }
        if mode == Mode::Off {
            return match existing {
                Some(_) => fs::remove_file(&self.entry),
                None => Ok(()),
            };
        }
        let text = render(exe, mode == Mode::Minimized)?;
        fs::create_dir_all(self.entry.parent().ok_or_else(|| invalid("No parent"))?)?;
        desktop_entry::write_atomic(&self.entry, text.as_bytes())
    }
    /// A moved AppImage leaves a stale Exec path behind; rewrite an enabled entry of ours.
    pub fn refresh(&self, exe: &Path) -> io::Result<()> {
        let Some(s) = self.contents()? else {
            return Ok(());
        };
        let mode = Self::mode_of(&s);
        if mode != Mode::Off
            && !s
                .lines()
                .any(|line| exec_line(exe, mode == Mode::Minimized).is_ok_and(|exec| line == exec))
        {
            self.set(mode, exe)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(a: &Autostart) -> String {
        fs::read_to_string(&a.entry).unwrap()
    }

    #[test]
    fn exec_is_quoted_and_carries_the_minimized_flag_only_when_asked() {
        let shown = render(Path::new("/opt/My Apps/Dual file.AppImage"), false).unwrap();
        assert!(shown.contains("\nExec=\"/opt/My Apps/Dual file.AppImage\"\n"));
        let minimized = render(Path::new("/opt/My Apps/Dual file.AppImage"), true).unwrap();
        assert!(minimized.contains("\nExec=\"/opt/My Apps/Dual file.AppImage\" --minimized\n"));
        for text in [shown, minimized] {
            assert!(text.starts_with("[Desktop Entry]\nType=Application\nName=Dualfile\n"));
            assert!(text.contains("\nX-GNOME-Autostart-enabled=true\n"));
            assert!(text.contains(&format!("\nIcon={ID}\n")) && text.contains("\nComment="));
            assert!(text.ends_with(&format!("{MARKER}\n")));
        }
        let odd = render(Path::new("/tmp/$cash`x\"%.AppImage"), true).unwrap();
        assert!(odd.contains("Exec=\"/tmp/\\\\$cash\\\\`x\\\\\"%%.AppImage\" --minimized\n"));
        assert!(render(Path::new("/tmp/x\nExec=evil"), false).is_err());
        assert!(render(Path::new("relative"), false).is_err());
    }

    #[test]
    fn config_home_respects_an_absolute_xdg_config_home_only() {
        let home = Path::new("/home/u");
        assert_eq!(config_home(None, home), Path::new("/home/u/.config"));
        assert_eq!(
            config_home(Some(OsStr::new("/data/cfg")), home),
            Path::new("/data/cfg")
        );
        assert_eq!(
            config_home(Some(OsStr::new("relative")), home),
            Path::new("/home/u/.config")
        );
        assert_eq!(
            config_home(Some(OsStr::new("")), home),
            Path::new("/home/u/.config")
        );
        assert_eq!(
            Autostart::new(Path::new("/data/cfg")).entry,
            Path::new("/data/cfg/autostart/de.thebexxs.dualfile.desktop")
        );
    }

    #[test]
    fn only_the_flag_after_the_program_name_requests_a_minimized_start() {
        assert!(minimized_requested(["dualfile", "--minimized"]));
        assert!(minimized_requested([
            "dualfile",
            "/some/dir",
            "--minimized"
        ]));
        assert!(!minimized_requested(["dualfile"]));
        assert!(!minimized_requested(["--minimized"]));
        assert!(!minimized_requested(["dualfile", "--minimized=1"]));
    }

    #[test]
    fn set_creates_the_directory_switches_modes_and_off_removes_the_entry() {
        let config = tempfile::tempdir().unwrap();
        let a = Autostart::new(config.path());
        assert_eq!(a.mode().unwrap(), Mode::Off);
        a.set(Mode::Off, Path::new("/tmp/app")).unwrap();
        assert!(!config.path().join("autostart").exists());
        a.set(Mode::Minimized, Path::new("/tmp/My App.AppImage"))
            .unwrap();
        assert_eq!(a.mode().unwrap(), Mode::Minimized);
        assert_eq!(
            entry(&a),
            render(Path::new("/tmp/My App.AppImage"), true).unwrap()
        );
        a.set(Mode::Shown, Path::new("/tmp/My App.AppImage"))
            .unwrap();
        assert_eq!(a.mode().unwrap(), Mode::Shown);
        assert!(!entry(&a).contains(MINIMIZED_FLAG));
        // Atomic replace: nothing but the entry is left in the directory.
        assert_eq!(fs::read_dir(a.entry.parent().unwrap()).unwrap().count(), 1);
        a.set(Mode::Off, Path::new("/tmp/My App.AppImage")).unwrap();
        assert!(!a.entry.exists());
        assert_eq!(a.mode().unwrap(), Mode::Off);
    }

    #[test]
    fn foreign_entry_is_neither_replaced_nor_removed_and_counts_as_off() {
        let config = tempfile::tempdir().unwrap();
        let a = Autostart::new(config.path());
        fs::create_dir_all(a.entry.parent().unwrap()).unwrap();
        let foreign = "[Desktop Entry]\nName=Other\nExec=/usr/bin/other\n";
        fs::write(&a.entry, foreign).unwrap();
        assert_eq!(a.mode().unwrap(), Mode::Off);
        assert!(a.set(Mode::Shown, Path::new("/tmp/app")).is_err());
        assert!(a.set(Mode::Off, Path::new("/tmp/app")).is_err());
        a.refresh(Path::new("/tmp/app")).unwrap();
        assert_eq!(entry(&a), foreign);
    }

    #[test]
    fn refresh_follows_a_moved_appimage_and_keeps_the_mode() {
        let config = tempfile::tempdir().unwrap();
        let a = Autostart::new(config.path());
        a.refresh(Path::new("/tmp/app")).unwrap();
        assert!(!a.entry.exists());
        for mode in [Mode::Minimized, Mode::Shown] {
            a.set(mode, Path::new("/old/Dualfile.AppImage")).unwrap();
            a.refresh(Path::new("/new place/Dualfile.AppImage"))
                .unwrap();
            assert_eq!(
                entry(&a),
                render(
                    Path::new("/new place/Dualfile.AppImage"),
                    mode == Mode::Minimized
                )
                .unwrap()
            );
            assert_eq!(a.mode().unwrap(), mode);
        }
    }

    #[test]
    fn refresh_leaves_a_current_or_user_disabled_entry_untouched() {
        let config = tempfile::tempdir().unwrap();
        let a = Autostart::new(config.path());
        let exe = Path::new("/tmp/Dualfile.AppImage");
        a.set(Mode::Minimized, exe).unwrap();
        let edited = entry(&a).replace("Name=Dualfile", "Name=Dualfile (mine)");
        fs::write(&a.entry, &edited).unwrap();
        a.refresh(exe).unwrap();
        assert_eq!(entry(&a), edited);
        for off in ["Hidden=true", "X-GNOME-Autostart-enabled=false"] {
            let disabled = render(exe, false)
                .unwrap()
                .replace("X-GNOME-Autostart-enabled=true", off);
            fs::write(&a.entry, &disabled).unwrap();
            assert_eq!(a.mode().unwrap(), Mode::Off);
            a.refresh(Path::new("/moved/Dualfile.AppImage")).unwrap();
            assert_eq!(entry(&a), disabled);
        }
        // Choosing a mode again takes the entry back; Off removes our disabled entry.
        a.set(Mode::Shown, exe).unwrap();
        assert_eq!(a.mode().unwrap(), Mode::Shown);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_entry_is_refused() {
        let config = tempfile::tempdir().unwrap();
        let a = Autostart::new(config.path());
        a.set(Mode::Shown, Path::new("/tmp/app")).unwrap();
        let target = config.path().join("elsewhere.desktop");
        fs::rename(&a.entry, &target).unwrap();
        std::os::unix::fs::symlink(&target, &a.entry).unwrap();
        assert!(a.mode().is_err());
        assert!(a.set(Mode::Off, Path::new("/tmp/app")).is_err());
        assert!(a.set(Mode::Minimized, Path::new("/tmp/app")).is_err());
        assert!(
            target.exists()
                && !fs::read_to_string(&target)
                    .unwrap()
                    .contains(MINIMIZED_FLAG)
        );
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_autostart_directory_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let config = tempfile::tempdir().unwrap();
        let a = Autostart::new(config.path());
        let dir = a.entry.parent().unwrap().to_owned();
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let result = a.set(Mode::Shown, Path::new("/tmp/app"));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
        assert_eq!(a.mode().unwrap(), Mode::Off);
    }
}
