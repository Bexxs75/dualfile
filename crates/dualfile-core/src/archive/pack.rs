//! Creating archives (`pack`). The archive is staged in a hidden file next to its final
//! location and moved into place without replacing anything; an existing file is never
//! touched, and an abort or error removes the staging file. Links are stored as links (never
//! followed), permissions lose setuid/setgid/sticky bits, sources are only read.

use super::{Format, TarCompression};
use crate::error::{Error, Result};
use crate::names::validate_name;
use crate::ops::{JobHandle, OpReport, ProgressEvent};
use crate::vfs::{open_regular, Entry, EntryKind, LocalVfs, Scheme, VPath, Vfs, WriteMode};
use std::io::{BufWriter, Read, Seek, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_PACK_DEPTH: usize = 128;
const BLOCK: usize = 64 * 1024;
const MSG_CHANGED: &str = "Die Datei hat sich während des Packens geändert";
const MSG_NO_RAR: &str = "RAR-Archive können nicht erstellt werden";
const MSG_NO_PLUGIN_PACK: &str = "Dieses Archivformat kann nicht erstellt werden";

type Staged = BufWriter<crate::vfs::LocalWrite>;

/// Formats `pack` can write: ZIP, TAR (plain, GZ, BZ2, XZ) and 7Z.
pub fn can_write(format: Format) -> bool {
    !matches!(format, Format::Rar | Format::Plugin)
}

/// Packs `sources` (read through `src_vfs`) into the new local archive file `dst_archive`.
/// Directories are packed recursively. The archive must not exist yet (`Error::Exists`, the
/// caller picks a new name for "keep both") and must not lie inside a source directory.
/// Problems with single items are collected in the report; a failure while writing or while
/// reading in the middle of a file aborts the whole archive (nothing is left behind).
pub fn pack(
    src_vfs: &dyn Vfs,
    sources: &[VPath],
    dst_archive: &VPath,
    format: Format,
    job: &JobHandle,
) -> OpReport {
    let mut report = OpReport::default();
    let fail_all = |report: &mut OpReport, error: fn() -> Error| {
        for source in sources {
            job.emit(ProgressEvent::ItemFailed(source.clone()));
            report.failed.push((source.clone(), error()));
        }
    };
    if !can_write(format) {
        if format == Format::Plugin {
            fail_all(&mut report, || Error::Unsupported(MSG_NO_PLUGIN_PACK));
        } else {
            fail_all(&mut report, || Error::Unsupported(MSG_NO_RAR));
        }
        return report;
    }
    if sources.is_empty() {
        report
            .failed
            .push((dst_archive.clone(), Error::InvalidInput("Keine Quellen")));
        return report;
    }
    if let Err(error) = check_destination(dst_archive) {
        report.failed.push((dst_archive.clone(), error));
        return report;
    }

    // Validate every source first; nothing is created when none is usable.
    let dst_parent = dst_archive.path().parent().map(Path::to_path_buf);
    let mut accepted: Vec<(VPath, Entry)> = Vec::new();
    let mut top_names = std::collections::HashSet::new();
    for source in sources {
        let entry = match src_vfs.stat(source) {
            Ok(entry) => entry,
            Err(error) => {
                job.emit(ProgressEvent::ItemFailed(source.clone()));
                report.failed.push((source.clone(), error));
                continue;
            }
        };
        let verdict = validate_name(&entry.name)
            .and_then(|_| {
                if top_names.insert(entry.name.clone()) {
                    Ok(())
                } else {
                    Err(Error::Exists(source.path().to_path_buf()))
                }
            })
            .and_then(|_| match dst_parent.as_deref() {
                Some(parent) => contains_destination(source, &entry, parent),
                None => Ok(()),
            });
        match verdict {
            Ok(()) => accepted.push((source.clone(), entry)),
            Err(error) => {
                job.emit(ProgressEvent::ItemFailed(source.clone()));
                report.failed.push((source.clone(), error));
            }
        }
    }
    if accepted.is_empty() {
        return report;
    }

    let staged = match LocalVfs.stage(dst_archive, WriteMode::NoReplace) {
        Ok(staged) => staged,
        Err(error) => {
            report.failed.push((dst_archive.clone(), error));
            return report;
        }
    };
    let temp_identity = identity_of(staged.file_ref());
    let mut sink = match Sink::new(format, BufWriter::new(staged)) {
        Ok(sink) => sink,
        Err(error) => {
            report
                .failed
                .push((dst_archive.clone(), located(error, dst_archive)));
            return report;
        }
    };

    let mut walker = Walker {
        src: src_vfs,
        job,
        temp_identity,
        failed: Vec::new(),
    };
    let mut done = Vec::new();
    let mut aborted = None;
    for (source, entry) in &accepted {
        if job.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let before = walker.failed.len();
        match walker.add(&mut sink, source, entry, &entry.name, 0) {
            Ok(()) => {
                if walker.failed.len() == before {
                    done.push(source.clone());
                    job.emit(ProgressEvent::ItemDone(source.clone()));
                }
            }
            Err(error) => {
                aborted = Some((source.clone(), error));
                break;
            }
        }
    }
    report.failed.append(&mut walker.failed);
    if let Some((source, error)) = aborted {
        // Dropping the sink removes the staging file.
        drop(sink);
        if matches!(error, Error::Cancelled) {
            report.cancelled = true;
        } else {
            job.emit(ProgressEvent::ItemFailed(source.clone()));
            report.failed.push((source, located(error, dst_archive)));
        }
        return report;
    }
    if report.cancelled || job.is_cancelled() {
        report.cancelled = true;
        return report;
    }
    let finished = sink.finish().and_then(|mut staged| {
        use crate::vfs::WriteFinish;
        staged.commit()
    });
    match finished {
        Ok(()) => report.done = done,
        Err(error) => report
            .failed
            .push((dst_archive.clone(), located(error, dst_archive))),
    }
    report
}

fn check_destination(dst: &VPath) -> Result<()> {
    if dst.scheme() != Scheme::Local {
        return Err(Error::Unsupported(super::MSG_READ_ONLY));
    }
    let name = dst
        .path()
        .file_name()
        .ok_or_else(|| Error::InvalidPath(dst.path().display().to_string()))?
        .to_string_lossy();
    validate_name(&name)?;
    match std::fs::symlink_metadata(dst.path()) {
        Ok(_) => return Err(Error::Exists(dst.path().to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(crate::error::from_io("Ziel prüfen", dst.path(), error)),
    }
    let parent = dst
        .path()
        .parent()
        .ok_or_else(|| Error::InvalidPath(dst.path().display().to_string()))?;
    let meta = std::fs::metadata(parent)
        .map_err(|e| crate::error::from_io("Zielordner prüfen", parent, e))?;
    if !meta.is_dir() {
        return Err(Error::NotADirectory(parent.to_path_buf()));
    }
    Ok(())
}

#[cfg(unix)]
fn identity_of(file: Option<&std::fs::File>) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = file?.metadata().ok()?;
    Some((meta.dev(), meta.ino()))
}
#[cfg(not(unix))]
fn identity_of(_file: Option<&std::fs::File>) -> Option<(u64, u64)> {
    None
}

#[cfg(unix)]
fn path_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}
#[cfg(not(unix))]
fn path_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Refuses a source directory that contains the folder the archive is written into
/// (device and inode of every ancestor of the destination folder are compared).
fn contains_destination(source: &VPath, entry: &Entry, dst_parent: &Path) -> Result<()> {
    if source.scheme() != Scheme::Local || entry.kind != EntryKind::Dir {
        return Ok(());
    }
    let resolved = std::fs::canonicalize(dst_parent)
        .map_err(|e| crate::error::from_io("Zielordner prüfen", dst_parent, e))?;
    let source_identity = path_identity(source.path());
    let canonical_source = std::fs::canonicalize(source.path()).ok();
    for ancestor in resolved.ancestors() {
        if canonical_source.as_deref() == Some(ancestor)
            || (source_identity.is_some() && path_identity(ancestor) == source_identity)
        {
            return Err(Error::DestinationInsideSource);
        }
    }
    Ok(())
}

struct Walker<'a> {
    src: &'a dyn Vfs,
    job: &'a JobHandle,
    temp_identity: Option<(u64, u64)>,
    failed: Vec<(VPath, Error)>,
}

