//! TAR, TAR.GZ, TAR.BZ2, TAR.XZ. A compressed tar has no random access: an entry is reached
//! by decompressing from the start and skipping forward; every skipped byte is counted.

use super::budget::{
    corrupt, map_io, Budget, CountSrc, Counted, ReadCtx, Stats, MSG_CORRUPT, MSG_INDEX_BYTES,
};
use super::index::{ArchiveLimits, IndexBuilder, Loc, RawEntry, RawKind};
use super::zipfmt::unix_time;
use super::{Source, TarCompression};
use crate::error::{Error, Result};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const XZ_MEMORY_LIMIT_KB: u32 = 512 * 1024;
const MSG_SPECIAL: &str = "Sonderdatei (nur Anzeige)";
const MSG_HARDLINK: &str = "Hartlink (nur Anzeige)";
const MSG_SPARSE: &str = "Sparse-Datei (nur Anzeige)";

#[derive(Clone, Debug)]
pub(crate) struct TarLoc {
    offset: u64,
    size: u64,
}

pub(crate) struct TarBackend {
    source: Source,
    compression: TarCompression,
}

fn decoder(
    source: &Source,
    compression: TarCompression,
    stats: Arc<Stats>,
) -> io::Result<Box<dyn Read + Send>> {
    let file = CountSrc {
        inner: BufReader::new(source.open()?),
        stats,
    };
    Ok(match compression {
        TarCompression::None => Box::new(file),
        TarCompression::Gzip => Box::new(flate2::read::MultiGzDecoder::new(file)),
        TarCompression::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(file)),
        TarCompression::Xz => Box::new(lzma_rust2::XzReader::new_mem_limit(
            file,
            true,
            XZ_MEMORY_LIMIT_KB,
        )),
    })
}

/// The first 512 decompressed bytes (for format detection).
pub(crate) fn first_block(source: &Source, compression: TarCompression) -> io::Result<Vec<u8>> {
    let stream = decoder(source, compression, Arc::new(Stats::default()))?;
    let mut block = Vec::new();
    stream.take(512).read_to_end(&mut block)?;
    Ok(block)
}

struct Exact<R> {
    inner: R,
    remaining: u64,
}
impl<R: Read> Read for Exact<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let cap = buf.len().min(self.remaining as usize);
        let n = self.inner.read(&mut buf[..cap])?;
        if n == 0 {
            return Err(corrupt(MSG_CORRUPT));
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}

impl TarBackend {
    pub(crate) fn open_entry(
        &self,
        loc: &TarLoc,
        ctx: &ReadCtx,
    ) -> io::Result<Box<dyn Read + Send>> {
        let stats = Arc::new(Stats::default());
        if self.compression == TarCompression::None {
            let mut file = self.source.open()?;
            file.seek(SeekFrom::Start(loc.offset))?;
            // Charge the bytes read to stats.compressed so the ratio guard sees ~1:1. A plain
            // tar is never a compression bomb; without this feed compressed stays 0 and the
            // guard falsely trips on any entry past ratio_min_bytes.
            let file = CountSrc {
                inner: file,
                stats: stats.clone(),
            };
            let counted = Counted::for_entry(file, ctx, stats, None);
            return Ok(Box::new(Exact {
                inner: counted,
                remaining: loc.size,
            }));
        }
        let stream = decoder(&self.source, self.compression, stats.clone())?;
        let mut counted = Counted::for_entry(stream, ctx, stats, None);
        let skipped = io::copy(&mut (&mut counted).take(loc.offset), &mut io::sink())?;
        if skipped != loc.offset {
            return Err(corrupt(MSG_CORRUPT));
        }
        Ok(Box::new(Exact {
            inner: counted,
            remaining: loc.size,
        }))
    }
}

