//! Parsers for what servers send as directory listings: `MLSD` facts (preferred) and the
//! Unix `LIST` format. The server is not trusted: every line is bounded, a line that does not
//! parse is skipped (counted), nothing here panics on odd input, and the number of entries is
//! capped. Names are returned as received; `names` decides whether they may be used.

use crate::transport::Limits;
use dualfile_core::vfs::EntryKind;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq)]
pub struct ListedEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Permission bits (`rwxr-xr-x` of `LIST`, `UNIX.mode` of `MLSD`).
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    /// Target of a link; only shown, never followed.
    pub link_target: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub entries: Vec<ListedEntry>,
    /// Lines that were not understood or were too long.
    pub skipped: usize,
    /// More entries than the limit: the rest was dropped.
    pub truncated: bool,
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_to_time(year: i64, month: i64, day: i64, h: i64, m: i64, s: i64) -> Option<SystemTime> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    if !(1900..=9999).contains(&year) {
        return None;
    }
    let secs = days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s;
    if secs >= 0 {
        Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(secs.unsigned_abs()))
    }
}

/// `YYYYMMDDHHMMSS[.fff]` (MLSD `modify`, MDTM replies) in UTC.
pub fn parse_mlsd_time(text: &str) -> Option<SystemTime> {
    let main = text.split('.').next()?;
    if main.len() != 14 || !main.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |range: std::ops::Range<usize>| main[range].parse::<i64>().ok();
    civil_to_time(
        n(0..4)?,
        n(4..6)?,
        n(6..8)?,
        n(8..10)?,
        n(10..12)?,
        n(12..14)?,
    )
}

fn within(line: &str, limits: &Limits) -> bool {
    line.len() <= limits.max_line_len
}

