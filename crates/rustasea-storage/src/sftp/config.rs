//! SFTP disk configuration (`[storage.disks.*]` with `driver = "sftp"`).
//!
//! Mirrors `league/flysystem-sftp-v3`'s connection keys (`host`, `port`,
//! `username`, `password`, `private_key_path`, `root`, `timeout`) plus an
//! optional SHA-256 host-key pin. The type is compiled unconditionally (the
//! facade needs it to parse an `sftp` disk even when the driver feature is
//! off); only the live disk implementation is feature-gated.

use serde::Deserialize;

use crate::error::{Result, StorageError};
use crate::facade::DiskSettings;

/// Default SSH port when `port` is omitted.
pub const DEFAULT_SFTP_PORT: u16 = 22;
/// Default remote root when `root` is omitted.
pub const DEFAULT_SFTP_ROOT: &str = "/";
/// Default per-request timeout (seconds) when `timeout` is omitted.
pub const DEFAULT_SFTP_TIMEOUT_SECS: u64 = 30;

/// Typed `[storage.disks.*]` configuration for the `sftp` driver.
///
/// `password` is redacted from [`Debug`] output so a debug dump or log line can
/// never leak the credential (security hygiene, ADOPT-025).
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct SftpDiskConfig {
    /// Remote host name or IP address.
    #[serde(default)]
    pub host: String,
    /// Remote SSH port (defaults to [`DEFAULT_SFTP_PORT`]).
    #[serde(default)]
    pub port: Option<u16>,
    /// SSH username.
    #[serde(default)]
    pub username: String,
    /// Password credential (prefer a secret manager or `SFTP_PASSWORD`).
    #[serde(default)]
    pub password: Option<String>,
    /// Path to an OpenSSH private key (preferred over `password` when set).
    #[serde(default)]
    pub private_key_path: Option<String>,
    /// Remote root every key is confined to (defaults to [`DEFAULT_SFTP_ROOT`]).
    #[serde(default)]
    pub root: Option<String>,
    /// Per-request timeout in seconds (defaults to [`DEFAULT_SFTP_TIMEOUT_SECS`]).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Expected SHA-256 host-key fingerprint (`SHA256:…`); when set, a mismatch
    /// aborts the connection.
    #[serde(default)]
    pub host_key: Option<String>,
    /// Shared Laravel-parity disk settings.
    #[serde(flatten, default)]
    pub settings: DiskSettings,
}

impl std::fmt::Debug for SftpDiskConfig {
    /// Render the config with the `password` masked.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpDiskConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("private_key_path", &self.private_key_path)
            .field("root", &self.root)
            .field("timeout", &self.timeout)
            .field("host_key", &self.host_key)
            .field("settings", &self.settings)
            .finish()
    }
}

