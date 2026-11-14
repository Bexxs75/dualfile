//! Host key trust for SFTP, on top of the same policy as FTPS certificates (`trust`).
//!
//! An SSH host key has no certificate chain, so it is always "an unknown issuer": the first
//! time the user must confirm the fingerprint (trust on first use), after that the pin for
//! (SFTP, host, port) decides alone. A key that differs from the pin is `PinMismatch`; there is
//! no exception path in the connection code (the pin is only replaced by resetting trust in
//! the profile). The check cannot be switched off.
//!
//! Accepted host key / signature algorithms (everything else is refused, also when a server
//! offers it as the only choice):
//! `ssh-ed25519`, `ecdsa-sha2-nistp256`, `ecdsa-sha2-nistp384`, `ecdsa-sha2-nistp521`,
//! `rsa-sha2-512`, `rsa-sha2-256`. Not accepted: `ssh-dss` and `ssh-rsa` (RSA with SHA-1),
//! OpenSSH certificates (never offered), anything unknown.

use crate::trust::{
    CertObservation, CertVerifier, ChainStatus, PinKey, Protocol, TrustError, TrustOutcome,
};
use sha2::{Digest, Sha256};

pub const ALLOWED_HOST_KEY_ALGORITHMS: &[&str] = &[
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "rsa-sha2-512",
    "rsa-sha2-256",
];

/// `Err` if the signature algorithm the server proved its key with is not on the list.
pub fn check_algorithm(signature_algorithm: &str) -> Result<(), TrustError> {
    if ALLOWED_HOST_KEY_ALGORITHMS.contains(&signature_algorithm) {
        Ok(())
    } else {
        // the name comes from the server: keep it short and printable
        let shown: String = signature_algorithm
            .chars()
            .filter(|c| !c.is_control())
            .take(40)
            .collect();
        Err(TrustError::AlgorithmNotAllowed(shown))
    }
}

/// Runs the whole host key check: algorithm list, then the pin / confirmation policy.
/// `key_type` is the type of the key itself (`ssh-rsa` for any RSA key), `blob` its wire
/// encoding; the fingerprint is the SHA-256 of the blob.
pub fn check_host_key(
    verifier: &dyn CertVerifier,
    host: &str,
    port: u16,
    signature_algorithm: &str,
    key_type: &str,
    blob: &[u8],
) -> Result<TrustOutcome, TrustError> {
    check_algorithm(signature_algorithm)?;
    let digest = Sha256::digest(blob);
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&digest);
    let observation = CertObservation {
        sha256,
        // no chain exists: always a decision of the user or the pin
        chain: ChainStatus::UnknownIssuer,
        hostname_ok: true,
        key_type: Some(key_type.to_owned()),
    };
    verifier.verify(&PinKey::new(Protocol::Sftp, host, port), &observation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listed_algorithms_pass() {
        for ok in ALLOWED_HOST_KEY_ALGORITHMS {
            assert!(check_algorithm(ok).is_ok());
        }
        for bad in [
            "ssh-dss",
            "ssh-rsa",
            "ssh-ed448",
            "",
            "rsa-sha2-256-cert-v01@openssh.com",
        ] {
            assert!(matches!(
                check_algorithm(bad),
                Err(TrustError::AlgorithmNotAllowed(_))
            ));
        }
    }
}
