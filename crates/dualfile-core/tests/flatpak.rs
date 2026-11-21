//! Child-process environment injection keeps parallel tests' environment untouched.
#[test]
fn flatpak_trash_uses_host_data_for_delete_list_restore_and_purge() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};
    const CHILD: &str = "DUALFILE_FLATPAK_TRASH_TEST";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let data = root.join("host-data");
        let paths = dualfile_core::system_places::DiscoveryPaths::system();
        assert_eq!(paths.data, data);
        assert_eq!(paths.config, root.join("host-config"));
        let file = root.join("sample.txt");
        fs::write(&file, "preserve me").unwrap();
        trash::delete(&file).unwrap();
        assert!(!file.exists());
        assert!(data.join("Trash/files/sample.txt").exists());
        let items = trash::os_limited::list().unwrap();
        assert_eq!(items.len(), 1);
        trash::os_limited::restore_all(items).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "preserve me");
        trash::delete(&file).unwrap();
        trash::os_limited::purge_all(trash::os_limited::list().unwrap()).unwrap();
        assert!(trash::os_limited::list().unwrap().is_empty());
        fs::write(&file, "empty me").unwrap();
        trash::delete(&file).unwrap();
        let table = dualfile_core::packaging::read_host_file(
            std::path::Path::new("/proc/self/mountinfo"),
            4 * 1024 * 1024,
        )
        .unwrap();
        let dirs = dualfile_core::trash_empty::discover(&table, &paths.data).unwrap();
        let fixture_trash = data.join("Trash");
        assert!(dirs.contains(&fixture_trash));
        // Never empty any other discovered trash directory on the test machine.
        let report = dualfile_core::trash_empty::empty(
            vec![fixture_trash],
            &dualfile_core::ops::JobHandle::silent(),
            std::sync::Arc::new(()),
        );
        assert!(report.failed.is_empty());
        assert!(trash::os_limited::list().unwrap().is_empty());
        assert!(!root.join("private-data/Trash").exists());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(root.path().join("host-data")).unwrap();
    let spawn = bin.join("flatpak-spawn");
    // Simulated host namespace; assert the sole host adapter's protocol.
    fs::write(
        &spawn,
        r#"#!/bin/sh
[ "$1" = --host ] || exit 90
shift
while [ "${1#--env=}" != "$1" ]; do shift; done
[ "$1" = head ] || exit 91
shift
exec /usr/bin/head "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&spawn, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "flatpak_trash_uses_host_data_for_delete_list_restore_and_purge",
            "--nocapture",
        ])
        .env(CHILD, root.path())
        .env("FLATPAK_ID", "com.dualfileapp.Dualfile")
        .env("HOME", root.path())
        .env("XDG_DATA_HOME", root.path().join("private-data"))
        .env("XDG_CONFIG_HOME", root.path().join("private-config"))
        .env("HOST_XDG_DATA_HOME", root.path().join("host-data"))
        .env("HOST_XDG_CONFIG_HOME", root.path().join("host-config"))
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
