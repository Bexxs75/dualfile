//! A plugin's file system as a `Vfs` (scheme `plugin:<id>!<path>`).
//!
//! Read-only unless the manifest has `write` *and* the plugin says it can write. Changes only
//! happen through the program's ordinary jobs (conflict handling, no-replace, delete plan):
//! a new file is written to a helper name next to its target and moved into place without
//! replacing (`create_write`); `rename` checks first that the target is free; the Vfs has no
//! trash (`Unsupported`, never replaced by a permanent delete); `remove_permanent` is only
//! reached through the delete plan of `ops`.
//!
//! Plugin text never enters an error message: errors are built from fixed phrases and the
//! program's own path; the plugin's text stays in `PluginError::diagnostic`.

use crate::client::PluginClient;
use crate::error::{ErrorCode, PluginError};
use crate::names::{display_name, name_problem, problems};
use crate::protocol::{PluginEntry, MAX_READ_CHUNK, MAX_WRITE_CHUNK};
use dualfile_core::error::{Error, Result};
use dualfile_core::vfs::{
    Caps, Entry, EntryKind, ReplaceMode, Scheme, VPath, Vfs, WriteFinish, WriteMode,
};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

use dualfile_core::plugins::MSG_PLUGIN_READ_ONLY as MSG_READ_ONLY;

pub struct PluginVfs {
    client: Arc<PluginClient>,
    /// The user allowed changes (a second condition next to `write` in the manifest).
    write_granted: bool,
}

impl PluginVfs {
    /// A read-only file system: even a manifest with `write` changes nothing until the user's
    /// grant is given (`with_write_grant`).
    pub fn new(client: Arc<PluginClient>) -> Self {
        Self::with_write_grant(client, false)
    }

    pub fn with_write_grant(client: Arc<PluginClient>, write_granted: bool) -> Self {
        Self {
            client,
            write_granted,
        }
    }

    fn may_write(&self) -> bool {
        self.write_granted && self.client.manifest().write
    }

    pub fn client(&self) -> &Arc<PluginClient> {
        &self.client
    }

    fn own(&self, path: &VPath) -> Result<()> {
        if path.scheme() == Scheme::Plugin && path.conn_id() == Some(self.client.id()) {
            Ok(())
        } else {
            Err(Error::InvalidPath(
                "Der Pfad gehört nicht zu diesem Plugin".into(),
            ))
        }
    }

    fn writable(&self) -> Result<()> {
        if self.may_write() {
            Ok(())
        } else {
            Err(Error::Unsupported(MSG_READ_ONLY))
        }
    }

    fn text(path: &VPath) -> String {
        path.path().to_string_lossy().into_owned()
    }

    fn lookup(&self, path: &VPath) -> Result<Option<Entry>> {
        let name = file_name(path);
        match self.client.fs_stat(&Self::text(path), &name) {
            Ok(entry) => Ok(Some(entry_from(&entry, name_problem(&entry.name)))),
            Err(PluginError::Reported {
                code: ErrorCode::NotFound,
                ..
            }) => Ok(None),
            Err(error) => Err(to_core(error, path)),
        }
    }

    fn require(&self, path: &VPath) -> Result<Entry> {
        self.lookup(path)?
            .ok_or_else(|| Error::NotFound(path.path().to_path_buf()))
    }

    /// An existing regular file with a usable name.
    fn usable_file(&self, path: &VPath) -> Result<Entry> {
        self.own(path)?;
        let entry = self.require(path)?;
        if let Some(problem) = &entry.error {
            return Err(Error::InvalidName(problem.clone()));
        }
        match entry.kind {
            EntryKind::File => Ok(entry),
            EntryKind::Dir => Err(Error::IsADirectory(path.path().to_path_buf())),
            EntryKind::Symlink => Err(Error::Unsupported("Verknüpfungen im Plugin folgen")),
        }
    }

