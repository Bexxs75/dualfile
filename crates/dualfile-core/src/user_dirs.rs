//! XDG user directories without shell evaluation. Existence checks belong to the places pool.
use serde::Serialize;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UserDir {
    pub kind: String,
    pub path: String,
}
pub fn parse(text: &str, home: &str) -> Vec<UserDir> {
    let mut out = Vec::new();
    for (key, kind) in [
        ("DESKTOP", "desktop"),
        ("DOCUMENTS", "documents"),
        ("DOWNLOAD", "downloads"),
        ("MUSIC", "music"),
        ("PICTURES", "pictures"),
        ("VIDEOS", "videos"),
    ] {
        let prefix = format!("XDG_{key}_DIR=");
        if let Some(value) = text
            .lines()
            .find_map(|line| line.trim().strip_prefix(&prefix))
        {
            if let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                let path = value
                    .strip_prefix("$HOME/")
                    .map(|v| format!("{}/{v}", home.trim_end_matches('/')))
                    .unwrap_or_else(|| value.to_owned());
                if path.starts_with('/')
                    && path != home
                    && !path.contains(['$', '`', '\\'])
                    && !path.chars().any(char::is_control)
                    && !path.split('/').any(|p| p == "..")
                {
                    out.push(UserDir {
                        kind: kind.into(),
                        path,
                    });
                }
            }
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_home_expansion_and_fixed_kinds() {
        let dirs = parse("XDG_DESKTOP_DIR=\"$HOME\"\nXDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\nXDG_MUSIC_DIR=\"/media/Music\"\nXDG_VIDEOS_DIR=\"$(evil)\"", "/home/u");
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0].path, "/home/u/Downloads");
    }
}
