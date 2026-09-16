//! Typed errors for module discovery, registration, and manifest toggling.

use std::path::PathBuf;

/// Errors raised while discovering, registering, or toggling modules.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    /// A module with the same name is already registered.
    #[error("module `{name}` is already registered")]
    Duplicate {
        /// The duplicated module name.
        name: String,
    },

    /// No module with that name is registered.
    #[error("unknown module `{name}`")]
    Unknown {
        /// The requested module name.
        name: String,
    },

    /// The module name is not a valid identifier.
    #[error("invalid module name `{name}`: {reason}")]
    InvalidName {
        /// The rejected name.
        name: String,
        /// Why the name was rejected.
        reason: String,
    },

    /// A manifest file could not be read or parsed.
    #[error("invalid module manifest at {}: {message}", path.display())]
    Manifest {
        /// The manifest file that failed.
        path: PathBuf,
        /// Parser diagnosis.
        message: String,
    },

    /// A manifest file could not be written.
    #[error("failed to write module manifest at {}: {source}", path.display())]
    Write {
        /// The file (or parent directory) that failed.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// A module directory could not be inspected.
    #[error("failed to inspect module at {}: {source}", path.display())]
    Discovery {
        /// The path that failed.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
}

/// Result type for module operations.
pub type ModuleResult<T> = Result<T, ModuleError>;
