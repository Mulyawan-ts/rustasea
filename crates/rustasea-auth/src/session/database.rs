/// Database-backed `tower_sessions::SessionStore` over the ORM connection pool.
///
/// Selected with `driver = "database"` in `config/session.toml`, this store
/// persists every session record in a `sessions` table so logins survive a
/// process restart — the property the in-memory [`tower_sessions::MemoryStore`]
/// cannot offer. The row shape matches the scaffold migration
/// (`create_sessions_table`):
///
/// ```sql
/// sessions(
///     id            VARCHAR(255) PRIMARY KEY, -- the tower-sessions session id
///     user_id       UUID NULL,                -- reserved; left NULL for now
///     payload       TEXT,                     -- JSON-encoded session data map
///     last_activity BIGINT                    -- the record's expiry (UNIX secs)
/// )
/// ```
///
/// The configured connection (`session.connection`) and table
/// (`session.table`) are supplied by [`DatabaseSessionStore::from_config`], which
/// resolves the connection through the ORM [`ConnectionResolver`]; callers that
/// already hold a [`DbPool`] use [`DatabaseSessionStore::new`] directly.
///
/// # Operational trade-offs
///
/// - **A database round trip on every request.** Unlike the in-memory store,
///   each `load`/`save`/`delete` hits the database, so a session read adds a
///   query to every authenticated request. Deployments under heavy load should
///   front this store with a cache (`tower_sessions::CachingSessionStore`) or a
///   connection pool sized for the added concurrency.
/// - **A cleanup / GC task is required.** Expired rows are *not* removed on
///   read; the store filters them out (returning `None`) but leaves the row for
///   a garbage collector. Run [`ExpiredDeletion::delete_expired`] periodically
///   (or the core-crate `continuously_delete_expired` loop, behind the
///   `tower-sessions-core` `deletion-task` feature) so the table does not grow
///   without bound. Because the column stores the record's **expiry**, the GC
///   condition is simply `last_activity <= now`; the session `lifetime` governs
///   how far in the future `expiry` is set when the guard writes a record.
/// - **`session.encrypt` is not implemented.** The `payload` column stores the
///   session data map as **plaintext JSON**. Enabling `encrypt = true` has no
///   effect on this store; do not treat the table as encrypted at rest. Encrypt
///   the underlying storage (disk/TDE) if confidentiality is required.
///
/// # Error handling
///
/// Every failure — an unreachable database, a **missing `sessions` table**, or
/// an unparsable payload — is surfaced as a typed
/// [`tower_sessions::session_store::Error`] (`Backend`/`Decode`), never a panic.
/// The guard maps these onto [`crate::error::AuthError::StoreUnavailable`].
use std::collections::HashMap;

use async_trait::async_trait;
use time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, ExpiredDeletion, SessionStore};

use rustasea_orm::{ConnectionResolver, DbPool, Value};

use crate::config::ConfigResult;
use crate::error::AuthConfigError;

use crate::config::SessionConfig;

/// Table column holding the JSON-encoded session data map.
const COLUMN_PAYLOAD: &str = "payload";
/// Table column holding the record expiry as UNIX seconds.
const COLUMN_LAST_ACTIVITY: &str = "last_activity";

/// A [`SessionStore`] persisting sessions in a relational `sessions` table.
///
/// Construct one with [`DatabaseSessionStore::from_config`] (config-driven) or
/// [`DatabaseSessionStore::new`] (explicit pool + table). Cloning is cheap: the
/// inner [`DbPool`] is a handle to a shared connection pool.
#[derive(Debug, Clone)]
pub struct DatabaseSessionStore {
    /// Shared ORM connection pool the store reads/writes through.
    pool: DbPool,
    /// Name of the sessions table (validated before every query).
    table: String,
}

impl DatabaseSessionStore {
    /// Create a store over `pool` using `table` for session rows.
    ///
    /// The table name is validated lazily on each query
    /// ([`DatabaseSessionStore::checked_table`]); an invalid identifier is a
    /// typed store error rather than injected SQL.
    pub fn new(pool: DbPool, table: impl Into<String>) -> Self {
        Self {
            pool,
            table: table.into(),
        }
    }

    /// Build a store from `session.toml`, resolving `config.connection`.
    ///
    /// The connection name is resolved through `resolver`: an empty name or the
    /// literal `"default"` selects the resolver's configured default connection
    /// (the Laravel `session.connection = null` behaviour), and any other name is
    /// resolved by name. The table comes from `config.table`.
    ///
    /// # Errors
    ///
    /// [`AuthConfigError::SessionStoreUnavailable`] when the configured
    /// connection cannot be resolved or opened.
    pub async fn from_config(
        config: &SessionConfig,
        resolver: &ConnectionResolver,
    ) -> ConfigResult<Self> {
        let name = config.connection.trim();
        let name = if name.is_empty() || name.eq_ignore_ascii_case("default") {
            None
        } else {
            Some(name)
        };
        let pool = resolver
            .resolve(name)
            .await
            .map_err(|error| AuthConfigError::SessionStoreUnavailable(error.to_string()))?;
        Ok(Self::new(pool, config.table.clone()))
    }