impl Walker<'_> {
    fn skip(&mut self, path: &VPath, error: Error) {
        self.job.emit(ProgressEvent::ItemFailed(path.clone()));
        self.failed.push((path.clone(), error));
    }

    /// Adds `path` (and below). `Err` is fatal and aborts the archive.
    fn add(
        &mut self,
        sink: &mut Sink,
        path: &VPath,
        entry: &Entry,
        archive_path: &str,
        depth: usize,
    ) -> Result<()> {
        if self.job.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match entry.kind {
            EntryKind::Dir => {
                sink.add_dir(archive_path, entry)?;
                let children = match self.src.list(path) {
                    Ok(children) => children,
                    Err(error) => {
                        self.skip(path, error);
                        return Ok(());
                    }
                };
                if depth + 1 > MAX_PACK_DEPTH {
                    self.skip(
                        path,
                        Error::InvalidPath("Ordner zu tief verschachtelt".into()),
                    );
                    return Ok(());
                }
                let mut children = children;
                children.sort_by(|a, b| a.name.cmp(&b.name));
                for child in children {
                    let child_path = match validate_name(&child.name)
                        .and_then(|_| path.join_entry(&child.name))
                    {
                        Ok(child_path) => child_path,
                        Err(error) => {
                            self.skip(path, error);
                            continue;
                        }
                    };
                    let child_archive = format!("{archive_path}/{}", child.name);
                    self.add(sink, &child_path, &child, &child_archive, depth + 1)?;
                }
                Ok(())
            }
            EntryKind::Symlink => match self.src.read_link(path) {
                Ok(target) => sink.add_link(archive_path, &target, entry),
                Err(error) => {
                    self.skip(path, error);
                    Ok(())
                }
            },
            EntryKind::File => self.add_file(sink, path, entry, archive_path),
        }
    }

    fn add_file(
        &mut self,
        sink: &mut Sink,
        path: &VPath,
        entry: &Entry,
        archive_path: &str,
    ) -> Result<()> {
        let opened: Result<(Box<dyn Read + Send>, u64)> = if self.src.is_local() {
            open_regular(path.path()).map(|(file, meta)| {
                if self.temp_identity.is_some() && identity_of(Some(&file)) == self.temp_identity {
                    return (Box::new(std::io::empty()) as Box<dyn Read + Send>, u64::MAX);
                }
                let size = meta.len();
                (Box::new(file) as Box<dyn Read + Send>, size)
            })
        } else {
            self.src.open_read(path).map(|reader| (reader, entry.size))
        };
        let (reader, size) = match opened {
            Ok((_, u64::MAX)) => return Ok(()), // the staging file itself is never packed
            Ok(pair) => pair,
            Err(error) => {
                self.skip(path, error);
                return Ok(());
            }
        };
        let mut counted = Counted {
            inner: reader.take(size),
            job: self.job,
            path: path.clone(),
            done: 0,
            total: size,
        };
        let written = sink.add_file(archive_path, entry, size, &mut counted);
        match written {
            Ok(()) if counted.done == size => Ok(()),
            Ok(()) => Err(Error::Archive(MSG_CHANGED)),
            Err(_) if self.job.is_cancelled() => Err(Error::Cancelled),
            Err(error) => Err(error),
        }
    }
}

