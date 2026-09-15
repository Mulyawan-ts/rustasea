//! Pusher HTTP broadcast driver (ADOPT-022).
//!
//! Parity target: Laravel's `pusher` broadcast driver. [`PusherBroadcaster`]
//! implements the Pusher HTTP API (`POST /apps/{app_id}/events`) with the
//! documented request signing ([`signing`]) and a swappable HTTP
//! [`PusherTransport`] so the driver is testable without a network.
//!
//! It also implements client-side channel authorization
//! ([`PusherBroadcaster::auth_signature`]) for `private-`/`presence-`
//! subscriptions — the `POST /broadcasting/auth` contract.
//!
//! ## Endpoint resolution
//!
//! * `host` set → `{scheme}://{host}:{port}/apps/{app_id}/events`
//!   (defaults `scheme = http`, `port = 6001`) — a self-hosted Pusher-compatible
//!   server (e.g. `soketi`).
//! * otherwise → `https://api-{cluster}.pusher.com/apps/{app_id}/events`.
//!
//! ## Data encoding
//!
//! The Pusher `data` field MUST be a **JSON-encoded string**, not a JSON object;
//! the driver stringifies the payload's `data` value accordingly.
//!
//! The module is split so no file exceeds the workspace line cap: [`config`]
//! (typed config + channel-auth response), [`broadcaster`] (the driver),
//! [`signing`] (the HMAC/MD5 helpers), [`transport`] (the HTTP seam), and
//! [`tests`].

mod broadcaster;
mod config;
pub mod signing;
pub mod transport;

pub use broadcaster::PusherBroadcaster;
pub use config::{ChannelAuth, PusherConfig};
pub use transport::PusherTransport;

/// Default request timeout in seconds.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 10;
/// Default self-hosted Pusher port.
pub(crate) const DEFAULT_PORT: u16 = 6001;
/// Default self-hosted Pusher scheme.
pub(crate) const DEFAULT_SCHEME: &str = "http";
/// Pusher API version for the `auth_version` query parameter.
pub(crate) const AUTH_VERSION: &str = "1.0";

#[cfg(test)]
mod tests;
