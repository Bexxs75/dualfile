//! Where passwords and key passphrases are kept between connects (Phase 6c).
//!
//! `SecretStore` is the interface to a persistent place (the system keychain in the app, a
//! double in tests). `SessionOnly` keeps secrets in memory only (overwritten when removed or
//! dropped). `SecretVault` joins both: a keychain that fails never leads to a secret on disk,
//! the secret stays in the session memory instead and the caller learns that (`StoreKind`).
//!
//! Keys are `dualfile:<profile id>` (`secret_key`). Nothing in here formats a secret into a
//! text; errors carry fixed phrases only.

use crate::secret::Secret;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The key a profile's secret is stored under.
pub fn secret_key(profile_id: &str) -> String {
    format!("dualfile:{profile_id}")
}

/// The store failed (no keychain service, locked, refused). Never carries a secret.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Der Schlüsselbund ist nicht verfügbar")]
pub struct SecretStoreError;

pub trait SecretStore: Send + Sync {
    /// `Ok(None)` if nothing is stored under `key`.
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError>;
    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError>;
    /// Removing what is not there is not an error.
    fn delete(&self, key: &str) -> Result<(), SecretStoreError>;
}

/// Where a secret ended up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreKind {
    Keychain,
    /// Only in the memory of this session (the keychain failed or is not used).
    Session,
}

/// Secrets in memory only; each one is overwritten when it is removed or the store is dropped.
#[derive(Default)]
pub struct SessionOnly(Mutex<HashMap<String, Secret>>);

impl SessionOnly {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

impl SecretStore for SessionOnly {
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }

    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned(), secret.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
        Ok(())
    }
}

/// A store for tests: in memory, can be switched to "broken" like a missing keychain service.
#[derive(Default)]
pub struct FakeKeychain {
    items: Mutex<HashMap<String, Secret>>,
    broken: AtomicBool,
}

impl FakeKeychain {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// While broken every call fails.
    pub fn set_broken(&self, broken: bool) {
        self.broken.store(broken, Ordering::SeqCst);
    }

    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    fn check(&self) -> Result<(), SecretStoreError> {
        if self.broken.load(Ordering::SeqCst) {
            Err(SecretStoreError)
        } else {
            Ok(())
        }
    }
}

impl SecretStore for FakeKeychain {
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
        self.check()?;
        Ok(self
            .items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }

    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.check()?;
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned(), secret.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        self.check()?;
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        Ok(())
    }
}

impl<T: SecretStore + ?Sized> SecretStore for Arc<T> {
    fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
        (**self).get(key)
    }
    fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        (**self).set(key, secret)
    }
    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        (**self).delete(key)
    }
}

/// The persistent store (keychain) with the session memory as the fallback.
pub struct SecretVault {
    primary: Box<dyn SecretStore>,
    session: SessionOnly,
}

impl SecretVault {
    pub fn new(primary: Box<dyn SecretStore>) -> Self {
        Self {
            primary,
            session: SessionOnly::new(),
        }
    }

    /// A vault without a persistent store: everything stays in the session memory.
    pub fn session_only() -> Self {
        Self::new(Box::new(UnavailableStore))
    }

    /// Stores the secret persistently if possible. If the keychain fails, the secret goes to
    /// the session memory (`StoreKind::Session`) and never to disk.
    pub fn set(&self, key: &str, secret: &Secret) -> StoreKind {
        match self.primary.set(key, secret) {
            Ok(()) => {
                // one place only: an older session copy would hide a changed keychain entry
                let _ = self.session.delete(key);
                StoreKind::Keychain
            }
            Err(_) => {
                let _ = self.session.set(key, secret);
                StoreKind::Session
            }
        }
    }

    /// Keeps the secret in the session memory only (not asked of the keychain).
    pub fn set_session(&self, key: &str, secret: &Secret) {
        let _ = self.session.set(key, secret);
    }

    /// The stored secret: the session memory first, then the keychain (a keychain error counts
    /// as "nothing stored").
    pub fn get(&self, key: &str) -> Option<Secret> {
        if let Ok(Some(secret)) = self.session.get(key) {
            return Some(secret);
        }
        self.primary.get(key).ok().flatten()
    }

