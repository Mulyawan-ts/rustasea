//! `audit_log` table migration and process-wide registration.
//!
//! The DDL is dialect-portable (UUID columns, JSONB/text `properties`,
//! `TIMESTAMPTZ` timestamps) so the same body runs on Postgres and SQLite.
//! Register it at application boot with [`register`] so `cargo artisan migrate`
//! creates the audit table; the scaffold also emits an equivalent migration.

use rustasea_orm::{Migration, Result};

/// `audit_log` table — the persisted activity/audit trail.
pub struct CreateAuditLogTable;

impl Migration for CreateAuditLogTable {
    /// Unique migration name.
    fn name(&self) -> &str {
        "2027_01_01_000008_create_audit_log_table"
    }

    /// Create the `audit_log` table plus its lookup indexes.
    fn up(&self) -> Result<String> {
        Ok("\
CREATE TABLE audit_log (\
id UUID PRIMARY KEY, \
batch_uuid UUID NULL, \
log_name VARCHAR(255) NOT NULL, \
description TEXT NOT NULL, \
subject_type VARCHAR(255) NULL, \
subject_id UUID NULL, \
 causer_type VARCHAR(255) NULL, \
 causer_id VARCHAR(255) NULL, \
properties JSONB NULL, \
created_at TIMESTAMPTZ NOT NULL, \
updated_at TIMESTAMPTZ NOT NULL\
); \
CREATE INDEX audit_log_subject_index ON audit_log (subject_type, subject_id); \
CREATE INDEX audit_log_causer_index ON audit_log (causer_type, causer_id); \
CREATE INDEX audit_log_batch_uuid_index ON audit_log (batch_uuid); \
CREATE INDEX audit_log_log_name_index ON audit_log (log_name)"
            .to_string())
    }

    /// Drop the `audit_log` table.
    fn down(&self) -> Result<String> {
        Ok("DROP TABLE IF EXISTS audit_log".to_string())
    }
}

/// Register the activity-log migration into the process-wide migrator.
///
/// Call once from application boot (or use `ActivityLogger` directly with the
/// scaffolded migration).
pub fn register() {
    rustasea_orm::register_migration(CreateAuditLogTable);
}
