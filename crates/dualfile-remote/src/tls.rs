//! TLS side of FTPS: what is observed about a server certificate (`observe_certificate`) and
//! the rustls verifier that hands exactly this observation to the trust policy. rustls never
//! decides on its own: a certificate is accepted only if `CertVerifier::verify` says so.

use crate::trust::{CertObservation, CertVerifier, ChainStatus, PinKey, TrustError};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, TrustAnchor, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use webpki::{EndEntityCert, KeyUsage};

fn algorithms() -> WebPkiSupportedAlgorithms {
    ring::default_provider().signature_verification_algorithms
}

/// Looks at the presented certificate: chain against `roots`, host name and fingerprint. No
/// policy decision is made here.
pub fn observe_certificate(
    host: &str,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    roots: &[TrustAnchor<'_>],
    now: UnixTime,
) -> CertObservation {
    observe_with(&algorithms(), host, end_entity, intermediates, roots, now)
}

fn observe_with(
    algs: &WebPkiSupportedAlgorithms,
    host: &str,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    roots: &[TrustAnchor<'_>],
    now: UnixTime,
) -> CertObservation {
    let sha256: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
    let Ok(cert) = EndEntityCert::try_from(end_entity) else {
        return CertObservation {
            sha256,
            chain: ChainStatus::Invalid,
            hostname_ok: false,
            key_type: None,
        };
    };
    let chain = match cert.verify_for_usage(
        algs.all,
        roots,
        intermediates,
        now,
        KeyUsage::server_auth(),
        None,
        None,
    ) {
        Ok(_) => ChainStatus::Valid,
        Err(webpki::Error::UnknownIssuer) => ChainStatus::UnknownIssuer,
        // A self-signed certificate made with the common `openssl req -x509` defaults carries
        // CA:TRUE. By chain it cannot be a server certificate, but it is the same case as any
        // other self-signed one: only the user's confirmation of the fingerprint (or a pin)
        // lets it pass.
        Err(webpki::Error::CaUsedAsEndEntity) => ChainStatus::UnknownIssuer,
        Err(webpki::Error::CertExpired { .. }) => ChainStatus::Expired,
        Err(webpki::Error::CertNotValidYet { .. }) => ChainStatus::NotYetValid,
        Err(_) => ChainStatus::Invalid,
    };
    let hostname_ok = ServerName::try_from(host)
        .ok()
        .is_some_and(|name| cert.verify_is_valid_for_subject_name(&name).is_ok());
    CertObservation {
        sha256,
        chain,
        hostname_ok,
        key_type: None,
    }
}

/// The trust anchors of the system. Unreadable or empty stores give an empty list: every
/// certificate is then "unknown issuer" and needs the user's confirmation.
pub fn system_roots() -> Vec<TrustAnchor<'static>> {
    rustls_native_certs::load_native_certs()
        .certs
        .iter()
        .filter_map(|cert| {
            webpki::anchor_from_trusted_cert(cert)
                .ok()
                .map(|anchor| anchor.to_owned())
        })
        .collect()
}

/// rustls verifier that lets a `CertVerifier` (trust policy) decide about the certificate.
pub struct PinningServerVerifier {
    verifier: Arc<dyn CertVerifier>,
    key: PinKey,
    roots: Vec<TrustAnchor<'static>>,
    algs: WebPkiSupportedAlgorithms,
    last_error: Mutex<Option<TrustError>>,
}

impl std::fmt::Debug for PinningServerVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinningServerVerifier")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl PinningServerVerifier {
    pub fn new(
        verifier: Arc<dyn CertVerifier>,
        key: PinKey,
        roots: Vec<TrustAnchor<'static>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            verifier,
            key,
            roots,
            algs: algorithms(),
            last_error: Mutex::new(None),
        })
    }

    /// The trust error of the last refused certificate (cleared by this call). rustls only
    /// reports a general failure; this is where the real reason is kept.
    pub fn take_error(&self) -> Option<TrustError> {
        self.last_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }
}

