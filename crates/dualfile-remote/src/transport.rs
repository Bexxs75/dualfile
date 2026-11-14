//! The narrow interface between the file-system layer (`FtpVfs`) and a real or fake FTP
//! connection. One transport value is one control connection (one login); a transfer is
//! started, driven and finished on it step by step, and only one transfer runs at a time.

use crate::error::TResult;
use std::time::{Duration, SystemTime};

/// What the server announced it can do (`FEAT`), reduced to what `FtpVfs` uses.
#[derive(Clone, Copy, Debug, Default)]
pub struct Features {
    pub mlsd: bool,
    pub mlst: bool,
    pub size: bool,
    pub mdtm: bool,
    /// `REST STREAM`: downloads can start at an offset.
    pub rest_stream: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreMode {
    /// `STOR`: create or truncate.
    Create,
    /// `APPE`: add to the end of an existing file.
    Append,
}

/// Bounds for everything a server sends that is read into memory.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Bytes of one directory listing.
    pub max_list_bytes: usize,
    pub max_list_lines: usize,
    /// Length of one listing line (longer lines are dropped).
    pub max_line_len: usize,
    /// Entries kept from one listing.
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_list_bytes: 16 << 20,
            max_list_lines: 200_000,
            max_line_len: 4096,
            max_entries: 100_000,
        }
    }
}

/// Time limits; every wait on the network ends within one of them.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    /// No reply / no data for this long ends the operation with `Timeout`.
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            idle: Duration::from_secs(60),
        }
    }
}

/// One FTP control connection. Paths are absolute server paths without control characters
/// (`FtpVfs` checks that before it calls); implementations must still refuse CR, LF and NUL
/// in anything they put on the wire (`TransportError::Rejected`).
///
/// A transport is dropped to close the connection (an open transfer is abandoned).
pub trait FtpTransport: Send + 'static {
    fn features(&self) -> Features;
    /// Raw listing lines of the directory `path` (`MLSD` if `mlsd`, else `LIST`), read within
    /// `limits`: too much data ends with `TransportError::TooLarge` (the transfer is aborted).
    fn list(&mut self, path: &str, mlsd: bool, limits: &Limits) -> TResult<Vec<String>>;
    /// The fact line of one entry (`MLST`).
    fn mlst(&mut self, path: &str) -> TResult<String>;
    fn size(&mut self, path: &str) -> TResult<u64>;
    fn mdtm(&mut self, path: &str) -> TResult<SystemTime>;
    fn mkdir(&mut self, path: &str) -> TResult<()>;
    fn rmdir(&mut self, path: &str) -> TResult<()>;
    fn delete(&mut self, path: &str) -> TResult<()>;
    /// `RNFR` + `RNTO`. Servers may replace an existing target; callers check first.
    fn rename(&mut self, from: &str, to: &str) -> TResult<()>;
    fn noop(&mut self) -> TResult<()>;

    /// Starts `RETR path` at `offset` (`REST`). Data is then taken with `read_data`.
    fn start_download(&mut self, path: &str, offset: u64) -> TResult<()>;
    /// 0 = end of the file.
    fn read_data(&mut self, buf: &mut [u8]) -> TResult<usize>;
    /// Closes the data connection and checks the server's completion reply.
    fn finish_download(&mut self) -> TResult<()>;

    fn start_upload(&mut self, path: &str, mode: StoreMode) -> TResult<()>;
    fn write_data(&mut self, buf: &[u8]) -> TResult<()>;
    /// Closes the data connection and checks the server's completion reply: only after this
    /// returns `Ok` is the data known to have arrived.
    fn finish_upload(&mut self) -> TResult<()>;
    /// Gives up a running transfer (best effort, the connection may be unusable afterwards).
    fn abort_transfer(&mut self);
    fn quit(&mut self);
}
