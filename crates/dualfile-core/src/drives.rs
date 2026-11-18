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
#[serde(rename_all = "camelCase")]
pub struct Drive {
    /// Display name (the last path component of the mount point; "/" for the root).
    pub name: String,
    /// Absolute local path of the mount point.
    pub path: String,
    pub device: String,
    pub fstype: String,
    pub kind: String,
    pub read_only: bool,
    pub source: String,
    pub mounted: bool,
    pub fstab: Option<crate::fstab::FstabInfo>,
    pub space: Option<crate::vfs::SpaceFree>,
    pub device_actions: Option<crate::devices::Actions>,
}

/// `/proc/mounts` escapes space, tab, newline and backslash as octal; undo that for the path.
pub fn unescape(field: &str) -> String {
    let mut out = Vec::new();
    let bytes = field.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let code = (bytes[i + 1] - b'0') as u16 * 64
                + (bytes[i + 2] - b'0') as u16 * 8
                + (bytes[i + 3] - b'0') as u16;
            if code <= 255 {
                out.push(code as u8);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn network_type(fs: &str) -> bool {
    fs.starts_with("nfs")
        || [
            "cifs",
            "smb3",
            "fuse.sshfs",
            "fuse.rclone",
            "davfs",
            "davfs2",
            "9p",
            "ceph",
            "glusterfs",
        ]
        .contains(&fs)
}
pub fn local_space_type(fs: &str) -> bool {
    [
        "ext2", "ext3", "ext4", "btrfs", "xfs", "vfat", "exfat", "ntfs3", "f2fs", "zfs", "tmpfs",
        "bcachefs", "jfs", "overlay",
    ]
    .contains(&fs)
}
impl Drive {
    pub fn new(path: String, fstype: &str, device: &str, options: &str) -> Self {
        let kind = if path == "/" {
            "root"
        } else if network_type(fstype) {
            "network"
        } else if path.starts_with("/run/media/") || path.starts_with("/media/") {
            "removable"
        } else if fstype == "autofs" {
            "automount"
        } else if fstype.starts_with("fuse") {
            "fuse"
        } else {
            "partition"
        };
        Self {
            name: Path::new(&path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&path)
                .to_owned(),
            path,
            fstype: fstype.into(),
            kind: kind.into(),
            // Network sources can embed credentials. They never enter the public model.
            device: if device.starts_with("/dev/")
                || device.starts_with("UUID=")
                || device.starts_with("LABEL=")
            {
                device.into()
            } else {
                String::new()
            },
            read_only: options.split(',').any(|v| v == "ro"),
            source: "mount".into(),
            mounted: true,
            fstab: None,
            space: None,
            device_actions: None,
        }
    }
}

/// Filesystems whose mount point may block a syscall for a long time: network mounts,
/// automount triggers and FUSE bridges (a dead server or a hung daemon never answers).
fn may_block(fs: &str) -> bool {
    network_type(fs) || fs == "autofs" || fs == "fuse" || fs.starts_with("fuse.")
}

/// What a probe of a mount point told us about the current user's access to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountAccess {
    Granted,
    /// The kernel refused read or search permission (`EACCES`).
    Denied,
    /// Not probed, timed out or failed for another reason. Never a reason to hide.
    Unknown,
}

/// Only mounted targets on filesystems that cannot hang are probed. Unmounted devices and
/// fstab entries have no access rights yet (those only exist after mounting), and a probe
/// of a network, automount or FUSE mount could block on a dead server or trigger a mount.
/// The root is always shown: it is the anchor of the whole list.
pub fn access_probe_allowed(drive: &Drive) -> bool {
    drive.mounted && drive.path != "/" && !may_block(&drive.fstype)
}

/// `access(2)` for reading and entering a directory, with the real user ID. Only a clear
/// "permission denied" counts as denied; a vanished mount point or any other error is unknown.
pub fn probe_access(path: &str) -> MountAccess {
    use rustix::fs::Access;
    match rustix::fs::access(path, Access::READ_OK | Access::EXEC_OK) {
        Ok(()) => MountAccess::Granted,
        Err(rustix::io::Errno::ACCESS) => MountAccess::Denied,
        Err(_) => MountAccess::Unknown,
    }
}

/// Drop mounted drives the user may not read or enter (for example `/root` on its own
/// btrfs subvolume): clicking such an entry could only ever report "permission denied".
/// `probe` is called only for drives that [`access_probe_allowed`]; the caller bounds it.
pub fn drop_inaccessible(
    drives: Vec<Drive>,
    mut probe: impl FnMut(&Drive) -> MountAccess,
) -> Vec<Drive> {
    drives
        .into_iter()
        .filter(|drive| !access_probe_allowed(drive) || probe(drive) != MountAccess::Denied)
        .collect()
}

/// Last successful kernel mount table. Classification is advisory: unavailable metadata
/// falls back to ordinary navigation, while a dirty cache keeps its last known value.
#[derive(Default)]
pub struct MountTableCache {
    state: std::sync::Mutex<MountTableState>,
    reading: std::sync::Mutex<()>,
}
#[derive(Default)]
struct MountTableState {
    table: Option<std::sync::Arc<str>>,
    generation: u64,
    loaded: Option<u64>,
}
impl MountTableCache {
    pub fn invalidate(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = state.generation.wrapping_add(1);
    }
    pub fn get_or_read(
        &self,
        read: impl FnOnce() -> Option<String>,
    ) -> Option<std::sync::Arc<str>> {
        let (table, generation, fresh) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.table.clone(),
                state.generation,
                state.loaded == Some(state.generation),
            )
        };
        if fresh {
            return table;
        }
        let Ok(_reading) = self.reading.try_lock() else {
            return table;
        };
        let result = read().filter(|table| !table.is_empty());
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(table) = result {
            state.table = Some(table.into());
            state.loaded = Some(generation);
        }
        state.table.clone()
    }
}

