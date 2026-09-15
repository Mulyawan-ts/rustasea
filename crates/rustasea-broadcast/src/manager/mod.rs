//! Broadcast manager — named connections, config resolution, and dispatch
//! (ADOPT-022).
//!
//! [`BroadcastingConfig`] parses the `[broadcasting]` config table (Laravel
//! `config/broadcasting.php` shape: a `default` selector plus a `connections`
//! map) and bridges the documented environment variables. [`BroadcastManager`]
//! owns the built connections (the in-process [`BroadcastHub`](crate::hub::BroadcastHub)
//! plus the optional `pusher`/`redis` drivers) and is the façade the app uses to
//! publish events and authorize channel subscriptions.
//!
//! ```toml
//! [broadcasting]
//! default = "hub"
//! [broadcasting.connections.pusher]
//! app_id = "123456"
//! key = "app-key"
//! secret = "app-secret"
//! cluster = "us2"
//! ```
//!
//! A missing `[broadcasting]` table is tolerated (the default `hub` connection
//! with no external drivers). A driver that is *selected but not fully
//! configured* surfaces a typed [`BroadcastError`] rather than silently
//! degrading to the in-process hub, so a migrated production config can never
//! quietly drop external delivery (ADOPT-022).
//!
//! The module is split so no file exceeds the workspace line cap:
//! [`config`] (config parsing + env bridge), [`registry`] (the connection
//! registry), [`slots`] (process-wide install points), and [`tests`].

mod config;
mod registry;
mod slots;

pub use config::BroadcastingConfig;
pub use registry::BroadcastManager;
pub use slots::{clear, config, gate, manager, set_config, set_gate, set_manager};

/// Connection name of the in-process hub (always available with the `ws`
/// feature).
pub const CONNECTION_HUB: &str = "hub";
/// Connection name of the Pusher HTTP driver (`pusher` feature).
pub const CONNECTION_PUSHER: &str = "pusher";
/// Connection name of the Redis Pub/Sub fan-out driver (`redis` feature).
pub const CONNECTION_REDIS: &str = "redis";

/// Connection used when `broadcasting.default` is absent.
pub const DEFAULT_CONNECTION: &str = "hub";

#[cfg(test)]
mod tests;