impl SftpDiskConfig {
    /// Effective SSH port (explicit `port` or [`DEFAULT_SFTP_PORT`]).
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_SFTP_PORT)
    }

    /// Effective remote root (explicit non-blank `root` or [`DEFAULT_SFTP_ROOT`]).
    pub fn root(&self) -> &str {
        match self.root.as_deref() {
            Some(root) if !root.trim().is_empty() => root,
            _ => DEFAULT_SFTP_ROOT,
        }
    }

    /// Effective per-request timeout in seconds.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout.unwrap_or(DEFAULT_SFTP_TIMEOUT_SECS)
    }

    /// Overlay the documented `SFTP_*` environment variables.
    ///
    /// Single-underscore Laravel-style names (`SFTP_HOST`, `SFTP_PORT`,
    /// `SFTP_USERNAME`, `SFTP_PASSWORD`, `SFTP_PRIVATE_KEY`, `SFTP_ROOT`,
    /// `SFTP_TIMEOUT`) are the only environment bridge for this disk. A blank
    /// value is ignored; an unparseable `SFTP_PORT` / `SFTP_TIMEOUT` is also
    /// ignored (the file/default value wins) so a typo cannot brick the disk.
    pub fn apply_env(&mut self) {
        if let Some(value) = env_non_empty("SFTP_HOST") {
            self.host = value;
        }
        if let Some(value) = env_non_empty("SFTP_USERNAME") {
            self.username = value;
        }
        if let Some(value) = env_non_empty("SFTP_PASSWORD") {
            self.password = Some(value);
        }
        if let Some(value) = env_non_empty("SFTP_PRIVATE_KEY") {
            self.private_key_path = Some(value);
        }
        if let Some(value) = env_non_empty("SFTP_ROOT") {
            self.root = Some(value);
        }
        if let Some(port) = env_non_empty("SFTP_PORT").and_then(|value| value.parse::<u16>().ok()) {
            self.port = Some(port);
        }
        if let Some(secs) =
            env_non_empty("SFTP_TIMEOUT").and_then(|value| value.parse::<u64>().ok())
        {
            self.timeout = Some(secs);
        }
    }

    /// Validate that the mandatory connection keys are present.
    ///
    /// # Errors
    ///
    /// [`StorageError::Config`] when `host` or `username` is blank.
    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            return Err(StorageError::Config(
                "sftp disk requires a non-empty `host`".into(),
            ));
        }
        if self.username.trim().is_empty() {
            return Err(StorageError::Config(
                "sftp disk requires a non-empty `username`".into(),
            ));
        }
        Ok(())
    }

    /// Reject a logical key that escapes the disk root (NFR-Sec-03).
    ///
    /// Absolute keys, `..`/`.` segments, empty segments (`a//b`, leading or
    /// trailing `/`) and ASCII control characters are rejected with
    /// [`StorageError::PathTraversal`] — mirroring the strictness of
    /// [`ObjectDisk`](crate::ObjectDisk)'s object-path grammar.
    pub fn validate_key(key: &str) -> Result<()> {
        if key.starts_with('/') {
            return Err(StorageError::PathTraversal(format!(
                "absolute key not allowed: {key}"
            )));
        }
        if key.is_empty() {
            return Err(StorageError::PathTraversal("empty key".into()));
        }
        for segment in key.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Err(StorageError::PathTraversal(key.to_string()));
            }
            if segment.chars().any(|c| c.is_ascii_control()) {
                return Err(StorageError::PathTraversal(key.to_string()));
            }
        }
        Ok(())
    }

    /// Join a confined key onto the configured root (`{root}/{key}`).
    ///
    /// The root's trailing slashes are trimmed; a root of `""` or `"/"` yields
    /// `/{key}`.
    ///
    /// # Errors
    ///
    /// [`StorageError::PathTraversal`] via [`SftpDiskConfig::validate_key`].
    pub fn remote_path(&self, key: &str) -> Result<String> {
        Self::validate_key(key)?;
        let root = self.root().trim_end_matches('/');
        if root.is_empty() {
            Ok(format!("/{key}"))
        } else {
            Ok(format!("{root}/{key}"))
        }
    }

    /// Remote directory for a [`list`](crate::sftp::SftpDisk::list) prefix.
    ///
    /// A blank/`/` prefix maps to the disk root; otherwise the prefix is joined
    /// like a key. Surrounding slashes are ignored.
    ///
    /// # Errors
    ///
    /// [`StorageError::PathTraversal`] when the prefix escapes the root.
    pub fn list_dir(&self, prefix: &str) -> Result<String> {
        let prefix = prefix.trim_matches('/');
        if prefix.is_empty() {
            let root = self.root().trim_end_matches('/');
            return Ok(if root.is_empty() {
                "/".to_string()
            } else {
                root.to_string()
            });
        }
        self.remote_path(prefix)
    }

    /// Normalize a `list` prefix into the key prefix used for results.
    pub fn relative_prefix(prefix: &str) -> String {
        prefix.trim_matches('/').to_string()
    }
}

/// Read an environment variable, treating unset or blank values as absent.
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