/// Reader that watches the job between blocks and reports progress.
struct Counted<'a, R> {
    inner: R,
    job: &'a JobHandle,
    path: VPath,
    done: u64,
    total: u64,
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.job.is_cancelled() {
            return Err(std::io::Error::other("cancelled"));
        }
        let limit = buf.len().min(BLOCK);
        let n = self.inner.read(&mut buf[..limit])?;
        self.done += n as u64;
        if n > 0 {
            self.job.emit(ProgressEvent::BytesCopied {
                path: self.path.clone(),
                bytes: self.done,
                total_bytes: self.total,
            });
        }
        Ok(n)
    }
}

enum Writer {
    Zip(Box<zip::ZipWriter<Staged>>),
    Tar(Box<tar::Builder<Staged>>),
    TarGz(Box<tar::Builder<flate2::write::GzEncoder<Staged>>>),
    TarBz2(Box<tar::Builder<bzip2::write::BzEncoder<Staged>>>),
    TarXz(Box<tar::Builder<lzma_rust2::XzWriter<Staged>>>),
    SevenZ(Box<sevenz_rust2::ArchiveWriter<Staged>>),
}

struct Sink {
    writer: Writer,
}

/// Write errors are created without a location; the destination archive names it.
fn located(error: Error, archive: &VPath) -> Error {
    match error {
        Error::Io { op, path, source } if path.as_os_str().is_empty() => Error::Io {
            op,
            path: archive.path().to_path_buf(),
            source,
        },
        other => other,
    }
}

