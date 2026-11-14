//! Locally mounted removable media (USB sticks, external disks, cards).
//!
//! udisks2 and most desktops auto-mount removable volumes under `/run/media/<user>/<label>`
//! or `/media/<user>/<label>`. We read `/proc/mounts`, keep the mount points below those
//! directories and present each as a drive the user can jump to without hunting through
//! the folder tree. Read-only; nothing is mounted or changed here.

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drive {
    /// Display name (the volume label, i.e. the last path component of the mount point).
    pub name: String,
    /// Absolute local path of the mount point.
    pub path: String,
}

/// `/proc/mounts` escapes space, tab, newline and backslash as octal; undo that for the path.
fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let bytes = field.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let oct = &field[i + 1..i + 4];
            if let Ok(code) = u8::from_str_radix(oct, 8) {
                out.push(code as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn is_removable_mount(point: &str) -> bool {
    for base in ["/run/media/", "/media/"] {
        if let Some(rest) = point.strip_prefix(base) {
            // Require <user>/<label>: a user directory and a volume below it, nothing shallower.
            let depth = rest
                .trim_end_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .count();
            if depth >= 2 {
                return true;
            }
        }
    }
    false
}

/// Parse the mount table text (so it is testable) into removable drives, de-duplicated and bounded.
pub fn parse_mounts(table: &str) -> Vec<Drive> {
    let mut out: Vec<Drive> = Vec::new();
    for line in table.lines() {
        let mut fields = line.split_whitespace();
        let _device = fields.next();
        let Some(point_raw) = fields.next() else {
            continue;
        };
        let point = unescape(point_raw);
        if !is_removable_mount(&point) {
            continue;
        }
        let name = Path::new(&point)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&point)
            .to_string();
        if out.iter().any(|d| d.path == point) {
            continue;
        }
        out.push(Drive { name, path: point });
        if out.len() >= 64 {
            break;
        }
    }
    out
}

/// Removable media mounted on this machine. Empty when `/proc/mounts` cannot be read.
pub fn local_drives() -> Vec<Drive> {
    match std::fs::read_to_string("/proc/mounts") {
        Ok(table) => parse_mounts(&table),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_removable_mounts_and_decodes_the_label() {
        let table = "\
/dev/sda1 / ext4 rw 0 0
/dev/sdb1 /run/media/tester/USB\\040Stick vfat rw 0 0
/dev/sdc1 /media/tester/Backup\\040Disk ext4 rw 0 0
tmpfs /run/media tmpfs rw 0 0
/dev/sdd1 /mnt/manual ext4 rw 0 0
";
        let drives = parse_mounts(table);
        assert_eq!(
            drives,
            vec![
                Drive {
                    name: "USB Stick".into(),
                    path: "/run/media/tester/USB Stick".into()
                },
                Drive {
                    name: "Backup Disk".into(),
                    path: "/media/tester/Backup Disk".into()
                },
            ]
        );
    }

    #[test]
    fn duplicate_mount_points_appear_once() {
        let table = "a /run/media/u/X ext4 rw 0 0\nb /run/media/u/X ext4 rw 0 0\n";
        assert_eq!(parse_mounts(table).len(), 1);
    }
}
