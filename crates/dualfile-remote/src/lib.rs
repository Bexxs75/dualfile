//! Server connections for Dualfile: FTP/FTPS and SFTP back ends for the virtual file system. All
//! network code lives in this crate so that `dualfile-core` builds without it.

#[cfg(unix)]
pub mod askpass;
pub mod error;
pub mod fake;
pub mod ftpvfs;
pub mod listing;
pub mod manager;
pub mod names;
#[cfg(unix)]
pub mod openssh_transport;
pub mod passive;
pub mod profile;
pub mod russh_transport;
pub mod secret;
pub mod secret_store;
pub mod select_connector;
pub mod sftp_fake;
pub mod sftp_transport;
pub mod sftp_trust;
pub mod sftpvfs;
pub mod smb;
#[cfg(unix)]
pub mod ssh_config;
pub mod suppa;
pub mod tls;
pub mod transport;
pub mod trust;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod sftp_tests;

#[cfg(unix)]
pub mod system_mount;
