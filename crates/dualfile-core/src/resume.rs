//! Resumable writes to back ends that keep a partial file next to the target (servers).
//!
//! The partial file is `<name>.dualfile-part`; the side file `<name>.dualfile-part.meta` holds
//! a small JSON document with the identity of the source the partial file is made from. A
//! write is only continued when everything matches; anything else starts from scratch, and a
//! partial file that has no matching meta file is never touched (another name is chosen).

use crate::error::{Error, Result};
use crate::vfs::{format_ipc_path, VPath, WriteFinish};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PART_SUFFIX: &str = ".dualfile-part";
pub const META_SUFFIX: &str = ".meta";
/// Every meta file starts with this exact text, so a damaged (truncated) meta file of ours
/// can be told from somebody else's file of the same name.
pub const META_PREFIX: &str = "{\"dualfile-part\":1";
/// Longest file name on typical servers; the partial name must stay below it.
const MAX_NAME: usize = 255;

/// Who the data in a partial file comes from: size, modification time (whole seconds) and a
/// hash of the source location. Two copies of the same unchanged source have equal identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIdentity {
    pub size: u64,
    pub modified_secs: Option<i64>,
    pub path_hash: String,
}

impl SourceIdentity {
    pub fn new(source: &VPath, size: u64, modified: Option<SystemTime>) -> Self {
        let digest = Sha256::digest(format_ipc_path(source).as_bytes());
        Self {
            size,
            modified_secs: modified.map(|time| match time.duration_since(UNIX_EPOCH) {
                Ok(after) => after.as_secs() as i64,
                Err(before) => -(before.duration().as_secs() as i64),
            }),
            path_hash: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        }
    }
}

/// Result of `Vfs::create_write_resumable`.
pub struct ResumableWrite {
    /// Bytes that are already in the partial file and verified; the writer appends after them
    /// (the caller reads the source from this offset). 0 = starting from scratch.
    pub existing_len: u64,
    /// True if an earlier partial file of this very source was found and accepted.
    pub identity_matches: bool,
    /// Dropping the writer without `commit` keeps the partial file and its meta file.
    pub writer: Box<dyn WriteFinish>,
}

/// The on-server meta file.
#[derive(Debug, Deserialize, Serialize)]
struct MetaFile {
    #[serde(rename = "dualfile-part")]
    marker: u32,
    source_size: u64,
    source_mtime: Option<i64>,
    source_path_hash: String,
    written: u64,
}

/// A parsed meta file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartMeta {
    pub identity: SourceIdentity,
    pub written: u64,
}

impl PartMeta {
    pub fn new(identity: SourceIdentity, written: u64) -> Self {
        Self { identity, written }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let file = MetaFile {
            marker: 1,
            source_size: self.identity.size,
            source_mtime: self.identity.modified_secs,
            source_path_hash: self.identity.path_hash.clone(),
            written: self.written,
        };
        serde_json::to_vec(&file).expect("meta serializes")
    }

    /// `None` if `bytes` is no valid meta file of ours.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 4096 {
            return None;
        }
        let file: MetaFile = serde_json::from_slice(bytes).ok()?;
        if file.marker != 1 {
            return None;
        }
        Some(Self {
            identity: SourceIdentity {
                size: file.source_size,
                modified_secs: file.source_mtime,
                path_hash: file.source_path_hash,
            },
            written: file.written,
        })
    }
}

/// What a meta file found next to a partial file says about who owns it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ownership {
    /// No meta file: the partial file (if any) is not ours; never touch it.
    Foreign,
    /// A meta file that looks like ours but is damaged: ours, start over.
    Damaged,
    /// A valid meta file.
    Ours(PartMeta),
}

