//! 7Z via `sevenz-rust2`. The library decodes a whole block through a callback, so an entry
//! stream is produced by a worker thread that hands the target file's bytes over a bounded
//! channel; everything decoded on the way (solid archives) is counted.

use super::budget::{
    corrupt, map_io, Budget, CountSrc, Counted, ReadCtx, Stats, MSG_CORRUPT, MSG_ENCRYPTED,
    MSG_INDEX_BYTES,
};
use super::index::{ArchiveLimits, IndexBuilder, Loc, RawEntry, RawKind};
use super::Source;
use crate::error::{Error, Result};
use sevenz_rust2::{Archive, BlockDecoder, EncoderMethod, Password};
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use std::time::SystemTime;

const MSG_SPECIAL: &str = "Sonderdatei (nur Anzeige)";
const MSG_PANIC: &str = "Archiv beschädigt oder nicht lesbar (interner Parserfehler)";
const MAX_LINK_BYTES: usize = 4096;
const UNIX_EXTENSION: u32 = 0x8000;

#[derive(Clone, Debug)]
pub(crate) struct SevenLoc {
    file_index: usize,
    block: Option<usize>,
    size: u64,
}

pub(crate) struct SevenBackend {
    source: Source,
    archive: Arc<Archive>,
}

fn seven_error(error: sevenz_rust2::Error, path: &Path) -> Error {
    match error {
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => {
            Error::Archive(MSG_ENCRYPTED)
        }
        sevenz_rust2::Error::MaxMemLimited { .. } => Error::Archive(MSG_INDEX_BYTES),
        sevenz_rust2::Error::Io(e, _) | sevenz_rust2::Error::FileOpen(e, _) => map_io(e, path),
        _ => Error::Archive(MSG_CORRUPT),
    }
}

fn io_of(error: sevenz_rust2::Error) -> io::Error {
    match error {
        sevenz_rust2::Error::Io(e, _) => e,
        _ => corrupt(MSG_CORRUPT),
    }
}

struct ChannelReader {
    receiver: Receiver<io::Result<Vec<u8>>>,
    chunk: Vec<u8>,
    position: usize,
    cancel: Arc<AtomicBool>,
    expected: u64,
    delivered: u64,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.position >= self.chunk.len() {
            match self.receiver.recv() {
                Ok(Ok(chunk)) => {
                    self.chunk = chunk;
                    self.position = 0;
                }
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return if self.delivered == self.expected {
                        Ok(0)
                    } else {
                        Err(corrupt(MSG_CORRUPT))
                    };
                }
            }
        }
        let n = buf.len().min(self.chunk.len() - self.position);
        buf[..n].copy_from_slice(&self.chunk[self.position..self.position + n]);
        self.position += n;
        self.delivered += n as u64;
        Ok(n)
    }
}

impl Drop for ChannelReader {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Decodes the block of `target` and sends the target's bytes. Everything decoded is counted.
fn decode_block(
    source: &Source,
    archive: &Archive,
    block: usize,
    target: usize,
    ctx: &ReadCtx,
    cancel: &AtomicBool,
    send: &mut dyn FnMut(Vec<u8>) -> bool,
) -> io::Result<()> {
    let stats = Arc::new(Stats::default());
    let mut src = CountSrc {
        inner: source.open()?,
        stats: stats.clone(),
    };
    let password = Password::empty();
    let mut index = archive.stream_map.block_first_file_index[block];
    let mut failure: Option<io::Error> = None;
    let decoder = BlockDecoder::new(1, block, archive, &password, &mut src);
    let result = decoder.for_each_entries(&mut |_entry, reader| {
        let this = index;
        index += 1;
        let mut counted = Counted::for_entry(reader, ctx, stats.clone(), None);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            match counted.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if this == target && !send(buf[..n].to_vec()) {
                        return Ok(false);
                    }
                }
                Err(error) => {
                    failure = Some(error);
                    return Ok(false);
                }
            }
        }
        Ok(this != target)
    });
    if let Some(error) = failure {
        return Err(error);
    }
    result.map(|_| ()).map_err(io_of)
}