    /// Like `usable_file`, and the name must not collide with a sibling (needs a listing).
    fn usable_unique_file(&self, path: &VPath) -> Result<Entry> {
        let entry = self.usable_file(path)?;
        if let Some(parent) = path.parent() {
            let name = file_name(path);
            let siblings = self.list(&parent)?;
            if siblings.iter().any(|e| e.name == name && e.error.is_some()) {
                return Err(Error::InvalidName(
                    "Der Name kollidiert mit einem anderen Eintrag".into(),
                ));
            }
        }
        Ok(entry)
    }

    fn exists(&self, path: &VPath) -> Result<bool> {
        Ok(self.lookup(path)?.is_some())
    }

    fn temp_path(&self, target: &VPath) -> Result<VPath> {
        let name = file_name(target);
        let unique = format!(
            "{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let long = format!(".{name}.dfpart-{unique}");
        let helper = if long.len() <= 255 {
            long
        } else {
            format!(".dfpart-{unique}")
        };
        target.sibling(&helper)
    }
}

fn file_name(path: &VPath) -> String {
    path.path()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn entry_from(listed: &PluginEntry, problem: Option<&'static str>) -> Entry {
    Entry {
        uid: None,
        gid: None,
        name: display_name(&listed.name),
        kind: listed.kind,
        size: listed.size,
        modified: listed.modified,
        mode: listed.mode,
        hidden: listed.name.starts_with('.'),
        link_target: None,
        broken_link: false,
        error: problem.map(str::to_owned),
    }
}

/// Maps a plugin failure to the program's error, from fixed phrases and our own path only.
pub fn to_core(error: PluginError, path: &VPath) -> Error {
    map_error(error, path.path().to_path_buf())
}

/// Like `to_core` for a place that is no `VPath` (the archive a packer works on).
pub(crate) fn map_error(error: PluginError, location: PathBuf) -> Error {
    let here = || -> PathBuf { location.clone() };
    match error {
        PluginError::Reported { code, .. } => match code {
            ErrorCode::NotFound => Error::NotFound(here()),
            ErrorCode::Exists => Error::Exists(here()),
            ErrorCode::PermissionDenied => Error::PermissionDenied(here()),
            ErrorCode::NotADirectory => Error::NotADirectory(here()),
            ErrorCode::IsADirectory => Error::IsADirectory(here()),
            ErrorCode::NotEmpty => Error::DirectoryNotEmpty(here()),
            ErrorCode::Unsupported => Error::Unsupported("Das Plugin unterstützt das nicht"),
            ErrorCode::Invalid => Error::InvalidInput("Das Plugin lehnt die Anfrage ab"),
            ErrorCode::TooLarge => Error::Plugin {
                kind: "pluginTooLarge",
                message: "Das Plugin lehnt die Anfrage als zu groß ab".into(),
            },
            ErrorCode::Other => Error::Plugin {
                kind: "pluginFailed",
                message: "Das Plugin meldet einen Fehler".into(),
            },
        },
        PluginError::Unsupported(what) => Error::Unsupported(what),
        other => Error::Plugin {
            kind: other.ui_kind(),
            message: other.to_string(),
        },
    }
}

struct PluginReader {
    client: Arc<PluginClient>,
    path: VPath,
    offset: u64,
    size: u64,
    buffer: Vec<u8>,
    position: usize,
}

impl Read for PluginReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.position >= self.buffer.len() {
            if self.offset >= self.size {
                return Ok(0);
            }
            let want = ((self.size - self.offset).min(MAX_READ_CHUNK as u64)) as usize;
            let data = self
                .client
                .fs_read(&PluginVfs::text(&self.path), self.offset, want)
                .map_err(|error| io::Error::other(error.to_string()))?;
            if data.is_empty() {
                // the file ended before the size we were told
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            self.offset += data.len() as u64;
            self.buffer = data;
            self.position = 0;
        }
        let n = out.len().min(self.buffer.len() - self.position);
        out[..n].copy_from_slice(&self.buffer[self.position..self.position + n]);
        self.position += n;
        Ok(n)
    }
}

struct PluginWrite {
    client: Arc<PluginClient>,
    handle: u32,
    generation: u64,
    temp: VPath,
    target: VPath,
    mode: WriteMode,
    rename_no_replace: bool,
    buffer: Vec<u8>,
    done: bool,
    failed: bool,
}

impl PluginWrite {
    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        self.client
            .fs_write_chunk(self.handle, self.generation, data)
            .map_err(|error| {
                self.failed = true;
                io::Error::other(error.to_string())
            })
    }

