//! User-local desktop integration. Never replace or remove an unrelated entry.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) const ID: &str = "com.dualfileapp.Dualfile";
pub(crate) const OLD_ID: &str = "de.thebexxs.dualfile";
pub(crate) const MARKER: &str = "X-Dualfile-Managed=true";
const ICON: &[u8] = include_bytes!("../icons/128x128.png");

pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

pub fn executable() -> io::Result<PathBuf> {
    executable_from(std::env::var_os("APPIMAGE"), std::env::current_exe)
}

// Inside an AppImage the running binary lives in a temporary mount (/tmp/.mount_*) that is
// gone after exit; only `$APPIMAGE` names the file the user really starts.
fn executable_from(
    app_image: Option<std::ffi::OsString>,
    current_exe: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    let path = app_image
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(current_exe)?;
    if !path.is_absolute() {
        return Err(invalid("Executable path must be absolute"));
    }
    Ok(path)
}

pub fn home() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| invalid("No absolute user home directory"))
}

// Desktop Entry escaping is applied after Exec argument quoting. Percent is a field code,
// not a shell character. Reject line breaks instead of allowing injected desktop keys.
pub(crate) fn quote_exec(exe: &Path) -> io::Result<String> {
    let raw = exe
        .to_str()
        .ok_or_else(|| invalid("Executable path is not UTF-8"))?;
    if !exe.is_absolute() || raw.chars().any(char::is_control) {
        return Err(invalid("Invalid executable path"));
    }
    let mut quoted = String::new();
    for c in raw.chars() {
        match c {
            '%' => quoted.push_str("%%"),
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            _ => quoted.push(c),
        }
    }
    Ok(format!("\"{quoted}\""))
}

pub fn render(exe: &Path) -> io::Result<String> {
    let exec = quote_exec(exe)?;
    Ok(format!("[Desktop Entry]\nType=Application\nName=Dualfile\nComment=Two-pane file manager\nComment[de]=Zwei-Fenster-Dateimanager\nExec={exec}\nIcon={ID}\nTerminal=false\nCategories=Utility;FileManager;\nStartupWMClass={ID}\n{MARKER}\n"))
}