fn collect<'a, R: Read + 'a>(
    entries: impl Iterator<Item = io::Result<tar::Entry<'a, R>>>,
    builder: &mut IndexBuilder,
    max_end: &mut u64,
) -> Result<()> {
    let path = Path::new("");
    for item in entries {
        let entry = item.map_err(|e| map_io(e, path))?;
        use tar::EntryType as T;
        let kind = match entry.header().entry_type() {
            T::Regular | T::Continuous => RawKind::File,
            T::Directory => RawKind::Dir,
            T::Symlink => RawKind::Symlink,
            T::Link => RawKind::Special(MSG_HARDLINK),
            T::Char | T::Block | T::Fifo => RawKind::Special(MSG_SPECIAL),
            T::GNUSparse => RawKind::Special(MSG_SPARSE),
            T::XGlobalHeader | T::XHeader | T::GNULongName | T::GNULongLink => continue,
            _ => RawKind::Special(MSG_SPECIAL),
        };
        let size = entry.size();
        let offset = entry.raw_file_position();
        *max_end = (*max_end).max(offset.saturating_add(size));
        let link_target = entry
            .link_name_bytes()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        builder.add(RawEntry {
            path: String::from_utf8_lossy(&entry.path_bytes()).into_owned(),
            kind,
            size,
            modified: entry
                .header()
                .mtime()
                .ok()
                .and_then(|seconds| unix_time(seconds as i64)),
            mode: entry.header().mode().ok(),
            link_target: matches!(kind, RawKind::Symlink)
                .then_some(link_target)
                .flatten(),
            loc: Loc::Tar(TarLoc { offset, size }),
            unreadable: None,
        })?;
    }
    Ok(())
}

/// Validate each header before exposing it to tar, so extension payloads cannot be
/// buffered before their size is checked. Only a header and bounded PAX data are retained.
struct MetadataGuard<R> {
    inner: R,
    max: usize,
    block: [u8; 512],
    position: usize,
    remaining: u64,
    pax_data: Option<Vec<u8>>,
    pax_remaining: u64,
    pax_size: Option<u64>,
    sparse: bool,
    ended: bool,
}

impl<R: Read> MetadataGuard<R> {
    fn new(inner: R, max: usize) -> Self {
        Self {
            inner,
            max,
            block: [0; 512],
            position: 512,
            remaining: 0,
            pax_data: None,
            pax_remaining: 0,
            pax_size: None,
            sparse: false,
            ended: false,
        }
    }

    fn header(&mut self) -> io::Result<()> {
        let header = tar::Header::from_byte_slice(&self.block);
        let kind = header.entry_type().as_byte();
        let metadata = matches!(kind, b'L' | b'K' | b'x' | b'g');
        let mut size = header.entry_size()?;
        if metadata && size > self.max as u64 {
            return Err(super::budget::limit(MSG_INDEX_BYTES));
        }
        if !metadata {
            size = self.pax_size.take().unwrap_or(size);
        }
        let recognized = header.as_gnu().is_some() || header.as_ustar().is_some();
        if recognized && kind == b'x' {
            self.pax_data = Some(Vec::new());
            self.pax_remaining = size;
            self.pax_size = None;
        } else if !(recognized && matches!(kind, b'L' | b'K')) {
            self.pax_size = None;
        }
        self.sparse = kind == b'S' && header.as_gnu().is_some_and(|gnu| gnu.is_extended());
        self.remaining = size.checked_add(511).ok_or_else(|| corrupt(MSG_CORRUPT))? & !511;
        Ok(())
    }

    fn finish_pax(&mut self) {
        if let Some(data) = self.pax_data.take() {
            // Match tar's first size property, including its malformed-data fallback.
            for extension in tar::PaxExtensions::new(&data) {
                let Ok(extension) = extension else { break };
                if extension.key() == Ok("size") {
                    self.pax_size = extension.value().ok().and_then(|v| v.parse::<u64>().ok());
                    break;
                }
            }
        }
    }
}

