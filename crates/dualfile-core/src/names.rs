use crate::error::{Error, Result};

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.trim().is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\0')
        || name.len() > 255
    {
        return Err(Error::InvalidName(name.to_owned()));
    }
    Ok(())
}

pub fn keep_both_name(name: &str, mut exists: impl FnMut(&str) -> bool) -> Result<String> {
    validate_name(name)?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    for index in 2_u32.. {
        let suffix = format!(" ({index})");
        let max_stem = 255usize.saturating_sub(suffix.len() + extension.len());
        let mut stem_end = stem.len().min(max_stem);
        while !stem.is_char_boundary(stem_end) {
            stem_end -= 1;
        }
        if stem_end == 0 {
            return Err(Error::InvalidName(name.to_owned()));
        }
        let candidate = format!("{}{suffix}{extension}", &stem[..stem_end]);
        if !exists(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("u32 name suffix space exhausted")
}

/// Splits `name` into stem and extension for "duplicate here". Files keep their extension,
/// `.tar.<ext>` counts as one (`a.tar.gz` -> `a` + `.tar.gz`), a leading dot is no extension
/// and folders never have one.
fn split_copy_name(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    let Some(dot) = name.rfind('.').filter(|&i| i > 0 && i + 1 < name.len()) else {
        return (name, "");
    };
    let stem = &name[..dot];
    if let Some(tar) = stem.strip_suffix(".tar").filter(|s| !s.is_empty()) {
        return (tar, &name[tar.len()..]);
    }
    (stem, &name[dot..])
}

/// Drops a trailing `_copy` / `_copyN` (N >= 2) so that copying `a_copy.txt` yields
/// `a_copy2.txt` instead of `a_copy_copy.txt`.
fn strip_copy_suffix(stem: &str) -> &str {
    let digits = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    let number = &stem[digits.len()..];
    let Some(base) = digits.strip_suffix("_copy").filter(|b| !b.is_empty()) else {
        return stem;
    };
    match number {
        "" => base,
        n if n.parse::<u32>().is_ok_and(|n| n >= 2) && !n.starts_with('0') => base,
        _ => stem,
    }
}

/// Name for a copy placed in the same folder: `name_copy.ext`, then `name_copy2.ext`,
/// `name_copy3.ext`, ... The first one for which `exists` is false. The suffix is not
/// translated on purpose (stable names, same as asked by testers).
pub fn copy_name(name: &str, is_dir: bool, mut exists: impl FnMut(&str) -> bool) -> Result<String> {
    validate_name(name)?;
    let (stem, extension) = split_copy_name(name, is_dir);
    let stem = strip_copy_suffix(stem);
    for index in 1_u32.. {
        let suffix = if index == 1 {
            "_copy".to_owned()
        } else {
            format!("_copy{index}")
        };
        let max_stem = 255usize.saturating_sub(suffix.len() + extension.len());
        let mut stem_end = stem.len().min(max_stem);
        while !stem.is_char_boundary(stem_end) {
            stem_end -= 1;
        }
        if stem_end == 0 {
            return Err(Error::InvalidName(name.to_owned()));
        }
        let candidate = format!("{}{suffix}{extension}", &stem[..stem_end]);
        if !exists(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("u32 name suffix space exhausted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_names() {
        for name in ["", "  ", ".", "..", "a/b", "a\0b"] {
            assert!(validate_name(name).is_err(), "{name:?}");
        }
        assert!(validate_name(&"x".repeat(256)).is_err());
        assert!(validate_name("Notiz.").is_ok());
        assert!(validate_name("Notiz ").is_ok());
    }

    #[test]
    fn creates_keep_both_names_without_collisions() {
        let reserved = ["report (2).txt", "report (3).txt"];
        let result =
            keep_both_name("report.txt", |candidate| reserved.contains(&candidate)).unwrap();
        assert_eq!(result, "report (4).txt");
        assert_eq!(keep_both_name("archive", |_| false).unwrap(), "archive (2)");
        assert_eq!(keep_both_name(".bashrc", |_| false).unwrap(), ".bashrc (2)");
        assert_eq!(
            keep_both_name(&"x".repeat(255), |_| false).unwrap().len(),
            255
        );
    }

    fn name(n: &str, dir: bool, taken: &[&str]) -> String {
        copy_name(n, dir, |c| taken.contains(&c)).unwrap()
    }

    #[test]
    fn copy_names_keep_extensions_and_count_up() {
        assert_eq!(name("foto.jpg", false, &[]), "foto_copy.jpg");
        assert_eq!(name("foto.jpg", false, &["foto_copy.jpg"]), "foto_copy2.jpg");
        assert_eq!(
            name("foto.jpg", false, &["foto_copy.jpg", "foto_copy2.jpg"]),
            "foto_copy3.jpg"
        );
        assert_eq!(name("archiv.tar.gz", false, &[]), "archiv_copy.tar.gz");
        assert_eq!(name("notiz", false, &[]), "notiz_copy");
        assert_eq!(name(".bashrc", false, &[]), ".bashrc_copy");
        assert_eq!(name("a.b.txt", false, &[]), "a.b_copy.txt");
        assert_eq!(name("v1.2", true, &[]), "v1.2_copy");
        assert_eq!(name("ende.", false, &[]), "ende._copy");
    }

    #[test]
    fn copy_of_a_copy_continues_the_numbering() {
        assert_eq!(name("foto_copy.jpg", false, &["foto_copy.jpg"]), "foto_copy2.jpg");
        assert_eq!(
            name("foto_copy2.jpg", false, &["foto_copy.jpg", "foto_copy2.jpg"]),
            "foto_copy3.jpg"
        );
        // `_copy1` and `_copy0` are not generated names, they stay ordinary stems.
        assert_eq!(name("a_copy1.txt", false, &[]), "a_copy1_copy.txt");
        assert_eq!(name("_copy.txt", false, &[]), "_copy_copy.txt");
    }

    #[test]
    fn copy_name_respects_the_length_limit() {
        let long = "x".repeat(255);
        assert_eq!(copy_name(&long, false, |_| false).unwrap().len(), 255);
        assert!(copy_name("a/b", false, |_| false).is_err());
    }
}
