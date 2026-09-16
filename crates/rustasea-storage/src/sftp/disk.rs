//! SFTP disk: a `Storage`/`ManagedDisk` over a remote SFTP server.
//!
//! Keys are confined lexically (absolute keys, `..`/`.`/empty segments and
//! control characters are rejected → [`StorageError::PathTraversal`]) and
//! resolved under the configured remote root. Operations share one cached
//! session; a connection-class failure drops it, reconnects once, and retries
//! the operation a single time (bounded — no retry loops).

use std::path::PathBuf;

use async_trait::async_trait;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{OpenFlags, StatusCode};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::error::{Result, StorageError};
use crate::manager::ManagedDisk;
use crate::sftp::config::SftpDiskConfig;
use crate::sftp::connection::{connect, SftpConnection};
use crate::storage::Storage;

/// A remote file operation, dispatched against the cached session.
///
/// Modelled as a value (rather than a borrowing closure) so the retry wrapper
/// can re-run it after a reconnect without a lending-future bound.
enum Command {
    /// Read the file at `path`.
    Get(String),
    /// Write `data` to `path`, creating `parent` first when present.
    Put {
        /// Absolute remote path.
        path: String,
        /// Parent directory to create best-effort, when not at the root.
        parent: Option<String>,
        /// Bytes to write.
        data: Vec<u8>,
    },
    /// Probe whether `path` exists.
    Exists(String),
    /// Delete the file at `path`.
    Delete(String),
    /// List directory `dir`, prefixing results with `rel`.
    List {
        /// Absolute remote directory.
        dir: String,
        /// Key prefix prepended to each returned entry.
        rel: String,
    },
}

/// The result of a [`Command`].
enum Outcome {
    /// A unit result (`put`/`delete`).
    Unit,
    /// File bytes (`get`).
    Bytes(Vec<u8>),
    /// Existence flag (`exists`).
    Bool(bool),
    /// Listed keys (`list`).
    Keys(Vec<String>),
}

/// A disk backed by a remote SFTP server.
///
/// Construct lazily with [`SftpDisk::new`] (connect on first operation) or
/// eagerly with [`SftpDisk::connect`] (connect immediately).
pub struct SftpDisk {
    /// Connection settings (env overlay already applied by the facade).
    config: SftpDiskConfig,
    /// Label used in error metadata (`sftp:host`).
    label: String,
    /// Cached session; `None` until the first operation (or after a drop).
    session: Mutex<Option<SftpConnection>>,
}

impl std::fmt::Debug for SftpDisk {
    /// Render the disk without the live session (which is not `Debug`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpDisk")
            .field("config", &self.config)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl SftpDisk {
    /// Create a disk that connects lazily on its first operation.
    pub fn new(config: SftpDiskConfig) -> Self {
        let label = format!("sftp:{}", config.host);
        Self {
            config,
            label,
            session: Mutex::new(None),
        }
    }

    /// Create a disk and connect immediately, surfacing connection errors now.
    ///
    /// # Errors
    ///
    /// [`StorageError::Config`] / [`StorageError::ConnectionFailed`] from
    /// [`connect`].
    pub async fn connect(config: SftpDiskConfig) -> Result<Self> {
        let disk = Self::new(config);
        let connection = connect(&disk.config).await?;
        *disk.session.lock().await = Some(connection);
        Ok(disk)
    }

    /// The disk's connection configuration.
    pub fn config(&self) -> &SftpDiskConfig {
        &self.config
    }

    /// List keys under `prefix`, returning keys relative to the disk root.
    ///
    /// Directory entries are suffixed with `/`. A blank/`/` prefix lists the
    /// disk root. Mirrors flysystem's `listContents`; the [`Storage`] trait is
    /// deliberately unchanged.
    ///
    /// # Errors
    ///
    /// [`StorageError::PathTraversal`] for an escaping prefix; connection or
    /// store errors as classified by the operation.
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let command = Command::List {
            dir: self.config.list_dir(prefix)?,
            rel: SftpDiskConfig::relative_prefix(prefix),
        };
        match self.run(&command).await? {
            Outcome::Keys(keys) => Ok(keys),
            _ => Err(StorageError::StoreUnavailable(
                "unexpected list result".into(),
            )),
        }
    }

    /// Run `command` against the cached session, reconnecting once on failure.
    ///
    /// A connection-class error invalidates the cached session, reconnects, and
    /// retries `command` a single time. A second failure is returned unchanged
    /// (bounded retry).
    async fn run(&self, command: &Command) -> Result<Outcome> {
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            *guard = Some(connect(&self.config).await?);
        }

        let first = {
            let session = guard
                .as_ref()
                .ok_or_else(|| StorageError::ConnectionFailed("no active session".into()))?;
            dispatch(session.sftp(), command).await
        };

        match first {
            Ok(outcome) => Ok(outcome),
            Err(error) if is_connection_error(&error) => {
                *guard = Some(connect(&self.config).await?);
                let session = guard
                    .as_ref()
                    .ok_or_else(|| StorageError::ConnectionFailed("no active session".into()))?;
                dispatch(session.sftp(), command).await
            }
            Err(error) => Err(error),
        }
    }
}