    fn flush_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.buffer);
        self.send(&data)
    }

    fn cleanup(&mut self) {
        let _ = self
            .client
            .fs_write_close(self.handle, self.generation, false);
        let _ = self.client.fs_remove(&PluginVfs::text(&self.temp));
    }
}

impl Write for PluginWrite {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.done || self.failed {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        self.buffer.extend_from_slice(data);
        while self.buffer.len() >= MAX_WRITE_CHUNK {
            let rest = self.buffer.split_off(MAX_WRITE_CHUNK);
            let chunk = std::mem::replace(&mut self.buffer, rest);
            self.send(&chunk)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WriteFinish for PluginWrite {
    fn commit(&mut self) -> Result<()> {
        if self.done || self.failed {
            return Err(Error::Plugin {
                kind: "pluginCrashed",
                message: "Die Verbindung zum Plugin wurde unterbrochen".into(),
            });
        }
        self.done = true;
        let target = self.target.clone();
        let result = (|| -> Result<()> {
            self.flush_buffer().map_err(|_| Error::Plugin {
                kind: "pluginCrashed",
                message: "Die Verbindung zum Plugin wurde unterbrochen".into(),
            })?;
            self.client
                .fs_write_close(self.handle, self.generation, true)
                .map_err(|e| to_core(e, &target))?;
            let replace = self.mode == WriteMode::Replace;
            if !replace && !self.rename_no_replace {
                // the plugin does not promise to refuse an existing target: check first (a
                // file that appears in the next instant can still be replaced)
                let name = file_name(&target);
                if self
                    .client
                    .fs_stat(&PluginVfs::text(&target), &name)
                    .is_ok()
                {
                    return Err(Error::Exists(target.path().to_path_buf()));
                }
            }
            self.client
                .fs_rename(
                    &PluginVfs::text(&self.temp),
                    &PluginVfs::text(&target),
                    replace,
                )
                .map_err(|e| to_core(e, &target))
        })();
        if result.is_err() {
            self.cleanup();
        }
        result
    }
}

impl Drop for PluginWrite {
    fn drop(&mut self) {
        // never committed: the helper file must not stay behind (best effort)
        if !self.done {
            self.cleanup();
        }
    }
}

impl Vfs for PluginVfs {
    fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
        self.own(p)?;
        let listed = self
            .client
            .fs_list(&Self::text(p))
            .map_err(|e| to_core(e, p))?;
        let marks = problems(listed.iter().map(|e| e.name.as_str()));
        Ok(listed
            .iter()
            .zip(marks)
            .map(|(entry, mark)| entry_from(entry, mark))
            .collect())
    }

    fn stat(&self, p: &VPath) -> Result<Entry> {
        self.own(p)?;
        self.require(p)
    }

    fn stat_file(&self, p: &VPath) -> Result<dualfile_core::vfs::FileStat> {
        let entry = self.usable_file(p)?;
        Ok(dualfile_core::vfs::FileStat {
            size: entry.size,
            modified: entry.modified,
        })
    }

    fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
        self.open_read_from(p, 0)
    }

    fn open_read_from(&self, p: &VPath, offset: u64) -> Result<Box<dyn Read + Send>> {
        let entry = self.usable_unique_file(p)?;
        Ok(Box::new(PluginReader {
            client: self.client.clone(),
            path: p.clone(),
            offset,
            size: entry.size,
            buffer: Vec::new(),
            position: 0,
        }))
    }

