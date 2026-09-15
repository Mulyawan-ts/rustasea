//! Process-wide install points for the broadcast config/gate/manager
//! (ADOPT-022).
//!
//! The app installs the parsed config, the authorization gate, and the built
//! [`BroadcastManager`](super::BroadcastManager) at boot; handlers read them
//! back through the accessors. Each cell is an [`RwLock`](std::sync::RwLock)
//! inside a [`OnceLock`](std::sync::OnceLock) so a later `configure()` (or a
//! test) can replace an installed value, while production installs it once.

use std::sync::{Arc, OnceLock, RwLock};

use crate::channel::Authorize;

use super::{BroadcastManager, BroadcastingConfig};

/// Process-wide parsed config installed by the app at boot (ADOPT-022).
static CONFIG: OnceLock<RwLock<Option<BroadcastingConfig>>> = OnceLock::new();

/// Access the config cell, initializing it to empty on first use.
fn config_cell() -> &'static RwLock<Option<BroadcastingConfig>> {
    CONFIG.get_or_init(|| RwLock::new(None))
}

/// Install the parsed broadcast config (called by the app at boot).
pub fn set_config(config: BroadcastingConfig) {
    if let Ok(mut guard) = config_cell().write() {
        *guard = Some(config);
    }
}

/// The installed config, or `None` before [`set_config`] runs.
pub fn config() -> Option<BroadcastingConfig> {
    config_cell().read().ok().and_then(|guard| guard.clone())
}

/// Process-wide authorization gate installed by the app at boot.
static GATE: OnceLock<RwLock<Option<Arc<dyn Authorize>>>> = OnceLock::new();

/// Access the gate cell, initializing it to empty on first use.
fn gate_cell() -> &'static RwLock<Option<Arc<dyn Authorize>>> {
    GATE.get_or_init(|| RwLock::new(None))
}

/// Install the channel authorization gate (called by the app at boot).
pub fn set_gate(gate: Arc<dyn Authorize>) {
    if let Ok(mut guard) = gate_cell().write() {
        *guard = Some(gate);
    }
}

/// The installed gate, or `None` before [`set_gate`] runs.
pub fn gate() -> Option<Arc<dyn Authorize>> {
    gate_cell().read().ok().and_then(|guard| guard.clone())
}

/// Process-wide built manager installed by the app at boot.
///
/// The route handler resolves this to authorize channel subscriptions and to
/// produce Pusher client-auth signatures without rebuilding the connection set
/// per request.
static MANAGER: OnceLock<RwLock<Option<Arc<BroadcastManager>>>> = OnceLock::new();

/// Access the manager cell, initializing it to empty on first use.
fn manager_cell() -> &'static RwLock<Option<Arc<BroadcastManager>>> {
    MANAGER.get_or_init(|| RwLock::new(None))
}

/// Install a built manager (called by the app at boot).
pub fn set_manager(manager: Arc<BroadcastManager>) {
    if let Ok(mut guard) = manager_cell().write() {
        *guard = Some(manager);
    }
}

/// The installed manager, or `None` before [`set_manager`] runs.
pub fn manager() -> Option<Arc<BroadcastManager>> {
    manager_cell().read().ok().and_then(|guard| guard.clone())
}

/// Clear the installed config, gate, and manager (test reset hook).
pub fn clear() {
    if let Ok(mut guard) = config_cell().write() {
        *guard = None;
    }
    if let Ok(mut guard) = gate_cell().write() {
        *guard = None;
    }
    if let Ok(mut guard) = manager_cell().write() {
        *guard = None;
    }
}
