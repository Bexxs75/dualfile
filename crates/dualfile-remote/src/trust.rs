//! Trust decisions for server certificates (FTPS) and host keys (SFTP, see `sftp_trust`).
//!
//! Three separate checks: (1) chain and validity period against the system roots, (2) host
//! name, (3) the pin stored for (protocol, host, port). Rules:
//! - an expired or not yet valid certificate and a chain that is broken for any other reason
//!   are never accepted, not even with confirmation;
//! - an unknown issuer (self-signed) and a host name mismatch are accepted only after the
//!   user confirmed exactly this fingerprint;
//! - a pin that does not match is a hard error (`PinMismatch`); no code path of the
//!   connection offers an exception. A pin is only replaced by an explicit, separate step
//!   (`PinStore::remove` through "reset trust" in the profile).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Ftps,
    Sftp,
    Smb,
    Webdav,
}

/// Where a pin applies: one (protocol, host, port).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PinKey {
    pub protocol: Protocol,
    /// Lower case.
    pub host: String,
    pub port: u16,
}

impl PinKey {
    pub fn new(protocol: Protocol, host: &str, port: u16) -> Self {
        Self {
            protocol,
            host: host.to_ascii_lowercase(),
            port,
        }
    }
}

/// A trusted certificate or host key: its SHA-256 (64 lower-case hex digits) plus, for SFTP,
/// the key type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_type: Option<String>,
}

