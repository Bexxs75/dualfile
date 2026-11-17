//! Display-only fstab parsing. Raw sources and options never survive parsing.
use crate::drives::{unescape, Drive};
use serde::Serialize;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FstabInfo {
    pub user_mountable: bool,
    pub permission: Option<String>,
    pub automount: bool,
    pub noauto: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FstabEntry {
    pub drive: Drive,
    pub hidden: bool,
}
pub fn parse_fstab(text: &str) -> Vec<FstabEntry> {
    let mut out = Vec::new();
    let bounded = String::from_utf8_lossy(&text.as_bytes()[..text.len().min(256 * 1024)]);
    for line in bounded.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() < 4 || f[0].starts_with('#') {
            continue;
        }
        let point = unescape(f[1]);
        let options: Vec<_> = f[3].split(',').collect();
        if !point.starts_with('/')
            || point.chars().any(char::is_control)
            || point == "/"
            || point == "/boot"
            || point.starts_with("/boot/")
            || point == "/efi"
            || ["swap", "none", "proc", "tmpfs"].contains(&f[2])
            || options.iter().any(|v| ["bind", "rbind"].contains(v))
        {
            continue;
        }
        let mut drive = Drive::new(point, f[2], f[0], f[3]);
        drive.source = "fstab".into();
        drive.mounted = false;
        drive.fstab = Some(FstabInfo {
            permission: ["users", "user", "owner"]
                .into_iter()
                .find(|p| options.contains(p))
                .map(str::to_owned),
            user_mountable: options
                .iter()
                .any(|v| ["user", "users", "owner"].contains(v)),
            automount: options.contains(&"x-systemd.automount"),
            noauto: options.contains(&"noauto"),
        });
        if let Some(label) = options.iter().find_map(|v| v.strip_prefix("x-gvfs-name=")) {
            let label = unescape(label);
            if !label.chars().any(char::is_control) {
                drive.name = label;
            }
        }
        out.push(FstabEntry {
            drive,
            hidden: options.contains(&"x-gvfs-hide"),
        });
        if out.len() == 256 {
            break;
        }
    }
    out
}
/// Display and action authorization share this conservative permission matrix.
/// The helper remains authoritative; account names never leave the caller.
pub fn permitted(
    permission: Option<&str>,
    unmount: bool,
    device_owned: bool,
    mounted_by_user: bool,
) -> bool {
    match permission {
        Some("users") => true,
        Some("user") => !unmount || mounted_by_user,
        Some("owner") => device_owned && (!unmount || mounted_by_user),
        _ => false,
    }
}
pub fn merge(mut drives: Vec<Drive>, entries: Vec<FstabEntry>) -> Vec<Drive> {
    for entry in entries {
        if entry.hidden {
            drives.retain(|d| d.path != entry.drive.path);
            continue;
        }
        if let Some(d) = drives.iter_mut().find(|d| d.path == entry.drive.path) {
            d.fstab = entry.drive.fstab;
            d.name = entry.drive.name;
        } else {
            drives.push(entry.drive);
        }
    }
    drives
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_matrix_checks_device_owner_and_mounting_user() {
        assert!(permitted(Some("users"), true, false, false));
        assert!(permitted(Some("user"), false, false, false));
        assert!(!permitted(Some("user"), true, false, false));
        assert!(permitted(Some("user"), true, false, true));
        assert!(!permitted(Some("owner"), false, false, true));
        assert!(permitted(Some("owner"), false, true, false));
        assert!(!permitted(Some("owner"), true, true, false));
        assert!(permitted(Some("owner"), true, true, true));
    }
    #[test]
    fn safe_bounded_display_and_merge() {
        let text = "host:/export /mnt/nfs nfs noauto,user 0 0\n//alice:secret@host/share /mnt/My\\040Disk cifs credentials=/root/secret,username=alice,password=secret,x-gvfs-name=Share 0 0\nUUID=abc /data ext4 x-systemd.automount,ro 0 0\n/dev/a /hide ext4 x-gvfs-hide 0 0\n/dev/a /bind none bind 0 0\n# comment\ngarbage";
        let entries = parse_fstab(text);
        assert_eq!(entries.len(), 4);
        assert!(entries[0].drive.fstab.as_ref().unwrap().user_mountable);
        assert_eq!(entries[1].drive.path, "/mnt/My Disk");
        assert!(entries[2].drive.fstab.as_ref().unwrap().automount);
        let public = format!("{:?} {}", entries, serde_json::to_string(&entries).unwrap());
        for secret in ["alice", "secret", "credentials", "username"] {
            assert!(!public.contains(secret));
        }
        let merged = merge(
            vec![Drive::new("/data".into(), "ext4", "/dev/a", "rw")],
            entries,
        );
        assert_eq!(merged.len(), 3);
        assert!(merged[0].mounted);
        assert_eq!(parse_fstab(&"a /mnt/x ext4 user\n".repeat(300)).len(), 256);
    }
}
