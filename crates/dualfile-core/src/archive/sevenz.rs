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

// Only the header descriptors are parsed here: nothing is decompressed or allocated from
// untrusted counts until every declared output size fits the directory budget, every coder
// fits the memory limit and the number of files fits `max_entries`. Counts are bounded by
// descriptor bytes, so this check cannot itself allocate from untrusted stream counts.
const MAX_CODER_MEMORY_KB: u64 = 512 * 1024;
const MAX_CODERS: u64 = 64;

/// Estimated decoder memory in KiB for one coder, `None` when the properties are not usable
/// (the library rejects those itself when it decodes).
fn coder_memory_kb(id: &[u8], props: &[u8]) -> Option<u64> {
    let word = || -> Option<u32> { Some(u32::from_le_bytes(props.get(1..5)?.try_into().ok()?)) };
    if id == EncoderMethod::ID_LZMA2 {
        let dict = lzma2_dict(props)?;
        let kb = dict >> 10;
        if kb > MAX_CODER_MEMORY_KB {
            return Some(kb);
        }
        Some(u64::from(lzma_rust2::lzma2_get_memory_usage(dict as u32)))
    } else if id == EncoderMethod::ID_LZMA {
        let dict = word()?;
        let kb = u64::from(dict >> 10);
        if kb > MAX_CODER_MEMORY_KB {
            return Some(kb);
        }
        lzma_rust2::lzma_get_memory_usage_by_props(dict, props[0])
            .ok()
            .map(u64::from)
    } else if id == EncoderMethod::ID_PPMD {
        Some(u64::from(word()? >> 10))
    } else {
        Some(0)
    }
}

/// LZMA2 dictionary size in bytes from its one property byte.
fn lzma2_dict(props: &[u8]) -> Option<u64> {
    let bits = u32::from(*props.first()?);
    match bits {
        0..=39 => Some(u64::from(2 | (bits & 1)) << (bits / 2 + 11)),
        40 => Some(0xFFFF_FFFF),
        _ => None,
    }
}

fn check_coder(id: &[u8], props: &[u8]) -> io::Result<()> {
    match coder_memory_kb(id, props) {
        Some(kb) if kb > MAX_CODER_MEMORY_KB => Err(super::budget::limit(MSG_INDEX_BYTES)),
        _ => Ok(()),
    }
}

enum Defined<'a> {
    None,
    All,
    Bits(&'a [u8]),
}

impl Defined<'_> {
    fn get(&self, index: u64) -> bool {
        match self {
            Defined::None => false,
            Defined::All => true,
            Defined::Bits(bits) => usize::try_from(index / 8)
                .ok()
                .and_then(|i| bits.get(i))
                .is_some_and(|b| b & (0x80 >> (index % 8)) != 0),
        }
    }
}

struct Streams<'a> {
    pack_pos: u64,
    pack_size: u64,
    folders: u64,
    /// Coders (id, properties) of the first folder.
    first: Vec<(&'a [u8], &'a [u8])>,
    /// First declared unpack size.
    unpack: u64,
}

struct HeaderProbe<'a> {
    bytes: &'a [u8],
    bound: u64,
}

impl<'a> HeaderProbe<'a> {
    fn skip(&mut self, n: u64) -> io::Result<()> {
        self.take(n).map(|_| ())
    }

