//! Policy selection shared by copy and move commands.
use dualfile_core::{
    ops::{should_verify, VerifyMode, VerifyPolicy},
    VPath,
};

pub fn mode(destination: &VPath, policy: VerifyPolicy) -> VerifyMode {
    if should_verify(destination, policy) {
        VerifyMode::Sha256
    } else {
        VerifyMode::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::{
        ops::{copy_verified, JobHandle},
        vfs::{Caps, Entry, LocalVfs, ReplaceMode, Vfs, WriteFinish, WriteMode},
        Result,
    };
    use std::{
        fs,
        io::{Read, Write},
    };
    struct CorruptWrite {
        inner: Box<dyn WriteFinish>,
        target: VPath,
    }
    impl Write for CorruptWrite {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    impl WriteFinish for CorruptWrite {
        fn commit(&mut self) -> Result<()> {
            self.inner.commit()?;
            let mut data = fs::read(self.target.path()).unwrap();
            if let Some(byte) = data.first_mut() {
                *byte ^= 1;
            }
            fs::write(self.target.path(), data).unwrap();
            Ok(())
        }
    }
    struct CorruptVfs;
    impl Vfs for CorruptVfs {
        fn list(&self, p: &VPath) -> Result<Vec<Entry>> {
            LocalVfs.list(p)
        }
        fn stat(&self, p: &VPath) -> Result<Entry> {
            LocalVfs.stat(p)
        }
        fn open_read(&self, p: &VPath) -> Result<Box<dyn Read + Send>> {
            LocalVfs.open_read(p)
        }
        fn create_write(&self, p: &VPath, m: WriteMode) -> Result<Box<dyn WriteFinish>> {
            Ok(Box::new(CorruptWrite {
                inner: LocalVfs.create_write(p, m)?,
                target: p.clone(),
            }))
        }
        fn mkdir(&self, p: &VPath) -> Result<()> {
            LocalVfs.mkdir(p)
        }
        fn rename(&self, a: &VPath, b: &VPath, m: ReplaceMode) -> Result<()> {
            LocalVfs.rename(a, b, m)
        }
        fn read_link(&self, p: &VPath) -> Result<String> {
            LocalVfs.read_link(p)
        }
        fn create_symlink(&self, t: &str, p: &VPath) -> Result<()> {
            LocalVfs.create_symlink(t, p)
        }
        fn trash(&self, p: &VPath) -> Result<()> {
            LocalVfs.trash(p)
        }
        fn remove_permanent(&self, p: &VPath) -> Result<()> {
            LocalVfs.remove_permanent(p)
        }
        fn capabilities(&self) -> Caps {
            LocalVfs.capabilities()
        }
    }

    #[test]
    fn always_reports_fixed_verification_error_for_corrupt_destination() {
        let source_dir = tempfile::tempdir().unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let source = VPath::local(source_dir.path().join("data")).unwrap();
        let destination = VPath::local(destination_dir.path()).unwrap();
        fs::write(source.path(), b"checksum data").unwrap();
        let report = copy_verified(
            &LocalVfs,
            &[source],
            &CorruptVfs,
            &destination,
            None,
            &JobHandle::silent(),
            mode(&destination, VerifyPolicy::Always),
        );
        assert_eq!(report.failed.len(), 1);
        let error = crate::convert::UiError::from(report.failed.into_iter().next().unwrap().1);
        assert_eq!(error.kind, "verifyFailed");
        assert_eq!(
            error.message,
            "Die Prüfsumme stimmt nach dem Übertragen nicht überein"
        );
        assert_ne!(
            fs::read(destination.path().join("data")).unwrap(),
            b"checksum data"
        );
    }
}
