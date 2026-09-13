//! `cargo xtask migrate` — run the framework's registered migrations.
//!
//! Mirrors the CLI `migrate` command wiring
//! (`crates/rustasea-cli/src/commands/ops.rs:312-429`): resolve the database URL
//! from the same layered config source, register the framework migrations, open a
//! real [`DbPool`], and execute the process-wide [`Migrator`]. The bootstrap is
//! intentionally duplicated rather than imported because the CLI's helpers are
//! crate-private; keep both call sites in sync when the config precedence changes.

use rustasea_config::ConfigLoader;
use rustasea_orm::{registered_migrator, ConnectionError, DatabaseConfig, DbPool, OrmError};

use crate::FAILURE;

/// Return the database URL from the process environment, when set and non-blank.
///
/// Mirrors the CLI's `database_url_from_env`: `DATABASE_URL` wins over the
/// nested `DATABASE__URL` overlay key, and both are checked before config so an
/// explicit environment value can never be shadowed by `config/database.toml`.
fn database_url_from_env() -> Option<String> {
    ["DATABASE_URL", "DATABASE__URL"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|url| !url.trim().is_empty()))
}

/// Resolve the database connection URL from layered config.
///
/// Precedence mirrors the CLI's `database_url` exactly (the two call sites are
/// kept in sync): `DATABASE_URL`, then `DATABASE__URL`, then the config default
/// connection — a named `[database.connections.<default>]` entry or the legacy
/// flat `database.url` folded into an implicit default — then the legacy
/// top-level `database_url` key. Named connections resolve through the shared
/// [`DatabaseConfig`], so single-URL behaviour is unchanged.
fn database_url() -> Result<String, String> {
    if let Some(url) = database_url_from_env() {
        return Ok(url);
    }

    let loader = ConfigLoader::load_from(&["config/database", "config/app"])
        .map_err(|error| format!("failed to load database config: {error}"))?;

    match DatabaseConfig::from_loader(&loader).and_then(|config| config.resolve_url(None)) {
        Ok(url) => return Ok(url),
        Err(OrmError::Connection(ConnectionError::NotConfigured)) => {}
        Err(error) => return Err(error.to_string()),
    }
    if let Ok(url) = loader.get_key::<String>("database_url") {
        if !url.trim().is_empty() {
            return Ok(url);
        }
    }
    Err("database URL not configured — set `database.url` in config/database.toml or the DATABASE_URL env var".to_string())
}

/// Run every pending migration against the configured database.
///
/// Registers the framework's built-in migrations (queue `jobs`/`failed_jobs`)
/// before executing the process-wide registry, matching `cargo artisan migrate`.
/// Returns a process exit code; arguments are accepted for forward compatibility.
pub fn run(_args: &[String]) -> i32 {
    rustasea_queue::register_queue_migrations();
    let migrator = registered_migrator();
    if migrator.is_empty() {
        eprintln!("xtask migrate: no migrations registered.");
        return FAILURE;
    }

    let url = match database_url() {
        Ok(url) => url,
        Err(message) => {
            eprintln!("xtask migrate: {message}");
            return FAILURE;
        }
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("xtask migrate: failed to start async runtime: {error}");
            return FAILURE;
        }
    };

    runtime.block_on(async move {
        let pool = match DbPool::connect(&url).await {
            Ok(pool) => pool,
            Err(error) => {
                eprintln!("xtask migrate: failed to connect to database: {error}");
                return FAILURE;
            }
        };

        let applied = match migrator.run(&pool).await {
            Ok(applied) => applied,
            Err(error) => {
                eprintln!("xtask migrate: {error}");
                pool.close().await;
                return FAILURE;
            }
        };

        if applied.is_empty() {
            println!("xtask migrate: nothing to migrate.");
        } else {
            println!("xtask migrate: applied {} migration(s):", applied.len());
            for name in &applied {
                println!("  migrated: {name}");
            }
        }
        pool.close().await;
        0
    })
}