/// Execute one [`Command`] against a live SFTP session.
async fn dispatch(sftp: &SftpSession, command: &Command) -> Result<Outcome> {
    match command {
        Command::Get(path) => {
            let bytes = sftp.read(path.as_str()).await.map_err(|e| {
                if is_not_found(&e) {
                    StorageError::NotFound(path.clone())
                } else {
                    map_sftp_error(path, e)
                }
            })?;
            Ok(Outcome::Bytes(bytes))
        }
        Command::Put { path, parent, data } => {
            if let Some(parent) = parent {
                create_dir_all(sftp, parent).await;
            }
            let mut file = sftp
                .open_with_flags(
                    path.as_str(),
                    OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                )
                .await
                .map_err(|e| map_sftp_error(path, e))?;
            file.write_all(data).await.map_err(map_io_error)?;
            file.shutdown().await.map_err(map_io_error)?;
            Ok(Outcome::Unit)
        }
        Command::Exists(path) => {
            let exists = sftp
                .try_exists(path.as_str())
                .await
                .map_err(|e| map_sftp_error(path, e))?;
            Ok(Outcome::Bool(exists))
        }
        Command::Delete(path) => match sftp.remove_file(path.as_str()).await {
            Ok(()) => Ok(Outcome::Unit),
            // Absent files are a no-op (parity with LocalDisk).
            Err(e) if is_not_found(&e) => Ok(Outcome::Unit),
            Err(e) => Err(map_sftp_error(path, e)),
        },
        Command::List { dir, rel } => {
            let entries = sftp
                .read_dir(dir.as_str())
                .await
                .map_err(|e| map_sftp_error(dir, e))?;
            let mut keys = Vec::new();
            for entry in entries {
                let name = entry.file_name();
                let is_dir = entry.file_type().is_dir();
                let mut key = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                if is_dir {
                    key.push('/');
                }
                keys.push(key);
            }
            keys.sort();
            Ok(Outcome::Keys(keys))
        }
    }
}

#[async_trait]
impl Storage for SftpDisk {
    async fn get(&self, key: &str) -> Result<Vec<u8>> {
        let path = self.config.remote_path(key)?;
        match self.run(&Command::Get(path)).await? {
            Outcome::Bytes(bytes) => Ok(bytes),
            _ => Err(StorageError::StoreUnavailable(
                "unexpected get result".into(),
            )),
        }
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let path = self.config.remote_path(key)?;
        let parent = parent_dir(&path);
        let command = Command::Put {
            path,
            parent,
            data: bytes.to_vec(),
        };
        self.run(&command).await.map(|_| ())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        let path = self.config.remote_path(key)?;
        match self.run(&Command::Exists(path)).await? {
            Outcome::Bool(exists) => Ok(exists),
            _ => Err(StorageError::StoreUnavailable(
                "unexpected exists result".into(),
            )),
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let path = self.config.remote_path(key)?;
        self.run(&Command::Delete(path)).await.map(|_| ())
    }
}

#[async_trait]
impl ManagedDisk for SftpDisk {
    fn path(&self, key: &str) -> Result<PathBuf> {
        // Lexical confinement only: the key grammar rejects traversal segments
        // without touching the network (mirrors ObjectDisk).
        SftpDiskConfig::validate_key(key)?;
        Ok(PathBuf::from(key))
    }

    fn label(&self) -> String {
        self.label.clone()
    }
}

/// Whether an error is connection-class (eligible for the single retry).
fn is_connection_error(error: &StorageError) -> bool {
    matches!(error, StorageError::ConnectionFailed(_))
}

/// Whether an SFTP error means the remote path does not exist.
fn is_not_found(error: &SftpError) -> bool {
    matches!(
        error,
        SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile
    )
}

/// Map an SFTP client failure into the storage error space.
///
/// Missing files surface as [`StorageError::NotFound`]; connection-class
/// failures (`NoConnection`, `ConnectionLost`, timeouts, send/recv errors)
/// surface as [`StorageError::ConnectionFailed`]; everything else is a
/// [`StorageError::StoreUnavailable`].
fn map_sftp_error(path: &str, error: SftpError) -> StorageError {
    if is_not_found(&error) {
        return StorageError::NotFound(path.to_string());
    }
    match error {
        SftpError::Timeout => {
            StorageError::ConnectionFailed(format!("timed out talking to {path}"))
        }
        SftpError::IO(message) => StorageError::ConnectionFailed(format!("{path}: {message}")),
        SftpError::UnexpectedBehavior(message) => {
            StorageError::ConnectionFailed(format!("{path}: {message}"))
        }
        SftpError::Status(status)
            if matches!(
                status.status_code,
                StatusCode::NoConnection | StatusCode::ConnectionLost
            ) =>
        {
            StorageError::ConnectionFailed(format!("{path}: {}", status.status_code))
        }
        other => StorageError::StoreUnavailable(format!("{path}: {other}")),
    }
}

/// Map a `tokio` I/O failure from a write into the storage error space.
fn map_io_error(error: std::io::Error) -> StorageError {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::BrokenPipe
        | ErrorKind::ConnectionAborted
        | ErrorKind::ConnectionReset
        | ErrorKind::NotConnected
        | ErrorKind::UnexpectedEof => StorageError::ConnectionFailed(error.to_string()),
        _ => StorageError::StoreUnavailable(error.to_string()),
    }
}

/// Best-effort recursive remote directory creation (errors ignored).
///
/// SFTP's `mkdir` is non-recursive, so each missing ancestor is created in
/// turn. Failures are swallowed because the following open surfaces any real
/// problem with a precise error.
async fn create_dir_all(sftp: &SftpSession, dir: &str) {
    if dir.is_empty() || dir == "/" {
        return;
    }
    let mut current = String::new();
    for segment in dir.split('/') {
        if segment.is_empty() {
            continue;
        }
        current.push('/');
        current.push_str(segment);
        if sftp.try_exists(current.as_str()).await.unwrap_or(false) {
            continue;
        }
        let _ = sftp.create_dir(current.as_str()).await;
    }
}

/// The parent directory of a remote path, or `None` at the root.
fn parent_dir(path: &str) -> Option<String> {
    match path.rfind('/') {
        Some(0) => None,
        Some(index) => Some(path[..index].to_string()),
        None => None,
    }
}
