//! Migrations-section config for the `[database]` table.
//!
//! Split out of [`crate::connections`] to keep the parent module within the
//! file-size limit. [`MigrationsConfig`] mirrors Laravel's `database.php`
//! `migrations` block and is re-exported from the parent.

use serde::Deserialize;

/// Laravel-parity `[database.migrations]` settings.
///
/// `table` names the tracking table the ORM [`crate::Migrator`] records each
/// applied migration in (default `migrations`). `update_date_on_publish` is
/// parsed for parity with Laravel, which rewrites the row timestamp when a
/// published migration is re-published; RustaSea stamps `executed_at` on apply,
/// so the flag is currently advisory.
#[derive(Debug, Clone, Deserialize)]
pub struct MigrationsConfig {
    /// Name of the migration tracking table (default `migrations`).
    #[serde(default = "default_migrations_table")]
    pub table: String,
    /// Whether a re-published migration refreshes its recorded date.
    #[serde(default)]
    pub update_date_on_publish: bool,
}

impl Default for MigrationsConfig {
    /// Laravel defaults: table `migrations`, `update_date_on_publish = true`.
    fn default() -> Self {
        Self {
            table: default_migrations_table(),
            update_date_on_publish: true,
        }
    }
}

/// Default migration tracking table name (Laravel `migrations`).
fn default_migrations_table() -> String {
    "migrations".to_string()
}