fn io_error(error: impl std::fmt::Display) -> Error {
    Error::Io {
        op: "Archiv schreiben",
        path: Default::default(),
        source: std::io::Error::other(error.to_string()),
    }
}

fn unix_seconds(time: Option<SystemTime>) -> Option<u64> {
    time?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn clean_mode(entry: &Entry, default: u32) -> u32 {
    entry.mode.map(|mode| mode & 0o777).unwrap_or(default)
}

fn zip_time(time: Option<SystemTime>) -> Option<zip::DateTime> {
    let time = time?;
    super::zipfmt::system_time_to_dos(time, super::zipfmt::local_offset_for_system(time))
}

impl Sink {
    fn new(format: Format, out: Staged) -> Result<Self> {
        let writer = match format {
            Format::Zip => Writer::Zip(Box::new(zip::ZipWriter::new(out))),
            Format::Tar(TarCompression::None) => Writer::Tar(Box::new(tar::Builder::new(out))),
            Format::Tar(TarCompression::Gzip) => Writer::TarGz(Box::new(tar::Builder::new(
                flate2::write::GzEncoder::new(out, flate2::Compression::default()),
            ))),
            Format::Tar(TarCompression::Bzip2) => Writer::TarBz2(Box::new(tar::Builder::new(
                bzip2::write::BzEncoder::new(out, bzip2::Compression::default()),
            ))),
            Format::Tar(TarCompression::Xz) => Writer::TarXz(Box::new(tar::Builder::new(
                lzma_rust2::XzWriter::new(out, lzma_rust2::XzOptions::with_preset(6))
                    .map_err(io_error)?,
            ))),
            Format::SevenZ => Writer::SevenZ(Box::new(
                sevenz_rust2::ArchiveWriter::new(out).map_err(io_error)?,
            )),
            Format::Rar => return Err(Error::Unsupported(MSG_NO_RAR)),
            Format::Plugin => return Err(Error::Unsupported(MSG_NO_PLUGIN_PACK)),
        };
        Ok(Self { writer })
    }

    fn zip_options(entry: &Entry, default_mode: u32, size: u64) -> zip::write::SimpleFileOptions {
        let mut options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(clean_mode(entry, default_mode))
            .large_file(size >= u64::from(u32::MAX) - 1024);
        if let Some(time) = zip_time(entry.modified) {
            options = options.last_modified_time(time);
        }
        options
    }

    fn tar_header(
        entry: &Entry,
        kind: tar::EntryType,
        default_mode: u32,
        size: u64,
    ) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(clean_mode(entry, default_mode));
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(unix_seconds(entry.modified).unwrap_or(0));
        header.set_size(size);
        header
    }

    fn sevenz_entry(
        name: &str,
        entry: &Entry,
        type_bits: u32,
        default_mode: u32,
        directory: bool,
    ) -> sevenz_rust2::ArchiveEntry {
        let mut item = if directory {
            sevenz_rust2::ArchiveEntry::new_directory(name)
        } else {
            sevenz_rust2::ArchiveEntry::new_file(name)
        };
        if let Some(date) = entry
            .modified
            .and_then(|time| sevenz_rust2::NtTime::try_from(time).ok())
        {
            item.last_modified_date = date;
            item.has_last_modified_date = true;
        }
        item.has_windows_attributes = true;
        item.windows_attributes = ((type_bits | clean_mode(entry, default_mode)) << 16)
            | 0x8000
            | if directory { 0x10 } else { 0x20 };
        item
    }

    fn add_dir(&mut self, name: &str, entry: &Entry) -> Result<()> {
        match &mut self.writer {
            Writer::Zip(zip) => zip
                .add_directory(name, Self::zip_options(entry, 0o755, 0))
                .map_err(io_error),
            Writer::Tar(tar) => append_tar(
                tar,
                entry,
                tar::EntryType::Directory,
                0o755,
                &format!("{name}/"),
            ),
            Writer::TarGz(tar) => append_tar(
                tar,
                entry,
                tar::EntryType::Directory,
                0o755,
                &format!("{name}/"),
            ),
            Writer::TarBz2(tar) => append_tar(
                tar,
                entry,
                tar::EntryType::Directory,
                0o755,
                &format!("{name}/"),
            ),
            Writer::TarXz(tar) => append_tar(
                tar,
                entry,
                tar::EntryType::Directory,
                0o755,
                &format!("{name}/"),
            ),
            Writer::SevenZ(seven) => seven
                .push_archive_entry::<&[u8]>(
                    Self::sevenz_entry(name, entry, 0o040_000, 0o755, true),
                    None,
                )
                .map(|_| ())
                .map_err(io_error),
        }
    }

    fn add_link(&mut self, name: &str, target: &str, entry: &Entry) -> Result<()> {
        match &mut self.writer {
            Writer::Zip(zip) => zip
                .add_symlink(name, target, Self::zip_options(entry, 0o777, 0))
                .map_err(io_error),
            Writer::Tar(tar) => link_tar(tar, entry, name, target),
            Writer::TarGz(tar) => link_tar(tar, entry, name, target),
            Writer::TarBz2(tar) => link_tar(tar, entry, name, target),
            Writer::TarXz(tar) => link_tar(tar, entry, name, target),
            Writer::SevenZ(seven) => {
                let item = Self::sevenz_entry(name, entry, 0o120_000, 0o777, false);
                seven
                    .push_archive_entry(item, Some(target.as_bytes()))
                    .map(|_| ())
                    .map_err(io_error)
            }
        }
    }

    fn add_file(
        &mut self,
        name: &str,
        entry: &Entry,
        size: u64,
        data: &mut dyn Read,
    ) -> Result<()> {
        match &mut self.writer {
            Writer::Zip(zip) => {
                zip.start_file(name, Self::zip_options(entry, 0o644, size))
                    .map_err(io_error)?;
                std::io::copy(data, &mut **zip)
                    .map(|_| ())
                    .map_err(io_error)
            }
            Writer::Tar(tar) => file_tar(tar, entry, name, size, data),
            Writer::TarGz(tar) => file_tar(tar, entry, name, size, data),
            Writer::TarBz2(tar) => file_tar(tar, entry, name, size, data),
            Writer::TarXz(tar) => file_tar(tar, entry, name, size, data),
            Writer::SevenZ(seven) => seven
                .push_archive_entry(
                    Self::sevenz_entry(name, entry, 0o100_000, 0o644, false),
                    Some(data),
                )
                .map(|_| ())
                .map_err(io_error),
        }
    }

    /// Writes the trailers and gives the staging file back for the final `commit`.
    fn finish(self) -> Result<crate::vfs::LocalWrite> {
        let out: Staged = match self.writer {
            Writer::Zip(zip) => zip.finish().map_err(io_error)?,
            Writer::Tar(tar) => tar.into_inner().map_err(io_error)?,
            Writer::TarGz(tar) => tar
                .into_inner()
                .and_then(|encoder| encoder.finish())
                .map_err(io_error)?,
            Writer::TarBz2(tar) => tar
                .into_inner()
                .and_then(|encoder| encoder.finish())
                .map_err(io_error)?,
            Writer::TarXz(tar) => tar
                .into_inner()
                .map_err(io_error)?
                .finish()
                .map_err(io_error)?,
            Writer::SevenZ(seven) => seven.finish().map_err(io_error)?,
        };
        out.into_inner().map_err(|e| io_error(e.error()))
    }
}

