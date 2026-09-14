//! Typed errors for the internationalization layer.
//!
//! A missing translation key is **not** an error: the raw key is returned
//! instead (Laravel parity). [`I18nError`] is reserved for loader failures such
//! as an unreadable directory, unreadable file, or malformed dictionary.

use std::path::PathBuf;

use thiserror::Error;

/// Alias for results produced by translation-loading operations.
pub type Result<T> = std::result::Result<T, I18nError>;

/// Failures raised while discovering or parsing translation dictionaries.
///
/// Missing keys never surface here — see the module docs.
#[derive(Debug, Error)]
pub enum I18nError {
    /// A translation directory exists but cannot be read.
    #[error("translation directory is not readable: `{}`", .path.display())]
    Directory {
        /// Directory that could not be read.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },

    /// A translation file exists but cannot be read.
    #[error("failed to read translation file `{}`: {source}", .path.display())]
    Io {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },

    /// A translation file is not valid TOML/JSON, or contains a non-string leaf.
    #[error("malformed translation file `{}`: {message}", .path.display())]
    Malformed {
        /// File that failed to parse.
        path: PathBuf,
        /// Human-readable reason.
        message: String,
    },

    /// A locale identifier was rejected because it is not a safe path segment.
    ///
    /// Locales must be non-empty and contain only ASCII alphanumerics, `-`, or
    /// `_`; anything else (path separators, `.`/`..`, absolute paths, spaces,
    /// non-ASCII) could escape the loader's base directory and is refused
    /// before any filesystem access.
    #[error("invalid locale `{locale}`: {reason}")]
    InvalidLocale {
        /// The rejected locale identifier.
        locale: String,
        /// Human-readable reason for the rejection.
        reason: &'static str,
    },

    /// A locale that was required to exist has no directory under the base.
    ///
    /// Unlike a tolerant load (which yields an empty dictionary), a strict load
    /// — used when reloading an active locale pair — surfaces this so a typo or
    /// missing translation set cannot silently degrade lookups.
    #[error("locale `{locale}` has no directory at `{}`", .path.display())]
    MissingLocale {
        /// Locale whose directory is missing.
        locale: String,
        /// Expected directory path.
        path: PathBuf,
    },
}

impl I18nError {
    /// Stable machine-readable variant name, e.g. `I18nError::Malformed`.
    ///
    /// Intended for structured logging and assertions that should not depend on
    /// the human-readable message.
    pub fn code(&self) -> &'static str {
        match self {
            I18nError::Directory { .. } => "I18nError::Directory",
            I18nError::Io { .. } => "I18nError::Io",
            I18nError::Malformed { .. } => "I18nError::Malformed",
            I18nError::InvalidLocale { .. } => "I18nError::InvalidLocale",
            I18nError::MissingLocale { .. } => "I18nError::MissingLocale",
        }
    }
}
