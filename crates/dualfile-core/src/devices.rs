//! Bounded lsblk parsing. Keep UUIDs internal; only UI labels and opaque action keys leave discovery.
use crate::drives::{unescape, Drive};
use serde::Serialize;
use serde_json::Value;

pub const COLUMNS: &str = "PATH,LABEL,FSTYPE,MOUNTPOINTS,RM,HOTPLUG,TRAN,SIZE,TYPE,RO,UUID,MAJ:MIN";
pub const CAP: usize = 1024 * 1024;

// Deliberately no Debug/Serialize: identifiers are input to opaque keys, not diagnostics.
#[derive(Clone)]
pub struct Device {
    pub path: String,
    pub whole: String,
    pub parent: Option<String>,
    pub whole_complete: bool,
    /// Whole disk with partitions: a hybrid ISO repeats its partition's filesystem here.
    pub partitioned: bool,
    pub number: String,
    pub label: String,
    pub uuid: String,
    pub fstype: String,
    pub kind: String,
    pub removable: bool,
    pub read_only: bool,
    pub mountpoints: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Actions {
    pub key: String,
    pub removable: bool,
    pub encrypted: bool,
    pub optical: bool,
}
impl Device {
    pub fn key(&self) -> String {
        use sha2::{Digest, Sha256};
        // Stable across application restarts so hidden device rows stay hidden.
        let identity = format!("{}:{}:{}:{}", self.path, self.whole, self.number, self.uuid);
        Sha256::digest(identity.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn encrypted(&self) -> bool {
        self.fstype == "crypto_LUKS"
    }
    pub fn browsable(&self) -> bool {
        ["disk", "part", "rom"].contains(&self.kind.as_str())
            && !self.partitioned
            && !self.fstype.is_empty()
            && !["swap", "LVM2_member", "linux_raid_member"].contains(&self.fstype.as_str())
    }
    pub fn matches_source(&self, source: &str) -> bool {
        unescape(source) == self.path
            || source
                .strip_prefix("UUID=")
                .or_else(|| source.strip_prefix("/dev/disk/by-uuid/"))
                .is_some_and(|s| !self.uuid.is_empty() && unescape(s) == self.uuid)
            || source
                .strip_prefix("LABEL=")
                .or_else(|| source.strip_prefix("/dev/disk/by-label/"))
                .is_some_and(|s| !self.label.is_empty() && unescape(s) == self.label)
    }
}
pub fn valid_path(path: &str) -> bool {
    path.starts_with("/dev/")
        && path.len() <= 4096
        && !path.chars().any(char::is_control)
        && path.split('/').all(|p| p != ".." && p != ".")
}
fn flag(v: &Value) -> bool {
    v == true || v == 1 || v == "1"
}
fn string(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}
pub fn parse_lsblk(json: &[u8]) -> Result<Vec<Device>, &'static str> {
    if json.len() > CAP {
        return Err("invalid device listing");
    }
    let value: Value = serde_json::from_slice(json).map_err(|_| "invalid device listing")?;
    let roots = value["blockdevices"]
        .as_array()
        .ok_or("invalid device listing")?;
    // Count every input row, including malformed rows and their omitted descendants.
    fn budget(nodes: &[Value], depth: usize, count: &mut usize) -> Result<(), &'static str> {
        if depth > 16 {
            return Err("invalid device listing");
        }
        for node in nodes {
            *count += 1;
            if *count > 512 {
                return Err("invalid device listing");
            }
            if let Some(children) = node["children"].as_array() {
                budget(children, depth + 1, count)?;
            }
        }
        Ok(())
    }
    budget(roots, 0, &mut 0)?;
    fn walk(nodes: &[Value], parent: Option<(&str, bool, &str)>, out: &mut Vec<Device>) -> bool {
        let mut complete = true;
        for node in nodes {
            let path = string(&node["path"]);
            if !valid_path(&path) {
                // An unknown parent also makes its descendants unsafe to authorize.
                complete = false;
                continue;
            }
            let number = string(&node["maj:min"]);
            if !number.split_once(':').is_some_and(|(a, b)| {
                !a.is_empty()
                    && !b.is_empty()
                    && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit())
            }) {
                complete = false;
                continue;
            }
            let removable = flag(&node["rm"])
                || (flag(&node["hotplug"])
                    && ["usb", "mmc", "ieee1394"]
                        .contains(&node["tran"].as_str().unwrap_or_default()))
                || parent.is_some_and(|(_, r, _)| r);
            let whole = parent.map_or(path.as_str(), |(p, _, _)| p).to_owned();
            let label = string(&node["label"]);
            let mountpoints = node["mountpoints"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|s| s.starts_with('/') && !s.chars().any(char::is_control))
                .map(str::to_owned)
                .collect();
            out.push(Device {
                path: path.clone(),
                parent: parent.map(|(_, _, p)| p.to_owned()),
                whole_complete: true,
                partitioned: node["children"]
                    .as_array()
                    .is_some_and(|c| c.iter().any(|c| c["type"] == "part")),
                whole: whole.clone(),
                number,
                label: if label.chars().any(char::is_control) {
                    String::new()
                } else {
                    label
                },
                uuid: string(&node["uuid"]),
                fstype: string(&node["fstype"]),
                kind: string(&node["type"]),
                removable,
                read_only: flag(&node["ro"]),
                mountpoints,
            });
            if let Some(children) = node["children"].as_array() {
                complete &= walk(children, Some((&whole, removable, &path)), out);
            }
        }
        complete
    }
    let mut out = Vec::new();
    for root in roots {
        let start = out.len();
        if !walk(std::slice::from_ref(root), None, &mut out) {
            // Keep usable rows, but do not power off a drive with unknown members.
            for device in &mut out[start..] {
                device.whole_complete = false;
            }
        }
    }
    Ok(out)
}
/// The kernel device number includes bind aliases and mounts omitted from the visible sidebar.
pub fn mountpoints(table: &str, devices: &[&Device]) -> Vec<String> {
    let mut points: Vec<_> = table
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let number = *fields.get(2)?;
            devices
                .iter()
                .any(|d| {
                    (!number.starts_with("0:") && d.number == number)
                        || line
                            .split_once(" - ")
                            .and_then(|(_, tail)| tail.split_whitespace().nth(1))
                            .is_some_and(|source| d.matches_source(source))
                })
                .then(|| unescape(fields.get(4).copied().unwrap_or_default()))
        })
        .filter(|p| p.starts_with('/'))
        .collect();
    points.sort();
    points.dedup();
    points
}
pub fn protected(point: &str) -> bool {
    ["/", "/home", "/run", "/var"].contains(&point)
        || [
            "/boot", "/efi", "/usr", "/etc", "/proc", "/sys", "/dev", "/bin", "/sbin", "/lib",
            "/lib64",
        ]
        .iter()
        .any(|p| std::path::Path::new(point).starts_with(p))
}
/// Merge device rows without duplicating fstab or mounted locations. Hidden fstab entries stay hidden.
pub fn merge(
    drives: &mut Vec<Drive>,
    devices: &[Device],
    table: &str,
    fstab: &[crate::fstab::FstabEntry],
    key: impl Fn(&Device) -> String,
) {
    for device in devices.iter().filter(|d| d.browsable()) {
        let points = mountpoints(table, &[device]);
        let whole: Vec<_> = devices.iter().filter(|d| d.whole == device.whole).collect();
        let safe_remove =
            device.whole_complete && !mountpoints(table, &whole).iter().any(|p| protected(p));
        let entries: Vec<_> = fstab
            .iter()
            .filter(|e| device.matches_source(&e.drive.device))
            .collect();
        let safe = !points.iter().any(|p| protected(p));
        let actions = Actions {
            key: key(device),
            removable: device.removable && safe_remove,
            encrypted: device.encrypted(),
            optical: device.kind == "rom",
        };
        let mut found = false;
        for drive in drives
            .iter_mut()
            .filter(|d| points.contains(&d.path) || device.matches_source(&d.device))
        {
            found = true;
            // Fstab user/owner policy is kept distinct from the removable-device policy.
            if safe
                && (device.removable
                    || drive.fstab.as_ref().is_some_and(|f| f.permission.is_none()))
            {
                drive.device_actions = Some(actions.clone());
                if device.removable {
                    drive.kind = "removable".into();
                }
            }
        }
        if !found && entries.is_empty() && device.removable && safe {
            let mut drive = Drive::new(
                points.first().cloned().unwrap_or_default(),
                &device.fstype,
                &device.path,
                if device.read_only { "ro" } else { "rw" },
            );
            drive.name = if device.label.is_empty() {
                device.path.clone()
            } else {
                device.label.clone()
            };
            drive.kind = "removable".into();
            drive.source = "device".into();
            drive.mounted = !points.is_empty();
            drive.device_actions = Some(actions);
            drives.push(drive);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<Device> {
        parse_lsblk(br#"{"blockdevices":[{"path":"/dev/sdb","maj:min":"8:16","type":"disk","rm":0,"hotplug":1,"tran":"usb","children":[{"path":"/dev/sdb1","maj:min":"8:17","type":"part","fstype":"ext4","label":"Stick","uuid":"private-uuid"},{"path":"/dev/sdb2","maj:min":"8:18","type":"part","fstype":"crypto_LUKS"}]},{"path":"/dev/sr0","maj:min":"11:0","type":"rom","rm":true,"fstype":"iso9660","ro":true}]}"#).unwrap()
    }
    #[test]
    fn usb_partitions_hotplug_luks_and_optical() {
        let devices = fixture();
        assert_eq!(devices.len(), 4);
        assert!(devices.iter().all(|d| d.removable));
        assert_eq!(devices[1].whole, "/dev/sdb");
        assert!(devices[2].encrypted());
        assert!(devices[3].read_only);
        let mut rows = Vec::new();
        merge(&mut rows, &devices, "", &[], |_| "opaque".into());
        assert_eq!(rows.len(), 3);
        assert!(!rows[0].mounted);
        let public = format!("{rows:?} {}", serde_json::to_string(&rows).unwrap());
        assert!(!public.contains("private-uuid"));
    }
    #[test]
    fn all_partitions_bind_aliases_and_system_mounts() {
        let devices = fixture();
        let table = "1 0 8:17 / /media/a rw - ext4 /dev/sdb1 rw\n2 0 8:18 / /mnt/hidden rw - ext4 /dev/sdb2 rw\n3 0 8:18 /sub /alias rw - ext4 /dev/sdb2 rw";
        let all: Vec<_> = devices.iter().filter(|d| d.whole == "/dev/sdb").collect();
        assert_eq!(
            mountpoints(table, &all),
            vec!["/alias", "/media/a", "/mnt/hidden"]
        );
        let mut rows = Vec::new();
        merge(
            &mut rows,
            &devices,
            "1 0 8:17 / / rw - ext4 /dev/sdb1 rw",
            &[],
            |_| "opaque".into(),
        );
        assert_eq!(rows.len(), 2); // The other partition is usable, but cannot power off the system drive.
        assert!(!rows[0].device_actions.as_ref().unwrap().removable);
    }
    #[test]
    fn btrfs_source_aliases_and_stable_private_keys() {
        let devices = fixture();
        let device = &devices[1];
        assert_eq!(device.key(), device.clone().key());
        assert!(!device.key().contains("private-uuid"));
        assert!(device.matches_source("/dev/disk/by-uuid/private-uuid"));
        assert!(device.matches_source("LABEL=Stick"));
        assert_eq!(
            mountpoints("1 0 0:38 /sub /data rw - btrfs /dev/sdb1 rw", &[device]),
            vec!["/data"]
        );
    }
    #[test]
    fn rejects_forged_malformed_and_oversized_data() {
        assert!(parse_lsblk(b"{}").is_err());
        assert!(parse_lsblk(&vec![b' '; CAP + 1]).is_err());
        assert!(!valid_path("/dev/../etc/passwd"));
        assert!(!valid_path("--help"));
    }
    #[test]
    fn internal_hotplug_disks_are_not_removable() {
        for transport in ["sata", "nvme", ""] {
            let json = serde_json::json!({"blockdevices": [{"path": "/dev/sda", "maj:min": "8:0", "type": "disk", "hotplug": 1, "tran": transport,
                "children": [{"path": "/dev/sda1", "maj:min": "8:1", "type": "part", "fstype": "ext4"}]}]});
            let all = parse_lsblk(&serde_json::to_vec(&json).unwrap()).unwrap();
            assert!(all.iter().all(|d| !d.removable));
            let mut rows = Vec::new();
            merge(&mut rows, &all, "", &[], Device::key);
            assert!(rows.is_empty());
        }
    }
    #[test]
    fn anonymous_device_numbers_never_match_unrelated_mounts() {
        let mut all = fixture();
        all[1].number = "0:38".into();
        let table = "1 0 0:38 / /other rw - btrfs /dev/other rw\n2 0 0:38 / /ours rw - btrfs /dev/disk/by-uuid/private-uuid rw";
        assert_eq!(mountpoints(table, &[&all[1]]), vec!["/ours"]);
    }
    #[test]
    fn malformed_rows_are_skipped_without_losing_good_rows_or_bounds() {
        let json = serde_json::json!({"blockdevices": [
            {"maj:min": "8:0", "children": [{"path": "/dev/sda1", "maj:min": "8:1", "rm": true}]},
            {"path": "/dev/bad", "maj:min": "bad"},
            {"path": "/dev/sdb", "maj:min": "8:16", "type": "disk", "rm": true, "fstype": "ext4"}]});
        let all = parse_lsblk(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, "/dev/sdb");
        let many = serde_json::json!({"blockdevices": vec![serde_json::json!({}); 513]});
        assert!(parse_lsblk(&serde_json::to_vec(&many).unwrap()).is_err());
        let mut nested = serde_json::json!({});
        for _ in 0..17 {
            nested = serde_json::json!({"children": [nested]});
        }
        assert!(parse_lsblk(
            &serde_json::to_vec(&serde_json::json!({"blockdevices": [nested]})).unwrap()
        )
        .is_err());
    }
    #[test]
    fn invalid_sibling_keeps_valid_partition_visible_but_disables_whole_drive_removal() {
        let all = parse_lsblk(br#"{"blockdevices":[{"path":"/dev/sdb","maj:min":"8:16","rm":true,"type":"disk","children":[{"path":"/dev/sdb1","maj:min":"8:17","type":"part","fstype":"ext4"},{"path":"/dev/sdb2"}]}]}"#).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|d| !d.whole_complete));
        let mut rows = Vec::new();
        merge(&mut rows, &all, "", &[], Device::key);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].device_actions.as_ref().unwrap().removable);
    }
    #[test]
    fn fstab_uuid_merges_and_hidden_entries_stay_hidden() {
        let devices = fixture();
        let entries = crate::fstab::parse_fstab(
            "UUID=private-uuid /data ext4 noauto\n/dev/sdb2 /hidden ext4 x-gvfs-hide",
        );
        let mut rows = crate::fstab::merge(Vec::new(), entries.clone());
        merge(&mut rows, &devices, "", &entries, |_| "opaque".into());
        assert_eq!(rows.len(), 2); // fstab + optical; no duplicate or hidden LUKS row.
        assert!(rows[0].device_actions.is_some());
    }
    #[test]
    fn hybrid_iso_stick_shows_only_its_partitions() {
        // Ubuntu installer stick: disk and first partition carry the same ISO filesystem.
        let all = parse_lsblk(br#"{"blockdevices":[{"path":"/dev/sdc","maj:min":"8:32","type":"disk","rm":true,"tran":"usb","fstype":"iso9660","label":"Ubuntu-Server 26.04.1 LTS amd64","uuid":"2026-08-06","children":[{"path":"/dev/sdc1","maj:min":"8:33","type":"part","fstype":"iso9660","label":"Ubuntu-Server 26.04.1 LTS amd64","uuid":"2026-08-06","mountpoints":["/media/u/Ubuntu-Server 26.04.1 LTS amd64"]},{"path":"/dev/sdc2","maj:min":"8:34","type":"part","fstype":"vfat","label":"ESP"}]}]}"#).unwrap();
        assert!(!all[0].browsable() && all[1].browsable() && all[2].browsable());
        let table = r"36 1 8:33 / /media/u/Ubuntu-Server\04026.04.1\040LTS\040amd64 ro - iso9660 /dev/sdc1 ro";
        let mut rows = vec![Drive::new(
            "/media/u/Ubuntu-Server 26.04.1 LTS amd64".into(),
            "iso9660",
            "/dev/sdc1",
            "ro",
        )];
        merge(&mut rows, &all, table, &[], Device::key);
        // Mounted partition + unmounted ESP; no extra unmounted row for the whole stick.
        assert_eq!(rows.len(), 2);
        assert!(rows[0].mounted && rows[0].device_actions.is_some());
        assert_eq!(rows[1].device, "/dev/sdc2");
    }
}
