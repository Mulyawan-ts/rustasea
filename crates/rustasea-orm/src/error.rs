/// Typed errors for the ORM layer.
use thiserror::Error;

/// Alias for results produced by ORM operations.
pub type Result<T> = std::result::Result<T, OrmError>;

/// Top-level ORM error type.
#[derive(Debug, Error)]
pub enum OrmError {
    /// No row matched the query (`firstOrFail`, `findOrFail`).
    #[error("model not found")]
    NotFound,

    /// Strict upsert was called with an empty `uniqueBy` list.
    #[error("upsert rejected: {0}")]
    Upsert(#[from] UpsertError),

    /// Invalid builder state (e.g. paginating an unpaged query twice).
    #[error("invalid query state: {0}")]
    InvalidState(String),

    /// A value could not be bound to the query.
    #[error("invalid bind value: {0}")]
    InvalidValue(String),

    /// A global scope was registered or combined illegally (duplicate id,
    /// empty table name, poisoned registry).
    #[error("invalid scope: {0}")]
    Scope(String),

    /// The configured driver is unsupported for this operation.
    #[error("unsupported driver: {0}")]
    UnsupportedDriver(String),

    /// Underlying storage/IO failure surfaced by the driver.
    #[error("storage error: {0}")]
    Storage(String),

    /// A query referenced a table that does not exist (or was not created).
    #[error("missing table: {table}")]
    MissingTable {
        /// Name of the missing table.
        table: String,
    },

    /// Connection-pool setup or acquisition failure (`connect`, `ping`).
    #[error("connection pool error: {0}")]
    Pool(String),

    /// Named-connection resolution failure (unknown name, missing field, …).
    #[error(transparent)]
    Connection(#[from] ConnectionError),

    /// Migration failure.
    #[error(transparent)]
    Migration(#[from] crate::migration::MigrationError),

    /// Transaction lifecycle failure (already committed or rolled back).
    #[error(transparent)]
    Transaction(#[from] crate::tx::TransactionError),

    /// Schema-builder failure (bad dialect, identifier, or blueprint shape).
    #[error(transparent)]
    Schema(#[from] SchemaError),

    /// Vector dimension mismatch: the column expects `expected` dimensions but
    /// the supplied embedding has `actual`.
    #[error("vector dimension mismatch: expected {expected}, got {actual}")]
    VectorDimensionMismatch {
        /// Expected column dimension.
        expected: usize,
        /// Actual embedding length.
        actual: usize,
    },

    /// An attribute cast could not convert a column value between its database
    /// representation and its Rust representation (corrupt or malformed data).
    #[error("cast error on `{column}`: {message}")]
    CastError {
        /// Column whose cast failed.
        column: String,
        /// Human-readable failure reason.
        message: String,
    },
}

impl From<sqlx::Error> for OrmError {
    /// Map a raw `sqlx` failure onto the ORM's storage error.
    fn from(error: sqlx::Error) -> Self {
        OrmError::Storage(error.to_string())
    }
}

/// Named-connection resolution errors raised by [`crate::connections`].
///
/// Every variant is a configuration problem detected *before* any socket is
/// opened: an unknown name, a missing required field, or a `driver`/URL scheme
/// disagreement. Driver-level failures surface separately as
/// [`OrmError::Pool`] / [`OrmError::UnsupportedDriver`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConnectionError {
    /// No connection is declared under the requested name.
    #[error("unknown database connection: {0}")]
    UnknownConnection(String),

    /// No connection map exists and no legacy `database.url` fallback is set.
    #[error("no database connection configured")]
    NotConfigured,

    /// The `[database]` config table exists but could not be deserialized.
    #[error("invalid database config: {0}")]
    InvalidConfig(String),

    /// A driver that cannot be mapped onto a supported URL scheme was declared.
    #[error("unsupported database driver `{driver}`")]
    UnsupportedDriver {
        /// The unrecognised driver (or URL scheme) string.
        driver: String,
    },

    /// A granular connection is missing a field required by its driver.
    #[error("connection is missing required field `{field}`")]
    MissingField {
        /// The missing field (e.g. `host`).
        field: String,
    },

    /// The declared `driver` disagrees with the URL scheme.
    #[error("driver `{driver}` does not match URL scheme `{scheme}`")]
    DriverMismatch {
        /// The declared driver.
        driver: String,
        /// The scheme parsed from the connection's `url`.
        scheme: String,
    },
}

/// Strict upsert errors — thrown before any round-trip.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpsertError {
    /// `upsert(rows, unique_by: [])` — previously silent, now strict.
    #[error("uniqueBy must be non-empty")]
    EmptyUniqueBy,

    /// Upsert row count exceeded the configured batch limit.
    #[error("upsert batch too large: {0} rows")]
    BatchTooLarge(usize),
}

/// Schema-builder errors raised by [`crate::schema`].
///
/// Every variant is detected *before* any SQL is executed: the caller asked for
/// an unsupported dialect, named a table/column with an illegal identifier, or
/// produced a blueprint that cannot be rendered (no columns, duplicate column).
/// The raw DDL string is only produced once validation passes, so a successful
/// [`crate::schema::Schema::create`] always yields executable SQL.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchemaError {
    /// The requested dialect is not one of `sqlite` / `postgres` / `mysql`.
    #[error("unknown dialect: {0}")]
    UnknownDialect(String),

    /// A table name was empty (or whitespace-only).
    #[error("empty table name")]
    EmptyTableName,

    /// A column name was empty (or whitespace-only).
    #[error("empty column name")]
    EmptyColumnName,

    /// A table or column identifier contained characters outside the allow-list.
    #[error("invalid identifier `{identifier}`")]
    InvalidIdentifier {
        /// The offending identifier.
        identifier: String,
    },

    /// A table-level `index`/`unique` declaration carried no columns.
    #[error("index declaration has no columns")]
    EmptyIndexColumns,

    /// A blueprint carried no column definitions.
    #[error("table `{table}` has no columns")]
    EmptyBlueprint {
        /// The table that was left empty.
        table: String,
    },

    /// The same column name was declared twice on one blueprint.
    #[error("duplicate column `{column}` on table `{table}`")]
    DuplicateColumn {
        /// The table being built.
        table: String,
        /// The repeated column name.
        column: String,
    },
}
