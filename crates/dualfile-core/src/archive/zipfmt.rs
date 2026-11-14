//! ZIP: index from the central directory; entries are decoded by hand from the local data
//! (stored, deflate, bzip2) so the decoder can be an owned, forward-only stream.

use super::budget::{
    corrupt, map_io, Budget, CountSrc, Counted, ReadCtx, Stats, MSG_CORRUPT, MSG_ENCRYPTED,
    MSG_INDEX_BYTES,
};
use super::index::{ArchiveLimits, IndexBuilder, Loc, RawEntry, RawKind};
use super::Source;
use crate::error::{Error, Result};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

const MSG_METHOD: &str = "Komprimierungsverfahren nicht unterstützt";
const MSG_SPECIAL: &str = "Sonderdatei (nur Anzeige)";
const MAX_LINK_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Method {
    Stored,
    Deflate,
    Bzip2,
}

#[derive(Clone, Debug)]
pub(crate) struct ZipLoc {
    data_start: u64,
    comp_size: u64,
    method: Method,
    crc: u32,
    size: u64,
}

pub(crate) struct ZipBackend {
    source: Source,
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn unix_time(seconds: i64) -> Option<SystemTime> {
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
    }
}

fn zip_error(error: ZipError, path: &Path) -> Error {
    match error {
        ZipError::Io(e) => map_io(e, path),
        _ => Error::Archive(MSG_CORRUPT),
    }
}

fn raw_stream(
    source: &Source,
    loc: &ZipLoc,
    stats: Arc<Stats>,
) -> io::Result<Box<dyn Read + Send>> {
    let mut file = source.open()?;
    file.seek(SeekFrom::Start(loc.data_start))?;
    let compressed = CountSrc {
        inner: file.take(loc.comp_size),
        stats,
    };
    Ok(match loc.method {
        Method::Stored => Box::new(compressed),
        Method::Deflate => Box::new(flate2::read::DeflateDecoder::new(compressed)),
        Method::Bzip2 => Box::new(bzip2::read::BzDecoder::new(compressed)),
    })
}

struct Crc32Reader<R> {
    inner: R,
    hasher: crc32fast::Hasher,
    expected: u32,
}

impl<R: Read> Read for Crc32Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n == 0 && !buf.is_empty() {
            if self.hasher.clone().finalize() != self.expected {
                return Err(corrupt(MSG_CORRUPT));
            }
        } else {
            self.hasher.update(&buf[..n]);
        }
        Ok(n)
    }
}

impl ZipBackend {
    pub(crate) fn open_entry(
        &self,
        loc: &ZipLoc,
        ctx: &ReadCtx,
    ) -> io::Result<Box<dyn Read + Send>> {
        let stats = Arc::new(Stats::default());
        let raw = raw_stream(&self.source, loc, stats.clone())?;
        let verified = Crc32Reader {
            inner: raw,
            hasher: crc32fast::Hasher::new(),
            expected: loc.crc,
        };
        Ok(Box::new(Counted::for_entry(
            verified,
            ctx,
            stats,
            Some(loc.size),
        )))
    }
}