/// Replaces `path` in one step, so a reader never sees a half-written entry.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| invalid("No parent"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub struct DesktopEntry {
    home: PathBuf,
}
impl DesktopEntry {
    pub fn new(home: PathBuf) -> io::Result<Self> {
        if !home.is_absolute() {
            return Err(invalid("Home must be absolute"));
        }
        Ok(Self {
            home: fs::canonicalize(home)?,
        })
    }
    fn entry(&self) -> PathBuf {
        self.home
            .join(format!(".local/share/applications/{ID}.desktop"))
    }
    fn icon(&self) -> PathBuf {
        self.home
            .join(format!(".local/share/icons/hicolor/128x128/apps/{ID}.png"))
    }
    // Refuse symlink components (including the final file); the fixed relative paths
    // cannot escape HOME even when XDG_DATA_HOME points elsewhere.
    fn safe(&self, path: &Path) -> io::Result<()> {
        let mut current = self.home.clone();
        for part in path
            .strip_prefix(&self.home)
            .map_err(|_| invalid("Outside home"))?
            .components()
        {
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(invalid("Symlink in desktop integration path"))
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn contents(&self) -> io::Result<Option<String>> {
        self.safe(&self.entry())?;
        match fs::read_to_string(self.entry()) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn exists(&self) -> io::Result<bool> {
        Ok(self.contents()?.is_some())
    }
    fn owned(s: &str) -> bool {
        s.lines().any(|line| line == MARKER)
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.safe(path)?;
        let parent = path.parent().ok_or_else(|| invalid("No parent"))?;
        fs::create_dir_all(parent)?;
        self.safe(path)?;
        write_atomic(path, bytes)
    }
    pub fn install(&self, exe: &Path) -> io::Result<()> {
        let text = render(exe)?;
        if self.contents()?.is_some_and(|s| !Self::owned(&s)) {
            return Err(invalid("Desktop entry belongs to another application"));
        }
        self.safe(&self.icon())?;
        // The icon path is unique to Dualfile (ID-named), and we only reach this step once the
        // desktop entry is absent or owned by us, so an icon already here is our own. Refresh it
        // when its bytes differ (e.g. an older version's icon) instead of refusing the install.
        match fs::read(self.icon()) {
            Ok(bytes) if bytes == ICON => {}
            Ok(_) => self.write(&self.icon(), ICON)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.write(&self.icon(), ICON)?,
            Err(e) => return Err(e),
        }
        self.write(&self.entry(), text.as_bytes())
    }
    /// Publish the new entry before removing a marked legacy entry. Unknown icons survive.
    pub fn migrate(&self, exe: &Path) -> io::Result<()> {
        let old_entry = self.entry().with_file_name(format!("{OLD_ID}.desktop"));
        let old_icon = self.icon().with_file_name(format!("{OLD_ID}.png"));
        self.safe(&old_entry)?;
        let text = match fs::read_to_string(&old_entry) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if !Self::owned(&text) {
            return Ok(());
        }
        self.safe(&old_icon)?;
        match self.contents()? {
            Some(text) if !Self::owned(&text) => {
                return Err(invalid("Desktop entry belongs to another application"))
            }
            Some(_) => {}
            None => {
                self.safe(&self.icon())?;
                match fs::read(self.icon()) {
                    Ok(bytes) if bytes != ICON => {
                        return Err(invalid("Unrecognized icon at new application path"))
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                self.install(exe)?;
            }
        }
        // Remove the entry last so a failed icon removal can be retried safely.
        if fs::read(&old_icon).is_ok_and(|bytes| bytes == ICON) {
            fs::remove_file(old_icon)?;
        }
        fs::remove_file(old_entry)
    }

    pub fn refresh(&self, exe: &Path) -> io::Result<()> {
        self.migrate(exe)?;
        if let Some(s) = self.contents()? {
            if Self::owned(&s) && s != render(exe)? {
                self.install(exe)?;
            }
        }
        Ok(())
    }
    pub fn remove(&self) -> io::Result<()> {
        let Some(s) = self.contents()? else {
            return Ok(());
        };
        if !Self::owned(&s) {
            return Err(invalid("Desktop entry belongs to another application"));
        }
        self.safe(&self.icon())?;
        fs::remove_file(self.entry())?;
        if fs::read(self.icon()).is_ok_and(|b| b == ICON) {
            fs::remove_file(self.icon())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_replaces_only_owned_legacy_files_and_is_repeatable() {
        let home = tempfile::tempdir().unwrap();
        let entry = DesktopEntry::new(home.path().to_owned()).unwrap();
        let exe = Path::new("/new/app");
        entry.migrate(exe).unwrap();
        assert!(!entry.exists().unwrap());
        let old_entry = entry.entry().with_file_name(format!("{OLD_ID}.desktop"));
        let old_icon = entry.icon().with_file_name(format!("{OLD_ID}.png"));
        entry
            .write(
                &old_entry,
                render(Path::new("/old/app"))
                    .unwrap()
                    .replace(ID, OLD_ID)
                    .as_bytes(),
            )
            .unwrap();
        entry.write(&old_icon, ICON).unwrap();
        entry.migrate(exe).unwrap();
        assert_eq!(
            fs::read_to_string(entry.entry()).unwrap(),
            render(exe).unwrap()
        );
        assert_eq!(fs::read(entry.icon()).unwrap(), ICON);
        assert!(!old_entry.exists());
        assert!(!old_icon.exists());
        entry.migrate(exe).unwrap();
        entry
            .write(&old_entry, format!("{MARKER}\n").as_bytes())
            .unwrap();
        entry.write(&old_icon, b"foreign icon").unwrap();
        entry.migrate(Path::new("/different/app")).unwrap();
        assert_eq!(
            fs::read_to_string(entry.entry()).unwrap(),
            render(exe).unwrap()
        );
        assert_eq!(fs::read(&old_icon).unwrap(), b"foreign icon");
    }

    #[test]
    fn migration_preserves_foreign_old_or_new_entries_and_icons() {
        let home = tempfile::tempdir().unwrap();
        let entry = DesktopEntry::new(home.path().to_owned()).unwrap();
        let old_entry = entry.entry().with_file_name(format!("{OLD_ID}.desktop"));
        let old_icon = entry.icon().with_file_name(format!("{OLD_ID}.png"));
        entry.write(&old_entry, b"foreign").unwrap();
        entry.write(&old_icon, ICON).unwrap();
        entry.migrate(Path::new("/new/app")).unwrap();
        assert!(!entry.exists().unwrap());
        assert_eq!(fs::read(&old_entry).unwrap(), b"foreign");
        assert!(old_icon.exists());
        entry
            .write(&old_entry, format!("{MARKER}\n").as_bytes())
            .unwrap();
        entry.write(&entry.entry(), b"foreign new").unwrap();
        assert!(entry.migrate(Path::new("/new/app")).is_err());
        assert!(old_entry.exists() && old_icon.exists());
        assert_eq!(fs::read(entry.entry()).unwrap(), b"foreign new");
        fs::remove_file(entry.entry()).unwrap();
        entry.write(&entry.icon(), b"foreign icon").unwrap();
        assert!(entry.migrate(Path::new("/new/app")).is_err());
        assert!(old_entry.exists() && old_icon.exists());
        assert_eq!(fs::read(entry.icon()).unwrap(), b"foreign icon");
    }

    #[test]
    fn quotes_spaces_percent_and_shell_characters() {
        let s = render(Path::new("/tmp/My App/$cash`x\"%.AppImage")).unwrap();
        assert!(s.contains("Exec=\"/tmp/My App/\\\\$cash\\\\`x\\\\\"%%.AppImage\"\n"));
        assert!(render(Path::new("/tmp/x\nExec=evil")).is_err());
        assert!(render(Path::new("relative")).is_err());
    }
    #[test]
    fn executable_is_the_appimage_file_and_the_binary_only_outside_an_appimage() {
        let mount = || Ok(PathBuf::from("/tmp/.mount_DualfXYZ/usr/bin/dualfile"));
        assert_eq!(
            executable_from(Some("/home/u/Apps/Dualfile.AppImage".into()), mount).unwrap(),
            Path::new("/home/u/Apps/Dualfile.AppImage")
        );
        let dev = || Ok(PathBuf::from("/w/target/debug/dualfile"));
        assert_eq!(
            executable_from(None, dev).unwrap(),
            Path::new("/w/target/debug/dualfile")
        );
        assert_eq!(
            executable_from(Some("".into()), dev).unwrap(),
            Path::new("/w/target/debug/dualfile")
        );
        assert!(executable_from(Some("relative.AppImage".into()), dev).is_err());
    }
    #[test]
    fn temporary_home_install_move_remove_and_foreign_file() {
        let home = tempfile::tempdir().unwrap();
        let d = DesktopEntry::new(home.path().to_owned()).unwrap();
        assert!(!d.exists().unwrap());
        d.install(Path::new("/tmp/My App.AppImage")).unwrap();
        assert!(d.exists().unwrap());
        assert_eq!(fs::read(d.icon()).unwrap(), ICON);
        d.refresh(Path::new("/tmp/Moved App.AppImage")).unwrap();
        assert!(fs::read_to_string(d.entry()).unwrap().contains("Moved App"));
        d.remove().unwrap();
        assert!(!d.entry().exists() && !d.icon().exists());
        fs::write(d.entry(), "[Desktop Entry]\nName=Other\n").unwrap();
        assert!(d.install(Path::new("/tmp/app")).is_err());
        assert!(d.remove().is_err());
        d.refresh(Path::new("/tmp/app")).unwrap();
        assert_eq!(
            fs::read_to_string(d.entry()).unwrap(),
            "[Desktop Entry]\nName=Other\n"
        );
    }
    #[test]
    fn install_refreshes_an_outdated_own_icon_instead_of_failing() {
        let home = tempfile::tempdir().unwrap();
        let d = DesktopEntry::new(home.path().to_owned()).unwrap();
        d.install(Path::new("/tmp/app")).unwrap();
        // Simulate an icon left by an earlier version with different bytes.
        fs::write(d.icon(), b"old version icon").unwrap();
        // Re-adding the entry must succeed and restore the current icon, not report a foreign icon.
        d.install(Path::new("/tmp/app")).unwrap();
        assert_eq!(fs::read(d.icon()).unwrap(), ICON);
    }
    #[test]
    fn retains_foreign_icon_when_removing_own_entry() {
        let home = tempfile::tempdir().unwrap();
        let d = DesktopEntry::new(home.path().to_owned()).unwrap();
        d.install(Path::new("/tmp/app")).unwrap();
        fs::write(d.icon(), b"different icon").unwrap();
        d.remove().unwrap();
        assert!(!d.entry().exists());
        assert_eq!(fs::read(d.icon()).unwrap(), b"different icon");
    }
    #[cfg(unix)]
    #[test]
    fn refuses_paths_leading_outside_home() {
        let home = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(other.path(), home.path().join(".local")).unwrap();
        let d = DesktopEntry::new(home.path().to_owned()).unwrap();
        assert!(d.install(Path::new("/tmp/app")).is_err());
        assert!(d.remove().is_err());
        assert_eq!(fs::read_dir(other.path()).unwrap().count(), 0);
    }
}
