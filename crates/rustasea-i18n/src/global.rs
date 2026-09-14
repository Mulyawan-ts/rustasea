//! Process-wide translator registry and the `__` / `trans_choice` helpers.
//!
//! Mirrors the mailer registry precedent (LARAVEL-001/016): a
//! `OnceLock<RwLock<Option<Arc<Translator>>>>` slot with a `set_*` installer and
//! an accessor. Application boot installs one [`Translator`]; request handlers
//! then call the free [`__`] and [`trans_choice`] helpers without threading a
//! translator through every signature.
//!
//! When no translator is installed, the helpers degrade gracefully: [`__`]
//! returns the raw key and [`trans_choice`] returns the raw key, so call sites
//! remain panic-free before boot wiring runs.

use std::sync::{Arc, OnceLock, RwLock};

use crate::message::Param;
use crate::translator::Translator;

/// Registry slot holding the process-wide translator.
static TRANSLATOR: OnceLock<RwLock<Option<Arc<Translator>>>> = OnceLock::new();

/// Access the lazily-initialized translator registry slot.
fn slot() -> &'static RwLock<Option<Arc<Translator>>> {
    TRANSLATOR.get_or_init(|| RwLock::new(None))
}

/// Install the process-wide translator used by [`__`] and [`trans_choice`].
pub fn set_translator(translator: Arc<Translator>) {
    *slot().write().unwrap_or_else(|p| p.into_inner()) = Some(translator);
}

/// Remove the process-wide translator, if any.
pub fn clear_translator() {
    *slot().write().unwrap_or_else(|p| p.into_inner()) = None;
}

/// The process-wide translator, when one has been installed.
pub fn translator() -> Option<Arc<Translator>> {
    slot().read().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Translate `key` using the process-wide translator, interpolating params.
///
/// Returns the raw `key` when no translator is installed or the key is missing
/// from both the active and fallback locales.
///
/// ```
/// let line = rustasea_i18n::__("auth.failed", &[]);
/// assert_eq!(line, "auth.failed");
/// ```
pub fn __(key: &str, params: &[Param<'_>]) -> String {
    match translator() {
        Some(translator) => translator.trans(key, params),
        None => key.to_string(),
    }
}

/// Translate `key` with pluralization using the process-wide translator.
///
/// Returns the raw `key` when no translator is installed or the key is missing
/// from both locales.
pub fn trans_choice(key: &str, count: i64, params: &[Param<'_>]) -> String {
    match translator() {
        Some(translator) => translator.trans_choice(key, count, params),
        None => key.to_string(),
    }
}