fn append_tar<W: Write>(
    tar: &mut tar::Builder<W>,
    entry: &Entry,
    kind: tar::EntryType,
    default_mode: u32,
    name: &str,
) -> Result<()> {
    let mut header = Sink::tar_header(entry, kind, default_mode, 0);
    tar.append_data(&mut header, name, std::io::empty())
        .map_err(io_error)
}

fn link_tar<W: Write>(
    tar: &mut tar::Builder<W>,
    entry: &Entry,
    name: &str,
    target: &str,
) -> Result<()> {
    let mut header = Sink::tar_header(entry, tar::EntryType::Symlink, 0o777, 0);
    tar.append_link(&mut header, name, target).map_err(io_error)
}

fn file_tar<W: Write>(
    tar: &mut tar::Builder<W>,
    entry: &Entry,
    name: &str,
    size: u64,
    data: &mut dyn Read,
) -> Result<()> {
    let mut header = Sink::tar_header(entry, tar::EntryType::Regular, 0o644, size);
    tar.append_data(&mut header, name, data).map_err(io_error)
}

// `Seek` is needed by the zip and 7z writers on the buffered staging file.
const _: fn() = || {
    fn needs_seek<T: Seek + Write>() {}
    needs_seek::<Staged>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::ArchiveVfs;
    use crate::ops::ProgressSink;
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const MTIME: u64 = 1_700_000_000; // 2023-11-14, even second

    fn big() -> Vec<u8> {
        (0..300_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect()
    }

    fn make_tree(root: &Path) {
        fs::create_dir_all(root.join("dir/nested")).unwrap();
        fs::create_dir(root.join("empty_dir")).unwrap();
        fs::create_dir_all(root.join("\u{00fc}n\u{00ef}")).unwrap();
        fs::write(root.join("hello.txt"), b"hello").unwrap();
        fs::write(root.join("empty.txt"), b"").unwrap();
        fs::write(root.join("dir/nested/deep.txt"), b"deep").unwrap();
        fs::write(
            root.join("\u{00fc}n\u{00ef}/\u{00f6}.txt"),
            "uni\u{00e4}".as_bytes(),
        )
        .unwrap();
        fs::write(root.join("big.bin"), big()).unwrap();
        let long = "l".repeat(120);
        fs::write(root.join("dir").join(format!("{long}.txt")), b"long").unwrap();
        fs::write(root.join("script.sh"), b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join("script.sh"), fs::Permissions::from_mode(0o4750))
                .unwrap();
            std::os::unix::fs::symlink("hello.txt", root.join("link")).unwrap();
            std::os::unix::fs::symlink("../../etc/passwd", root.join("dir/outside")).unwrap();
        }
        for file in ["hello.txt", "big.bin", "script.sh"] {
            let handle = fs::File::options()
                .write(true)
                .open(root.join(file))
                .unwrap();
            handle
                .set_modified(UNIX_EPOCH + Duration::from_secs(MTIME))
                .unwrap();
        }
    }

    fn sources_of(root: &Path) -> Vec<VPath> {
        let mut names: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        names.sort();
        names.iter().map(|p| VPath::local(p).unwrap()).collect()
    }

    /// path -> (kind, content-or-link-target, mode, modified secs)
    type Snapshot = BTreeMap<String, (EntryKind, Vec<u8>, Option<u32>, Option<u64>)>;

    fn snapshot_vfs(vfs: &dyn Vfs, dir: &VPath, prefix: &str, out: &mut Snapshot) {
        for entry in vfs.list(dir).unwrap() {
            let path = dir.join_entry(&entry.name).unwrap();
            let name = format!("{prefix}/{}", entry.name);
            let content = match entry.kind {
                EntryKind::File => {
                    let mut data = Vec::new();
                    vfs.open_read(&path)
                        .unwrap()
                        .read_to_end(&mut data)
                        .unwrap();
                    data
                }
                EntryKind::Symlink => vfs.read_link(&path).unwrap().into_bytes(),
                EntryKind::Dir => Vec::new(),
            };
            let modified = (entry.kind == EntryKind::File)
                .then(|| unix_seconds(entry.modified))
                .flatten();
            let mode = entry.mode.map(|m| m & 0o7777);
            out.insert(name.clone(), (entry.kind, content, mode, modified));
            if entry.kind == EntryKind::Dir {
                snapshot_vfs(vfs, &path, &name, out);
            }
        }
    }

    fn formats() -> Vec<(Format, &'static str)> {
        vec![
            (Format::Zip, "a.zip"),
            (Format::Tar(TarCompression::None), "a.tar"),
            (Format::Tar(TarCompression::Gzip), "a.tar.gz"),
            (Format::Tar(TarCompression::Bzip2), "a.tar.bz2"),
            (Format::Tar(TarCompression::Xz), "a.tar.xz"),
            (Format::SevenZ, "a.7z"),
        ]
    }

    #[test]
    fn roundtrip_for_every_format_keeps_content_links_modes_and_times() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        make_tree(&src);
        let mut expected = Snapshot::new();
        let src_root = VPath::local(&src).unwrap();
        snapshot_vfs(&LocalVfs, &src_root, "", &mut expected);
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();
        for (format, name) in formats() {
            let target = out.join(name);
            let report = pack(
                &LocalVfs,
                &sources_of(&src),
                &VPath::local(&target).unwrap(),
                format,
                &JobHandle::silent(),
            );
            assert!(report.failed.is_empty(), "{name}: {:?}", report.failed);
            assert_eq!(report.done.len(), sources_of(&src).len(), "{name}");
            let archive = ArchiveVfs::open_local(&target).unwrap();
            assert_eq!(archive.format(), format, "{name}");
            let mut actual = Snapshot::new();
            snapshot_vfs(&archive, &archive.root(), "", &mut actual);
            assert_eq!(
                actual.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>(),
                "{name}: names"
            );
            for (path, (kind, content, mode, modified)) in &expected {
                let (a_kind, a_content, a_mode, a_modified) = &actual[path];
                assert_eq!(a_kind, kind, "{name}: kind of {path}");
                assert_eq!(a_content, content, "{name}: content of {path}");
                if *kind == EntryKind::File && path == "/script.sh" {
                    // setuid is never stored, the permission bits are
                    assert_eq!(*a_mode, Some(0o750), "{name}: mode");
                    assert_eq!(*mode, Some(0o4750));
                }
                if *kind == EntryKind::File && path == "/hello.txt" {
                    let delta = a_modified.unwrap().abs_diff(modified.unwrap());
                    assert!(delta <= 2, "{name}: time of {path}: {a_modified:?}");
                }
            }
            assert_eq!(actual["/link"].0, EntryKind::Symlink, "{name}");
            assert_eq!(actual["/link"].1, b"hello.txt", "{name}");
        }
        // sources were only read
        let mut after = Snapshot::new();
        snapshot_vfs(&LocalVfs, &src_root, "", &mut after);
        assert_eq!(after, expected);
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn an_existing_archive_is_never_touched() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        make_tree(&src);
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();
        for (format, name) in formats() {
            let target = out.join(name);
            fs::write(&target, b"precious").unwrap();
            let report = pack(
                &LocalVfs,
                &sources_of(&src),
                &VPath::local(&target).unwrap(),
                format,
                &JobHandle::silent(),
            );
            assert!(matches!(report.failed[0].1, Error::Exists(_)), "{name}");
            assert!(report.done.is_empty());
            assert_eq!(fs::read(&target).unwrap(), b"precious", "{name}");
        }
        assert_eq!(
            leftovers(&out).len(),
            formats().len(),
            "{:?}",
            leftovers(&out)
        );
        // a dangling link at the destination counts as existing, too
        #[cfg(unix)]
        {
            let link = out.join("dangling.zip");
            std::os::unix::fs::symlink("nowhere", &link).unwrap();
            let report = pack(
                &LocalVfs,
                &sources_of(&src),
                &VPath::local(&link).unwrap(),
                Format::Zip,
                &JobHandle::silent(),
            );
            assert!(matches!(report.failed[0].1, Error::Exists(_)));
            assert!(!out.join("nowhere").exists());
        }
    }

    struct CancelSoon(Mutex<Option<JobHandle>>, Mutex<u32>);
    impl ProgressSink for CancelSoon {
        fn event(&self, event: ProgressEvent) {
            if matches!(event, ProgressEvent::BytesCopied { .. }) {
                let mut seen = self.1.lock().unwrap();
                *seen += 1;
                if *seen >= 3 {
                    if let Some(job) = self.0.lock().unwrap().as_ref() {
                        job.cancel();
                    }
                }
            }
        }
    }

    #[test]
    fn cancel_leaves_neither_archive_nor_staging_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        make_tree(&src);
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();
        for (format, name) in formats() {
            let sink = Arc::new(CancelSoon(Mutex::new(None), Mutex::new(0)));
            let job = JobHandle::new(sink.clone());
            *sink.0.lock().unwrap() = Some(job.clone());
            let report = pack(
                &LocalVfs,
                &sources_of(&src),
                &VPath::local(out.join(name)).unwrap(),
                format,
                &job,
            );
            assert!(report.cancelled, "{name}");
            assert!(report.done.is_empty(), "{name}");
            assert!(leftovers(&out).is_empty(), "{name}: {:?}", leftovers(&out));
        }
        // cancelled before the first block: nothing is created either
        let job = JobHandle::silent();
        job.cancel();
        let report = pack(
            &LocalVfs,
            &sources_of(&src),
            &VPath::local(out.join("x.zip")).unwrap(),
            Format::Zip,
            &job,
        );
        assert!(report.cancelled);
        assert!(leftovers(&out).is_empty());
    }

    #[test]
    fn destination_inside_a_source_is_refused_and_nothing_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        make_tree(&src);
        // directly in the source, in a sub folder, and through a link
        fs::create_dir(src.join("dir/inner")).unwrap();
        let mut targets = vec![src.join("a.zip"), src.join("dir/inner/a.zip")];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(src.join("dir"), tmp.path().join("alias")).unwrap();
            targets.push(tmp.path().join("alias/a.zip"));
        }
        for target in targets {
            let report = pack(
                &LocalVfs,
                &[VPath::local(&src).unwrap()],
                &VPath::local(&target).unwrap(),
                Format::Zip,
                &JobHandle::silent(),
            );
            assert!(
                matches!(report.failed[0].1, Error::DestinationInsideSource),
                "{target:?}: {:?}",
                report.failed
            );
            assert!(!target.exists());
        }
        let found: Vec<_> = leftovers(&src.join("dir"));
        assert!(found.iter().all(|n| !n.contains("part")), "{found:?}");
        // a file source next to the archive is fine
        let ok = pack(
            &LocalVfs,
            &[VPath::local(src.join("hello.txt")).unwrap()],
            &VPath::local(src.join("single.zip")).unwrap(),
            Format::Zip,
            &JobHandle::silent(),
        );
        assert!(ok.failed.is_empty(), "{:?}", ok.failed);
    }

    #[test]
    fn rar_bad_names_and_duplicate_names_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        make_tree(&src);
        let sources = sources_of(&src);
        let job = JobHandle::silent();
        let report = pack(
            &LocalVfs,
            &sources,
            &VPath::local(tmp.path().join("a.rar")).unwrap(),
            Format::Rar,
            &job,
        );
        assert_eq!(report.failed.len(), sources.len());
        assert!(!tmp.path().join("a.rar").exists());
        let report = pack(
            &LocalVfs,
            &sources,
            &VPath::local(tmp.path().join(" ")).unwrap(),
            Format::Zip,
            &job,
        );
        assert!(matches!(report.failed[0].1, Error::InvalidName(_)));
        let report = pack(
            &LocalVfs,
            &sources,
            &VPath::local(tmp.path().join("no/such/dir/a.zip")).unwrap(),
            Format::Zip,
            &job,
        );
        assert!(!report.failed.is_empty());
        // the same name twice: second is refused, archive still made from the first
        let a = VPath::local(src.join("hello.txt")).unwrap();
        let report = pack(
            &LocalVfs,
            &[a.clone(), a],
            &VPath::local(tmp.path().join("d.zip")).unwrap(),
            Format::Zip,
            &job,
        );
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.done.len(), 1);
        assert_eq!(
            ArchiveVfs::open_local(tmp.path().join("d.zip"))
                .unwrap()
                .node_count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_reported_and_the_rest_is_packed() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("ok.txt"), b"ok").unwrap();
        fs::write(src.join("secret.txt"), b"secret").unwrap();
        fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(src.join("secret.txt")).is_ok() {
            return; // running as root: nothing to test
        }
        let report = pack(
            &LocalVfs,
            &[VPath::local(&src).unwrap()],
            &VPath::local(tmp.path().join("a.tar.gz")).unwrap(),
            Format::Tar(TarCompression::Gzip),
            &JobHandle::silent(),
        );
        assert_eq!(report.failed.len(), 1);
        assert!(
            report.done.is_empty(),
            "a source with a failed part is not 'done'"
        );
        let archive = ArchiveVfs::open_local(tmp.path().join("a.tar.gz")).unwrap();
        let listing: Vec<String> = archive
            .list(&VPath::archive(archive.outer().clone(), "/src").unwrap())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(listing, vec!["ok.txt"]);
    }

    #[test]
    fn packing_from_inside_an_archive_works_through_the_router() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        make_tree(&src);
        let first = tmp.path().join("one.zip");
        let report = pack(
            &LocalVfs,
            &[VPath::local(src.join("dir")).unwrap()],
            &VPath::local(&first).unwrap(),
            Format::Zip,
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty());
        let router = crate::VfsRouter::new();
        let inner = VPath::archive(VPath::local(&first).unwrap(), "/dir").unwrap();
        let vfs = router.vfs_for(&inner).unwrap();
        let second = tmp.path().join("two.tar.xz");
        let report = pack(
            &*vfs,
            &[inner],
            &VPath::local(&second).unwrap(),
            Format::Tar(TarCompression::Xz),
            &JobHandle::silent(),
        );
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let archive = ArchiveVfs::open_local(&second).unwrap();
        let mut got = Snapshot::new();
        snapshot_vfs(&archive, &archive.root(), "", &mut got);
        assert_eq!(got["/dir/nested/deep.txt"].1, b"deep");
    }
}