/// Only local paths on potentially blocking mounts use the navigation deadline.
/// Inspect the raw table so autofs and desktop FUSE bridges are included as well.
pub fn navigation_mount(path: &str, table: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    table
        .lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let point = unescape(left.split_whitespace().nth(4)?);
            let fs = right.split_whitespace().next()?;
            let inside = point == "/"
                || path == point
                || path
                    .strip_prefix(&point)
                    .is_some_and(|s| s.starts_with('/'));
            inside.then_some((point, may_block(fs)))
        })
        .max_by_key(|(point, _)| point.len())
        .and_then(|(point, bounded)| bounded.then_some(point))
}

/// Keep the topmost mount ID at a mount point; autofs is admitted only by fstab merge.
pub fn parse_mountinfo(table: &str, self_mount: Option<&str>) -> Vec<Drive> {
    let mut points = std::collections::BTreeMap::new();
    for line in table.lines().take(16384) {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let l: Vec<_> = left.split_whitespace().collect();
        let r: Vec<_> = right.split_whitespace().collect();
        if l.len() < 6 || r.len() < 3 {
            continue;
        }
        let Ok(id) = l[0].parse::<u64>() else {
            continue;
        };
        let path = unescape(l[4]);
        if !path.starts_with('/')
            || path.chars().any(char::is_control)
            || PSEUDO_FSTYPES.contains(&r[0])
            || is_system_mount(&path)
            || is_fuse_self_mount(&path)
            || self_mount == Some(&path)
        {
            continue;
        }
        if points.get(&path).is_none_or(|(old, _)| id > *old) {
            points.insert(path.clone(), (id, Drive::new(path, r[0], r[1], l[5])));
        }
    }
    points.into_values().take(256).map(|(_, d)| d).collect()
}

/// Firmware/boot mounts that aren't useful file-manager destinations (EFI system partition, /boot).
fn is_system_mount(point: &str) -> bool {
    point == "/boot" || point == "/efi" || point.starts_with("/boot/")
}

/// FUSE self-mounts that aren't real drives: any running AppImage's runtime mount
/// (named `.mount_XXXXXX` by convention, our own and others alike) and KDE's
/// KIO-FUSE bridge (`kio-fuse-XXXXXX`). Matched on the mount-point name so it is
/// independent of the fuse fstype, which carries the app name.
fn is_fuse_self_mount(point: &str) -> bool {
    Path::new(point)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(".mount_") || n.starts_with("kio-fuse"))
}

/// Parse the mount table text (so it is testable) into browsable drives, de-duplicated and bounded.
/// `self_mount` is the running AppImage's own mount point (`$APPDIR`), dropped so it is not shown as a drive.
pub fn parse_mounts(table: &str, self_mount: Option<&str>) -> Vec<Drive> {
    let mut out: Vec<Drive> = Vec::new();
    for line in table.lines() {
        let mut fields = line.split_whitespace();
        let device = fields.next().unwrap_or("");
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
        if !point.starts_with('/')
            || point.chars().any(char::is_control)
            || self_mount == Some(point.as_str())
        {
            continue;
        }
        if is_system_mount(&point) {
            continue;
        }
        if is_fuse_self_mount(&point) {
            continue;
        }
        if out.iter().any(|d| d.path == point) {
            continue;
        }
        out.push(Drive::new(
            point,
            fstype,
            device,
            fields.next().unwrap_or(""),
        ));
        if out.len() >= 64 {
            break;
        }
    }
    out
}

