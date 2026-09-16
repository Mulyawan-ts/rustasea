//! SFTP storage disk (ADOPT-025) — `league/flysystem-sftp-v3` parity.
//!
//! The [`SftpDiskConfig`] type is always compiled (the facade must parse an
//! `sftp` disk even when the driver is off); the live [`SftpDisk`] and its
//! transport are gated behind the `sftp` feature.
//!
//! # Environment bridge
//!
//! Unlike the cloud drivers, SFTP has no layered `ConfigLoader` bridge in this
//! crate, so a single-underscore `SFTP_*` overlay is applied by
//! [`SftpDiskConfig::apply_env`]: `SFTP_HOST`, `SFTP_PORT`, `SFTP_USERNAME`,
//! `SFTP_PASSWORD`, `SFTP_PRIVATE_KEY` (→ `private_key_path`), `SFTP_ROOT`, and
//! `SFTP_TIMEOUT`.

pub mod config;

pub use config::SftpDiskConfig;

#[cfg(feature = "sftp")]
pub mod connection;
#[cfg(feature = "sftp")]
pub mod disk;

#[cfg(feature = "sftp")]
pub use disk::SftpDisk;

#[cfg(test)]
mod tests;
