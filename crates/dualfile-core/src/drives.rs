//! Locally mounted filesystems shown in the quick-access bar.
//!
//! Like a twin-pane file manager's drive bar, we read `/proc/mounts` and present every
//! real mount (the root, extra partitions and btrfs subvolumes, network shares, and
//! auto-mounted removable media under `/run/media` or `/media`) as a place the user can
//! jump to. Kernel pseudo-filesystems (proc, sysfs, cgroup, tmpfs, snap/loop squashfs,
//! container overlays, …) are filtered out. Read-only; nothing is mounted or changed here.

use serde::Serialize;
use std::path::Path;

/// Filesystem types that are kernel/virtual mounts, not browsable storage. Dropped from the bar.
const PSEUDO_FSTYPES: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "ramfs",
    "cgroup",
    "cgroup2",
    "pstore",
    "bpf",
    "mqueue",
    "hugetlbfs",
    "debugfs",
    "tracefs",
    "securityfs",
    "selinuxfs",
    "configfs",
    "fusectl",
    "efivarfs",
    "autofs",
    "rpc_pipefs",
    "nsfs",
    "binfmt_misc",
    "squashfs", // snap/loop images
    "overlay",  // container layers
    "fuse.gvfsd-fuse",
    "fuse.portal",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drive {
    /// Display name (the last path component of the mount point; "/" for the root).
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

/// Parse the mount table text (so it is testable) into browsable drives, de-duplicated and bounded.
pub fn parse_mounts(table: &str) -> Vec<Drive> {
    let mut out: Vec<Drive> = Vec::new();
    for line in table.lines() {
        let mut fields = line.split_whitespace();
        let _device = fields.next();
        let Some(point_raw) = fields.next() else {
            continue;
        };
        let Some(fstype) = fields.next() else {
            continue;
        };
        if PSEUDO_FSTYPES.contains(&fstype) {
            continue;
        }
        let point = unescape(point_raw);
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

/// Filesystems mounted on this machine. Empty when `/proc/mounts` cannot be read.
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
    fn keeps_real_mounts_drops_pseudo_and_decodes_the_label() {
        let table = "\
/dev/sda2 / btrfs rw,subvol=/@ 0 0
/dev/sda2 /home btrfs rw,subvol=/@home 0 0
/dev/sdb1 /run/media/tester/USB\\040Stick vfat rw 0 0
proc /proc proc rw 0 0
sysfs /sys sysfs rw 0 0
tmpfs /run tmpfs rw 0 0
/dev/loop0 /snap/core/1 squashfs ro 0 0
overlay /var/lib/docker/overlay2/x overlay rw 0 0
cgroup2 /sys/fs/cgroup cgroup2 rw 0 0
";
        let drives = parse_mounts(table);
        assert_eq!(
            drives,
            vec![
                Drive {
                    name: "/".into(),
                    path: "/".into()
                },
                Drive {
                    name: "home".into(),
                    path: "/home".into()
                },
                Drive {
                    name: "USB Stick".into(),
                    path: "/run/media/tester/USB Stick".into()
                },
            ]
        );
    }

    #[test]
    fn duplicate_mount_points_appear_once() {
        let table = "a /mnt/x ext4 rw 0 0\nb /mnt/x ext4 rw 0 0\n";
        assert_eq!(parse_mounts(table).len(), 1);
    }
}