/// Filesystems mounted on this machine. Empty when `/proc/mounts` cannot be read.
pub fn local_drives() -> Vec<Drive> {
    match std::fs::read_to_string("/proc/mounts") {
        Ok(table) => parse_mounts(&table, std::env::var("APPDIR").ok().as_deref()),
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
        let drives = parse_mounts(table, None);
        assert_eq!(
            drives,
            vec![
                Drive::new("/".into(), "btrfs", "/dev/sda2", "rw"),
                Drive::new("/home".into(), "btrfs", "/dev/sda2", "rw"),
                Drive::new(
                    "/run/media/tester/USB Stick".into(),
                    "vfat",
                    "/dev/sdb1",
                    "rw"
                ),
            ]
        );
    }

    #[test]
    fn duplicate_mount_points_appear_once() {
        let table = "a /mnt/x ext4 rw 0 0\nb /mnt/x ext4 rw 0 0\n";
        assert_eq!(parse_mounts(table, None).len(), 1);
    }

    #[test]
    fn drops_boot_and_efi_system_mounts() {
        let table = "\
/dev/sda2 / btrfs rw 0 0
/dev/sda1 /boot/efi vfat rw 0 0
/dev/sda3 /boot ext4 rw 0 0
/dev/sdb1 /run/media/tester/Stick vfat rw 0 0
";
        let drives = parse_mounts(table, None);
        assert_eq!(
            drives.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            vec!["/", "/run/media/tester/Stick"]
        );
    }

    #[test]
    fn drops_the_running_appimage_self_mount() {
        let table = "\
/dev/sda2 / btrfs rw 0 0
Dualfile.AppImage /tmp/.mount_DualfiAbc123 fuse.Dualfile.AppImage ro 0 0
";
        let drives = parse_mounts(table, Some("/tmp/.mount_DualfiAbc123"));
        assert_eq!(
            drives.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            vec!["/"]
        );
    }

    #[test]
    fn drops_other_appimage_self_mounts_and_kio_fuse_but_keeps_real_fuse_drives() {
        // Even without knowing our own $APPDIR (self_mount None), no running AppImage's
        // mount nor the KIO-FUSE bridge should appear — but a real fuse drive (sshfs) stays.
        let table = "\
/dev/sda2 / btrfs rw 0 0
artcral.AppImage /tmp/.mount_artcralCDHCI fuse.artcral.AppImage ro 0 0
raider.AppImage /tmp/.mount_raiderWgiXCQ fuse.raider.AppImage ro 0 0
Dualfile.AppImage /tmp/.mount_DualfiCAPeMP fuse.Dualfile.AppImage ro 0 0
kio-fuse /run/user/1000/kio-fuse-dSRHXL fuse.kio-fuse rw 0 0
user@host:/ /home/tester/sshfs fuse.sshfs rw 0 0
";
        let drives = parse_mounts(table, None);
        assert_eq!(
            drives.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            vec!["/", "/home/tester/sshfs"]
        );
    }
}

