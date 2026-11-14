//! Dualfile plugins: external programs that offer file systems, packers, columns and previews
//! over a JSON line protocol. A plugin is never trusted: its package is verified (owner,
//! permissions, no symlinks, SHA-256 of every file, ELF only), it runs in its own process with
//! an empty environment, resource limits and a Landlock + seccomp sandbox that is required
//! (no sandbox, no start), and everything it answers is checked against a strict schema.
//!
//! This crate contains no network code and no UI. See `protocol` for the wire format.

pub mod client;
pub mod error;
pub mod host;
pub mod manifest;
pub mod names;
pub mod package;
pub mod packer;
pub mod protocol;
pub mod runtime;
pub mod sandbox;
pub mod vfs;

pub use client::{ColumnItem, DisabledCause, DisabledHook, Health, PluginClient};
pub use error::{Diagnostic, ErrorCode, PluginError, PluginResult};
pub use host::{ResourceLimits, Session};
pub use manifest::{Kind, Manifest};
pub use package::{discover, refuse_root, verify_package, PinnedState, VerifiedPackage};
pub use packer::PluginPacker;
pub use runtime::{PluginRuntime, RuntimeConfig};
pub use vfs::PluginVfs;

/// Paths of the example plugin programs built by `build.rs` (feature `example-binaries`), for
/// the tests of this crate and of the program that embeds it. `EXAMPLE_PLUGIN` is the normal
/// read-only example; `EXAMPLE_PLUGIN_TESTBIN` is the test double (writable tree and
/// misbehaving modes) and must never be installed for a user.
#[cfg(feature = "example-binaries")]
#[doc(hidden)]
pub mod example_binaries {
    pub const EXAMPLE_PLUGIN: &str = env!("DUALFILE_EXAMPLE_PLUGIN");
    pub const EXAMPLE_PLUGIN_TESTBIN: &str = env!("DUALFILE_EXAMPLE_PLUGIN_TESTBIN");
}

#[cfg(feature = "example-binaries")]
#[doc(hidden)]
pub mod testing;