/// Classifies the content of a meta file (`None` = no such file).
pub fn classify_meta(bytes: Option<&[u8]>) -> Ownership {
    match bytes {
        None => Ownership::Foreign,
        Some(bytes) => match PartMeta::parse(bytes) {
            Some(meta) => Ownership::Ours(meta),
            None if bytes.starts_with(META_PREFIX.as_bytes()) => Ownership::Damaged,
            None => Ownership::Foreign,
        },
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumeDecision {
    /// Continue after `len` verified bytes.
    Resume { len: u64 },
    /// Start from scratch; `reason` says why a partial file was not continued.
    Fresh(&'static str),
}

/// The resume rule: the identity is equal **and** the recorded length is not larger than the
/// source **and** the size the server reports is exactly the recorded length.
pub fn decide_resume(
    meta: &PartMeta,
    identity: &SourceIdentity,
    server_len: Option<u64>,
) -> ResumeDecision {
    if &meta.identity != identity {
        ResumeDecision::Fresh("Die Quelle hat sich geändert")
    } else if meta.written > identity.size {
        ResumeDecision::Fresh("Die Hilfsdatei ist länger als die Quelle")
    } else if server_len != Some(meta.written) {
        ResumeDecision::Fresh("Die Größe der Hilfsdatei stimmt nicht mit dem Vermerk überein")
    } else {
        ResumeDecision::Resume { len: meta.written }
    }
}

/// Name of the `attempt`-th candidate for the partial file of `name` (0 = the plain
/// `<name>.dualfile-part`). Names too long for a server are refused.
pub fn part_name(name: &str, attempt: u32) -> Result<String> {
    let candidate = if attempt == 0 {
        format!("{name}{PART_SUFFIX}")
    } else {
        format!("{name}{PART_SUFFIX}-{}", attempt + 1)
    };
    if candidate.len() + META_SUFFIX.len() > MAX_NAME {
        return Err(Error::InvalidName(name.to_owned()));
    }
    Ok(candidate)
}

pub fn meta_name(part: &str) -> String {
    format!("{part}{META_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn identity(size: u64) -> SourceIdentity {
        SourceIdentity::new(
            &VPath::local("/a/b").unwrap(),
            size,
            Some(UNIX_EPOCH + Duration::from_secs(1000)),
        )
    }

    #[test]
    fn meta_roundtrip_and_ownership() {
        let meta = PartMeta::new(identity(10), 4);
        let bytes = meta.to_bytes();
        assert!(bytes.starts_with(META_PREFIX.as_bytes()));
        assert_eq!(PartMeta::parse(&bytes), Some(meta.clone()));
        assert_eq!(classify_meta(Some(&bytes)), Ownership::Ours(meta));
        assert_eq!(classify_meta(None), Ownership::Foreign);
        assert_eq!(classify_meta(Some(b"hello")), Ownership::Foreign);
        // somebody else's JSON of the same name is not ours
        assert_eq!(classify_meta(Some(b"{\"a\":1}")), Ownership::Foreign);
        // a cut-off meta file of ours is damaged, not foreign
        assert_eq!(
            classify_meta(Some(&bytes[..bytes.len() - 5])),
            Ownership::Damaged
        );
    }

    #[test]
    fn decision_table() {
        let id = identity(10);
        let ok = PartMeta::new(id.clone(), 4);
        assert_eq!(
            decide_resume(&ok, &id, Some(4)),
            ResumeDecision::Resume { len: 4 }
        );
        assert!(matches!(
            decide_resume(&ok, &id, Some(3)),
            ResumeDecision::Fresh(_)
        ));
        assert!(matches!(
            decide_resume(&ok, &id, None),
            ResumeDecision::Fresh(_)
        ));
        assert!(matches!(
            decide_resume(&ok, &identity(11), Some(4)),
            ResumeDecision::Fresh(_)
        ));
        let mut other = id.clone();
        other.modified_secs = Some(1);
        assert!(matches!(
            decide_resume(&ok, &other, Some(4)),
            ResumeDecision::Fresh(_)
        ));
        let mut other = id.clone();
        other.path_hash = "x".into();
        assert!(matches!(
            decide_resume(&ok, &other, Some(4)),
            ResumeDecision::Fresh(_)
        ));
        let long = PartMeta::new(id.clone(), 11);
        assert!(matches!(
            decide_resume(&long, &id, Some(11)),
            ResumeDecision::Fresh(_)
        ));
        // everything written: resuming with zero bytes left is allowed
        let full = PartMeta::new(id.clone(), 10);
        assert_eq!(
            decide_resume(&full, &id, Some(10)),
            ResumeDecision::Resume { len: 10 }
        );
    }

    #[test]
    fn part_names() {
        assert_eq!(part_name("a.txt", 0).unwrap(), "a.txt.dualfile-part");
        assert_eq!(part_name("a.txt", 1).unwrap(), "a.txt.dualfile-part-2");
        assert_eq!(meta_name("a.txt.dualfile-part"), "a.txt.dualfile-part.meta");
        assert!(part_name(&"x".repeat(250), 0).is_err());
    }
}