    /// Where a secret is, without asking the keychain unless `probe_keychain` (reading a
    /// keychain can make the system ask the user to unlock it).
    pub fn kind(&self, key: &str, probe_keychain: bool) -> Option<StoreKind> {
        if matches!(self.session.get(key), Ok(Some(_))) {
            return Some(StoreKind::Session);
        }
        if probe_keychain && matches!(self.primary.get(key), Ok(Some(_))) {
            return Some(StoreKind::Keychain);
        }
        None
    }

    /// Removes the secret from both places. `false` if the keychain could not be asked (the
    /// session copy is gone in any case).
    pub fn delete(&self, key: &str) -> bool {
        let _ = self.session.delete(key);
        self.primary.delete(key).is_ok()
    }

    /// Drops every session secret (app exit).
    pub fn clear_session(&self) {
        self.session.clear();
    }
}

/// "No keychain": every call fails, so everything falls back to the session memory.
struct UnavailableStore;

impl SecretStore for UnavailableStore {
    fn get(&self, _: &str) -> Result<Option<Secret>, SecretStoreError> {
        Err(SecretStoreError)
    }
    fn set(&self, _: &str, _: &Secret) -> Result<(), SecretStoreError> {
        Err(SecretStoreError)
    }
    fn delete(&self, _: &str) -> Result<(), SecretStoreError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_carry_the_profile_id() {
        assert_eq!(secret_key("p1"), "dualfile:p1");
    }

    #[test]
    fn the_keychain_is_used_when_it_works() {
        let keychain = FakeKeychain::new();
        let vault = SecretVault::new(Box::new(keychain.clone()));
        assert_eq!(
            vault.set("dualfile:p1", &Secret::new("pw")),
            StoreKind::Keychain
        );
        assert_eq!(keychain.keys(), ["dualfile:p1"]);
        assert_eq!(vault.get("dualfile:p1").unwrap().expose(), "pw");
        assert_eq!(vault.kind("dualfile:p1", true), Some(StoreKind::Keychain));
        // without probing the keychain stays untouched
        assert_eq!(vault.kind("dualfile:p1", false), None);
        assert!(vault.delete("dualfile:p1"));
        assert!(vault.get("dualfile:p1").is_none());
        assert!(keychain.keys().is_empty());
    }

    #[test]
    fn a_failing_keychain_falls_back_to_the_session_memory_only() {
        let keychain = FakeKeychain::new();
        keychain.set_broken(true);
        let vault = SecretVault::new(Box::new(keychain.clone()));
        assert_eq!(
            vault.set("dualfile:p1", &Secret::new("pw")),
            StoreKind::Session
        );
        assert_eq!(vault.get("dualfile:p1").unwrap().expose(), "pw");
        assert_eq!(vault.kind("dualfile:p1", false), Some(StoreKind::Session));
        // never in the (recovering) keychain by itself
        keychain.set_broken(false);
        assert!(keychain.keys().is_empty());
        // deleting reports that the keychain could not be asked, the session copy is gone
        keychain.set_broken(true);
        assert!(!vault.delete("dualfile:p1"));
        assert!(vault.get("dualfile:p1").is_none());
    }

    #[test]
    fn a_later_keychain_entry_replaces_the_session_copy() {
        let keychain = FakeKeychain::new();
        keychain.set_broken(true);
        let vault = SecretVault::new(Box::new(keychain.clone()));
        vault.set("k", &Secret::new("old"));
        keychain.set_broken(false);
        assert_eq!(vault.set("k", &Secret::new("new")), StoreKind::Keychain);
        assert_eq!(vault.get("k").unwrap().expose(), "new");
        assert_eq!(vault.kind("k", false), None);
    }

    #[test]
    fn the_session_vault_and_clearing_drop_secrets() {
        let vault = SecretVault::session_only();
        assert_eq!(vault.set("k", &Secret::new("pw")), StoreKind::Session);
        assert!(vault.get("k").is_some());
        vault.clear_session();
        assert!(vault.get("k").is_none());
    }

    #[test]
    fn errors_and_formatting_never_show_a_secret() {
        let text = format!("{} {:?}", SecretStoreError, SecretStoreError);
        assert!(!text.contains("pw-marker"));
        let keychain = FakeKeychain::new();
        let _ = keychain.set("k", &Secret::new("pw-marker"));
        // the store has no Debug output of its own that prints values
        assert!(!format!("{:?}", Secret::new("pw-marker")).contains("pw-marker"));
    }
}
