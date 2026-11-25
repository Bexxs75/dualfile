//! Trash view: list, restore and purge items of the freedesktop trash. Items are always matched
//! by their stable `id` against a fresh listing, never by paths sent from the UI.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Serialize;
use trash::TrashItemSize;

use crate::error::{Error, Result};

/// shortcut: the list is capped here (newest first) so a huge trash cannot stall the UI or the
/// per-item metadata calls; `TrashList::truncated` tells the UI that entries are missing.
pub const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    /// Stable identifier of the trash item (lossy text of the crate's `id`).
    pub id: String,
    pub name: String,
    pub original_parent: String,
    /// Unix seconds; `None` when the trash info carries no usable date.
    pub deleted_at: Option<i64>,
    /// Bytes of a file; `None` for folders and when unknown.
    pub size: Option<u64>,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashList {
    pub entries: Vec<TrashEntry>,
    /// More than [`MAX_ENTRIES`] items exist; only the newest are listed.
    pub truncated: bool,
}

/// Pure mapping from the crate's item fields to the DTO.
pub fn to_entry(
    id: &OsStr,
    name: &OsStr,
    original_parent: &Path,
    time_deleted: i64,
    size: Option<TrashItemSize>,
) -> TrashEntry {
    TrashEntry {
        id: id.to_string_lossy().into_owned(),
        name: name.to_string_lossy().into_owned(),
        original_parent: original_parent.to_string_lossy().into_owned(),
        deleted_at: (time_deleted >= 0).then_some(time_deleted),
        size: match size {
            Some(TrashItemSize::Bytes(bytes)) => Some(bytes),
            _ => None,
        },
        is_dir: matches!(size, Some(TrashItemSize::Entries(_))),
    }
}

