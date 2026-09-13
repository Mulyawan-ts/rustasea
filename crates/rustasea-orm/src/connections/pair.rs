//! Read/write pool pairs for one named SQL connection.
//!
//! Extracted from [`crate::connections`] so the parent module stays focused on
//! config parsing and resolution. [`ConnectionPair`] is re-exported from the
//! parent, so `rustasea_orm::connections::ConnectionPair` keeps resolving.

use crate::db::DbPool;
use crate::error::Result;

/// A read/write pair of pools for one named connection.
///
/// Returned by [`crate::connections::ConnectionResolver`]. With no `read`
/// overlay, `read` and `write` are clones of the *same* pool (identity holds —
/// [`ConnectionPair::is_split`] is `false`); with a split they are distinct.
///
/// Routing rule: [`ConnectionPair::fetch_json`] is a read and uses the read
/// pool; [`ConnectionPair::execute_bind`] / [`ConnectionPair::execute_script`]
/// are writes and use the write pool (see [`crate::db`] for the rationale).
#[derive(Debug, Clone)]
pub struct ConnectionPair {
    read: DbPool,
    write: DbPool,
}

impl ConnectionPair {
    /// Pair a read pool with a write pool.
    pub fn new(read: DbPool, write: DbPool) -> Self {
        Self { read, write }
    }

    /// The pool serving reads (`SELECT` and other observers).
    pub fn read(&self) -> &DbPool {
        &self.read
    }

    /// The pool serving writes (mutations, scripts, transactions).
    pub fn write(&self) -> &DbPool {
        &self.write
    }

    /// Consume the pair, yielding the read pool.
    pub fn into_read(self) -> DbPool {
        self.read
    }

    /// Consume the pair, yielding the write pool.
    pub fn into_write(self) -> DbPool {
        self.write
    }

    /// Whether reads and writes target distinct pools (identity comparison).
    pub fn is_split(&self) -> bool {
        self.read.identity() != self.write.identity()
    }

    /// Ping both pools (read first); surfaces the first failure.
    pub async fn ping(&self) -> Result<()> {
        self.read.ping().await?;
        self.write.ping().await
    }

    /// Close both pools (a shared instance closed twice is a no-op).
    pub async fn close(&self) {
        self.read.close().await;
        self.write.close().await;
    }
}
