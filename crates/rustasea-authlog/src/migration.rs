//! `authentication_log` table migration and process-wide registration.
//!
//! The DDL is dialect-portable (UUID `id`, `VARCHAR` identity columns,
//! `TIMESTAMPTZ` timestamps) so the same body runs on Postgres and SQLite.
//! Register it at application boot with [`register`] so `cargo artisan migrate`
//! creates the table; the scaffold also emits an equivalent migration.
//!
//! `user_id` is a `VARCHAR(255)`, not a UUID: [`rustasea_auth::AuthUser::id`]
//! is a `String`, and the authentication log must accept the same opaque
//! identity the auth guards use (this preempts the UUID-cast friction seen with
//! the activity log's `causer_id`).
//!
//! [`rustasea_auth::AuthUser::id`]: https://docs.rs/rustasea-auth

use rustasea_orm::{Migration, Result};

/// `authentication_log` table — the persisted sign-in history.
pub struct CreateAuthenticationLogTable;

impl Migration for CreateAuthenticationLogTable {
    /// Unique migration name.
    fn name(&self) -> &str {
        "2027_01_01_000009_create_authentication_log_table"
    }

    /// Create the `authentication_log` table plus its lookup indexes.
    fn up(&self) -> Result<String> {
        Ok("\
CREATE TABLE authentication_log (\
id UUID PRIMARY KEY, \
user_id VARCHAR(255) NULL, \
email VARCHAR(255) NULL, \
guard_name VARCHAR(255) NULL, \
event VARCHAR(64) NOT NULL, \
ip_address VARCHAR(45) NULL, \
user_agent TEXT NULL, \
successful BOOLEAN NOT NULL, \
login_at TIMESTAMPTZ NULL, \
logout_at TIMESTAMPTZ NULL, \
cleared_by_user_at TIMESTAMPTZ NULL, \
created_at TIMESTAMPTZ NOT NULL, \
updated_at TIMESTAMPTZ NOT NULL\
); \
CREATE INDEX authentication_log_user_id_index ON authentication_log (user_id); \
CREATE INDEX authentication_log_event_index ON authentication_log (event); \
CREATE INDEX authentication_log_ip_address_index ON authentication_log (ip_address)"
            .to_string())
    }

    /// Drop the `authentication_log` table.
    fn down(&self) -> Result<String> {
        Ok("DROP TABLE IF EXISTS authentication_log".to_string())
    }
}

/// Register the authentication-log migration into the process-wide migrator.
///
/// Call once from application boot (or use `AuthenticationLogLogger` directly
/// with the scaffolded migration).
pub fn register() {
    rustasea_orm::register_migration(CreateAuthenticationLogTable);
}
