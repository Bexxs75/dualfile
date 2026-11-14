//! Reads a few metadata tags used by the multi-rename placeholders: the EXIF capture date and
//! camera for images, and the ID3 tags for audio files. Reading never fails a batch — an
//! unreadable, non-media or tag-less file simply yields empty fields. Parsing is done by the
//! `kamadak-exif` and `id3` crates.
//!
//! Only regular files no larger than [`MAX_MEDIA_BYTES`] are parsed: a special file (FIFO,
//! device) could block `open`, and a large or crafted container could make the EXIF reader
//! allocate a lot of memory (it reads the TIFF body into a buffer). Returned string fields are
//! bounded in length.

use serde::Serialize;
use std::path::Path;

/// Largest file whose metadata is parsed. The EXIF reader may buffer the container body, so this
/// bounds the memory one file can cost; real photo/audio headers are far smaller.
const MAX_MEDIA_BYTES: u64 = 64 << 20;
/// Longest tag value kept (characters). A tag is free-form, attacker-influenced text.
const MAX_FIELD_CHARS: usize = 240;

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MediaTags {
    /// EXIF DateTimeOriginal as `"YYYY:MM:DD HH:MM:SS"`, empty when absent.
    pub taken: String,
    /// Camera make and model, empty when absent.
    pub camera: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    /// ID3 track number, 0 when absent.
    pub track: u32,
}

/// Reads the EXIF and ID3 tags of one local file. Any failure yields empty fields for that file.
pub fn read_tags(path: &Path) -> MediaTags {
    let mut tags = MediaTags::default();
    // Only touch real, reasonably-sized regular files. `metadata` follows symlinks but only
    // stats (it never blocks on a FIFO the way `open` would), so a special file is excluded here
    // before any parser opens it, and an oversized container is skipped.
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() <= MAX_MEDIA_BYTES => {}
        _ => return tags,
    }
    read_exif(path, &mut tags);
    read_id3(path, &mut tags);
    tags
}

/// Collapses whitespace, drops control characters and bounds the length; the caller still
/// validates the final name.
fn clean(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_FIELD_CHARS)
        .collect()
}

fn read_exif(path: &Path, tags: &mut MediaTags) {
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut reader = std::io::BufReader::new(&file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return;
    };
    // Read the ASCII value directly: `display_value()` renders dates as "YYYY-MM-DD HH:MM:SS"
    // and quotes strings, but EXIF stores the date natively as "YYYY:MM:DD HH:MM:SS", which is
    // the form the rename placeholders parse.
    let ascii = |tag| {
        exif.get_field(tag, exif::In::PRIMARY)
            .and_then(|field| match &field.value {
                exif::Value::Ascii(parts) => parts
                    .first()
                    .map(|bytes| clean(&String::from_utf8_lossy(bytes))),
                _ => None,
            })
            .filter(|s| !s.is_empty())
    };
    if let Some(taken) = ascii(exif::Tag::DateTimeOriginal).or_else(|| ascii(exif::Tag::DateTime)) {
        tags.taken = taken;
    }
    let make = ascii(exif::Tag::Make).unwrap_or_default();
    let model = ascii(exif::Tag::Model).unwrap_or_default();
    tags.camera = clean(&format!("{make} {model}"));
}

fn read_id3(path: &Path, tags: &mut MediaTags) {
    use id3::TagLike;
    let Ok(tag) = id3::Tag::read_from_path(path) else {
        return;
    };
    if let Some(v) = tag.artist() {
        tags.artist = clean(v);
    }
    if let Some(v) = tag.title() {
        tags.title = clean(v);
    }
    if let Some(v) = tag.album() {
        tags.album = clean(v);
    }
    if let Some(n) = tag.track() {
        tags.track = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unreadable_or_non_media_files_yield_empty_tags() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_tags(&dir.path().join("fehlt")), MediaTags::default());

        let plain = dir.path().join("text.txt");
        std::fs::write(&plain, b"kein Medium").unwrap();
        assert_eq!(read_tags(&plain), MediaTags::default());

        // A directory is not a regular file and must be skipped, never parsed.
        assert_eq!(read_tags(dir.path()), MediaTags::default());
    }

    #[test]
    fn oversized_files_are_skipped_without_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("riesig.tiff");
        let file = std::fs::File::create(&big).unwrap();
        // A sparse file whose apparent length exceeds the cap: metadata reports it, so we skip
        // it without ever handing it to the EXIF reader.
        file.set_len(MAX_MEDIA_BYTES + 1).unwrap();
        assert_eq!(read_tags(&big), MediaTags::default());
    }

    #[test]
    fn exif_capture_date_is_read_in_colon_form() {
        // A minimal little-endian TIFF with a single IFD0 DateTime (tag 0x0132) entry. EXIF
        // stores the date with colons; the reader must preserve that, not reformat it.
        let date = b"2021:07:15 14:32:08\0"; // 20 bytes incl. NUL
        let mut tiff: Vec<u8> = vec![
            0x49, 0x49, 0x2a, 0x00, // "II", 42
            0x08, 0x00, 0x00, 0x00, // IFD0 at offset 8
            0x01, 0x00, // one entry
            0x32, 0x01, // tag 0x0132 (DateTime)
            0x02, 0x00, // type 2 (ASCII)
            0x14, 0x00, 0x00, 0x00, // count 20
            0x1a, 0x00, 0x00, 0x00, // value at offset 26
            0x00, 0x00, 0x00, 0x00, // next IFD = 0
        ];
        tiff.extend_from_slice(date); // at offset 26
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foto.tif");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&tiff)
            .unwrap();

        let tags = read_tags(&path);
        assert_eq!(tags.taken, "2021:07:15 14:32:08");
    }

    #[test]
    fn id3_tags_are_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lied.mp3");
        std::fs::File::create(&file)
            .unwrap()
            .write_all(&[0u8; 16])
            .unwrap();
        let mut tag = id3::Tag::new();
        use id3::TagLike;
        tag.set_artist("Künstler");
        tag.set_title("Titel");
        tag.set_album("Album");
        tag.set_track(7);
        tag.write_to_path(&file, id3::Version::Id3v24).unwrap();

        let tags = read_tags(&file);
        assert_eq!(tags.artist, "Künstler");
        assert_eq!(tags.title, "Titel");
        assert_eq!(tags.album, "Album");
        assert_eq!(tags.track, 7);
    }

    #[test]
    fn clean_collapses_whitespace_drops_control_chars_and_bounds_length() {
        assert_eq!(clean("  Canon   EOS \n R5\t"), "Canon EOS R5");
        assert_eq!(clean("a\0b"), "ab");
        assert_eq!(clean(&"x".repeat(1000)).chars().count(), MAX_FIELD_CHARS);
    }
}
