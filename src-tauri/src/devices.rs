//! Device actions resolve fresh identities and reserve every affected mount before invoking udisks.
use crate::{
    convert::UiError,
    places::{fresh_table, helper_error, mount_error},
};
use dualfile_core::{
    appenv,
    devices::{self, Device},
    drives::Drive,
    fstab, places_pool,
};
use std::{path::PathBuf, process::Command, sync::Arc, time::Duration};

pub(crate) fn key(device: &Device) -> String {
    device.key()
}
pub(crate) fn list() -> Result<Vec<Device>, UiError> {
    let mut command = Command::new("lsblk");
    command.args(["--json", "--bytes", "--tree", "--output", devices::COLUMNS]);
    let output = appenv::capture_isolated(command, Duration::from_millis(750), devices::CAP)
        .map_err(helper_error)?;
    devices::parse_lsblk(&output).map_err(|_| mount_error("notFound"))
}
fn entries() -> Result<Vec<fstab::FstabEntry>, UiError> {
    let text = crate::places::bounded_read("/etc/fstab".into(), 256 * 1024)
        .filter(|s| s.len() < 256 * 1024)
        .ok_or_else(|| mount_error("timeout"))?;
    Ok(fstab::parse_fstab(&text))
}
fn resolve<'a>(devices: &'a [Device], identity: &str) -> Result<&'a Device, UiError> {
    devices
        .iter()
        .find(|d| key(d) == identity)
        .ok_or_else(|| mount_error("notFound"))
}
fn members<'a>(all: &'a [Device], device: &'a Device, whole: bool) -> Vec<&'a Device> {
    all.iter()
        .filter(|d| {
            if whole {
                d.whole == device.whole
            } else {
                d.path == device.path
            }
        })
        .collect()
}
fn authorize(
    device: &Device,
    all: &[Device],
    table: &str,
    fstab_target: Option<&str>,
    whole: bool,
) -> Result<(), UiError> {
    let points = devices::mountpoints(table, &members(all, device, whole));
    if !device.browsable()
        || (whole && !device.whole_complete)
        || points.iter().any(|p| devices::protected(p))
        || (fstab_target.is_none() && !device.removable)
    {
        return Err(mount_error("notPermitted"));
    }
    if fstab_target.is_some_and(devices::protected) {
        return Err(mount_error("notPermitted"));
    }
    Ok(())
}
fn fstab_device<'a>(all: &'a [Device], entry: &fstab::FstabEntry) -> Result<&'a Device, UiError> {
    if let Some(device) = all.iter().find(|d| d.matches_source(&entry.drive.device)) {
        return Ok(device);
    }
    let path = entry.drive.device.clone();
    if !devices::valid_path(&path) {
        return Err(mount_error("notPermitted"));
    }
    let resolved = canonical(path)?;
    all.iter()
        .find(|d| std::path::Path::new(&d.path) == resolved)
        .ok_or_else(|| mount_error("notFound"))
}
fn canonical(path: String) -> Result<PathBuf, UiError> {
    static POOL: std::sync::OnceLock<places_pool::Pool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| places_pool::Pool::new(2))
        .run_keyed(path.clone(), Duration::from_millis(200), move || {
            std::fs::canonicalize(path)
        })
        .ok_or_else(|| mount_error("timeout"))?
        .map_err(|_| mount_error("notFound"))
}
fn fstab_policy(entry: &fstab::FstabEntry) -> Result<(), UiError> {
    let point = &entry.drive.path;
    if entry.hidden
        || devices::protected(point)
        || !point.starts_with('/')
        || point.split('/').any(|p| p == "." || p == "..")
        || entry
            .drive
            .fstab
            .as_ref()
            .is_none_or(|f| f.permission.is_some() || f.automount)
        || dualfile_core::drives::network_type(&entry.drive.fstype)
    {
        return Err(mount_error("notPermitted"));
    }
    Ok(())
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Mount,
    Unmount,
    Eject,
    PowerOff,
}
impl Action {
    fn removal(self) -> bool {
        matches!(self, Self::Eject | Self::PowerOff)
    }
}
fn selected<'a>(
    all: &'a [Device],
    identity: &str,
    target: Option<&str>,
    removal: bool,
) -> Result<&'a Device, UiError> {
    let device = resolve(all, identity)?;
    let entries = entries()?;
    if let Some(point) = target {
        let entry = entries
            .iter()
            .find(|e| e.drive.path == point && !e.hidden)
            .ok_or_else(|| mount_error("notFound"))?;
        fstab_policy(entry)?;
        // Refuse aliases rather than risk mounting over a protected resolved directory.
        if canonical(point.to_owned())? != std::path::Path::new(point)
            || key(fstab_device(all, entry)?) != identity
        {
            return Err(mount_error("notPermitted"));
        }
    } else if !removal {
        // Every fstab entry uses its dedicated endpoint and authorization matrix.
        // This also prevents a raw removable-device request bypassing fstab target validation.
        for entry in &entries {
            let source = &entry.drive.device;
            let matches = if device.matches_source(source) {
                true
            } else if devices::valid_path(source) {
                match canonical(source.clone()) {
                    Ok(path) => path == std::path::Path::new(&device.path),
                    Err(error) if error.kind == "notFound" => false,
                    Err(error) => return Err(error),
                }
            } else {
                false
            };
            if matches {
                return Err(mount_error("notPermitted"));
            }
        }
    }
    Ok(device)
}
fn scope(
    all: &[Device],
    device: &Device,
    table: &str,
    target: Option<&str>,
    whole: bool,
) -> Vec<PathBuf> {
    // Even partition actions reserve the drive identity, preventing parallel power-off.
    let mut scope: Vec<PathBuf> = devices::mountpoints(table, &members(all, device, whole))
        .into_iter()
        .map(Into::into)
        .collect();
    scope.extend(
        members(all, device, whole)
            .iter()
            .map(|d| PathBuf::from(&d.path)),
    );
    scope.push(device.whole.clone().into());
    if let Some(target) = target {
        scope.push(target.into());
    }
    scope.sort();
    scope.dedup();
    scope
}
fn check_scope(
    all: &[Device],
    device: &Device,
    table: &str,
    target: Option<&str>,
    reserved: &[PathBuf],
    whole: bool,
) -> Result<(), UiError> {
    if scope(all, device, table, target, whole)
        .iter()
        .all(|p| reserved.contains(p))
    {
        Ok(())
    } else {
        Err(mount_error("busy"))
    }
}
fn helper(
    verb: &str,
    path: &str,
    lease: Arc<crate::mount_guard::Lease>,
) -> Result<(), appenv::HelperError> {
    let mut command = Command::new("udisksctl");
    command.args([verb, "--block-device", path]);
    appenv::run_isolated_guarded(command, Duration::from_secs(120), lease)
}
fn unmount_partition(
    device: &Device,
    table: &str,
    lease: Arc<crate::mount_guard::Lease>,
) -> Result<Result<(), appenv::HelperError>, UiError> {
    let entries = entries()?;
    let points = devices::mountpoints(table, &[device]);
    if let Some(entry) = entries.iter().find(|e| {
        (points.contains(&e.drive.path) || device.matches_source(&e.drive.device))
            && e.drive
                .fstab
                .as_ref()
                .is_some_and(|f| f.permission.is_some())
    }) {
        crate::places::authorize_current_user_entry(entry, true)?;
        let mut command = Command::new("umount");
        command.args(["--", &entry.drive.path]);
        return Ok(appenv::run_isolated_guarded(
            command,
            Duration::from_secs(60),
            lease,
        ));
    }
    Ok(helper("unmount", &device.path, lease))
}
fn reconcile(
    result: Result<(), appenv::HelperError>,
    observed: impl FnOnce() -> Result<bool, UiError>,
) -> Result<(), UiError> {
    if result.is_ok() || matches!(result, Err(appenv::HelperError::Timeout)) {
        if observed()? {
            return Ok(());
        }
        if result.is_ok() {
            return Err(mount_error("busy"));
        }
    }
    result.map_err(helper_error)
}
/// USB Eject deliberately means unmount all and power off; optical Eject opens the tray.
fn removal_command(action: Action, device: &Device) -> Command {
    if action == Action::Eject && device.kind == "rom" {
        let mut command = Command::new("eject");
        command.args(["--", &device.path]);
        command
    } else {
        let mut command = Command::new("udisksctl");
        command.args(["power-off", "--block-device", &device.whole]);
        command
    }
}
fn reconcile_removal(
    result: Result<(), appenv::HelperError>,
    observed: impl FnOnce() -> Result<bool, UiError>,
) -> Result<(), UiError> {
    // Device-node removal lags the successful reply. Only uncertain outcomes need discovery.
    if result.is_ok() {
        return Ok(());
    }
    reconcile(result, observed)
}
fn unlocked(all: &[Device], container: &Device) -> bool {
    all.iter()
        .any(|d| d.parent.as_deref() == Some(&container.path))
}
fn lock_container(
    all: &[Device],
    container: &Device,
    run: impl FnOnce(&str, &str) -> Result<(), appenv::HelperError>,
    read: impl FnOnce() -> Result<Vec<Device>, UiError>,
) -> Result<(), UiError> {
    if !container.encrypted() || !unlocked(all, container) {
        return Ok(());
    }
    let identity = key(container);
    reconcile(run("lock", &container.path), || {
        let current = read()?;
        Ok(!unlocked(&current, resolve(&current, &identity)?))
    })
}
fn action(identity: &str, action: Action, target: Option<&str>) -> Result<Option<String>, UiError> {
    let all = list()?;
    let device = selected(&all, identity, target, action.removal())?;
    let table = fresh_table()?;
    authorize(device, &all, &table, target, action.removal())?;
    let reserved = scope(&all, device, &table, target, action.removal());
    let lease = Arc::new(
        crate::mount_guard::registry()
            .unmount_many_wait(reserved.clone(), Duration::from_secs(2))?,
    );
    let all = list()?;
    let device = selected(&all, identity, target, action.removal())?;
    let table = fresh_table()?;
    authorize(device, &all, &table, target, action.removal())?;
    check_scope(&all, device, &table, target, &reserved, action.removal())?;
    let points = devices::mountpoints(&table, &[device]);
    if let Some(point) = target {
        let is_mounted = crate::places::mounted(&table, point);
        if is_mounted != (action == Action::Unmount)
            || is_mounted && !points.iter().any(|p| p == point)
        {
            return Err(mount_error(if is_mounted { "busy" } else { "notFound" }));
        }
    }
    match action {
        Action::Mount => {
            if device.encrypted() {
                return Err(mount_error("notPermitted"));
            }
            if !points.is_empty() {
                return Err(mount_error("busy"));
            }
            let result = helper("mount", &device.path, lease.clone());
            let mut mounted_at = None;
            reconcile(result, || {
                let current = list()?;
                let device = resolve(&current, identity)?;
                let points = devices::mountpoints(&fresh_table()?, &[device]);
                mounted_at = target
                    .and_then(|p| points.iter().find(|s| *s == p))
                    .cloned()
                    .or_else(|| {
                        if target.is_none() {
                            points.first().cloned()
                        } else {
                            None
                        }
                    });
                Ok(mounted_at.is_some())
            })?;
            Ok(mounted_at)
        }
        Action::Unmount => {
            if points.is_empty() {
                return Err(mount_error("notFound"));
            }
            let result = helper("unmount", &device.path, lease.clone());
            reconcile(result, || {
                Ok(devices::mountpoints(&fresh_table()?, &[device]).is_empty())
            })?;
            Ok(None)
        }
        Action::Eject | Action::PowerOff => {
            if !device.removable || target.is_some() {
                return Err(mount_error("notPermitted"));
            }
            let original: Vec<_> = members(&all, device, true)
                .into_iter()
                .map(|d| (d.path.clone(), key(d)))
                .collect();
            // Work from the deepest block layers to the parent. Every partition, including
            // encrypted mappings and hidden mounts, participates in admission and rechecks.
            for (_, id) in original.iter().rev() {
                let current = list()?;
                let device = resolve(&current, id)?;
                let table = fresh_table()?;
                check_scope(&current, device, &table, None, &reserved, true)?;
                if devices::mountpoints(&table, &[device]).is_empty() {
                    continue;
                }
                let result = unmount_partition(device, &table, lease.clone())?;
                reconcile(result, || {
                    Ok(devices::mountpoints(&fresh_table()?, &[device]).is_empty())
                })?;
            }
            // Unmount first, then close unlocked encryption layers from the inside out.
            for container in members(&all, device, true)
                .into_iter()
                .rev()
                .filter(|d| d.encrypted())
            {
                let current = list()?;
                let container = resolve(&current, &key(container))?;
                let table = fresh_table()?;
                check_scope(&current, container, &table, None, &reserved, true)?;
                if !devices::mountpoints(&table, &members(&current, container, true)).is_empty() {
                    return Err(mount_error("busy"));
                }
                lock_container(
                    &current,
                    container,
                    |verb, path| helper(verb, path, lease.clone()),
                    list,
                )?;
            }
            let current = list()?;
            let device = resolve(&current, identity)?;
            let table = fresh_table()?;
            authorize(device, &current, &table, None, true)?;
            check_scope(&current, device, &table, None, &reserved, true)?;
            if members(&current, device, true)
                .iter()
                .any(|d| !original.contains(&(d.path.clone(), key(d))))
                || !devices::mountpoints(&table, &members(&current, device, true)).is_empty()
            {
                return Err(mount_error("busy"));
            }
            let result = appenv::run_isolated_guarded(
                removal_command(action, device),
                Duration::from_secs(120),
                lease.clone(),
            );
            reconcile_removal(result, || {
                let current = list()?;
                if action == Action::Eject && device.kind == "rom" {
                    // An ejected optical drive remains present, but no longer has media.
                    Ok(!current
                        .iter()
                        .any(|d| d.path == device.path && !d.fstype.is_empty()))
                } else {
                    Ok(!current.iter().any(|d| d.whole == device.whole))
                }
            })?;
            Ok(None)
        }
    }
}
pub(crate) fn fstab_action(point: &str, unmount: bool) -> Result<(), UiError> {
    if !point.starts_with('/')
        || point.split('/').any(|s| s == ".." || s == ".")
        || devices::protected(point)
    {
        return Err(mount_error("notPermitted"));
    }
    let all = list()?;
    let entries = entries()?;
    let entry = entries
        .iter()
        .find(|e| e.drive.path == point)
        .ok_or_else(|| mount_error("notFound"))?;
    fstab_policy(entry)?;
    let id = key(fstab_device(&all, entry)?);
    action(
        &id,
        if unmount {
            Action::Unmount
        } else {
            Action::Mount
        },
        Some(point),
    )
    .map(|_| ())
}
pub(crate) fn affected(
    identity: &str,
    whole: bool,
    paths: &[String],
) -> Result<Vec<usize>, UiError> {
    let all = list()?;
    let device = resolve(&all, identity)?;
    let table = fresh_table()?;
    authorize(device, &all, &table, None, whole)?;
    let mut indices = Vec::new();
    for point in devices::mountpoints(&table, &members(&all, device, whole)) {
        indices.extend(crate::mount_guard::affected_paths(
            &PathBuf::from(point),
            paths,
        )?);
    }
    indices.sort();
    indices.dedup();
    Ok(indices)
}
#[tauri::command]
pub async fn device_list(window: tauri::WebviewWindow) -> Result<Vec<Drive>, UiError> {
    crate::places::main_only(window.label())?;
    tauri::async_runtime::spawn_blocking(|| {
        let mut rows = Vec::new();
        devices::merge(&mut rows, &list()?, &fresh_table()?, &entries()?, key);
        Ok(rows)
    })
    .await
    .map_err(|_| UiError::internal())?
}
#[tauri::command]
pub async fn device_mount(
    window: tauri::WebviewWindow,
    device_key: String,
) -> Result<String, UiError> {
    crate::places::main_only(window.label())?;
    tauri::async_runtime::spawn_blocking(move || {
        action(&device_key, Action::Mount, None)?.ok_or_else(|| mount_error("notFound"))
    })
    .await
    .map_err(|_| UiError::internal())?
}
async fn remove(
    window: tauri::WebviewWindow,
    device_key: String,
    operation: Action,
) -> Result<(), UiError> {
    crate::places::main_only(window.label())?;
    tauri::async_runtime::spawn_blocking(move || action(&device_key, operation, None).map(|_| ()))
        .await
        .map_err(|_| UiError::internal())?
}
#[tauri::command]
pub async fn device_unmount(
    window: tauri::WebviewWindow,
    device_key: String,
) -> Result<(), UiError> {
    remove(window, device_key, Action::Unmount).await
}
#[tauri::command]
pub async fn device_eject(window: tauri::WebviewWindow, device_key: String) -> Result<(), UiError> {
    remove(window, device_key, Action::Eject).await
}
#[tauri::command]
pub async fn device_power_off(
    window: tauri::WebviewWindow,
    device_key: String,
) -> Result<(), UiError> {
    remove(window, device_key, Action::PowerOff).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<Device> {
        devices::parse_lsblk(br#"{"blockdevices":[{"path":"/dev/sdb","maj:min":"8:16","type":"disk","rm":true,"children":[{"path":"/dev/sdb1","maj:min":"8:17","type":"part","fstype":"ext4"},{"path":"/dev/sdb2","maj:min":"8:18","type":"part","fstype":"ext4"}]}]}"#).unwrap()
    }
    #[test]
    fn forged_nonremovable_and_system_targets_are_refused() {
        let mut all = fixture();
        assert!(resolve(&all, "/dev/sdb1").is_err());
        all[1].removable = false;
        assert!(authorize(&all[1], &all, "", None, false).is_err());
        assert!(authorize(&all[1], &all, "", Some("/data"), false).is_ok());
        assert!(authorize(
            &all[1],
            &all,
            "1 0 8:18 / / rw - ext4 /dev/sdb2 rw",
            Some("/data"),
            true
        )
        .is_err());
    }
    #[test]
    fn whole_drive_reservation_catches_jobs_on_other_partitions_and_aliases() {
        let all = fixture();
        let table = "1 0 8:17 / /media/one rw - ext4 /dev/sdb1 rw\n2 0 8:18 / /media/two rw - ext4 /dev/sdb2 rw\n3 0 8:18 /sub /alias rw - ext4 /dev/sdb2 rw";
        let registry = Arc::new(crate::mount_guard::Registry::default());
        let job = registry.job(vec!["/alias/source".into()]).unwrap();
        let points = scope(&all, &all[1], table, None, true);
        assert!(registry.unmount_many(points.clone()).is_err());
        assert!(registry
            .job(vec!["/media/one/still-available".into()])
            .is_ok());
        drop(job);
        let _lease = registry.unmount_many(points.clone()).unwrap();
        assert!(registry.job(vec!["/media/two/destination".into()]).is_err());
        assert!(registry.job(vec!["/dev/sdb2".into()]).is_err());
        assert!(registry.job(vec!["/unrelated/path".into()]).is_ok());
        assert!(check_scope(
            &all,
            &all[1],
            &format!("{table}\n4 0 8:18 / /new rw - ext4 /dev/sdb2 rw"),
            None,
            &points,
            true
        )
        .is_err());
    }
    #[test]
    fn local_fstab_partition_on_system_disk_does_not_reserve_root() {
        let mut all = fixture();
        for device in &mut all {
            device.removable = false;
        }
        let table = "1 0 8:18 / / rw - ext4 /dev/sdb2 rw";
        assert!(authorize(&all[1], &all, table, Some("/data"), false).is_ok());
        let registry = Arc::new(crate::mount_guard::Registry::default());
        let _job = registry.job(vec!["/home/work/file".into()]).unwrap();
        let _lease = registry
            .unmount_many(scope(&all, &all[1], table, Some("/data"), false))
            .unwrap();
        assert!(registry.job(vec!["/home/another/file".into()]).is_ok());
        assert!(registry.job(vec!["/data/file".into()]).is_err());
    }
    #[test]
    fn fstab_polkit_matrix_stays_separate_from_removable_policy() {
        for (line, allowed) in [
            ("/dev/sdb1 /data ext4 noauto", true),
            ("UUID=abc /data ext4 defaults", true),
            ("host:/export /data nfs noauto", false),
            ("/dev/sdb1 /data ext4 user,noauto", false),
            ("/dev/sdb1 /data ext4 owner,noauto", false),
            ("/dev/sdb1 /data ext4 x-systemd.automount", false),
            ("/dev/sdb1 /usr/data ext4 noauto", false),
            ("/dev/sdb1 /data ext4 x-gvfs-hide", false),
        ] {
            let entry = fstab::parse_fstab(line).remove(0);
            assert_eq!(fstab_policy(&entry).is_ok(), allowed);
        }
    }
    #[test]
    fn optical_eject_never_powers_off_and_usb_eject_is_deliberate() {
        let optical = devices::parse_lsblk(br#"{"blockdevices":[{"path":"/dev/sr0","maj:min":"11:0","type":"rom","rm":true,"fstype":"iso9660"}]}"#).unwrap();
        let command = removal_command(Action::Eject, &optical[0]);
        assert_eq!(command.get_program(), "eject");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--", "/dev/sr0"]);
        let all = fixture();
        for action in [Action::Eject, Action::PowerOff] {
            let command = removal_command(action, &all[1]);
            assert_eq!(command.get_program(), "udisksctl");
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                ["power-off", "--block-device", "/dev/sdb"]
            );
        }
    }
    #[test]
    fn successful_removal_accepts_lagging_lsblk_but_timeout_requires_absence() {
        let lagging = fixture();
        let mut reads = 0;
        assert!(reconcile_removal(Ok(()), || {
            reads += 1;
            Ok(lagging.is_empty())
        })
        .is_ok());
        assert_eq!(reads, 0);
        assert!(
            reconcile_removal(Err(appenv::HelperError::Timeout), || Ok(lagging.is_empty()))
                .is_err()
        );
        assert!(reconcile_removal(Err(appenv::HelperError::Timeout), || Ok(true)).is_ok());
    }
    #[test]
    fn unlocked_luks_is_locked_and_reconciled_before_removal() {
        let all = devices::parse_lsblk(br#"{"blockdevices":[{"path":"/dev/sdb","maj:min":"8:16","type":"disk","rm":true,"children":[{"path":"/dev/sdb1","maj:min":"8:17","type":"part","fstype":"crypto_LUKS","children":[{"path":"/dev/mapper/secret","maj:min":"253:0","type":"crypt","fstype":"ext4"}]}]}]}"#).unwrap();
        let locked = all[..2].to_vec();
        let mut calls = Vec::new();
        assert!(lock_container(
            &all,
            &all[1],
            |verb, path| {
                calls.push((verb.to_owned(), path.to_owned()));
                Ok(())
            },
            || Ok(locked.clone())
        )
        .is_ok());
        assert_eq!(calls, vec![("lock".to_owned(), "/dev/sdb1".to_owned())]);
        assert!(lock_container(
            &all,
            &all[1],
            |_, _| Err(appenv::HelperError::Timeout),
            || Ok(locked.clone())
        )
        .is_ok());
        assert!(lock_container(
            &all,
            &all[1],
            |_, _| Err(appenv::HelperError::Timeout),
            || Ok(all.clone())
        )
        .is_err());
        assert!(lock_container(&all, &all[1], |_, _| Ok(()), || Ok(all.clone())).is_err());
        assert!(lock_container(
            &locked,
            &locked[1],
            |_, _| panic!("already locked"),
            || panic!("no helper ran")
        )
        .is_ok());
    }
    #[test]
    fn timeout_rereads_state_and_never_assumes_success() {
        assert!(reconcile(Err(appenv::HelperError::Timeout), || Ok(true)).is_ok());
        assert!(reconcile(Err(appenv::HelperError::Timeout), || Ok(false)).is_err());
        assert!(reconcile(Ok(()), || Err(mount_error("timeout"))).is_err());
        assert!(reconcile(
            Err(appenv::HelperError::Failed("polkit dismissed".into())),
            || panic!("not a timeout")
        )
        .is_err());
    }
}
