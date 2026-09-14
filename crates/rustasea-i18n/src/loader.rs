//! Dictionary discovery and parsing.
//!
//! [`TranslationLoader`] reads every `*.toml` / `*.json` file in a locale
//! directory (`resources/lang/{locale}/`). Each file's stem becomes the key
//! namespace, so `auth.toml` contributes keys under `auth.*` and
//! `__("auth.failed")` resolves to the `failed` entry in `auth.toml`.
//!
//! Nested tables/objects are flattened into dot-notation keys, so
//! `[passwords] reset = "…"` in `auth.toml` is reachable as
//! `auth.passwords.reset`.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{I18nError, Result};

/// Default base directory for language dictionaries.
pub const DEFAULT_LANG_DIR: &str = "resources/lang";

/// A flattened set of `full.dot.key -> message` translations for one locale.
#[derive(Debug, Clone, Default)]
pub struct Translations {
    entries: HashMap<String, String>,
}

impl Translations {
    /// Create an empty dictionary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a full dot-notation key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// Insert a full dot-notation key. Returns any previous value.
    pub fn insert(&mut self, key: impl Into<String>, message: impl Into<String>) -> Option<String> {
        self.entries.insert(key.into(), message.into())
    }

    /// Number of loaded entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no entries were loaded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over `(full_key, message)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// Discovers and parses translation dictionaries from a base directory.
///
/// The loader is cheap to clone and holds no state beyond the base path, so it
/// can be reused to load several locales.
#[derive(Debug, Clone)]
pub struct TranslationLoader {
    base: PathBuf,
}

impl Default for TranslationLoader {
    /// Use [`DEFAULT_LANG_DIR`] (`resources/lang`) as the base directory.
    fn default() -> Self {
        Self::new(DEFAULT_LANG_DIR)
    }
}

impl TranslationLoader {
    /// Create a loader rooted at `base` (the directory that contains one
    /// sub-directory per locale).
    pub fn new<P: Into<PathBuf>>(base: P) -> Self {
        Self { base: base.into() }
    }

