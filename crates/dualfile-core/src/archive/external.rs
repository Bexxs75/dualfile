//! Archive formats served by a plugin ("packer"). The plugin crate implements
//! `ExternalPacker`; this layer builds the usual index from what it lists, so every safety rule
//! of the built-in formats applies unchanged: entry names are classified here (a name that
//! could leave the archive is kept as an unsafe, never readable entry), the entry/name/depth/
//! memory limits count, a size announced by the plugin is cross-checked against the bytes that
//! actually arrive, and everything read is charged to the job budgets. The built-in formats
//! always win: a plugin is only asked for a file the built-in detection does not recognize.

use super::budget::{map_io, Counted, ReadCtx, Stats};
use super::index::{ArchiveLimits, IndexBuilder, Loc, RawEntry, RawKind};
use super::Source;
use crate::error::{Error, Result};
use crate::vfs::is_valid_connection_id;
use std::io::{Cursor, Read};
use std::sync::Arc;

const MSG_TOO_BIG: &str = "Das Archiv ist für das Plugin zu groß";
const MSG_MEMBER_TOO_BIG: &str = "Die Datei im Archiv ist für das Plugin zu groß";

/// One member as the plugin lists it. The path is the plugin's raw text; the index classifies it.
#[derive(Clone, Debug)]
pub struct ExternalEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// The plugin layer found the name unusable (kept visible, never readable).
    pub problem: Option<&'static str>,
}

/// The plugin side of a packer. Calls may take long and fail; they are made from the
/// archive index thread or the reading thread, never from the UI thread.
pub trait ExternalPacker: Send + Sync {
    /// Largest archive (bytes) the plugin accepts.
    fn max_input_bytes(&self) -> u64;
    fn list(&self, name: &str, archive: &[u8]) -> Result<Vec<ExternalEntry>>;
    fn read(&self, name: &str, archive: &[u8], member: &str) -> Result<Vec<u8>>;
}

/// A registered packer: which file name extensions (lowercase, without the dot) it serves.
#[derive(Clone)]
pub struct PackerBinding {
    pub id: String,
    pub extensions: Vec<String>,
    pub packer: Arc<dyn ExternalPacker>,
}

impl PackerBinding {
    pub fn new(id: &str, extensions: Vec<String>, packer: Arc<dyn ExternalPacker>) -> Result<Self> {
        if !is_valid_connection_id(id) {
            return Err(Error::InvalidInput("Ungültige Plugin-Kennung"));
        }
        let valid = |ext: &String| {
            !ext.is_empty()
                && ext.len() <= 16
                && ext
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        };
        if extensions.is_empty() || !extensions.iter().all(valid) {
            return Err(Error::InvalidInput("Ungültige Archiv-Endung"));
        }
        Ok(Self {
            id: id.to_owned(),
            extensions,
            packer,
        })
    }

    pub(crate) fn serves(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        self.extensions
            .iter()
            .any(|ext| lower.len() > ext.len() + 1 && lower.ends_with(&format!(".{ext}")))
    }
}

pub(crate) struct ExternalBackend {
    packer: Arc<dyn ExternalPacker>,
    name: String,
    data: Arc<Vec<u8>>,
}

/// The binding that serves `name`, if any.
pub(crate) fn find<'a>(bindings: &'a [PackerBinding], name: &str) -> Option<&'a PackerBinding> {
    bindings.iter().find(|binding| binding.serves(name))
}

pub(crate) fn index(
    source: &Source,
    name: &str,
    binding: &PackerBinding,
    limits: &ArchiveLimits,
    builder: &mut IndexBuilder,
) -> Result<ExternalBackend> {
    let max = binding.packer.max_input_bytes();
    let path = std::path::Path::new(name);
    if source.len().map_err(|e| map_io(e, path))? > max {
        return Err(Error::Archive(MSG_TOO_BIG));
    }
    let mut data = Vec::new();
    source
        .open()
        .and_then(|file| file.take(max + 1).read_to_end(&mut data))
        .map_err(|e| map_io(e, path))?;
    if data.len() as u64 > max {
        return Err(Error::Archive(MSG_TOO_BIG));
    }
    let entries = binding.packer.list(name, &data)?;
    if entries.len() > limits.max_entries {
        return Err(Error::Archive(super::budget::MSG_INDEX_BYTES));
    }
    for entry in entries {
        builder.add(RawEntry {
            loc: Loc::External(entry.path.clone()),
            path: entry.path,
            kind: if entry.is_dir {
                RawKind::Dir
            } else {
                RawKind::File
            },
            size: entry.size,
            modified: None,
            mode: None,
            link_target: None,
            unreadable: entry.problem,
        })?;
    }
    Ok(ExternalBackend {
        packer: binding.packer.clone(),
        name: name.to_owned(),
        data: Arc::new(data),
    })
}

impl ExternalBackend {
    pub(crate) fn open_entry(
        &self,
        member: &str,
        size: u64,
        ctx: &ReadCtx,
    ) -> Result<Box<dyn Read + Send>> {
        if size > ctx.limits.max_file_bytes {
            return Err(Error::Archive(MSG_MEMBER_TOO_BIG));
        }
        let bytes = self.packer.read(&self.name, &self.data, member)?;
        Ok(Box::new(Counted::for_entry(
            Cursor::new(bytes),
            ctx,
            Arc::new(Stats::default()),
            Some(size),
        )))
    }
}
