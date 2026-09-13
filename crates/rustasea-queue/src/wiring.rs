//! Config-driven queue driver registration — wire `[queue]` into the registry.
//!
//! [`register_from_config`] turns a parsed [`QueueConfig`] into live drivers in
//! the central [`crate::registry::Queue`] registry:
//!
//! * a `sync` connection installs a fresh [`crate::driver::SyncDriver`];
//! * a `database` connection installs a [`crate::driver::DatabaseDriver`] when a
//!   [`DbPool`] is supplied, using the configured `jobs` / `failed_jobs` table
//!   names;
//! * a `redis` connection installs a [`crate::driver::RedisDriver`] when the
//!   `redis` feature is enabled and a URL is available.
//!
//! The configured `queue.default` is recorded so
//! [`crate::registry::QueueRegistry::default_connection`] returns it instead of
//! the historical `sync` fallback; when no config is registered the fallback is
//! preserved, so existing callers are unaffected.
//!
//! ## Redis URL resolution
//!
//! RustaSea has no `[database.connections.<name>]` Redis section, so the redis
//! URL is read from the `REDIS_URL` environment variable by
//! [`register_from_config`]. Callers that own the URL (tests, multi-tenant
//! boots) should use [`register_from_config_with`] and pass it explicitly.

use std::sync::{Arc, RwLock};

use rustasea_orm::DbPool;

use crate::config::QueueConfig;
use crate::driver::{DatabaseDriver, SyncDriver};
use crate::error::Result;
use crate::registry::Queue;

/// The configured default connection, if [`register_from_config`] ran.
///
/// `None` preserves the historical `sync` fallback in
/// [`crate::registry::QueueRegistry::default_connection`].
static CONFIGURED_DEFAULT: RwLock<Option<&'static str>> = RwLock::new(None);

/// Record the default connection named by a parsed config.
///
/// The name is leaked into a `'static` string so it can be handed back by
/// [`crate::registry::QueueRegistry::default_connection`] without a lifetime.
/// Called once at boot per config; the last writer wins.
pub fn set_default_connection(name: &str) {
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    if let Ok(mut guard) = CONFIGURED_DEFAULT.write() {
        *guard = Some(leaked);
    }
}

/// The configured default connection, or `None` when nothing was configured.
pub fn configured_default_connection() -> Option<&'static str> {
    CONFIGURED_DEFAULT.read().ok().and_then(|guard| *guard)
}

/// Register every connection in `config`, reading the redis URL from `REDIS_URL`.
///
/// A convenience wrapper over [`register_from_config_with`] for callers that do
/// not resolve the redis URL themselves. The `REDIS_URL` environment variable is
/// read when a `redis` connection is declared.
///
/// # Errors
///
/// Any [`crate::error::QueueConfigError`] from validation, or a typed driver
/// error while building the redis driver.
pub fn register_from_config(config: &QueueConfig, pool: Option<DbPool>) -> Result<()> {
    let redis_url = std::env::var("REDIS_URL").ok();
    register_from_config_with(config, pool, redis_url.as_deref())
}

/// Register every connection in `config` with an explicit `redis_url`.
///
/// The `database` driver is installed only when `pool` is `Some`; a `database`
/// connection without a pool is skipped (a worker/dispatch then surfaces the
/// usual typed `UnknownConnection`). The `redis` driver is installed only when
/// the `redis` feature is enabled and `redis_url` is non-empty; without the
/// feature the connection is skipped so a Redis-less build still boots.
///
/// # Errors
///
/// [`crate::error::QueueConfigError`] from [`QueueConfig::validate`] (unknown
/// connection, unsupported driver, missing field), or a typed driver error
/// while building the redis driver.
pub fn register_from_config_with(
    config: &QueueConfig,
    pool: Option<DbPool>,
    redis_url: Option<&str>,
) -> Result<()> {
    config.validate()?;
    set_default_connection(&config.default);

    for (name, connection) in &config.connections {
        if connection.is_sync() {
            Queue::register_driver(name.clone(), Arc::new(SyncDriver::new()));
        } else if connection.is_database() {
            if let Some(pool) = pool.clone() {
                let driver = DatabaseDriver::with_tables(
                    pool,
                    config.jobs_table(connection),
                    config.failed_table(),
                );
                Queue::register_driver(name.clone(), Arc::new(driver));
            }
        } else if connection.is_redis() {
            register_redis_connection(name, redis_url)?;
        }
    }
    Ok(())
}

/// Install a redis driver under `name` when the `redis` feature is enabled.
///
/// Without the feature this is a no-op (the connection is validated as
/// supported but not wired), so a Redis-less build boots cleanly.
#[cfg(feature = "redis")]
fn register_redis_connection(name: &str, redis_url: Option<&str>) -> Result<()> {
    let driver = crate::driver::RedisDriver::from_url(redis_url)?;
    Queue::register_driver(name.to_string(), Arc::new(driver));
    Ok(())
}

/// Feature-disabled stub: a `redis` connection is validated but not wired.
#[cfg(not(feature = "redis"))]
fn register_redis_connection(_name: &str, _redis_url: Option<&str>) -> Result<()> {
    Ok(())
}

impl crate::registry::QueueRegistry {
    /// Register every connection declared in `config` (boot-time).
    ///
    /// A thin wrapper over [`register_from_config`]: installs the `sync` driver,
    /// the `database` driver when `pool` is `Some`, and the `redis` driver when
    /// the `redis` feature is enabled and `REDIS_URL` is set. Records
    /// `config.default` so [`crate::registry::QueueRegistry::default_connection`]
    /// returns it.
    ///
    /// # Errors
    ///
    /// Any [`crate::error::QueueConfigError`] (unknown connection, unsupported
    /// driver, missing field) or a typed driver error while building redis.
    pub fn configure_from(config: &QueueConfig, pool: Option<DbPool>) -> Result<()> {
        register_from_config(config, pool)
    }
}