    /// The configured base directory.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Load every dictionary for `locale`.
    ///
    /// A missing locale directory yields an empty [`Translations`] (tolerated,
    /// mirroring [`rustasea_config::ConfigLoader`]); an unreadable directory,
    /// unreadable file, or malformed dictionary is a typed [`I18nError`].
    ///
    /// The `locale` is validated as a single safe path segment before any
    /// filesystem access; traversal or absolute-path inputs fail with
    /// [`I18nError::InvalidLocale`] and never touch the filesystem.
    pub fn load_locale(&self, locale: &str) -> Result<Translations> {
        let dir = self.locale_dir(locale)?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Translations::new());
            }
            Err(source) => return Err(I18nError::Directory { path: dir, source }),
        };

        self.collect_files(entries)
    }

    /// Load every dictionary for `locale`, requiring the directory to exist.
    ///
    /// Behaves like [`TranslationLoader::load_locale`] except that a missing
    /// locale directory is a typed [`I18nError::MissingLocale`] rather than an
    /// empty dictionary. This is the strict load used when (re)loading an
    /// active locale pair, so a missing locale cannot silently degrade lookups.
    ///
    /// The `locale` is validated exactly as in [`TranslationLoader::load_locale`].
    pub fn load_locale_required(&self, locale: &str) -> Result<Translations> {
        let dir = self.locale_dir(locale)?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(I18nError::MissingLocale {
                    locale: locale.to_string(),
                    path: dir,
                });
            }
            Err(source) => return Err(I18nError::Directory { path: dir, source }),
        };

        self.collect_files(entries)
    }

    /// Resolve `locale` to a confined directory under the base path.
    ///
    /// Rejects any `locale` that is not a single safe path segment, preventing
    /// `../..`, absolute paths, and other traversal inputs from escaping the
    /// base directory.
    fn locale_dir(&self, locale: &str) -> Result<PathBuf> {
        validate_locale(locale)?;
        Ok(self.base.join(locale))
    }

    /// Parse every dictionary file from a directory listing into `Translations`.
    fn collect_files(&self, entries: fs::ReadDir) -> Result<Translations> {
        let mut files: Vec<PathBuf> = entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_dictionary(path))
            .collect();
        files.sort();

        let mut translations = Translations::new();
        for path in files {
            self.load_file(&path, &mut translations)?;
        }
        Ok(translations)
    }

    /// Parse a single dictionary file into `out`.
    fn load_file(&self, path: &Path, out: &mut Translations) -> Result<()> {
        let namespace = namespace_of(path).ok_or_else(|| I18nError::Malformed {
            path: path.to_path_buf(),
            message: "dictionary file has no usable file stem".to_string(),
        })?;
        let contents = fs::read_to_string(path).map_err(|source| I18nError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        match extension(path).as_deref() {
            Some("toml") => {
                let value: toml::Value =
                    toml::from_str(&contents).map_err(|e| I18nError::Malformed {
                        path: path.to_path_buf(),
                        message: e.to_string(),
                    })?;
                flatten_toml(namespace, &value, path, out)?;
            }
            Some("json") => {
                let value: serde_json::Value =
                    serde_json::from_str(&contents).map_err(|e| I18nError::Malformed {
                        path: path.to_path_buf(),
                        message: e.to_string(),
                    })?;
                flatten_json(namespace, &value, path, out)?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Validate a locale identifier as a single safe path segment.
///
/// Accepts non-empty strings whose characters are ASCII alphanumerics, `-`, or
/// `_`. Rejects empty input, path separators, `.`/`..`, absolute paths, spaces,
/// and any non-ASCII character — all of which could escape the base directory.
fn validate_locale(locale: &str) -> Result<()> {
    let reject = |reason: &'static str| {
        Err(I18nError::InvalidLocale {
            locale: locale.to_string(),
            reason,
        })
    };

    if locale.is_empty() {
        return reject("locale must not be empty");
    }
    if locale == "." || locale == ".." {
        return reject("locale must not be a path component `.` or `..`");
    }
    for ch in locale.chars() {
        let allowed = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_';
        if !allowed {
            return reject("locale may only contain ASCII alphanumerics, `-`, and `_`");
        }
    }
    Ok(())
}

/// Recursively flatten a TOML value into `namespace`-prefixed dot keys.
fn flatten_toml(
    prefix: &str,
    value: &toml::Value,
    path: &Path,
    out: &mut Translations,
) -> Result<()> {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                flatten_toml(&join(prefix, key), child, path, out)?;
            }
        }
        toml::Value::String(message) => {
            out.insert(prefix.to_string(), message.clone());
        }
        other => {
            return Err(I18nError::Malformed {
                path: path.to_path_buf(),
                message: format!(
                    "expected a string at `{prefix}`, found {}",
                    other.type_str()
                ),
            });
        }
    }
    Ok(())
}

/// Recursively flatten a JSON value into `namespace`-prefixed dot keys.
fn flatten_json(
    prefix: &str,
    value: &serde_json::Value,
    path: &Path,
    out: &mut Translations,
) -> Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                flatten_json(&join(prefix, key), child, path, out)?;
            }
        }
        serde_json::Value::String(message) => {
            out.insert(prefix.to_string(), message.clone());
        }
        other => {
            return Err(I18nError::Malformed {
                path: path.to_path_buf(),
                message: format!("expected a string at `{prefix}`, found {other}"),
            });
        }
    }
    Ok(())
}

/// Join a dot-notation `prefix` with a child `key`.
fn join(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

/// Return the lower-cased file extension, when present and valid UTF-8.
fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
}

/// Return the file stem used as the key namespace.
fn namespace_of(path: &Path) -> Option<&str> {
    path.file_stem().and_then(OsStr::to_str)
}

/// True when `path` is a `*.toml` or `*.json` dictionary (case-insensitive).
fn is_dictionary(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("toml" | "json"))
}