impl SevenBackend {
    pub(crate) fn open_entry(
        &self,
        loc: &SevenLoc,
        ctx: &ReadCtx,
    ) -> io::Result<Box<dyn Read + Send>> {
        let Some(block) = loc.block else {
            return Ok(Box::new(io::empty()));
        };
        let (sender, receiver) = sync_channel::<io::Result<Vec<u8>>>(4);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let source = self.source.clone();
        let archive = self.archive.clone();
        let ctx = ReadCtx {
            job: ctx.job.clone(),
            op: ctx.op.clone(),
            limits: ctx.limits.clone(),
        };
        let target = loc.file_index;
        std::thread::Builder::new()
            .name("dualfile-7z-decode".into())
            .stack_size(8 << 20)
            .spawn(move || {
                let data_sender = sender.clone();
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    decode_block(
                        &source,
                        &archive,
                        block,
                        target,
                        &ctx,
                        &worker_cancel,
                        &mut |chunk| data_sender.send(Ok(chunk)).is_ok(),
                    )
                }));
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        let _ = sender.send(Err(error));
                    }
                    Err(_) => {
                        let _ = sender.send(Err(corrupt(MSG_PANIC)));
                    }
                }
            })?;
        Ok(Box::new(ChannelReader {
            receiver,
            chunk: Vec::new(),
            position: 0,
            cancel,
            expected: loc.size,
            delivered: 0,
        }))
    }
}

/// Reads the small contents of link entries (their targets), block by block.
fn read_link_targets(
    source: &Source,
    archive: &Archive,
    wanted: &[usize],
    budget: &Budget,
) -> HashMap<usize, String> {
    let mut found = HashMap::new();
    let mut by_block: HashMap<usize, Vec<usize>> = HashMap::new();
    for file in wanted {
        if let Some(block) = archive.stream_map.file_block_index[*file] {
            by_block.entry(block).or_default().push(*file);
        }
    }
    let Ok(file) = source.open() else {
        return found;
    };
    let mut src = CountSrc {
        inner: file,
        stats: Arc::new(Stats::default()),
    };
    let password = Password::empty();
    for (block, files) in by_block {
        let stats = Arc::new(Stats::default());
        let mut index = archive.stream_map.block_first_file_index[block];
        let last = files.iter().copied().max().unwrap_or(0);
        let mut ok = true;
        let _ = BlockDecoder::new(1, block, archive, &password, &mut src).for_each_entries(
            &mut |_entry, reader| {
                let this = index;
                index += 1;
                let mut counted =
                    Counted::for_index(reader, vec![budget.clone()], stats.clone(), None);
                let mut data = Vec::new();
                let read = (&mut counted)
                    .take(MAX_LINK_BYTES as u64 + 1)
                    .read_to_end(&mut data);
                let drained = io::copy(&mut counted, &mut io::sink());
                if read.is_err() || drained.is_err() {
                    ok = false;
                    return Ok(false);
                }
                if files.contains(&this) && data.len() <= MAX_LINK_BYTES {
                    found.insert(this, String::from_utf8_lossy(&data).into_owned());
                }
                Ok(this < last)
            },
        );
        if !ok {
            break;
        }
    }
    found
}

// Only the encoded-header descriptor is parsed here: decompression must not start until
// every declared output size fits the directory budget. Counts are bounded by descriptor
// bytes, so this check cannot itself allocate from untrusted stream counts.
struct HeaderProbe<'a> {
    bytes: &'a [u8],
    bound: u64,
}