pub(crate) fn index(
    source: &Source,
    limits: &ArchiveLimits,
    budget: &Budget,
    builder: &mut IndexBuilder,
) -> Result<ZipBackend> {
    let path = Path::new("");
    let stats = Arc::new(Stats::default());
    let directory_budget = Budget::index(limits.index_dir_bytes, limits.index_time);
    let reader = Counted::for_index(
        source.open().map_err(|e| map_io(e, path))?,
        vec![budget.clone(), directory_budget],
        stats,
        None,
    );
    let mut zip = ZipArchive::new(reader).map_err(|e| zip_error(e, path))?;
    if zip.len() > limits.max_entries {
        return Err(Error::Archive(MSG_INDEX_BYTES));
    }
    for number in 0..zip.len() {
        let entry = {
            let file = zip.by_index_raw(number).map_err(|e| zip_error(e, path))?;
            let method = match file.compression() {
                CompressionMethod::Stored => Some(Method::Stored),
                CompressionMethod::Deflated => Some(Method::Deflate),
                CompressionMethod::Bzip2 => Some(Method::Bzip2),
                _ => None,
            };
            let unix_mode = file.unix_mode();
            let file_type = unix_mode.map(|mode| mode & 0o170_000);
            let kind = if file.is_dir() {
                RawKind::Dir
            } else if file.is_symlink() || file_type == Some(0o120_000) {
                RawKind::Symlink
            } else if matches!(
                file_type,
                Some(0o010_000 | 0o020_000 | 0o060_000 | 0o140_000)
            ) {
                RawKind::Special(MSG_SPECIAL)
            } else {
                RawKind::File
            };
            let unreadable = if file.encrypted() {
                Some(MSG_ENCRYPTED)
            } else if method.is_none() || file.data_start().is_none() {
                Some(MSG_METHOD)
            } else {
                None
            };
            let modified = file
                .last_modified()
                .and_then(|time| dos_to_system_time(time, local_offset_for_dos(time)));
            let loc = match (method, file.data_start()) {
                (Some(method), Some(data_start)) => Some(ZipLoc {
                    data_start,
                    comp_size: file.compressed_size(),
                    method,
                    crc: file.crc32(),
                    size: file.size(),
                }),
                _ => None,
            };
            (
                String::from_utf8_lossy(file.name_raw()).into_owned(),
                kind,
                file.size(),
                modified,
                unix_mode,
                loc,
                unreadable,
            )
        };
        let (name, kind, size, modified, mode, loc, unreadable) = entry;
        // A link's target is the (small) content of its entry; links are never followed.
        let link_target = match (&kind, &loc, unreadable) {
            (RawKind::Symlink, Some(loc), None)
                if loc.size <= MAX_LINK_BYTES && loc.comp_size <= MAX_LINK_BYTES =>
            {
                read_link_target(source, loc)
            }
            _ => None,
        };
        builder.add(RawEntry {
            path: name,
            kind,
            size,
            modified,
            mode,
            link_target,
            loc: loc.map_or(Loc::None, Loc::Zip),
            unreadable,
        })?;
    }
    Ok(ZipBackend {
        source: source.clone(),
    })
}

fn read_link_target(source: &Source, loc: &ZipLoc) -> Option<String> {
    let stats = Arc::new(Stats::default());
    let stream = raw_stream(source, loc, stats).ok()?;
    let mut data = Vec::new();
    stream
        .take(MAX_LINK_BYTES + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() as u64 <= MAX_LINK_BYTES).then(|| String::from_utf8_lossy(&data).into_owned())
}

/// Convert timezone-free ZIP fields using an explicit UTC offset.
pub(crate) fn dos_to_system_time(time: zip::DateTime, offset_secs: i32) -> Option<SystemTime> {
    let days = days_from_civil(
        i64::from(time.year()),
        i64::from(time.month()),
        i64::from(time.day()),
    );
    unix_time(
        days * 86_400
            + i64::from(time.hour()) * 3600
            + i64::from(time.minute()) * 60
            + i64::from(time.second())
            - i64::from(offset_secs),
    )
}

pub(crate) fn system_time_to_dos(time: SystemTime, offset_secs: i32) -> Option<zip::DateTime> {
    let secs = i64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_secs())
        .ok()?
        .checked_add(i64::from(offset_secs))?;
    let days = secs / 86_400;
    let rest = secs % 86_400;
    // civil date from days since 1970-01-01 (proleptic Gregorian)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    if !(1980..=2107).contains(&year) {
        return None;
    }
    zip::DateTime::from_date_and_time(
        year as u16,
        month as u8,
        day as u8,
        (rest / 3600) as u8,
        (rest % 3600 / 60) as u8,
        (rest % 60) as u8,
    )
    .ok()
}

#[cfg(test)]
mod dos_tests {
    use super::*;
    #[test]
    fn offsets_roundtrip_across_transition_dates() {
        for (month, day) in [(3, 29), (10, 25), (1, 1)] {
            for offset in [3600, 7200, -18000, 0] {
                let fields =
                    zip::DateTime::from_date_and_time(2026, month, day, 13, 59, 0).unwrap();
                let expected = unix_time(
                    days_from_civil(2026, month.into(), day.into()) * 86400 + 13 * 3600 + 59 * 60
                        - i64::from(offset),
                )
                .unwrap();
                assert_eq!(dos_to_system_time(fields, offset), Some(expected));
                assert_eq!(system_time_to_dos(expected, offset), Some(fields));
            }
        }
    }
    #[test]
    fn known_zip_local_1359_is_utc_1259() {
        let fields = zip::DateTime::from_date_and_time(2026, 11, 13, 13, 59, 0).unwrap();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(
                "entry",
                zip::write::SimpleFileOptions::default().last_modified_time(fields),
            )
            .unwrap();
        let data = writer.finish().unwrap();
        let mut archive = zip::ZipArchive::new(data).unwrap();
        let time = archive.by_index(0).unwrap().last_modified().unwrap();
        assert_eq!(
            dos_to_system_time(time, 3600),
            unix_time(days_from_civil(2026, 11, 13) * 86400 + 12 * 3600 + 59 * 60)
        );
    }
}

