//! Names that come from a plugin are not trusted (same rules as for server and archive
//! entries): a name that could escape its folder, hide behind control characters or collide
//! with another entry is marked; marked entries are shown but never opened, copied or written.

use std::collections::HashMap;

/// Longest name accepted from a plugin, in bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// Characters that are not control characters but can still hide or reorder text in the UI:
/// bidirectional overrides/isolates, zero-width and other invisible format characters. A plugin
/// could use them to spoof a name (e.g. a right-to-left override to disguise an extension), so
/// they are treated like control characters — rejected in usable names, replaced in shown names.
pub(crate) fn is_spoofing_char(c: char) -> bool {
    matches!(c,
        '\u{061C}'                 // Arabic letter mark
        | '\u{180E}'               // Mongolian vowel separator
        | '\u{200B}'..='\u{200F}'  // zero-width space/joiners + LRM/RLM
        | '\u{202A}'..='\u{202E}'  // bidi embeddings and override
        | '\u{2060}'..='\u{2064}'  // word joiner and invisible operators
        | '\u{2066}'..='\u{2069}'  // bidi isolates
        | '\u{FEFF}'               // zero-width no-break space / BOM
    )
}

/// Why a name is unusable (`None` = fine).
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name == "." || name == ".." {
        Some("Ungültiger Name vom Plugin")
    } else if name.len() > MAX_NAME_BYTES {
        Some("Der Name vom Plugin ist zu lang")
    } else if name.contains('/') || name.contains('\\') {
        Some("Der Name vom Plugin enthält einen Pfadtrenner")
    } else if name.chars().any(|c| c.is_control() || is_spoofing_char(c)) {
        Some("Der Name vom Plugin enthält Steuer- oder unsichtbare Zeichen")
    } else {
        None
    }
}

/// A name that is safe to show: control characters and separators replaced, bounded.
pub fn display_name(name: &str) -> String {
    let shown: String = name
        .chars()
        .map(|c| {
            if c.is_control() || is_spoofing_char(c) || c == '/' || c == '\\' {
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

/// For every name the reason it is unusable: a bad name, or a collision (the same name twice
/// or names that differ only by case). All members of a collision are marked.
pub fn problems<'a>(names: impl Iterator<Item = &'a str> + Clone) -> Vec<Option<&'static str>> {
    let mut groups: HashMap<String, usize> = HashMap::new();
    for name in names.clone() {
        *groups.entry(name.to_lowercase()).or_default() += 1;
    }
    names
        .map(|name| {
            name_problem(name).or_else(|| {
                (groups[&name.to_lowercase()] > 1).then_some(
                    "Der Name kollidiert mit einem anderen Eintrag (Groß-/Kleinschreibung)",
                )
            })
        })
        .collect()
}

/// A path inside an archive offered by a packer plugin: relative, `/`-separated, every
/// component a valid name, bounded depth. Returns the components.
pub fn archive_member_path(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() || path.len() > 4096 || path.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 64 || parts.iter().any(|part| name_problem(part).is_some()) {
        return None;
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let list = ["Readme", "README", "other", "dup", "dup"];
        let found = problems(list.iter().copied());
        assert!(found[0].is_some() && found[1].is_some());
        assert!(found[2].is_none());
        assert!(found[3].is_some() && found[4].is_some());
    }

    #[test]
    fn display_names_hide_control_characters() {
        assert_eq!(display_name("a\nb/c\\d"), "a\u{fffd}b\u{fffd}c\u{fffd}d");
        assert!(display_name(&"é".repeat(300)).len() <= MAX_NAME_BYTES);
    }

    #[test]
    fn invisible_and_bidi_characters_are_marked_and_hidden() {
        // A right-to-left override or a zero-width space can disguise a name in the UI.
        for bad in [
            "bild\u{202e}gpj.exe",
            "a\u{200b}b",
            "x\u{2066}y",
            "\u{feff}start",
        ] {
            assert!(name_problem(bad).is_some(), "{bad:?}");
        }
        assert_eq!(display_name("a\u{202e}b"), "a\u{fffd}b");
        // Ordinary accented and non-Latin letters stay usable.
        for good in ["café", "日本語", "Datei"] {
            assert!(name_problem(good).is_none(), "{good:?}");
        }
    }

    #[test]
    fn archive_member_paths_are_relative_and_clean() {
        assert_eq!(archive_member_path("a/b.txt"), Some(vec!["a", "b.txt"]));
        for bad in ["", "/a", "a//b", "a/../b", "a/./b", "a\\b", "a/\nb", "a/"] {
            assert!(archive_member_path(bad).is_none(), "{bad:?}");
        }
        assert!(archive_member_path(&vec!["d"; 65].join("/")).is_none());
    }
}
