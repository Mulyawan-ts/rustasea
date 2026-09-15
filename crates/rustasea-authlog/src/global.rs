//! Process-wide logger install helpers.
//!
//! The HTTP layer resolves the authentication logger through a process-wide
//! slot, so the same instance serves every request. This mirrors the activity
//! log's registry but keeps the cell local to this crate: the slot is
//! poison-tolerant and never panics, and [`record_event`] is a no-op when no
//! logger is installed (a CLI/queue/test context).

use std::sync::{Arc, OnceLock, RwLock};

use crate::error::Result;
use crate::event::AuthLogEvent;
use crate::recorder::AuthenticationLogLogger;

/// The process-wide logger slot.
fn logger_cell() -> &'static RwLock<Option<Arc<AuthenticationLogLogger>>> {
    static CELL: OnceLock<RwLock<Option<Arc<AuthenticationLogLogger>>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(None))
}

/// Install `logger` as the process-wide authentication recorder.
pub fn install(logger: Arc<AuthenticationLogLogger>) {
    if let Ok(mut slot) = logger_cell().write() {
        *slot = Some(logger);
    }
}

/// Remove the process-wide authentication recorder.
pub fn clear() {
    if let Ok(mut slot) = logger_cell().write() {
        *slot = None;
    }
}

/// The installed logger, or `None` when none is installed.
///
/// Poison-tolerant: a poisoned lock yields `None` rather than panicking.
pub fn logger() -> Option<Arc<AuthenticationLogLogger>> {
    logger_cell()
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().map(Arc::clone))
}

/// Record `event` through the installed logger, or no-op when none is installed.
///
/// This is the best-effort convenience the HTTP layer calls: a context with no
/// logger (CLI, queue, unit tests) simply skips recording and returns `Ok(())`.
pub async fn record_event(event: &AuthLogEvent) -> Result<()> {
    match logger() {
        Some(logger) => logger.record(event).await,
        None => Ok(()),
    }
}