impl HeaderProbe<'_> {
    fn skip(&mut self, n: u64) -> io::Result<()> {
        if n > self.bytes.len() as u64 {
            return Err(corrupt(MSG_CORRUPT));
        }
        self.bytes = &self.bytes[n as usize..];
        Ok(())
    }

    fn byte(&mut self) -> io::Result<u8> {
        let value = *self.bytes.first().ok_or_else(|| corrupt(MSG_CORRUPT))?;
        self.skip(1)?;
        Ok(value)
    }

    fn number(&mut self) -> io::Result<u64> {
        let first = self.byte()?;
        let mut value = 0;
        for i in 0..8 {
            let mask = 0x80 >> i;
            if first & mask == 0 {
                return Ok(value | (u64::from(first & (mask - 1)) << (8 * i)));
            }
            value |= u64::from(self.byte()?) << (8 * i);
        }
        Ok(value)
    }

    fn count(&mut self) -> io::Result<u64> {
        let n = self.number()?;
        if n > self.bound {
            return Err(corrupt(MSG_CORRUPT));
        }
        Ok(n)
    }

    fn expect(&mut self, value: u8) -> io::Result<()> {
        if self.byte()? != value {
            return Err(corrupt(MSG_CORRUPT));
        }
        Ok(())
    }

    fn digests(&mut self, count: u64) -> io::Result<()> {
        let defined = if self.byte()? != 0 {
            count
        } else {
            let mut defined = 0;
            for offset in (0..count).step_by(8) {
                let bits = self.byte()?;
                for bit in 0..(count - offset).min(8) {
                    defined += u64::from(bits & (0x80 >> bit) != 0);
                }
            }
            defined
        };
        self.skip(defined * 4)
    }

    fn encoded_sizes(&mut self, max: u64) -> io::Result<()> {
        let mut id = self.byte()?;
        if id == 0x06 {
            // PackInfo
            self.number()?;
            let count = self.count()?;
            id = self.byte()?;
            if id == 0x09 {
                // Size
                for _ in 0..count {
                    self.number()?;
                }
                id = self.byte()?;
            }
            if id == 0x0a {
                // CRC
                self.digests(count)?;
                id = self.byte()?;
            }
            if id != 0 {
                return Err(corrupt(MSG_CORRUPT));
            }
            id = self.byte()?;
        }
        if id != 0x07 {
            // UnpackInfo
            return Err(corrupt(MSG_CORRUPT));
        }
        self.expect(0x0b)?; // Folder
        let folders = self.count()?;
        self.expect(0)?; // External folders are unsupported by the decoder.
        let mut outputs = 0u64;
        for _ in 0..folders {
            let coders = self.count()?;
            let (mut input, mut output) = (0u64, 0u64);
            for _ in 0..coders {
                let flags = self.byte()?;
                self.skip(u64::from(flags & 0x0f))?;
                let (ins, outs) = if flags & 0x10 == 0 {
                    (1, 1)
                } else {
                    (self.count()?, self.count()?)
                };
                input = input
                    .checked_add(ins)
                    .filter(|v| *v <= self.bound)
                    .ok_or_else(|| corrupt(MSG_CORRUPT))?;
                output = output
                    .checked_add(outs)
                    .filter(|v| *v <= self.bound)
                    .ok_or_else(|| corrupt(MSG_CORRUPT))?;
                if flags & 0x20 != 0 {
                    let size = self.count()?;
                    self.skip(size)?;
                }
                if flags & 0x80 != 0 {
                    return Err(corrupt(MSG_CORRUPT));
                }
            }
            let pairs = output.checked_sub(1).ok_or_else(|| corrupt(MSG_CORRUPT))?;
            for _ in 0..pairs {
                self.number()?;
                self.number()?;
            }
            let packed = input
                .checked_sub(pairs)
                .ok_or_else(|| corrupt(MSG_CORRUPT))?;
            if packed != 1 {
                for _ in 0..packed {
                    self.number()?;
                }
            }
            outputs = outputs
                .checked_add(output)
                .filter(|v| *v <= self.bound)
                .ok_or_else(|| corrupt(MSG_CORRUPT))?;
        }
        self.expect(0x0c)?; // CodersUnpackSize
        for _ in 0..outputs {
            if self.number()? > max {
                return Err(super::budget::limit(MSG_INDEX_BYTES));
            }
        }
        Ok(())
    }
}

fn check_header_size<R: Read + Seek>(reader: &mut R, max: u64) -> io::Result<()> {
    let len = reader.seek(SeekFrom::End(0))?;
    reader.rewind()?;
    let mut start = [0u8; 32];
    reader.read_exact(&mut start)?;
    if &start[..6] != b"7z\xbc\xaf\x27\x1c"
        || start[6] != 0
        || crc32fast::hash(&start[12..]) != u32::from_le_bytes(start[8..12].try_into().unwrap())
    {
        return Err(corrupt(MSG_CORRUPT));
    }
    // Do not let recovery mode discover an unchecked descriptor elsewhere in the file.
    if start[8..].iter().all(|b| *b == 0) {
        return Err(corrupt(MSG_CORRUPT));
    }
    let offset = u64::from_le_bytes(start[12..20].try_into().unwrap());
    let size = u64::from_le_bytes(start[20..28].try_into().unwrap());
    if size > max {
        return Err(super::budget::limit(MSG_INDEX_BYTES));
    }
    let pos = 32u64
        .checked_add(offset)
        .ok_or_else(|| corrupt(MSG_CORRUPT))?;
    if pos.checked_add(size).is_none_or(|end| end > len) {
        return Err(corrupt(MSG_CORRUPT));
    }
    reader.seek(SeekFrom::Start(pos))?;
    let mut descriptor = Vec::new();
    (&mut *reader).take(size).read_to_end(&mut descriptor)?;
    if descriptor.len() as u64 != size {
        return Err(corrupt(MSG_CORRUPT));
    }
    let mut probe = HeaderProbe {
        bytes: &descriptor,
        bound: size,
    };
    if probe.byte()? == 0x17 {
        probe.encoded_sizes(max)?;
    }
    reader.rewind()?;
    Ok(())
}

