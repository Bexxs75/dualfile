//! User-local desktop integration. Never replace or remove an unrelated entry.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

const ID: &str = "de.thebexxs.dualfile";
const MARKER: &str = "X-Dualfile-Managed=true";
const ICON: &[u8] = include_bytes!("../icons/128x128.png");

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

pub fn executable() -> io::Result<PathBuf> {
    let path = std::env::var_os("APPIMAGE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)?;
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
pub fn render(exe: &Path) -> io::Result<String> {
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
    Ok(format!("[Desktop Entry]\nType=Application\nName=Dualfile\nComment=Two-pane file manager\nComment[de]=Zwei-Fenster-Dateimanager\nExec=\"{quoted}\"\nIcon={ID}\nTerminal=false\nCategories=Utility;FileManager;\nStartupWMClass={ID}\n{MARKER}\n"))
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
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
    pub fn install(&self, exe: &Path) -> io::Result<()> {
        let text = render(exe)?;
        if self.contents()?.is_some_and(|s| !Self::owned(&s)) {
            return Err(invalid("Desktop entry belongs to another application"));
        }
        self.safe(&self.icon())?;
        match fs::read(self.icon()) {
            Ok(bytes) if bytes != ICON => {
                return Err(invalid("Icon belongs to another application"))
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.write(&self.icon(), ICON)?,
            Err(e) => return Err(e),
        }
        self.write(&self.entry(), text.as_bytes())
    }
    pub fn refresh(&self, exe: &Path) -> io::Result<()> {
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
    fn quotes_spaces_percent_and_shell_characters() {
        let s = render(Path::new("/tmp/My App/$cash`x\"%.AppImage")).unwrap();
        assert!(s.contains("Exec=\"/tmp/My App/\\\\$cash\\\\`x\\\\\"%%.AppImage\"\n"));
        assert!(render(Path::new("/tmp/x\nExec=evil")).is_err());
        assert!(render(Path::new("relative")).is_err());
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