/// Newest first; entries without a date last; ties by name.
pub fn sort_entries(entries: &mut [TrashEntry]) {
    entries.sort_by(|a, b| {
        b.deleted_at
            .cmp(&a.deleted_at)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(target_os = "linux")]
fn trash_error(op: &'static str, error: trash::Error) -> Error {
    match error {
        trash::Error::RestoreCollision { path, .. } | trash::Error::RestoreTwins { path, .. } => {
            Error::Exists(path)
        }
        other => Error::Io {
            op,
            path: PathBuf::from("/"),
            source: std::io::Error::other(other),
        },
    }
}

/// The items for `ids` from a fresh listing; any unknown id fails the whole request.
#[cfg(target_os = "linux")]
fn select(ids: &[String]) -> Result<Vec<trash::TrashItem>> {
    if ids.is_empty() || ids.len() > MAX_ENTRIES {
        return Err(Error::InvalidInput("Auswahl im Papierkorb"));
    }
    let wanted: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
    let selected: Vec<_> = trash::os_limited::list()
        .map_err(|e| trash_error("Papierkorb auflisten", e))?
        .into_iter()
        .filter(|item| wanted.contains(item.id.to_string_lossy().as_ref()))
        .collect();
    if selected.len() != wanted.len() {
        return Err(Error::NotFound(PathBuf::from("Papierkorb")));
    }
    Ok(selected)
}

#[cfg(target_os = "linux")]
pub fn list() -> Result<TrashList> {
    let mut items =
        trash::os_limited::list().map_err(|e| trash_error("Papierkorb auflisten", e))?;
    items.sort_by_key(|item| std::cmp::Reverse(item.time_deleted));
    let truncated = items.len() > MAX_ENTRIES;
    items.truncate(MAX_ENTRIES);
    let mut entries: Vec<_> = items
        .iter()
        .map(|item| {
            let size = trash::os_limited::metadata(item).ok().map(|m| m.size);
            to_entry(
                &item.id,
                &item.name,
                &item.original_parent,
                item.time_deleted,
                size,
            )
        })
        .collect();
    sort_entries(&mut entries);
    Ok(TrashList { entries, truncated })
}

/// Restores to the original locations; an occupied location gives `Error::Exists`. Returns the
/// restored original paths.
#[cfg(target_os = "linux")]
pub fn restore(ids: &[String]) -> Result<Vec<String>> {
    let selected = select(ids)?;
    let paths = selected
        .iter()
        .map(|item| item.original_path().to_string_lossy().into_owned())
        .collect();
    trash::os_limited::restore_all(selected)
        .map_err(|e| trash_error("Papierkorb-Einträge wiederherstellen", e))?;
    Ok(paths)
}

/// Deletes for good. The caller must have confirmed with the user.
#[cfg(target_os = "linux")]
pub fn purge(ids: &[String]) -> Result<usize> {
    let selected = select(ids)?;
    let count = selected.len();
    trash::os_limited::purge_all(selected)
        .map_err(|e| trash_error("Papierkorb-Einträge löschen", e))?;
    Ok(count)
}

#[cfg(not(target_os = "linux"))]
pub fn list() -> Result<TrashList> {
    Err(Error::Unsupported("Papierkorb-Ansicht (nur Linux)"))
}
#[cfg(not(target_os = "linux"))]
pub fn restore(_ids: &[String]) -> Result<Vec<String>> {
    Err(Error::Unsupported("Papierkorb-Ansicht (nur Linux)"))
}
#[cfg(not(target_os = "linux"))]
pub fn purge(_ids: &[String]) -> Result<usize> {
    Err(Error::Unsupported("Papierkorb-Ansicht (nur Linux)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_file_dir_and_unknown() {
        let file = to_entry(
            OsStr::new("/t/info/a.trashinfo"),
            OsStr::new("a.txt"),
            Path::new("/home/u"),
            1_700_000_000,
            Some(TrashItemSize::Bytes(12)),
        );
        assert_eq!(
            (file.size, file.is_dir, file.deleted_at),
            (Some(12), false, Some(1_700_000_000))
        );
        assert_eq!(file.id, "/t/info/a.trashinfo");
        let dir = to_entry(
            OsStr::new("i"),
            OsStr::new("d"),
            Path::new("/x"),
            -1,
            Some(TrashItemSize::Entries(3)),
        );
        assert_eq!((dir.size, dir.is_dir, dir.deleted_at), (None, true, None));
        let unknown = to_entry(OsStr::new("i"), OsStr::new("u"), Path::new("/x"), 5, None);
        assert_eq!((unknown.size, unknown.is_dir), (None, false));
    }

    #[test]
    fn sorts_newest_first_undated_last() {
        let e = |name: &str, t: i64| {
            to_entry(OsStr::new(name), OsStr::new(name), Path::new("/"), t, None)
        };
        let mut v = vec![e("b", 5), e("undated", -1), e("a", 5), e("new", 9)];
        sort_entries(&mut v);
        let names: Vec<_> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["new", "a", "b", "undated"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn empty_selection_is_rejected() {
        assert!(matches!(select(&[]), Err(Error::InvalidInput(_))));
    }

    /// A per-mount style trash `<base>/.Trash-<uid>` with `files/` and `info/` (mode 0700) and
    /// one entry `name` whose .trashinfo points to `<base>/dest/<name>`.
    #[cfg(target_os = "linux")]
    fn mount_trash(base: &Path, name: &str) -> (PathBuf, trash::TrashItem) {
        use std::os::unix::fs::PermissionsExt;
        let root = base.join(format!(".Trash-{}", unsafe { libc::geteuid() }));
        for dir in [root.clone(), root.join("files"), root.join("info")] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let dest = base.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let info = root.join(format!("info/{name}.trashinfo"));
        std::fs::write(
            &info,
            format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-11-25T10:00:00\n",
                dest.join(name).display()
            ),
        )
        .unwrap();
        let item = trash::TrashItem {
            id: info.into_os_string(),
            name: name.into(),
            original_parent: dest,
            time_deleted: 0,
        };
        (root, item)
    }

    /// The victim: a directory outside any trash with a file inside.
    #[cfg(target_os = "linux")]
    fn victim(base: &Path, name: &str) -> PathBuf {
        let victim = base.join("victim");
        std::fs::create_dir_all(victim.join(name)).unwrap();
        std::fs::write(victim.join(name).join("keep"), b"keep").unwrap();
        victim
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn own_trusted_mount_trash_restores_and_purges() {
        let temp = tempfile::tempdir().unwrap();
        let (root, item) = mount_trash(temp.path(), "doc");
        std::fs::write(root.join("files/doc"), b"doc").unwrap();
        trash::os_limited::restore_all([item.clone()]).unwrap();
        assert_eq!(std::fs::read(temp.path().join("dest/doc")).unwrap(), b"doc");
        assert!(!root.join("files/doc").exists());
        assert!(!root.join("info/doc.trashinfo").exists());

        let (root, item) = mount_trash(temp.path(), "tree");
        std::fs::create_dir_all(root.join("files/tree/sub")).unwrap();
        std::fs::write(root.join("files/tree/sub/payload"), b"x").unwrap();
        // A symlink inside the trashed tree is unlinked, its target stays.
        let outside = victim(temp.path(), "linked");
        std::os::unix::fs::symlink(outside.join("linked"), root.join("files/tree/link")).unwrap();
        trash::os_limited::purge_all([item]).unwrap();
        assert!(!root.join("files/tree").exists());
        assert!(!root.join("info/tree.trashinfo").exists());
        assert!(outside.join("linked/keep").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreign_trash_with_files_symlinked_into_home_is_not_touched() {
        let temp = tempfile::tempdir().unwrap();
        let (root, item) = mount_trash(temp.path(), "doc");
        let victim = victim(temp.path(), "doc");
        // The attack: files/ is a link to a directory of the victim.
        std::fs::remove_dir(root.join("files")).unwrap();
        std::os::unix::fs::symlink(&victim, root.join("files")).unwrap();
        assert!(
            dualfile_platform::trash_dir::owned_trash_dir(&root, unsafe { libc::geteuid() })
                .is_err()
        );
        assert!(trash::os_limited::purge_all([item.clone()]).is_err());
        assert!(victim.join("doc/keep").exists());
        assert!(trash::os_limited::restore_all([item]).is_err());
        assert!(victim.join("doc/keep").exists());
        assert!(!temp.path().join("dest/doc").exists());
        assert!(root.join("info/doc.trashinfo").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn symlinked_trash_root_is_not_touched() {
        let temp = tempfile::tempdir().unwrap();
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let (real, item) = mount_trash(&elsewhere, "doc");
        std::fs::write(real.join("files/doc"), b"doc").unwrap();
        let mount = temp.path().join("mount");
        std::fs::create_dir(&mount).unwrap();
        let link = mount.join(real.file_name().unwrap());
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let item = trash::TrashItem {
            id: link.join("info/doc.trashinfo").into_os_string(),
            ..item
        };
        assert!(trash::os_limited::purge_all([item.clone()]).is_err());
        assert!(trash::os_limited::restore_all([item]).is_err());
        assert!(real.join("files/doc").exists());
        assert!(real.join("info/doc.trashinfo").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn trash_writable_by_others_is_not_trusted() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let (root, item) = mount_trash(temp.path(), "doc");
        std::fs::write(root.join("files/doc"), b"doc").unwrap();
        for part in [root.clone(), root.join("files"), root.join("info")] {
            for mode in [0o777, 0o775] {
                std::fs::set_permissions(&part, std::fs::Permissions::from_mode(mode)).unwrap();
                let checked = dualfile_platform::trash_dir::owned_trash_dir(&root, unsafe {
                    libc::geteuid()
                });
                let refused = checked.unwrap_err();
                assert!(refused
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<dualfile_platform::trash_dir::OtherWritable>())
                    .is_some_and(|loose| loose.0 == mode));
                assert!(trash::os_limited::purge_all([item.clone()]).is_err());
                assert!(trash::os_limited::restore_all([item.clone()]).is_err());
                assert!(root.join("files/doc").exists());
                assert!(!temp.path().join("dest/doc").exists());
            }
            std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        // A foreign owner is refused too (checked without changing UIDs).
        assert!(dualfile_platform::trash_dir::owned_trash_dir(
            &root,
            unsafe { libc::geteuid() }.wrapping_add(1)
        )
        .is_err());
        trash::os_limited::purge_all([item]).unwrap();
        assert!(!root.join("files/doc").exists());
    }
}