#[cfg(test)]
mod mountinfo_tests {
    use super::*;
    #[test]
    fn topmost_mount_unicode_escapes_and_sensitive_options() {
        let text = "1 0 8:1 / / rw - ext4 /dev/a rw\n2 1 8:2 / /mnt/Übung\\040Disk ro - ext4 /dev/b ro\n3 1 8:2 /bind /alias rw - ext4 /dev/b rw\n4 1 0:1 / /net rw - autofs systemd-1 rw\n5 1 0:2 / /net rw - cifs //alice:secret@host/share rw,credentials=/root/secret,username=alice\n6 1 0:3 / /ssh rw - fuse.sshfs alice@host:/ rw\n7 1 0:4 / /run/user/1000/gvfs rw - fuse.gvfsd-fuse gvfs rw\n8 1 0:5 / /tmp/.mount_Test rw - fuse.Test Test rw";
        let mounts = parse_mountinfo(text, None);
        assert_eq!(mounts.len(), 5);
        assert!(mounts
            .iter()
            .any(|d| d.path == "/mnt/Übung Disk" && d.read_only));
        assert_eq!(
            mounts.iter().find(|d| d.path == "/net").unwrap().fstype,
            "cifs"
        );
        let public = format!("{:?} {}", mounts, serde_json::to_string(&mounts).unwrap());
        for secret in ["alice", "secret", "credentials"] {
            assert!(!public.contains(secret));
        }
        let duplicate = parse_mountinfo(
            "2 0 8:1 / /mnt rw - ext4 /dev/a rw\n3 0 8:2 / /mnt ro - xfs /dev/b ro",
            None,
        );
        assert_eq!(duplicate.len(), 1);
        assert_eq!(duplicate[0].device, "/dev/b");
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;
    #[test]
    fn classification_cache_survives_read_failure_and_refreshes_after_invalidation() {
        let cache = MountTableCache::default();
        // No cache and failed classification: the command chooses ordinary navigation.
        assert!(cache.get_or_read(|| None).is_none());
        assert!(cache.get_or_read(|| Some(String::new())).is_none());
        let nfs = "2 1 0:1 / /net rw - nfs host:/ rw";
        let table = cache.get_or_read(|| Some(nfs.into())).unwrap();
        assert_eq!(
            navigation_mount("/net/folder", &table).as_deref(),
            Some("/net")
        );
        assert_eq!(
            &*cache
                .get_or_read(|| panic!("cached listings must not read proc"))
                .unwrap(),
            nfs
        );
        cache.invalidate();
        assert_eq!(&*cache.get_or_read(|| None).unwrap(), nfs);
        let local = "2 1 8:1 / /net rw - ext4 /dev/a rw";
        let table = cache.get_or_read(|| Some(local.into())).unwrap();
        assert_eq!(navigation_mount("/net/folder", &table), None);
    }
    #[test]
    fn notification_during_read_keeps_cache_dirty() {
        let cache = MountTableCache::default();
        cache.get_or_read(|| {
            cache.invalidate();
            Some("old".into())
        });
        assert_eq!(&*cache.get_or_read(|| Some("new".into())).unwrap(), "new");
    }
    #[test]
    fn remote_vfs_archives_plugins_and_home_are_not_bounded() {
        let table = "1 0 8:1 / / rw - ext4 /dev/a rw\n2 1 0:1 / /net rw - nfs host:/ rw\n3 1 0:2 / /auto rw - autofs systemd rw\n4 1 0:3 / /gvfs rw - fuse.gvfsd-fuse gvfs rw";
        for path in [
            "sftp:id!/",
            "ftp:id!/",
            "webdav:id!/",
            "archive:/net/test.zip!/",
            "plugin:id!/",
            "/home/me",
            "/network",
        ] {
            assert_eq!(navigation_mount(path, table), None, "{path}");
        }
        for (path, root) in [
            ("/net/folder", "/net"),
            ("/auto", "/auto"),
            ("/gvfs/share", "/gvfs"),
        ] {
            assert_eq!(navigation_mount(path, table).as_deref(), Some(root));
        }
    }
}

#[cfg(test)]
mod access_tests {
    use super::*;
    fn drive(path: &str, fstype: &str, mounted: bool) -> Drive {
        let mut drive = Drive::new(path.into(), fstype, "/dev/a", "rw");
        drive.mounted = mounted;
        drive
    }
    #[test]
    fn only_mounted_local_non_root_targets_are_probed() {
        assert!(access_probe_allowed(&drive("/root", "btrfs", true)));
        assert!(access_probe_allowed(&drive("/mnt/win", "fuseblk", true)));
        // The root stays; unmounted devices have no access rights yet.
        assert!(!access_probe_allowed(&drive("/", "btrfs", true)));
        assert!(!access_probe_allowed(&drive("/mnt/usb", "vfat", false)));
        // Anything that can hang or trigger a mount is never touched.
        for fs in [
            "nfs",
            "nfs4",
            "cifs",
            "smb3",
            "fuse.sshfs",
            "fuse.rclone",
            "davfs",
            "9p",
            "ceph",
            "autofs",
            "fuse",
        ] {
            assert!(!access_probe_allowed(&drive("/mnt/net", fs, true)), "{fs}");
        }
    }
    #[test]
    fn denied_mounts_are_dropped_and_unknown_or_unprobed_ones_stay() {
        let drives = vec![
            drive("/", "btrfs", true),
            drive("/home", "btrfs", true),
            drive("/root", "btrfs", true),
            drive("/mnt/slow", "ext4", true),
            drive("/mnt/dead-nfs", "nfs4", true),
            drive("/mnt/usb", "vfat", false),
        ];
        let mut probed = Vec::new();
        let kept = drop_inaccessible(drives, |d| {
            probed.push(d.path.clone());
            match d.path.as_str() {
                "/home" => MountAccess::Granted,
                "/mnt/slow" => MountAccess::Unknown,
                _ => MountAccess::Denied,
            }
        });
        // "/", the NFS mount and the unmounted stick are kept without being probed.
        assert_eq!(probed, ["/home", "/root", "/mnt/slow"]);
        assert_eq!(
            kept.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["/", "/home", "/mnt/slow", "/mnt/dead-nfs", "/mnt/usb"]
        );
    }
    #[test]
    fn probe_reports_denied_only_for_missing_permission() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let open = tmp.path().join("open");
        let locked = tmp.path().join("locked");
        std::fs::create_dir(&open).unwrap();
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(probe_access(open.to_str().unwrap()), MountAccess::Granted);
        assert_eq!(
            probe_access(tmp.path().join("missing").to_str().unwrap()),
            MountAccess::Unknown
        );
        // Root passes every permission check, so the denial is only observable as a user.
        if !rustix::process::getuid().is_root() {
            assert_eq!(probe_access(locked.to_str().unwrap()), MountAccess::Denied);
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
