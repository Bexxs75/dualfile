//! A packer plugin as the archive layer's `ExternalPacker`: the core builds the index and
//! applies all safety rules; this only forwards `packer.list` / `packer.read` to the plugin.

use crate::client::PluginClient;
use crate::protocol::{MAX_PACKER_INPUT, MAX_PACKER_READ};
use crate::vfs::map_error;
use dualfile_core::archive::{ExternalEntry, ExternalPacker};
use dualfile_core::error::{Error, Result};
use std::path::PathBuf;
use std::sync::Arc;

pub struct PluginPacker {
    client: Arc<PluginClient>,
}

impl PluginPacker {
    pub fn new(client: Arc<PluginClient>) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &Arc<PluginClient> {
        &self.client
    }
}

impl ExternalPacker for PluginPacker {
    fn max_input_bytes(&self) -> u64 {
        MAX_PACKER_INPUT as u64
    }

    fn list(&self, name: &str, archive: &[u8]) -> Result<Vec<ExternalEntry>> {
        let entries = self
            .client
            .packer_list(name, archive)
            .map_err(|e| map_error(e, PathBuf::from(name)))?;
        Ok(entries
            .into_iter()
            .map(|entry| ExternalEntry {
                path: entry.path,
                is_dir: entry.is_dir,
                size: entry.size,
                problem: entry.problem,
            })
            .collect())
    }

    fn read(&self, name: &str, archive: &[u8], member: &str) -> Result<Vec<u8>> {
        let data = self
            .client
            .packer_read(name, archive, member)
            .map_err(|e| map_error(e, PathBuf::from(name)))?;
        if data.len() > MAX_PACKER_READ {
            return Err(Error::Archive("Die Datei im Archiv ist zu groß"));
        }
        Ok(data)
    }
}
