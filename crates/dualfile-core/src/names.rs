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
}
