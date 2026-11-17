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
}