    fn take(&mut self, n: u64) -> io::Result<&'a [u8]> {
        if n > self.bytes.len() as u64 {
            return Err(corrupt(MSG_CORRUPT));
        }
        let (head, tail) = self.bytes.split_at(n as usize);
        self.bytes = tail;
        Ok(head)
    }

    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
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

    fn digests(&mut self, count: u64) -> io::Result<Defined<'a>> {
        if self.byte()? != 0 {
            self.skip(count.checked_mul(4).ok_or_else(|| corrupt(MSG_CORRUPT))?)?;
            return Ok(Defined::All);
        }
        let bits = self.take(count.div_ceil(8))?;
        let mut defined = 0u64;
        for (i, byte) in bits.iter().enumerate() {
            let valid = (count - 8 * i as u64).min(8) as u32;
            defined += u64::from((byte & (0xFFu8 << (8 - valid))).count_ones());
        }
        self.skip(defined * 4)?;
        Ok(Defined::Bits(bits))
    }

    /// StreamsInfo up to and including its closing 0. `size_limit` caps every declared
    /// unpack size (used for the encoded header, not for file data).
    fn streams(&mut self, size_limit: Option<u64>) -> io::Result<Streams<'a>> {
        let mut out = Streams {
            pack_pos: 0,
            pack_size: 0,
            folders: 0,
            first: Vec::new(),
            unpack: 0,
        };
        let mut id = self.byte()?;
        if id == 0x06 {
            // PackInfo
            out.pack_pos = self.number()?;
            let count = self.count()?;
            id = self.byte()?;
            if id == 0x09 {
                // Size
                for i in 0..count {
                    let size = self.number()?;
                    if i == 0 {
                        out.pack_size = size;
                    }
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
        if id == 0 {
            return Ok(out);
        }
        if id != 0x07 {
            // UnpackInfo
            return Err(corrupt(MSG_CORRUPT));
        }
        self.expect(0x0b)?; // Folder
        let folders = self.count()?;
        out.folders = folders;
        self.expect(0)?; // External folders are unsupported by the decoder.
        let mut outputs = 0u64;
        for index in 0..folders {
            let coders = self.count()?;
            if coders > MAX_CODERS {
                return Err(corrupt(MSG_CORRUPT));
            }
            let (mut input, mut output) = (0u64, 0u64);
            for _ in 0..coders {
                let flags = self.byte()?;
                let coder_id = self.take(u64::from(flags & 0x0f))?;
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
                let mut props: &[u8] = &[];
                if flags & 0x20 != 0 {
                    let size = self.count()?;
                    props = self.take(size)?;
                }
                if flags & 0x80 != 0 {
                    return Err(corrupt(MSG_CORRUPT));
                }
                check_coder(coder_id, props)?;
                if index == 0 {
                    out.first.push((coder_id, props));
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
        for i in 0..outputs {
            let size = self.number()?;
            if size_limit.is_some_and(|max| size > max) {
                return Err(super::budget::limit(MSG_INDEX_BYTES));
            }
            if i == 0 {
                out.unpack = size;
            }
        }
        let mut crcs = Defined::None;
        let mut id = self.byte()?;
        if id == 0x0a {
            crcs = self.digests(folders)?;
            id = self.byte()?;
        }
        if id == 0x08 {
            self.substreams(folders, &crcs)?;
            id = self.byte()?;
        }
        if id != 0 {
            return Err(corrupt(MSG_CORRUPT));
        }
        Ok(out)
    }

    /// SubStreamsInfo (after its 0x08), including its closing 0.
    fn substreams(&mut self, folders: u64, crcs: &Defined) -> io::Result<()> {
        fn next(counts: &mut Option<HeaderProbe>) -> io::Result<u64> {
            counts.as_mut().map_or(Ok(1), HeaderProbe::count)
        }
        let mut id = self.byte()?;
        let region = self.bytes;
        let has_counts = id == 0x0d;
        if has_counts {
            for _ in 0..folders {
                self.count()?;
            }
            id = self.byte()?;
        }
        let bound = self.bound;
        let again = || {
            has_counts.then_some(HeaderProbe {
                bytes: region,
                bound,
            })
        };
        if id == 0x09 {
            let mut counts = again();
            for _ in 0..folders {
                for _ in 1..next(&mut counts)? {
                    self.number()?;
                }
            }
            id = self.byte()?;
        }
        if id == 0x0a {
            let mut counts = again();
            let mut total = 0u64;
            for index in 0..folders {
                let n = next(&mut counts)?;
                if !(n == 1 && crcs.get(index)) {
                    total = total
                        .checked_add(n)
                        .filter(|v| *v <= self.bound)
                        .ok_or_else(|| corrupt(MSG_CORRUPT))?;
                }
            }
            self.digests(total)?;
            id = self.byte()?;
        }
        if id != 0 {
            return Err(corrupt(MSG_CORRUPT));
        }
        Ok(())
    }

    /// Header (after its 0x01): checks the coders and the number of files.
    fn header(&mut self, max_entries: usize) -> io::Result<()> {
        let mut id = self.byte()?;
        if id == 0x02 {
            // ArchiveProperties
            while self.byte()? != 0 {
                let size = self.count()?;
                self.skip(size)?;
            }
            id = self.byte()?;
        }
        if id == 0x04 {
            self.streams(None)?;
            id = self.byte()?;
        }
        if id == 0x05 && self.number()? > max_entries as u64 {
            return Err(super::budget::limit(MSG_INDEX_BYTES));
        }
        Ok(())
    }
}

/// Decodes the encoded header when it is a single Copy/LZMA/LZMA2 coder (everything a
/// 7z writer produces) so that its file count can be checked before the library allocates
/// for it. `None` when the library will report the header as encrypted.
fn decode_header<R: Read + Seek>(
    reader: &mut R,
    streams: &Streams,
    len: u64,
) -> io::Result<Option<Vec<u8>>> {
    if streams
        .first
        .iter()
        .any(|(id, _)| *id == EncoderMethod::ID_AES256_SHA256)
    {
        return Ok(None);
    }
    let [(id, props)] = streams.first.as_slice() else {
        return Err(corrupt(MSG_CORRUPT));
    };
    if streams.folders != 1 {
        return Err(corrupt(MSG_CORRUPT));
    }
    let start = 32u64
        .checked_add(streams.pack_pos)
        .filter(|start| {
            start
                .checked_add(streams.pack_size)
                .is_some_and(|end| end <= len)
        })
        .ok_or_else(|| corrupt(MSG_CORRUPT))?;
    reader.seek(SeekFrom::Start(start))?;
    let input = io::BufReader::new((&mut *reader).take(streams.pack_size));
    let size = streams.unpack;
    let decoder: Box<dyn Read + '_> = if *id == EncoderMethod::ID_COPY {
        Box::new(input)
    } else if *id == EncoderMethod::ID_LZMA2 {
        let dict = lzma2_dict(props).ok_or_else(|| corrupt(MSG_CORRUPT))?;
        Box::new(lzma_rust2::Lzma2Reader::new(input, dict as u32, None))
    } else if *id == EncoderMethod::ID_LZMA && props.len() >= 5 {
        let dict = u32::from_le_bytes(props[1..5].try_into().unwrap());
        let reader = lzma_rust2::LzmaReader::new_with_props(input, size, props[0], dict, None)
            .map_err(|_| corrupt(MSG_CORRUPT))?;
        Box::new(reader)
    } else {
        return Err(corrupt(MSG_CORRUPT));
    };
    let mut out = Vec::new();
    decoder.take(size).read_to_end(&mut out)?;
    if out.len() as u64 != size {
        return Err(corrupt(MSG_CORRUPT));
    }
    Ok(Some(out))
}

fn check_header_size<R: Read + Seek>(
    reader: &mut R,
    max: u64,
    max_entries: usize,
) -> io::Result<()> {
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
    match probe.byte()? {
        0x17 => {
            let streams = probe.streams(Some(max))?;
            if let Some(decoded) = decode_header(reader, &streams, len)? {
                let mut inner = HeaderProbe {
                    bytes: &decoded,
                    bound: decoded.len() as u64,
                };
                // A nested encoded header is never written by a 7z implementation.
                inner.expect(0x01)?;
                inner.header(max_entries)?;
            }
        }
        0x01 => probe.header(max_entries)?,
        _ => {}
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
    check_header_size(&mut reader, limits.index_dir_bytes, limits.max_entries)
        .map_err(|e| map_io(e, path))?;
    let archive =
        Archive::read(&mut reader, &Password::empty()).map_err(|e| seven_error(e, path))?;
    if archive.files.len() > limits.max_entries {
        return Err(Error::Archive(MSG_INDEX_BYTES));
    }
    // The probe saw the same coders; this guards against a header it parsed differently.
    for block in &archive.blocks {
        for coder in &block.coders {
            check_coder(coder.encoder_method_id(), coder.properties())
                .map_err(|e| map_io(e, path))?;
        }
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
