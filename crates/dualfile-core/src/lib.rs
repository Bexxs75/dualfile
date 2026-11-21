pub use dualfile_platform as packaging;
pub mod appenv;
pub mod archive;
pub mod compare;
pub mod connections;
pub mod dupes;
pub mod error;
pub mod jobs;
pub mod lister;
pub mod mediatags;
pub mod multirename;
pub mod names;
pub mod ops;
pub mod plugins;
pub mod resume;
pub mod router;
mod scan_fs;
pub mod search;
pub mod settings;
pub mod similar;
pub mod undo;
pub mod usage;
pub mod vfs;

pub use error::{Error, Result};
pub use router::{open_path, VfsRouter};
pub use vfs::{format_ipc_path, parse_ipc_path, LocalVfs, VPath, Vfs};

#[cfg(test)]
mod remote_tests;

#[cfg(unix)]
pub mod drives;
pub mod system_places;

pub mod fstab;
pub mod places_pool;
pub mod user_dirs;

#[cfg(target_os = "linux")]
pub mod trash_empty;
pub mod trash_view;

pub mod devices;