// Non-Unix retains the previous UTC interpretation until a native timezone provider exists.
#[cfg(not(unix))]
pub(crate) fn local_offset_for_dos(_: zip::DateTime) -> i32 {
    0
}
#[cfg(not(unix))]
pub(crate) fn local_offset_for_system(_: SystemTime) -> i32 {
    0
}

#[cfg(unix)]
fn tm_seconds(tm: &libc::tm) -> i64 {
    days_from_civil(
        i64::from(tm.tm_year) + 1900,
        i64::from(tm.tm_mon) + 1,
        i64::from(tm.tm_mday),
    ) * 86400
        + i64::from(tm.tm_hour) * 3600
        + i64::from(tm.tm_min) * 60
        + i64::from(tm.tm_sec)
}
#[cfg(unix)]
pub(crate) fn local_offset_for_system(time: SystemTime) -> i32 {
    let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
        return 0;
    };
    let Ok(seconds) = libc::time_t::try_from(duration.as_secs()) else {
        return 0;
    };
    // SAFETY: both pointers refer to initialized, correctly sized storage; localtime_r owns no shared output buffer.
    unsafe {
        let mut local: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&seconds, &mut local).is_null() {
            return 0;
        }
        (tm_seconds(&local) - seconds) as i32
    }
}
#[cfg(unix)]
pub(crate) fn local_offset_for_dos(fields: zip::DateTime) -> i32 {
    let naive = dos_to_system_time(fields, 0)
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut exact = Vec::new();
    let mut normalized = Vec::new();
    for is_dst in [0, 1] {
        // SAFETY: mktime receives an initialized tm and normalizes it in place.
        unsafe {
            let mut local: libc::tm = std::mem::zeroed();
            local.tm_year = i32::from(fields.year()) - 1900;
            local.tm_mon = i32::from(fields.month()) - 1;
            local.tm_mday = i32::from(fields.day());
            local.tm_hour = i32::from(fields.hour());
            local.tm_min = i32::from(fields.minute());
            local.tm_sec = i32::from(fields.second());
            local.tm_isdst = is_dst;
            let seconds = libc::mktime(&mut local);
            if seconds == -1 {
                continue;
            }
            normalized.push(seconds);
            if tm_seconds(&local) == naive {
                exact.push(seconds);
            }
        }
    }
    // Fold: choose the earlier occurrence. Gap: normalize forward across the missing interval.
    exact
        .into_iter()
        .min()
        .or_else(|| normalized.into_iter().max())
        .map_or(0, |seconds| (naive - seconds) as i32)
}

#[cfg(all(test, unix))]
mod timezone_tests {
    use super::*;
    #[test]
    fn provider_uses_date_specific_offsets_and_deterministic_transitions() {
        // A separate process isolates TZ from every parallel test; no process-global set_var.
        if std::env::var_os("DUALFILE_TZ_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "archive::zipfmt::timezone_tests::provider_uses_date_specific_offsets_and_deterministic_transitions"])
                .env("DUALFILE_TZ_CHILD", "1")
                .env("TZ", "CET-1CEST,M3.5.0,M10.5.0/3")
                .status().unwrap();
            assert!(status.success());
            return;
        }
        for (month, day, hour, expected) in [
            (1, 1, 12, 3600),
            (7, 1, 12, 7200),
            (10, 25, 2, 7200),
            (3, 29, 2, 3600),
        ] {
            let fields = zip::DateTime::from_date_and_time(2026, month, day, hour, 30, 0).unwrap();
            assert_eq!(local_offset_for_dos(fields), expected);
            let time = dos_to_system_time(fields, expected).unwrap();
            if month != 3 {
                assert_eq!(local_offset_for_system(time), expected);
            } else {
                assert_eq!(
                    system_time_to_dos(time, local_offset_for_system(time))
                        .unwrap()
                        .hour(),
                    3
                );
            }
        }
        assert!(system_time_to_dos(UNIX_EPOCH, 0).is_none());
        assert!(system_time_to_dos(UNIX_EPOCH - Duration::from_secs(1), 0).is_none());
    }
}
