//! Unit tests for the config-driven connection parser and resolver.
//!
//! Covers named-connection parsing, URL resolution, the legacy flat-URL
//! synthesis, granular field building, the read/write split overlays, and the
//! typed errors for unknown names, missing fields, driver/URL mismatch, and
//! unsupported drivers.
//!
//! The suite is split across sibling submodules to respect the file-size
//! standard: [`driver`] covers the driver matrix, [`precedence`] the default
//! selector and pool precedence, and [`read_write`] the read/write overlays.

mod driver;
mod precedence;
mod read_write;

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A throwaway directory holding a `database.toml` for loader tests.
pub(super) struct TempConfigDir {
    path: PathBuf,
}

impl TempConfigDir {
    /// Create an empty temp directory with a process-unique name.
    pub(super) fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustasea-orm-connections-{}-{}",
            std::process::id(),
            unique
        ));
        std::fs::create_dir_all(&path).expect("create temp config dir");
        Self { path }
    }

    /// Write `contents` to `database.toml` inside the directory.
    pub(super) fn write_database(&self, contents: &str) {
        std::fs::write(self.path.join("database.toml"), contents)
            .expect("write temp database.toml");
    }

    /// The directory path, for handing to [`ConfigLoader::load_from_dir`].
    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempConfigDir {
    /// Remove the temp directory and all of its contents.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// TOML exercising the full three-connection surface plus a legacy `url`.
///
/// Mirrors the shipped `config/database.toml` shape, including the inline
/// `[database].pool` table, so the documented config is covered by a test.
pub(super) const THREE_CONNECTIONS: &str = r#"
[database]
default = "sqlite"
url = "sqlite://legacy.sqlite"
pool = { min = 1, max = 10, idle_timeout = 600 }

[database.connections.sqlite]
driver = "sqlite"
url = "sqlite://database.sqlite?mode=rwc"

[database.connections.pgsql]
driver = "postgres"
host = "db.example"
port = 5433
database = "app"
username = "user"
password = "pass"

[database.connections.mysql]
driver = "mysql"
host = "db.example"
database = "app"
username = "user"
"#;

/// Parse the three-connection config through the layered loader.
pub(super) fn load_three() -> DatabaseConfig {
    let dir = TempConfigDir::new();
    dir.write_database(THREE_CONNECTIONS);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");
    DatabaseConfig::from_loader(&loader).expect("parse database config")
}

/// TOML with a granular `[….read]` overlay and a URL `[….read]` overlay.
pub(super) const SPLIT_CONNECTIONS: &str = r#"
[database]
default = "pgsql"

[database.connections.pgsql]
driver = "postgres"
host = "primary.internal"
port = 5432
database = "app"
username = "user"
password = "pass"

[database.connections.pgsql.read]
host = "replica.internal"

[database.connections.mysql]
driver = "mysql"
host = "primary.internal"
database = "app"
username = "user"

[database.connections.mysql.read]
url = "mysql://user@replica.internal:3306/app"
"#;

/// Parse the split-endpoint config through the layered loader.
pub(super) fn load_split() -> DatabaseConfig {
    let dir = TempConfigDir::new();
    dir.write_database(SPLIT_CONNECTIONS);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");
    DatabaseConfig::from_loader(&loader).expect("parse database config")
}