    fn read_range(&self, p: &VPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let entry = self.usable_file(p)?;
        let mut data = Vec::new();
        let mut at = offset;
        while data.len() < len && at < entry.size {
            let want = (len - data.len()).min(MAX_READ_CHUNK);
            let chunk = self
                .client
                .fs_read(&Self::text(p), at, want)
                .map_err(|e| to_core(e, p))?;
            if chunk.is_empty() {
                break;
            }
            at += chunk.len() as u64;
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }

    fn create_write(&self, p: &VPath, mode: WriteMode) -> Result<Box<dyn WriteFinish>> {
        self.own(p)?;
        self.writable()?;
        let name = file_name(p);
        if let Some(problem) = name_problem(&name) {
            return Err(Error::InvalidName(problem.to_owned()));
        }
        let caps = self.client.fs_capabilities().map_err(|e| to_core(e, p))?;
        if !caps.write {
            return Err(Error::Unsupported(MSG_READ_ONLY));
        }
        match self.lookup(p)? {
            Some(existing) if mode == WriteMode::NoReplace => {
                let _ = existing;
                return Err(Error::Exists(p.path().to_path_buf()));
            }
            Some(existing) if existing.kind == EntryKind::Dir => {
                return Err(Error::IsADirectory(p.path().to_path_buf()))
            }
            _ => {}
        }
        let temp = self.temp_path(p)?;
        let (handle, generation) = self
            .client
            .fs_write_open(&Self::text(&temp), false)
            .map_err(|e| to_core(e, p))?;
        Ok(Box::new(PluginWrite {
            client: self.client.clone(),
            handle,
            generation,
            temp,
            target: p.clone(),
            mode,
            rename_no_replace: caps.rename_no_replace,
            buffer: Vec::new(),
            done: false,
            failed: false,
        }))
    }

    fn mkdir(&self, p: &VPath) -> Result<()> {
        self.own(p)?;
        self.writable()?;
        if let Some(problem) = name_problem(&file_name(p)) {
            return Err(Error::InvalidName(problem.to_owned()));
        }
        if self.exists(p)? {
            return Err(Error::Exists(p.path().to_path_buf()));
        }
        self.client
            .fs_mkdir(&Self::text(p))
            .map_err(|e| to_core(e, p))
    }

    fn rename(&self, from: &VPath, to: &VPath, mode: ReplaceMode) -> Result<()> {
        self.own(from)?;
        self.own(to)?;
        self.writable()?;
        if let Some(problem) = name_problem(&file_name(to)) {
            return Err(Error::InvalidName(problem.to_owned()));
        }
        self.require(from)?;
        let replace = mode == ReplaceMode::Replace;
        if !replace && self.exists(to)? {
            return Err(Error::Exists(to.path().to_path_buf()));
        }
        self.client
            .fs_rename(&Self::text(from), &Self::text(to), replace)
            .map_err(|e| to_core(e, to))
    }

    fn read_link(&self, _p: &VPath) -> Result<String> {
        Err(Error::Unsupported("Verknüpfungen im Plugin lesen"))
    }

    fn create_symlink(&self, _target: &str, _at: &VPath) -> Result<()> {
        Err(Error::Unsupported("Verknüpfungen im Plugin anlegen"))
    }

    fn trash(&self, _p: &VPath) -> Result<()> {
        // never replaced by a permanent delete
        Err(Error::Unsupported("Papierkorb im Plugin-Dateisystem"))
    }

    fn remove_permanent(&self, p: &VPath) -> Result<()> {
        self.own(p)?;
        self.writable()?;
        self.require(p)?;
        self.client
            .fs_remove(&Self::text(p))
            .map_err(|e| to_core(e, p))
    }

    fn capabilities(&self) -> Caps {
        Caps {
            can_trash: false,
            can_rename_in_place: self.may_write(),
        }
    }

    fn remote_endpoint(&self) -> Option<(Scheme, &str)> {
        Some((Scheme::Plugin, self.client.id()))
    }

    fn is_read_only(&self) -> bool {
        !self.may_write()
    }
}
