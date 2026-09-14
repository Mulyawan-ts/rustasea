//! RustaSea i18n — translation dictionaries, interpolation, and pluralization.
//!
//! This crate is the RustaSea analogue of `Illuminate\Translation` (M3). It
//! loads locale dictionaries from `resources/lang/{locale}/*.toml` or `*.json`
//! and resolves dot-namespaced keys (`auth.failed` → file `auth.toml`, key
//! `failed`) against an active locale with a fallback locale.
//!
//! # Layout
//!
//! ```text
//! resources/lang/en/auth.toml   ->  auth.*
//! resources/lang/en/validation.json -> validation.*
//! ```
//!
//! Nested tables/objects flatten into dot keys, so `[passwords] reset = "…"`
//! inside `auth.toml` is reachable as `auth.passwords.reset`.
//!
//! # Lookup semantics
//!
//! - The active locale is consulted first, then the fallback locale.
//! - A key missing from **both** returns the raw key string — never an error.
//! - `:name` placeholders are replaced from `(name, value)` pairs.
//! - `trans_choice` selects a `|`-separated plural form for a count.
//!
//! # Example
//!
//! ```no_run
//! use rustasea_i18n::{TranslationLoader, Translator};
//!
//! # fn main() -> Result<(), rustasea_i18n::I18nError> {
//! let loader = TranslationLoader::default();
//! let translator = Translator::load(&loader, "en", "en")?;
//! assert_eq!(translator.trans("auth.failed", &[]), "These credentials do not match our records.");
//! # Ok(())
//! # }
//! ```
//!
//! # Process-wide helpers
//!
//! Install a translator once at boot and call [`__`] / [`trans_choice`] from
//! anywhere:
//!
//! ```no_run
//! use std::sync::Arc;
//! use rustasea_i18n::{set_translator, TranslationLoader, Translator};
//!
//! # fn main() -> Result<(), rustasea_i18n::I18nError> {
//! let translator = Translator::load(&TranslationLoader::default(), "en", "en")?;
//! set_translator(Arc::new(translator));
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod global;
pub mod loader;
pub mod message;
pub mod translator;

pub use error::{I18nError, Result};
pub use global::{clear_translator, set_translator, trans_choice, translator, __};
pub use loader::{TranslationLoader, Translations, DEFAULT_LANG_DIR};
pub use message::{choose_form, interpolate, Param};
pub use translator::Translator;

#[cfg(test)]
mod tests;