/// Parses `MLSD` lines (`fact=value;fact=value; name`). `cdir` and `pdir` entries and the
/// names `.`/`..` are dropped.
pub fn parse_mlsd(lines: &[String], limits: &Limits) -> Parsed {
    let mut out = Parsed::default();
    for line in lines {
        if out.entries.len() >= limits.max_entries {
            out.truncated = true;
            break;
        }
        if !within(line, limits) {
            out.skipped += 1;
            continue;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        // facts, one space, then the name (which may contain spaces and `;`)
        let Some((facts, name)) = line.split_once(' ') else {
            out.skipped += 1;
            continue;
        };
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let mut kind = None;
        let mut size = 0u64;
        let mut modified = None;
        let mut mode = None;
        let mut link_target = None;
        let mut skip = false;
        for fact in facts.split(';').filter(|f| !f.is_empty()) {
            let Some((key, value)) = fact.split_once('=') else {
                continue;
            };
            match key.to_ascii_lowercase().as_str() {
                "type" => {
                    let lower = value.to_ascii_lowercase();
                    match lower.as_str() {
                        "file" => kind = Some(EntryKind::File),
                        "dir" => kind = Some(EntryKind::Dir),
                        "cdir" | "pdir" => skip = true,
                        other => {
                            if let Some(target) = other
                                .strip_prefix("os.unix=slink:")
                                .or_else(|| other.strip_prefix("os.unix=symlink:"))
                            {
                                kind = Some(EntryKind::Symlink);
                                // keep the original case of the target
                                let at = value.len() - target.len();
                                link_target = Some(value[at..].to_owned());
                            } else if other == "os.unix=slink" || other == "os.unix=symlink" {
                                kind = Some(EntryKind::Symlink);
                            } else {
                                // sockets, devices, unknown types: shown as files would
                                // invite reading them
                                skip = true;
                            }
                        }
                    }
                }
                "size" | "sizd" => {
                    if let Ok(parsed) = value.parse::<u64>() {
                        size = parsed;
                    }
                }
                "modify" => modified = parse_mlsd_time(value),
                "unix.mode" => {
                    if let Ok(parsed) = u32::from_str_radix(value, 8) {
                        mode = Some(parsed & 0o7777);
                    }
                }
                _ => {}
            }
        }
        if skip {
            continue;
        }
        let Some(kind) = kind else {
            out.skipped += 1;
            continue;
        };
        out.entries.push(ListedEntry {
            uid: None,
            gid: None,
            name: name.to_owned(),
            kind,
            size: if kind == EntryKind::File { size } else { 0 },
            modified,
            mode,
            link_target,
        });
    }
    out
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn parse_mode(perms: &str) -> Option<u32> {
    let bytes = perms.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let mut mode = 0u32;
    for (index, bit) in [
        0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001,
    ]
    .into_iter()
    .enumerate()
    {
        match bytes[index + 1] {
            b'r' | b'w' | b'x' | b's' | b't' => mode |= bit,
            b'-' | b'S' | b'T' => {}
            _ => return None,
        }
    }
    Some(mode)
}

/// Splits off the next whitespace separated field; returns (field, rest after the separator run).
fn next_field(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start_matches([' ', '\t']);
    if text.is_empty() {
        return None;
    }
    match text.find([' ', '\t']) {
        Some(end) => Some((&text[..end], &text[end..])),
        None => Some((text, "")),
    }
}

/// Parses Unix style `LIST` lines
/// (`drwxr-xr-x 2 owner group 4096 Nov 12 20:00 name`, year instead of time for old files).
/// `now` decides the year of entries listed with a time of day.
pub fn parse_list_unix(lines: &[String], limits: &Limits, now: SystemTime) -> Parsed {
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let now_year = 1970 + now_secs.div_euclid(31_556_952);
    let mut out = Parsed::default();
    for line in lines {
        if out.entries.len() >= limits.max_entries {
            out.truncated = true;
            break;
        }
        if !within(line, limits) {
            out.skipped += 1;
            continue;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() || line.starts_with("total ") {
            continue;
        }
        match parse_list_line(line, now_secs, now_year) {
            Some(entry) => {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                out.entries.push(entry);
            }
            None => out.skipped += 1,
        }
    }
    out
}

/// `size month day year-or-time name` (the month must be a real month name).
fn size_and_date(rest: &str) -> Option<(u64, &str, &str, &str, &str)> {
    let (size, rest) = next_field(rest)?;
    let (month, rest) = next_field(rest)?;
    let (day, rest) = next_field(rest)?;
    let (year_or_time, rest) = next_field(rest)?;
    if !MONTHS.contains(&month.to_ascii_lowercase().as_str()) {
        return None;
    }
    let name = rest.strip_prefix([' ', '\t']).unwrap_or(rest);
    Some((size.parse().ok()?, month, day, year_or_time, name))
}

fn parse_list_line(line: &str, now_secs: i64, now_year: i64) -> Option<ListedEntry> {
    let (perms, rest) = next_field(line)?;
    let type_char = perms.chars().next()?;
    let kind = match type_char {
        '-' => EntryKind::File,
        'd' => EntryKind::Dir,
        'l' => EntryKind::Symlink,
        _ => return None, // devices, pipes, sockets
    };
    let mode = parse_mode(perms)?;
    let (_links, rest) = next_field(rest)?;
    let (_owner, rest) = next_field(rest)?;
    // some servers print no group column; the size is then the 4th field. Try with group
    // first and fall back when the next fields do not look like "size month day".
    let (_group, after_group) = next_field(rest)?;
    let parsed = size_and_date(after_group).or_else(|| size_and_date(rest))?;
    let (size, month, day, year_or_time, raw_name) = parsed;
    let raw_name = raw_name.to_owned();
    let month_index = MONTHS
        .iter()
        .position(|m| *m == month.to_ascii_lowercase())? as i64
        + 1;
    let day: i64 = day.parse().ok()?;
    let modified = if let Some((h, m)) = year_or_time.split_once(':') {
        let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
        let candidate = civil_to_time(now_year, month_index, day, h, m, 0);
        // a time of day in the future (more than a day ahead) belongs to last year
        match candidate {
            Some(time) => {
                let secs = time
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                if secs > now_secs + 86_400 {
                    civil_to_time(now_year - 1, month_index, day, h, m, 0)
                } else {
                    Some(time)
                }
            }
            None => None,
        }
    } else {
        civil_to_time(year_or_time.parse().ok()?, month_index, day, 0, 0, 0)
    };
    let (name, link_target) = if kind == EntryKind::Symlink {
        match raw_name.split_once(" -> ") {
            Some((name, target)) => (name.to_owned(), Some(target.to_owned())),
            None => (raw_name, None),
        }
    } else {
        (raw_name, None)
    };
    if name.is_empty() {
        return None;
    }
    Some(ListedEntry {
        uid: None,
        gid: None,
        name,
        kind,
        size: if kind == EntryKind::File { size } else { 0 },
        modified,
        mode: Some(mode),
        link_target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }
    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }
    fn limits() -> Limits {
        Limits::default()
    }

    #[test]
    fn mlsd_facts_names_and_types() {
        let parsed = parse_mlsd(
            &lines(&[
                "type=cdir;modify=20260101000000; .",
                "type=pdir;modify=20260101000000; ..",
                "type=file;size=1234;modify=20261112203015.123;UNIX.mode=0644; a file; with ;semicolons.txt",
                "Type=DIR;Modify=20200229120000;unix.mode=0755; sub dir",
                "type=OS.unix=slink:../Target;size=9; link",
                "type=OS.unix=slink; bare-link",
                "type=file;size=7; ",
                "size=5; no-type",
                "type=OS.unix=socket; sock",
                "garbage without facts",
                "type=file;size=abc;modify=bad; odd.txt",
            ]),
            &limits(),
        );
        let names: Vec<&str> = parsed.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "a file; with ;semicolons.txt",
                "sub dir",
                "link",
                "bare-link",
                "odd.txt"
            ]
        );
        let file = &parsed.entries[0];
        assert_eq!(
            (file.kind, file.size, file.mode),
            (EntryKind::File, 1234, Some(0o644))
        );
        assert_eq!(
            parse_mlsd_time("20261112203015"),
            parsed.entries[0].modified
        );
        assert_eq!(parsed.entries[1].kind, EntryKind::Dir);
        assert_eq!(
            parsed.entries[1].modified,
            parse_mlsd_time("20200229120000")
        );
        assert_eq!(parsed.entries[2].link_target.as_deref(), Some("../Target"));
        assert_eq!(parsed.entries[2].kind, EntryKind::Symlink);
        assert_eq!(
            parsed.entries[4].size, 0,
            "unparsable size is zero, the entry is kept"
        );
        // `size=7; ` has an empty name, `no-type`/garbage lack a type: skipped, never fatal
        assert_eq!(parsed.skipped, 2);
        assert!(!parsed.truncated);
    }

    #[test]
    fn mlsd_time_is_utc_and_validated() {
        assert_eq!(parse_mlsd_time("19700101000000"), Some(UNIX_EPOCH));
        assert_eq!(parse_mlsd_time("20000101000000"), Some(at(946_684_800)));
        assert_eq!(parse_mlsd_time("20240229235959"), Some(at(1_709_251_199)));
        for bad in [
            "",
            "2026",
            "20261301000000",
            "20260132000000",
            "20260101250000",
            "2026010100000x",
            "00000101000000",
        ] {
            assert_eq!(parse_mlsd_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn unix_list_formats() {
        let now = parse_mlsd_time("20261112203000").unwrap();
        let parsed = parse_list_unix(
            &lines(&[
                "total 12",
                "drwxr-xr-x   2 ftp      ftp          4096 Nov 12 20:00 pub",
                "-rw-r--r--   1 ftp      ftp          1234 Mar  3  2019 old file.txt",
                "lrwxrwxrwx   1 ftp      ftp             7 Nov 12 20:00 latest -> pub/new",
                "-rwxr-xr-x   1 ftp      ftp             0 Dec 31 23:59 future.sh",
                "crw-rw-rw-   1 root     root         1, 3 Nov 12 20:00 null",
                "drwxr-xr-x   2 ftp      ftp          4096 Nov 12 20:00 .",
                "drwxr-xr-x   2 ftp      ftp          4096 Nov 12 20:00 ..",
                "-rw-r--r--   1 ftp      12 Jan  1  2020 nogroup.txt",
                "not a listing line",
                "-rw-r--r-- 1 u g notanumber Jan 1 2020 bad-size",
                "",
            ]),
            &limits(),
            now,
        );
        let names: Vec<&str> = parsed.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["pub", "old file.txt", "latest", "future.sh", "nogroup.txt"]
        );
        assert_eq!(parsed.entries[0].kind, EntryKind::Dir);
        assert_eq!(parsed.entries[0].mode, Some(0o755));
        assert_eq!(
            parsed.entries[0].modified,
            parse_mlsd_time("20261112200000")
        );
        assert_eq!(parsed.entries[1].size, 1234);
        assert_eq!(
            parsed.entries[1].modified,
            parse_mlsd_time("20190303000000")
        );
        assert_eq!(parsed.entries[2].kind, EntryKind::Symlink);
        assert_eq!(parsed.entries[2].link_target.as_deref(), Some("pub/new"));
        // Dec 31 is after "now": it belongs to the previous year
        assert_eq!(
            parsed.entries[3].modified,
            parse_mlsd_time("20251231235900")
        );
        assert_eq!(parsed.entries[4].size, 12);
        assert_eq!(parsed.skipped, 3, "device, text and bad size are skipped");
    }

    #[test]
    fn limits_are_enforced() {
        let small = Limits {
            max_entries: 3,
            max_line_len: 40,
            ..Limits::default()
        };
        let many: Vec<String> = (0..10)
            .map(|i| format!("type=file;size={i}; f{i}"))
            .collect();
        let parsed = parse_mlsd(&many, &small);
        assert_eq!(parsed.entries.len(), 3);
        assert!(parsed.truncated);
        let long = vec![
            format!("type=file;size=1; {}", "n".repeat(100)),
            "type=file;size=1; ok".to_owned(),
        ];
        let parsed = parse_mlsd(&long, &small);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.skipped, 1);
        let list: Vec<String> = (0..10)
            .map(|i| format!("-rw-r--r-- 1 u g {i} Jan  1  2020 f{i}"))
            .collect();
        let parsed = parse_list_unix(
            &list,
            &Limits {
                max_entries: 4,
                ..Limits::default()
            },
            UNIX_EPOCH,
        );
        assert_eq!(parsed.entries.len(), 4);
        assert!(parsed.truncated);
    }

    #[test]
    fn hostile_lines_never_panic() {
        let nasty = [
            "",
            " ",
            ";;;;; ",
            "type=file; ",
            "=;=; x",
            "type=; x",
            &"\u{0}".repeat(10),
            "-rw-r--r-- 1 u g 99999999999999999999999 Jan 1 2020 huge",
            "-rw-r--r-- 1 u g 1 Jan 99 2020 badday",
            "-rw-r--r-- 1 u g 1 Foo 1 2020 badmonth",
            "-rw-r--r-- 1 u g 1 Jan 1 99:99 badtime",
            "drwx",
            "l",
            "ü ü ü ü ü ü ü ü ü",
            "-rw-r--r-- 1 u g 1 Jan 1 2020 \u{202e}evil\u{0}name\r",
        ];
        let owned: Vec<String> = nasty.iter().map(|s| s.to_string()).collect();
        let _ = parse_mlsd(&owned, &limits());
        let parsed = parse_list_unix(&owned, &limits(), UNIX_EPOCH);
        // a name with control characters is passed on as received; `names` marks it unusable
        assert!(parsed.entries.iter().any(|e| e.name.contains('\0')));
    }
}
