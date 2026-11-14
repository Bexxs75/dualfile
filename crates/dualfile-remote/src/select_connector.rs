//! Per-profile selection, shared by initial connection and subsequent transfer sessions.
use crate::error::TResult;
use crate::manager::SftpConnector;
use crate::profile::{SftpConnectParams, SshTransport};
use crate::secret::Secret;
use crate::sftp_transport::SftpTransport;
use crate::trust::CertVerifier;
use std::sync::Arc;

pub struct SelectConnector<B, O> {
    pub builtin: B,
    pub openssh: O,
}
impl<B: SftpConnector, O: SftpConnector> SftpConnector for SelectConnector<B, O> {
    type Transport = Box<dyn SftpTransport>;
    fn connect(
        &self,
        params: &SftpConnectParams,
        secret: &Secret,
        verifier: Arc<dyn CertVerifier>,
    ) -> TResult<Self::Transport> {
        match params.profile.transport {
            SshTransport::Builtin => Ok(Box::new(self.builtin.connect(params, secret, verifier)?)),
            SshTransport::Openssh => Ok(Box::new(self.openssh.connect(params, secret, verifier)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TransportError;
    use crate::profile::{SftpAuth, SftpProfile};
    use crate::trust::{CertObservation, PinKey, TrustError, TrustOutcome};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake(Arc<AtomicUsize>);
    impl SftpConnector for Fake {
        type Transport = crate::sftp_fake::FakeSftpTransport;
        fn connect(
            &self,
            _: &SftpConnectParams,
            _: &Secret,
            _: Arc<dyn CertVerifier>,
        ) -> TResult<Self::Transport> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(TransportError::Auth)
        }
    }
    struct Verifier;
    impl CertVerifier for Verifier {
        fn verify(&self, _: &PinKey, _: &CertObservation) -> Result<TrustOutcome, TrustError> {
            unreachable!()
        }
    }
    #[test]
    fn profile_selects_exactly_one_connector() {
        let builtin = Arc::new(AtomicUsize::new(0));
        let openssh = Arc::new(AtomicUsize::new(0));
        let selected = SelectConnector {
            builtin: Fake(builtin.clone()),
            openssh: Fake(openssh.clone()),
        };
        let mut params = SftpConnectParams::new(SftpProfile {
            transport: SshTransport::Builtin,
            name: "n".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            auth: SftpAuth::Password,
            start_folder: None,
        });
        assert!(selected
            .connect(&params, &Secret::new(""), Arc::new(Verifier))
            .is_err());
        assert_eq!(
            (
                builtin.load(Ordering::SeqCst),
                openssh.load(Ordering::SeqCst)
            ),
            (1, 0)
        );
        params.profile.transport = SshTransport::Openssh;
        assert!(selected
            .connect(&params, &Secret::new(""), Arc::new(Verifier))
            .is_err());
        assert_eq!(
            (
                builtin.load(Ordering::SeqCst),
                openssh.load(Ordering::SeqCst)
            ),
            (1, 1)
        );
    }
}