pub(crate) fn index(
    source: &Source,
    limits: &ArchiveLimits,
    budget: &Budget,
    builder: &mut IndexBuilder,
) -> Result<SevenBackend> {
    let path = Path::new("");
    let directory_budget = Budget::index(limits.index_dir_bytes, limits.index_time);
    let mut reader = Counted::for_index(
        source.open().map_err(|e| map_io(e, path))?,
        vec![budget.clone(), directory_budget],
        Arc::new(Stats::default()),
        None,
    );
    check_header_size(&mut reader, limits.index_dir_bytes).map_err(|e| map_io(e, path))?;
    let archive =
        Archive::read(&mut reader, &Password::empty()).map_err(|e| seven_error(e, path))?;
    if archive.files.len() > limits.max_entries {
        return Err(Error::Archive(MSG_INDEX_BYTES));
    }
    let encrypted_block = |block: usize| {
        archive.blocks[block]
            .coders
            .iter()
            .any(|coder| coder.encoder_method_id() == EncoderMethod::ID_AES256_SHA256)
    };
    let unix_mode =
        |attributes: u32| (attributes & UNIX_EXTENSION != 0).then_some(attributes >> 16);
    // Pass 1: classify every entry and find the links whose target has to be read.
    struct Planned {
        kind: RawKind,
        mode: Option<u32>,
        unreadable: Option<&'static str>,
    }
    let mut planned: Vec<Option<Planned>> = Vec::with_capacity(archive.files.len());
    let mut link_files = Vec::new();
    for (number, file) in archive.files.iter().enumerate() {
        if file.is_anti_item {
            planned.push(None);
            continue;
        }
        let mode = file
            .has_windows_attributes
            .then(|| unix_mode(file.windows_attributes))
            .flatten();
        let file_type = mode.map(|mode| mode & 0o170_000);
        let kind = if file.is_directory {
            RawKind::Dir
        } else if file_type == Some(0o120_000) {
            RawKind::Symlink
        } else if matches!(
            file_type,
            Some(0o010_000 | 0o020_000 | 0o060_000 | 0o140_000)
        ) {
            RawKind::Special(MSG_SPECIAL)
        } else {
            RawKind::File
        };
        let block = archive.stream_map.file_block_index[number];
        let unreadable = block
            .filter(|block| encrypted_block(*block))
            .map(|_| MSG_ENCRYPTED);
        if matches!(kind, RawKind::Symlink) && unreadable.is_none() && file.has_stream {
            link_files.push(number);
        }
        planned.push(Some(Planned {
            kind,
            mode,
            unreadable,
        }));
    }
    let targets = read_link_targets(source, &archive, &link_files, budget);
    // Pass 2: add to the index.
    for (number, (file, plan)) in archive.files.iter().zip(planned).enumerate() {
        let Some(plan) = plan else { continue };
        let modified = file
            .has_last_modified_date
            .then(|| Some(SystemTime::from(file.last_modified_date)))
            .flatten();
        builder.add(RawEntry {
            path: file.name.clone(),
            kind: plan.kind,
            size: file.size,
            modified,
            mode: plan.mode,
            link_target: targets.get(&number).cloned(),
            loc: Loc::SevenZ(SevenLoc {
                file_index: number,
                block: archive.stream_map.file_block_index[number],
                size: file.size,
            }),
            unreadable: plan.unreadable,
        })?;
    }
    Ok(SevenBackend {
        source: source.clone(),
        archive: Arc::new(archive),
    })
}
