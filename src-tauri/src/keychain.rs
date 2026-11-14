//! The system keychain behind `SecretStore` (Secret Service / KWallet on Linux through the
//! `keyring` crate, pure Rust over D-Bus). Where no keychain back end exists the store is
//! missing and the vault keeps secrets in the session memory only.
//!
//! The keychain's own error texts are dropped: `SecretStoreError` says "not available" and
//! nothing else, so a secret can never travel in an error.

use dualfile_remote::secret_store::SecretStore;

#[cfg(target_os = "linux")]
mod system {
    use dualfile_remote::secret::Secret;
    use dualfile_remote::secret_store::{SecretStore, SecretStoreError};

    const SERVICE: &str = "dualfile";

    pub struct KeyringStore;

    impl KeyringStore {
        fn entry(key: &str) -> Result<keyring::Entry, SecretStoreError> {
            keyring::Entry::new(SERVICE, key).map_err(|_| SecretStoreError)
        }
    }

    impl SecretStore for KeyringStore {
        fn get(&self, key: &str) -> Result<Option<Secret>, SecretStoreError> {
            match Self::entry(key)?.get_password() {
                Ok(text) => Ok(Some(Secret::new(text))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err(SecretStoreError),
            }
        }

        fn set(&self, key: &str, secret: &Secret) -> Result<(), SecretStoreError> {
            Self::entry(key)?
                .set_password(secret.expose())
                .map_err(|_| SecretStoreError)
        }

        fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
            match Self::entry(key)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(SecretStoreError),
            }
        }
    }
}

/// The store the app uses.
#[cfg(target_os = "linux")]
pub fn system_store() -> Box<dyn SecretStore> {
    Box::new(system::KeyringStore)
}

#[cfg(not(target_os = "linux"))]
pub fn system_store() -> Box<dyn SecretStore> {
    Box::new(NoKeychain)
}

/// "No keychain on this platform": every call fails and the vault falls back to the session.
#[cfg(not(target_os = "linux"))]
struct NoKeychain;

#[cfg(not(target_os = "linux"))]
impl SecretStore for NoKeychain {
    fn get(
        &self,
        _: &str,
    ) -> Result<
        Option<dualfile_remote::secret::Secret>,
        dualfile_remote::secret_store::SecretStoreError,
    > {
        Err(dualfile_remote::secret_store::SecretStoreError)
    }
    fn set(
        &self,
        _: &str,
        _: &dualfile_remote::secret::Secret,
    ) -> Result<(), dualfile_remote::secret_store::SecretStoreError> {
        Err(dualfile_remote::secret_store::SecretStoreError)
    }
    fn delete(&self, _: &str) -> Result<(), dualfile_remote::secret_store::SecretStoreError> {
        Ok(())
    }
}
