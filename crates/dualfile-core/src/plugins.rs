//! What the settings keep about plugins: which ones the user confirmed, with the pinned
//! manifest hash and the granted options. No secrets. The plugin crate turns an entry into
//! its `PinnedState`; this crate only stores and validates the data.

use crate::vfs::is_valid_connection_id;
use serde::{Deserialize, Serialize};

/// Refusal text of a plugin file system that may not be changed (no `write` in the manifest, or
/// the user did not grant it); the UI error kind is `readOnly`.
pub const MSG_PLUGIN_READ_ONLY: &str = "Dieses Plugin-Dateisystem ist schreibgeschützt";

/// Entries in the settings at most.
pub const MAX_PLUGINS: usize = 50;

/// Why an entry was switched off by the program (not by the user).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DisableReason {
    /// Too many failures in a row (crash, time-out, protocol errors).
    Failures,
    /// The package no longer matches the confirmed manifest.
    Locked,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PluginEntry {
    pub id: String,
    /// SHA-256 (hex) of the `plugin.json` that the user confirmed.
    pub manifest_sha256: String,
    pub enabled: bool,
    /// Started without the sandbox, after an explicit second confirmation.
    pub unsandboxed: bool,
    /// The user allowed the plugin's file system to be changed (also needs `write` in the
    /// manifest).
    pub write_granted: bool,
    /// Failures in a row at the last use (shown in the plugin list).
    pub failures: u32,
    pub disabled_reason: Option<DisableReason>,
}

fn is_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl PluginEntry {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !is_valid_connection_id(&self.id) {
            return Err("Ungültige Plugin-Kennung");
        }
        if !is_hash(&self.manifest_sha256) {
            return Err("Ungültiger Manifest-Hash");
        }
        Ok(())
    }
}

/// At most `MAX_PLUGINS`, every entry valid, ids unique.
pub fn validate_plugins(list: &[PluginEntry]) -> Result<(), &'static str> {
    if list.len() > MAX_PLUGINS {
        return Err("Zu viele Plugins");
    }
    for (index, entry) in list.iter().enumerate() {
        entry.validate()?;
        if list[..index].iter().any(|other| other.id == entry.id) {
            return Err("Doppelte Plugin-Kennung");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> PluginEntry {
        PluginEntry {
            id: id.into(),
            manifest_sha256: "ab".repeat(32),
            ..PluginEntry::default()
        }
    }

    #[test]
    fn entries_are_validated() {
        assert!(validate_plugins(&[entry("a"), entry("b-1")]).is_ok());
        assert!(validate_plugins(&[entry("a"), entry("a")]).is_err());
        assert!(validate_plugins(&[entry("A")]).is_err());
        assert!(validate_plugins(&[entry("a/b")]).is_err());
        let mut bad = entry("a");
        bad.manifest_sha256 = "xyz".into();
        assert!(validate_plugins(&[bad]).is_err());
        let many: Vec<PluginEntry> = (0..=MAX_PLUGINS).map(|i| entry(&format!("p{i}"))).collect();
        assert!(validate_plugins(&many).is_err());
        assert!(validate_plugins(&many[..MAX_PLUGINS]).is_ok());
    }

    #[test]
    fn defaults_grant_nothing() {
        let entry = PluginEntry::default();
        assert!(!entry.enabled && !entry.unsandboxed && !entry.write_granted);
    }
}