impl ServerCertVerifier for PinningServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let observation = observe_with(
            &self.algs,
            &server_name.to_str(),
            end_entity,
            intermediates,
            &self.roots,
            now,
        );
        match self.verifier.verify(&self.key, &observation) {
            Ok(_) => Ok(ServerCertVerified::assertion()),
            Err(error) => {
                *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(error);
                Err(TlsError::General("Vertrauensprüfung fehlgeschlagen".into()))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Client configuration with the ring provider. Session resumption stays on (rustls default):
/// many FTPS servers insist on it for data connections.
pub fn client_config(verifier: Arc<PinningServerVerifier>) -> Arc<ClientConfig> {
    let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::{
        hex, ConfirmTrust, MemoryPins, PinStore, PolicyVerifier, Protocol, TrustDecision,
        TrustRequest,
    };
    use rustls::pki_types::pem::PemObject;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    const LEAF_PEM: &[u8] = include_bytes!("../tests-data/selfsigned-leaf.pem");
    const CA_PEM: &[u8] = include_bytes!("../tests-data/selfsigned-ca.pem");
    /// `openssl x509 -fingerprint -sha256` of the two files.
    const LEAF_SHA256: &str = "4893055c7ee76cd51191cd8860f36cd8e1dfc7cdd00f40dbacac9b5572687bac";
    const CA_SHA256: &str = "ded47d2608e511f9e80f5e4cde644404fe30101b2df49e17438245060ec01e7e";

    fn der(pem: &[u8]) -> CertificateDer<'static> {
        CertificateDer::from_pem_slice(pem).unwrap()
    }
    fn at(secs: u64) -> UnixTime {
        UnixTime::since_unix_epoch(Duration::from_secs(secs))
    }
    /// The certificates are valid from 2026-01-01 to 2030-01-01.
    fn inside() -> UnixTime {
        at(1_830_297_600) // 2028-01-01
    }
    fn before() -> UnixTime {
        at(1_700_000_000) // 2023
    }
    fn after() -> UnixTime {
        at(1_950_000_000) // 2031
    }
    fn observe(host: &str, now: UnixTime) -> CertObservation {
        observe_certificate(host, &der(LEAF_PEM), &[], &[], now)
    }

    struct Answer {
        decision: TrustDecision,
        asked: AtomicUsize,
    }
    impl ConfirmTrust for Answer {
        fn confirm(&self, _: &TrustRequest) -> TrustDecision {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.decision
        }
    }
    fn pinning(
        decision: TrustDecision,
    ) -> (Arc<PinningServerVerifier>, Arc<MemoryPins>, Arc<Answer>) {
        let pins = Arc::new(MemoryPins::new());
        let answer = Arc::new(Answer {
            decision,
            asked: AtomicUsize::new(0),
        });
        let policy = Arc::new(PolicyVerifier::new(pins.clone(), answer.clone()));
        let verifier = PinningServerVerifier::new(
            policy,
            PinKey::new(Protocol::Ftps, "localhost", 21),
            Vec::new(),
        );
        (verifier, pins, answer)
    }
    fn call(v: &PinningServerVerifier, now: UnixTime) -> Result<ServerCertVerified, TlsError> {
        let name = ServerName::try_from("localhost").unwrap();
        v.verify_server_cert(&der(LEAF_PEM), &[], &name, &[], now)
    }

    #[test]
    fn a_self_signed_certificate_has_an_unknown_issuer_and_its_fingerprint_matches_openssl() {
        let obs = observe("localhost", inside());
        assert_eq!(obs.chain, ChainStatus::UnknownIssuer);
        assert_eq!(hex(&obs.sha256), LEAF_SHA256);
        assert_eq!(obs.key_type, None);
    }

    #[test]
    fn a_self_signed_ca_certificate_is_treated_like_any_self_signed_one() {
        let obs = observe_certificate("localhost", &der(CA_PEM), &[], &[], inside());
        assert_eq!(obs.chain, ChainStatus::UnknownIssuer);
        assert_eq!(hex(&obs.sha256), CA_SHA256);
        assert!(obs.hostname_ok);
    }

    #[test]
    fn the_host_name_is_checked_for_dns_names_and_ip_addresses() {
        assert!(observe("localhost", inside()).hostname_ok);
        assert!(observe("127.0.0.1", inside()).hostname_ok);
        assert!(!observe("other.example", inside()).hostname_ok);
        assert!(!observe("127.0.0.2", inside()).hostname_ok);
        assert!(!observe("not a host name", inside()).hostname_ok);
    }

    #[test]
    fn the_validity_period_is_checked() {
        assert_eq!(
            observe("localhost", before()).chain,
            ChainStatus::NotYetValid
        );
        assert_eq!(observe("localhost", after()).chain, ChainStatus::Expired);
    }

    #[test]
    fn garbage_is_an_invalid_chain() {
        let obs = observe_certificate(
            "localhost",
            &CertificateDer::from(vec![1, 2, 3]),
            &[],
            &[],
            inside(),
        );
        assert_eq!(obs.chain, ChainStatus::Invalid);
        assert!(!obs.hostname_ok);
    }

    #[test]
    fn the_verifier_accepts_after_trust_always_and_then_asks_no_more() {
        let (v, pins, answer) = pinning(TrustDecision::TrustAlways);
        assert!(call(&v, inside()).is_ok());
        assert!(v.take_error().is_none());
        assert_eq!(answer.asked.load(Ordering::SeqCst), 1);
        let pin = pins
            .get(&PinKey::new(Protocol::Ftps, "localhost", 21))
            .unwrap();
        assert_eq!(pin.sha256, LEAF_SHA256);
        assert!(call(&v, inside()).is_ok());
        assert_eq!(answer.asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_verifier_refuses_without_confirmation_and_keeps_the_reason() {
        let (v, pins, answer) = pinning(TrustDecision::Reject);
        assert!(call(&v, inside()).is_err());
        assert!(matches!(v.take_error(), Some(TrustError::NotConfirmed(_))));
        assert!(v.take_error().is_none(), "taken once");
        assert_eq!(answer.asked.load(Ordering::SeqCst), 1);
        assert!(pins
            .get(&PinKey::new(Protocol::Ftps, "localhost", 21))
            .is_none());
    }

    #[test]
    fn an_expired_certificate_is_refused_without_asking() {
        let (v, _pins, answer) = pinning(TrustDecision::TrustAlways);
        assert!(call(&v, after()).is_err());
        assert_eq!(v.take_error(), Some(TrustError::Expired));
        assert!(call(&v, before()).is_err());
        assert_eq!(v.take_error(), Some(TrustError::Expired));
        assert_eq!(answer.asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn system_roots_and_client_config_can_be_built() {
        let _ = system_roots();
        let (v, _, _) = pinning(TrustDecision::Reject);
        assert!(!v.supported_verify_schemes().is_empty());
        let _config = client_config(v);
    }
}
