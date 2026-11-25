//! Per-drive trash folders (#56) against a simulated mount table: the Flatpak host adapter
//! (`flatpak-spawn --host head /proc/mounts`) is replaced by a script that serves only the
//! test's own "drives", so the real trash folders of the machine are never seen or touched.
//! Runs in a child process to keep the environment and the umask of other tests untouched.
#[cfg(target_os = "linux")]
#[test]
fn per_drive_trash_folders_are_checked_for_list_delete_and_restore() {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};
    const CHILD: &str = "DUALFILE_TRASH_MOUNTS_TEST";
    let mode_of = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let uid = unsafe { libc::getuid() };
        let home_trash = root.join("host-data/Trash");
        // 1. umask 002 (Ubuntu default): a new per-drive trash is still created with 0700,
        //    so the item is listed and can be restored.
        unsafe { libc::umask(0o002) };
        let drive = root.join("drive");
        let doc = drive.join("doc.txt");
        fs::write(&doc, "mine").unwrap();
        trash::delete(&doc).unwrap();
        let own = drive.join(format!(".Trash-{uid}"));
        for part in [own.clone(), own.join("files"), own.join("info")] {
            assert_eq!(mode_of(&part), 0o700, "{part:?}");
        }
        assert!(own.join("files/doc.txt").exists());
        let items = trash::os_limited::list().unwrap();
        let mine: Vec<_> = items
            .into_iter()
            .filter(|i| i.original_parent == drive)
            .collect();
        assert_eq!(mine.len(), 1);
        trash::os_limited::restore_all(mine).unwrap();
        assert_eq!(fs::read_to_string(&doc).unwrap(), "mine");

        // 2. An existing own trash that others may write to (created by another program
        //    under umask 002) is tightened to 0700 and stays usable and visible.
        for part in [own.clone(), own.join("files"), own.join("info")] {
            fs::set_permissions(&part, fs::Permissions::from_mode(0o775)).unwrap();
        }
        trash::delete(&doc).unwrap();
        assert!(own.join("files/doc.txt").exists());
        fs::set_permissions(&own, fs::Permissions::from_mode(0o777)).unwrap();
        let listed = trash::os_limited::list().unwrap();
        assert!(listed.iter().any(|i| i.original_parent == drive));
        for part in [own.clone(), own.join("files"), own.join("info")] {
            assert_eq!(mode_of(&part), 0o700, "{part:?}");
        }

        // 3. A prepared trash (files/ is a link into the victim's home) is neither listed
        //    nor written to: a deleted file goes to the home trash instead.
        let foreign = root.join("foreign");
        let victim = root.join("victim-home");
        let planted = foreign.join(format!(".Trash-{uid}"));
        fs::create_dir_all(planted.join("info")).unwrap();
        std::os::unix::fs::symlink(&victim, planted.join("files")).unwrap();
        fs::create_dir_all(victim.join("Documents")).unwrap();
        fs::write(victim.join("Documents/keep.txt"), "keep").unwrap();
        fs::write(
            planted.join("info/Documents.trashinfo"),
            format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-11-25T10:00:00\n",
                foreign.join("Documents").display()
            ),
        )
        .unwrap();
        let listed = trash::os_limited::list().unwrap();
        assert!(!listed
            .iter()
            .any(|i| Path::new(&i.id).starts_with(&planted)));
        let secret = foreign.join("secret.txt");
        fs::write(&secret, "secret").unwrap();
        trash::delete(&secret).unwrap();
        assert!(!secret.exists());
        assert!(!victim.join("secret.txt").exists());
        assert_eq!(
            fs::read_to_string(home_trash.join("files/secret.txt")).unwrap(),
            "secret"
        );
        assert_eq!(fs::read_dir(planted.join("info")).unwrap().count(), 1);
        assert!(victim.join("Documents/keep.txt").exists());

        // 4. A trash root that is a symlink to an own, valid trash elsewhere is refused too.
        let linked = root.join("linked");
        let elsewhere = root.join("elsewhere-trash");
        for part in [
            elsewhere.clone(),
            elsewhere.join("files"),
            elsewhere.join("info"),
        ] {
            fs::create_dir_all(&part).unwrap();
            fs::set_permissions(&part, fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::os::unix::fs::symlink(&elsewhere, linked.join(format!(".Trash-{uid}"))).unwrap();
        let note = linked.join("note.txt");
        fs::write(&note, "note").unwrap();
        trash::delete(&note).unwrap();
        assert!(!elsewhere.join("files/note.txt").exists());
        assert!(home_trash.join("files/note.txt").exists());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    for dir in ["bin", "host-data", "drive", "foreign", "linked"] {
        fs::create_dir(root.path().join(dir)).unwrap();
    }
    let mounts: String = ["drive", "foreign", "linked"]
        .iter()
        .map(|d| format!("test {} tmpfs rw 0 0\n", root.path().join(d).display()))
        .collect();
    fs::write(root.path().join("mounts"), mounts).unwrap();
    let spawn = bin.join("flatpak-spawn");
    fs::write(
        &spawn,
        r#"#!/bin/sh
[ "$1" = --host ] || exit 90
shift
[ "$1" = --watch-bus ] || exit 92
shift
while [ "${1#--env=}" != "$1" ]; do shift; done
[ "$1" = head ] || exit 91
shift
for last; do :; done
[ "$last" = /proc/mounts ] && exec cat "$DUALFILE_TRASH_MOUNTS_TEST/mounts"
exec /usr/bin/head "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&spawn, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "per_drive_trash_folders_are_checked_for_list_delete_and_restore",
            "--nocapture",
        ])
        .env(CHILD, root.path())
        .env("FLATPAK_ID", "com.dualfileapp.Dualfile")
        .env("HOME", root.path())
        .env("XDG_DATA_HOME", root.path().join("private-data"))
        .env("HOST_XDG_DATA_HOME", root.path().join("host-data"))
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
