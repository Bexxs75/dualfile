//! Names that come from a server are not trusted. A name that could escape its folder, inject
//! commands or collide with another entry is marked; marked entries are shown but can never be
//! opened, copied or written locally (the file-system layer refuses them).

use crate::listing::ListedEntry;
use std::collections::HashMap;

/// Longest name accepted from a server, in bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// Why a name from a listing is unusable (`None` = fine).
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name == "." || name == ".." {
        Some("Ungültiger Name vom Server")
    } else if name.len() > MAX_NAME_BYTES {
        Some("Der Name vom Server ist zu lang")
    } else if name.contains('/') || name.contains('\\') {
        Some("Der Name vom Server enthält einen Pfadtrenner")
    } else if name.chars().any(char::is_control) {
        Some("Der Name vom Server enthält Steuerzeichen")
    } else {
        None
    }
}

/// A name that is safe to show: control characters and separators replaced.
pub fn display_name(name: &str) -> String {
    let shown: String = name
        .chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect();
    if shown.len() > MAX_NAME_BYTES {
        let mut end = MAX_NAME_BYTES;
        while !shown.is_char_boundary(end) {
            end -= 1;
        }
        shown[..end].to_owned()
    } else {
        shown
    }
}

/// For every entry the reason it is unusable: a bad name, or a collision with another entry
/// (the same name twice, or names that differ only by case: on a case-insensitive disk one
/// would overwrite the other). All members of a collision are marked.
pub fn problems(entries: &[ListedEntry]) -> Vec<Option<&'static str>> {
    let mut groups: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        *groups.entry(entry.name.to_lowercase()).or_default() += 1;
    }
    entries
        .iter()
        .map(|entry| {
            name_problem(&entry.name).or_else(|| {
                (groups[&entry.name.to_lowercase()] > 1).then_some(
                    "Der Name kollidiert mit einem anderen Eintrag (Groß-/Kleinschreibung)",
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::vfs::EntryKind;

    fn entry(name: &str) -> ListedEntry {
        ListedEntry {
            uid: None,
            gid: None,
            name: name.into(),
            kind: EntryKind::File,
            size: 0,
            modified: None,
            mode: None,
            link_target: None,
        }
    }

    #[test]
    fn hostile_names_are_marked() {
        let long = "n".repeat(256);
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "../x",
            "a\\b",
            "a\0b",
            "a\nb",
            "a\rb",
            "a\x1bb",
            "a\x7fb",
            long.as_str(),
        ] {
            assert!(name_problem(bad).is_some(), "{bad:?}");
        }
        for good in [
            "a",
            "a b",
            "ü",
            ".hidden",
            "...",
            "a.b",
            &"n".repeat(255),
            "Notiz.",
        ] {
            assert!(name_problem(good).is_none(), "{good:?}");
        }
    }

    #[test]
    fn collisions_mark_every_member() {
        let list = vec![
            entry("Readme"),
            entry("README"),
            entry("other"),
            entry("dup"),
            entry("dup"),
        ];
        let found = problems(&list);
        assert!(found[0].is_some() && found[1].is_some());
        assert!(found[2].is_none());
        assert!(found[3].is_some() && found[4].is_some());
    }

    #[test]
    fn display_names_hide_control_characters() {
        assert_eq!(display_name("a\nb/c\\d"), "a\u{fffd}b\u{fffd}c\u{fffd}d");
        assert!(display_name(&"é".repeat(300)).len() <= MAX_NAME_BYTES);
    }
}