impl Pin {
    pub fn key(&self) -> PinKey {
        PinKey::new(self.protocol, &self.host, self.port)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What the TLS layer found out about the presented certificate (no policy in here).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertObservation {
    pub sha256: [u8; 32],
    pub chain: ChainStatus,
    /// The certificate is valid for the host name that was connected to.
    pub hostname_ok: bool,
    pub key_type: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChainStatus {
    Valid,
    /// Not signed by a trusted root (includes self-signed certificates).
    UnknownIssuer,
    Expired,
    NotYetValid,
    /// Any other defect (bad signature, wrong usage, ...).
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustReason {
    UnknownIssuer,
    HostnameMismatch,
}

/// What the user is asked to confirm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustRequest {
    pub key: PinKey,
    pub sha256: String,
    pub key_type: Option<String>,
    pub reasons: Vec<TrustReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustDecision {
    Reject,
    /// Accept this connection only; nothing is stored.
    TrustOnce,
    /// Accept and store the fingerprint as the pin.
    TrustAlways,
}

/// The user's answer, asked while the connection is being made. An implementation may
/// answer at once from a list the user filled in before (two-step connect).
pub trait ConfirmTrust: Send + Sync {
    fn confirm(&self, request: &TrustRequest) -> TrustDecision;
}

/// Never trusts anything that needs confirmation.
pub struct RejectAll;
impl ConfirmTrust for RejectAll {
    fn confirm(&self, _: &TrustRequest) -> TrustDecision {
        TrustDecision::Reject
    }
}

pub trait PinStore: Send + Sync {
    fn get(&self, key: &PinKey) -> Option<Pin>;
    fn set(&self, pin: Pin);
    /// Only for the explicit "reset trust" step of the profile editor.
    fn remove(&self, key: &PinKey) -> Option<Pin>;
}

#[derive(Default)]
pub struct MemoryPins(Mutex<HashMap<PinKey, Pin>>);

impl MemoryPins {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PinStore for MemoryPins {
    fn get(&self, key: &PinKey) -> Option<Pin> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, pin: Pin) {
        self.0.lock().unwrap().insert(pin.key(), pin);
    }
    fn remove(&self, key: &PinKey) -> Option<Pin> {
        self.0.lock().unwrap().remove(key)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TrustError {
    #[error("Das Zertifikat des Servers ist abgelaufen oder noch nicht gültig")]
    Expired,
    #[error("Die Zertifikatskette des Servers ist ungültig")]
    InvalidChain,
    /// The server presents another certificate/key than the pinned one. Both fingerprints are
    /// shown side by side; the pin can only be replaced by resetting trust in the profile.
    #[error(
        "Das Zertifikat des Servers hat sich geändert (gespeichert {expected}, jetzt {actual})"
    )]
    PinMismatch { expected: String, actual: String },
    /// The user did not confirm the certificate.
    #[error("Das Zertifikat des Servers wurde nicht bestätigt")]
    NotConfirmed(TrustRequest),
    /// SFTP: the server's host key uses an algorithm that is not accepted (see `sftp_trust`).
    #[error("Der Server verwendet einen nicht erlaubten Schlüsselalgorithmus: {0}")]
    AlgorithmNotAllowed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustOutcome {
    /// Valid chain and host name, no pin involved.
    ValidChain,
    /// Matches the stored pin.
    Pinned,
    TrustedOnce,
    TrustedAlways,
}

/// Decides about one observed certificate.
pub trait CertVerifier: Send + Sync {
    fn verify(
        &self,
        key: &PinKey,
        observation: &CertObservation,
    ) -> Result<TrustOutcome, TrustError>;
}

/// The rules from the module documentation.
pub struct PolicyVerifier {
    pins: Arc<dyn PinStore>,
    confirm: Arc<dyn ConfirmTrust>,
}

impl PolicyVerifier {
    pub fn new(pins: Arc<dyn PinStore>, confirm: Arc<dyn ConfirmTrust>) -> Self {
        Self { pins, confirm }
    }
}

impl CertVerifier for PolicyVerifier {
    fn verify(
        &self,
        key: &PinKey,
        observation: &CertObservation,
    ) -> Result<TrustOutcome, TrustError> {
        // (1) never acceptable, with or without a pin
        match observation.chain {
            ChainStatus::Expired | ChainStatus::NotYetValid => return Err(TrustError::Expired),
            ChainStatus::Invalid => return Err(TrustError::InvalidChain),
            ChainStatus::Valid | ChainStatus::UnknownIssuer => {}
        }
        let actual = hex(&observation.sha256);
        // (3) a pin decides alone: equal = trusted, different = error without exception
        if let Some(pin) = self.pins.get(key) {
            return if pin.sha256 == actual {
                Ok(TrustOutcome::Pinned)
            } else {
                Err(TrustError::PinMismatch {
                    expected: pin.sha256,
                    actual,
                })
            };
        }
        // (1)+(2) without a pin: only a clean chain and host name pass by themselves
        let mut reasons = Vec::new();
        if observation.chain == ChainStatus::UnknownIssuer {
            reasons.push(TrustReason::UnknownIssuer);
        }
        if !observation.hostname_ok {
            reasons.push(TrustReason::HostnameMismatch);
        }
        if reasons.is_empty() {
            return Ok(TrustOutcome::ValidChain);
        }
        let request = TrustRequest {
            key: key.clone(),
            sha256: actual.clone(),
            key_type: observation.key_type.clone(),
            reasons,
        };
        match self.confirm.confirm(&request) {
            TrustDecision::Reject => Err(TrustError::NotConfirmed(request)),
            TrustDecision::TrustOnce => Ok(TrustOutcome::TrustedOnce),
            TrustDecision::TrustAlways => {
                self.pins.set(Pin {
                    protocol: key.protocol,
                    host: key.host.clone(),
                    port: key.port,
                    sha256: actual,
                    key_type: observation.key_type.clone(),
                });
                Ok(TrustOutcome::TrustedAlways)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting {
        answer: TrustDecision,
        asked: AtomicUsize,
        last: Mutex<Option<TrustRequest>>,
    }
    impl Counting {
        fn new(answer: TrustDecision) -> Arc<Self> {
            Arc::new(Self {
                answer,
                asked: AtomicUsize::new(0),
                last: Mutex::new(None),
            })
        }
        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
    }
    impl ConfirmTrust for Counting {
        fn confirm(&self, request: &TrustRequest) -> TrustDecision {
            self.asked.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() = Some(request.clone());
            self.answer
        }
    }

    fn key() -> PinKey {
        PinKey::new(Protocol::Ftps, "Files.Example.org", 21)
    }
    fn obs(byte: u8, chain: ChainStatus, hostname_ok: bool) -> CertObservation {
        CertObservation {
            sha256: [byte; 32],
            chain,
            hostname_ok,
            key_type: None,
        }
    }
    fn verifier(answer: TrustDecision) -> (PolicyVerifier, Arc<MemoryPins>, Arc<Counting>) {
        let pins = Arc::new(MemoryPins::new());
        let ask = Counting::new(answer);
        (PolicyVerifier::new(pins.clone(), ask.clone()), pins, ask)
    }

    #[test]
    fn a_clean_certificate_passes_without_asking_or_pinning() {
        let (v, pins, ask) = verifier(TrustDecision::Reject);
        assert_eq!(
            v.verify(&key(), &obs(1, ChainStatus::Valid, true)),
            Ok(TrustOutcome::ValidChain)
        );
        assert_eq!(ask.asked(), 0);
        assert!(pins.get(&key()).is_none());
    }

    #[test]
    fn expired_and_broken_chains_are_never_acceptable() {
        let (v, pins, ask) = verifier(TrustDecision::TrustAlways);
        for chain in [ChainStatus::Expired, ChainStatus::NotYetValid] {
            assert_eq!(
                v.verify(&key(), &obs(1, chain, true)),
                Err(TrustError::Expired)
            );
            assert_eq!(
                v.verify(&key(), &obs(1, chain, false)),
                Err(TrustError::Expired)
            );
        }
        assert_eq!(
            v.verify(&key(), &obs(1, ChainStatus::Invalid, true)),
            Err(TrustError::InvalidChain)
        );
        assert_eq!(ask.asked(), 0, "the user is not even asked");
        assert!(pins.get(&key()).is_none());
        // not even when the very certificate is pinned
        pins.set(Pin {
            protocol: Protocol::Ftps,
            host: "files.example.org".into(),
            port: 21,
            sha256: hex(&[1; 32]),
            key_type: None,
        });
        assert_eq!(
            v.verify(&key(), &obs(1, ChainStatus::Expired, true)),
            Err(TrustError::Expired)
        );
    }

    #[test]
    fn self_signed_and_host_mismatch_need_the_users_confirmation() {
        for (chain, host_ok, reasons) in [
            (
                ChainStatus::UnknownIssuer,
                true,
                vec![TrustReason::UnknownIssuer],
            ),
            (
                ChainStatus::Valid,
                false,
                vec![TrustReason::HostnameMismatch],
            ),
            (
                ChainStatus::UnknownIssuer,
                false,
                vec![TrustReason::UnknownIssuer, TrustReason::HostnameMismatch],
            ),
        ] {
            let (v, pins, ask) = verifier(TrustDecision::Reject);
            let error = v.verify(&key(), &obs(7, chain, host_ok)).unwrap_err();
            let TrustError::NotConfirmed(request) = error else {
                panic!("{error:?}")
            };
            assert_eq!(request.reasons, reasons);
            assert_eq!(request.sha256, hex(&[7; 32]));
            assert_eq!(ask.asked(), 1);
            assert!(
                pins.get(&key()).is_none(),
                "a rejected certificate is not stored"
            );
        }
    }

    #[test]
    fn trusting_once_stores_nothing_and_asks_again_next_time() {
        let (v, pins, ask) = verifier(TrustDecision::TrustOnce);
        let o = obs(3, ChainStatus::UnknownIssuer, true);
        assert_eq!(v.verify(&key(), &o), Ok(TrustOutcome::TrustedOnce));
        assert_eq!(v.verify(&key(), &o), Ok(TrustOutcome::TrustedOnce));
        assert_eq!(ask.asked(), 2);
        assert!(pins.get(&key()).is_none());
    }

    #[test]
    fn trusting_always_pins_and_a_changed_certificate_is_refused_without_asking() {
        let (v, pins, ask) = verifier(TrustDecision::TrustAlways);
        let o = obs(3, ChainStatus::UnknownIssuer, false);
        assert_eq!(v.verify(&key(), &o), Ok(TrustOutcome::TrustedAlways));
        let pin = pins.get(&key()).unwrap();
        assert_eq!(pin.sha256, hex(&[3; 32]));
        assert_eq!((pin.protocol, pin.port), (Protocol::Ftps, 21));
        // the same certificate now passes without a question
        assert_eq!(v.verify(&key(), &o), Ok(TrustOutcome::Pinned));
        assert_eq!(ask.asked(), 1);
        // another certificate: refused, with both fingerprints, and still no question
        for chain in [ChainStatus::Valid, ChainStatus::UnknownIssuer] {
            let changed = obs(4, chain, true);
            assert_eq!(
                v.verify(&key(), &changed),
                Err(TrustError::PinMismatch {
                    expected: hex(&[3; 32]),
                    actual: hex(&[4; 32])
                })
            );
        }
        assert_eq!(ask.asked(), 1, "a pin mismatch has no exception path");
        assert_eq!(
            pins.get(&key()).unwrap().sha256,
            hex(&[3; 32]),
            "the pin is untouched"
        );
    }

    #[test]
    fn pins_are_per_protocol_host_and_port() {
        let (v, _pins, ask) = verifier(TrustDecision::TrustAlways);
        let o = obs(5, ChainStatus::UnknownIssuer, true);
        v.verify(&key(), &o).unwrap();
        for other in [
            PinKey::new(Protocol::Sftp, "files.example.org", 21),
            PinKey::new(Protocol::Ftps, "files.example.org", 990),
            PinKey::new(Protocol::Ftps, "other.example.org", 21),
        ] {
            // not pinned there: the user is asked again, a different certificate is no mismatch
            assert_eq!(
                v.verify(&other, &obs(9, ChainStatus::UnknownIssuer, true)),
                Ok(TrustOutcome::TrustedAlways)
            );
        }
        assert_eq!(ask.asked(), 4);
        // the host name key is case-insensitive
        assert_eq!(
            v.verify(&PinKey::new(Protocol::Ftps, "FILES.example.ORG", 21), &o),
            Ok(TrustOutcome::Pinned)
        );
    }

    #[test]
    fn resetting_trust_is_the_only_way_to_replace_a_pin() {
        let (v, pins, _ask) = verifier(TrustDecision::TrustAlways);
        v.verify(&key(), &obs(3, ChainStatus::UnknownIssuer, true))
            .unwrap();
        assert!(v
            .verify(&key(), &obs(4, ChainStatus::UnknownIssuer, true))
            .is_err());
        assert!(pins.remove(&key()).is_some());
        assert_eq!(
            v.verify(&key(), &obs(4, ChainStatus::UnknownIssuer, true)),
            Ok(TrustOutcome::TrustedAlways)
        );
    }

    #[test]
    fn pins_serialize_without_secrets_and_roundtrip() {
        let pin = Pin {
            protocol: Protocol::Sftp,
            host: "h".into(),
            port: 22,
            sha256: hex(&[0xab; 32]),
            key_type: Some("ssh-ed25519".into()),
        };
        let json = serde_json::to_string(&pin).unwrap();
        assert_eq!(serde_json::from_str::<Pin>(&json).unwrap(), pin);
        assert!(json.contains("\"sftp\""));
    }
}