impl<R: Read> Read for MetadataGuard<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.position == 512 {
            if self.sparse {
                let mut extension = tar::GnuExtSparseHeader::new();
                self.inner.read_exact(extension.as_mut_bytes())?;
                self.sparse = extension.is_extended();
                self.block.copy_from_slice(extension.as_bytes());
                self.position = 0;
            } else if self.remaining != 0 {
                let cap = self.remaining.min(buf.len() as u64) as usize;
                let n = self.inner.read(&mut buf[..cap])?;
                if n == 0 {
                    return Err(corrupt(MSG_CORRUPT));
                }
                self.remaining -= n as u64;
                if let Some(data) = &mut self.pax_data {
                    let payload = self.pax_remaining.min(n as u64) as usize;
                    data.extend_from_slice(&buf[..payload]);
                    self.pax_remaining -= payload as u64;
                }
                return Ok(n);
            } else {
                self.finish_pax();
                // After the end marker, pass trailers through for decoder checksum checks.
                if self.ended {
                    return self.inner.read(buf);
                }
                if self.inner.read(&mut self.block[..1])? == 0 {
                    return Ok(0);
                }
                self.inner.read_exact(&mut self.block[1..])?;
                self.ended = self.block.iter().all(|b| *b == 0);
                if !self.ended {
                    self.header()?;
                }
                self.position = 0;
            }
        }
        let n = buf.len().min(512 - self.position);
        buf[..n].copy_from_slice(&self.block[self.position..self.position + n]);
        self.position += n;
        Ok(n)
    }
}

/// Uncompressed archives can skip file data without scanning it; use the same guard
/// for headers and PAX payloads while preserving seek-based indexing of large files.
fn check_metadata_sizes<R: Read + Seek>(reader: R, max: usize) -> io::Result<()> {
    let mut guard = MetadataGuard::new(reader, max);
    let mut block = [0; 512];
    while guard.read(&mut block)? != 0 {
        if guard.ended {
            return Ok(());
        }
        if guard.position == 512 && !guard.sparse && guard.pax_data.is_none() {
            let offset = i64::try_from(guard.remaining).map_err(|_| corrupt(MSG_CORRUPT))?;
            guard.inner.seek(SeekFrom::Current(offset))?;
            guard.remaining = 0;
        }
    }
    Ok(())
}

pub(crate) fn index(
    source: &Source,
    compression: TarCompression,
    limits: &ArchiveLimits,
    budget: &Budget,
    builder: &mut IndexBuilder,
) -> Result<TarBackend> {
    let path = Path::new("");
    let mut max_end = 0u64;
    if compression == TarCompression::None {
        let mut file = source.open().map_err(|e| map_io(e, path))?;
        check_metadata_sizes(&mut file, limits.max_entry_meta).map_err(|e| map_io(e, path))?;
        file.rewind().map_err(|e| map_io(e, path))?;
        let mut archive = tar::Archive::new(file);
        let entries = archive.entries_with_seek().map_err(|e| map_io(e, path))?;
        collect(entries, builder, &mut max_end)?;
        if max_end > source.len().map_err(|e| map_io(e, path))? {
            return Err(Error::Archive(MSG_CORRUPT));
        }
    } else {
        let stats = Arc::new(Stats::default());
        let stream = decoder(source, compression, stats.clone()).map_err(|e| map_io(e, path))?;
        let counted = Counted::for_index(
            stream,
            vec![budget.clone()],
            stats.clone(),
            Some((limits.ratio_min_bytes, limits.ratio_max)),
        );
        let mut archive = tar::Archive::new(MetadataGuard::new(counted, limits.max_entry_meta));
        let entries = archive.entries().map_err(|e| map_io(e, path))?;
        collect(entries, builder, &mut max_end)?;
        // Read the stream to its end: the checksum trailer of gzip/bzip2/xz only counts
        // when it is reached, and it is what detects flipped bytes inside entry data.
        let mut rest = archive.into_inner();
        io::copy(&mut rest, &mut io::sink()).map_err(|e| map_io(e, path))?;
        if stats.produced.load(Ordering::Relaxed) < max_end {
            return Err(Error::Archive(MSG_CORRUPT));
        }
    }
    Ok(TarBackend {
        source: source.clone(),
        compression,
    })
}