    /// The configured table name.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The shared connection pool.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Validate the table identifier and return it, or a typed store error.
    ///
    /// Accepts `[A-Za-z_][A-Za-z0-9_]*`. The name is only ever interpolated
    /// into SQL after this check, so a hostile config value cannot inject SQL.
    fn checked_table(&self) -> session_store::Result<&str> {
        let table = self.table.trim();
        let mut chars = table.chars();
        let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid {
            Ok(table)
        } else {
            Err(session_store::Error::Backend(format!(
                "invalid sessions table name {table:?}"
            )))
        }
    }

    /// Write `record` as an upsert (UPDATE, else INSERT).
    ///
    /// Both [`SessionStore::create`] and [`SessionStore::save`] funnel here. On
    /// the astronomically unlikely id collision the newer record overwrites the
    /// older one (the id is a 128-bit random value).
    async fn write(&self, record: &Record) -> session_store::Result<()> {
        let table = self.checked_table()?;
        let payload = serde_json::to_string(&record.data)
            .map_err(|error| session_store::Error::Encode(error.to_string()))?;
        let last_activity = record.expiry_date.unix_timestamp();
        let id = record.id.to_string();

        let update = format!(
            "UPDATE {table} SET {COLUMN_PAYLOAD} = $1, {COLUMN_LAST_ACTIVITY} = $2 WHERE id = $3"
        );
        let affected = self
            .pool
            .execute_bind(
                &update,
                &[
                    Value::Text(payload.clone()),
                    Value::Int(last_activity),
                    Value::Text(id.clone()),
                ],
            )
            .await
            .map_err(backend)?;

        if affected == 0 {
            let insert = format!(
                "INSERT INTO {table} (id, {COLUMN_PAYLOAD}, {COLUMN_LAST_ACTIVITY}) \
                 VALUES ($1, $2, $3)"
            );
            self.pool
                .execute_bind(
                    &insert,
                    &[
                        Value::Text(id),
                        Value::Text(payload),
                        Value::Int(last_activity),
                    ],
                )
                .await
                .map_err(backend)?;
        }
        Ok(())
    }
}

#[async_trait]
impl SessionStore for DatabaseSessionStore {
    /// Insert a new session row (upsert; see [`DatabaseSessionStore::write`]).
    async fn create(&self, session_record: &mut Record) -> session_store::Result<()> {
        self.write(session_record).await
    }

    /// Persist an existing session row (upsert; see [`DatabaseSessionStore::write`]).
    async fn save(&self, session_record: &Record) -> session_store::Result<()> {
        self.write(session_record).await
    }

    /// Load a session record, or `None` when it is absent or already expired.
    ///
    /// An expired row is filtered out here but **not deleted** — a GC task must
    /// reap it (see the module-level trade-offs).
    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let table = self.checked_table()?;
        let sql = format!(
            "SELECT {COLUMN_PAYLOAD}, {COLUMN_LAST_ACTIVITY} FROM {table} WHERE id = $1 LIMIT 1"
        );
        let rows = self
            .pool
            .fetch_json(&sql, &[Value::Text(session_id.to_string())])
            .await
            .map_err(backend)?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };

        let payload = row
            .get(COLUMN_PAYLOAD)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                session_store::Error::Decode(format!("missing {COLUMN_PAYLOAD} column"))
            })?;
        let data: HashMap<String, serde_json::Value> = serde_json::from_str(payload)
            .map_err(|error| session_store::Error::Decode(error.to_string()))?;
        let last_activity = row
            .get(COLUMN_LAST_ACTIVITY)
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                session_store::Error::Decode(format!("missing {COLUMN_LAST_ACTIVITY} column"))
            })?;
        let expiry_date = OffsetDateTime::from_unix_timestamp(last_activity)
            .map_err(|error| session_store::Error::Decode(error.to_string()))?;

        if expiry_date <= OffsetDateTime::now_utc() {
            return Ok(None);
        }
        Ok(Some(Record {
            id: *session_id,
            data,
            expiry_date,
        }))
    }

    /// Delete a session row by id.
    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        let table = self.checked_table()?;
        let sql = format!("DELETE FROM {table} WHERE id = $1");
        self.pool
            .execute_bind(&sql, &[Value::Text(session_id.to_string())])
            .await
            .map_err(backend)?;
        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for DatabaseSessionStore {
    /// Delete every row whose stored expiry is at or before now.
    ///
    /// Because `last_activity` holds the record's expiry, this is a single
    /// `DELETE … WHERE last_activity <= now`. Schedule it (or the
    /// feature-gated [`ExpiredDeletion::continuously_delete_expired`] loop)
    /// alongside the app so the table stays bounded.
    async fn delete_expired(&self) -> session_store::Result<()> {
        let table = self.checked_table()?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let sql = format!("DELETE FROM {table} WHERE {COLUMN_LAST_ACTIVITY} <= $1");
        self.pool
            .execute_bind(&sql, &[Value::Int(now)])
            .await
            .map_err(backend)?;
        Ok(())
    }
}

/// Map an ORM error onto a typed tower-sessions backend error (never a panic).
fn backend(error: rustasea_orm::OrmError) -> session_store::Error {
    session_store::Error::Backend(error.to_string())
}
