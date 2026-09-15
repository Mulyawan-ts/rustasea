//! `lang:check` — translation dictionary consistency checker.
//!
//! Mirrors `bottelet/translation-checker` for the RustaSea i18n layout. The
//! command loads every locale under a base directory (`resources/lang` by
//! default) through [`rustasea_i18n::TranslationLoader`] and reports three
//! classes of finding:
//!
//! - **missing** — a key present in the reference locale (the `--locale` flag
//!   or the configured `app_locale`) but absent from another locale. Fails.
//! - **duplicate** — the same message value used by two or more keys within a
//!   single locale. Fails.
//! - **unused** — a key never referenced by a `__`/`trans_choice`/
//!   `i18n_trans`/`i18n_trans_choice` literal in the scanned sources.
//!   Informational only; never fails the command.
//!
//! A missing base directory or a base with no locale sub-directories is
//! tolerated (exit 0) so the command is safe to wire into CI before any
//! dictionaries exist.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Serialize;

use crate::artisan::{Command, Io};
use crate::error::{project_root, CliError, CliResult};
use crate::output;
use rustasea_i18n::{TranslationLoader, Translations, DEFAULT_LANG_DIR};

mod scan;

/// Source roots scanned for translation-helper calls, relative to the project.
const SCAN_DIRS: [&str; 3] = ["crates", "app", "routes"];

/// Sentinel key reported when the reference locale has no directory at all.
const REFERENCE_ABSENT_KEY: &str = "*";

/// A key present in the reference locale but absent from `locale`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingEntry {
    /// The dictionary key.
    pub key: String,
    /// The locale that lacks the key.
    pub locale: String,
}

/// Two or more keys within one locale sharing the same message value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicateGroup {
    /// The locale the duplicate lives in.
    pub locale: String,
    /// The shared message value.
    pub value: String,
    /// The keys that share the value, sorted.
    pub keys: Vec<String>,
}

/// Machine-readable `lang:check` report (emitted by `--json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckReport {
    /// The reference locale the check was anchored to.
    pub locale: String,
    /// Keys missing from one or more non-reference locales.
    pub missing: Vec<MissingEntry>,
    /// Keys never referenced by a translation helper.
    pub unused: Vec<String>,
    /// Duplicate message values, grouped per locale.
    pub duplicates: Vec<DuplicateGroup>,
}

impl CheckReport {
    /// Whether the report contains any failing finding.
    fn has_failures(&self) -> bool {
        !self.missing.is_empty() || !self.duplicates.is_empty()
    }
}

/// Parsed `lang:check` options.
#[derive(Debug, Default)]
struct Options {
    /// Reference locale (`--locale`), resolved from config when absent.
    locale: Option<String>,
    /// Base language directory (`--path`), defaulting to [`DEFAULT_LANG_DIR`].
    path: Option<PathBuf>,
    /// Whether to emit the JSON report.
    json: bool,
}

/// Outcome of walking the base directory.
enum Outcome {
    /// The base directory is missing or unreadable (tolerated).
    NoBase,
    /// The base directory holds no locale sub-directories (tolerated).
    NoLocales,
    /// A report was produced.
    Report(Box<CheckReport>),
}

/// `lang:check` — report missing, unused and duplicate translation entries.
pub struct LangCheck;

#[async_trait]
impl Command for LangCheck {
    /// Command signature.
    fn signature(&self) -> &'static str {
        "lang:check"
    }

    /// Usage line rendered by `list`.
    fn usage(&self) -> Option<&'static str> {
        Some("lang:check [--locale=xx] [--path=resources/lang] [--json]")
    }

    /// One-line help rendered by `list`.
    fn help(&self) -> Option<&'static str> {
        Some("Check translation dictionaries for missing, unused and duplicate entries")
    }

    /// Execute: load every locale and report the findings.
    async fn run(&self, args: Vec<String>, io: &mut Io) -> CliResult<()> {
        let options = parse_args(&args)?;
        let base = options
            .path
            .unwrap_or_else(|| PathBuf::from(DEFAULT_LANG_DIR));
        let reference = options.locale.unwrap_or_else(resolve_locale);
        let scan_roots = scan_roots();

        match build_report(&base, &reference, &scan_roots)? {
            Outcome::NoBase => {
                io.line(format!(
                    "No translation directory at {} — nothing to check.",
                    base.display()
                ));
                Ok(())
            }
            Outcome::NoLocales => {
                io.line(format!(
                    "No translation locales found under {}",
                    base.display()
                ));
                Ok(())
            }
            Outcome::Report(report) => render(*report, options.json, io),
        }
    }
}

/// Parse the raw `lang:check` argument vector.
///
/// Supports `--locale=<x>` / `--locale <x>`, `--path=<p>` / `--path <p>`, and
/// the `--json` flag. Any other flag is a typed
/// [`CliError::InvalidArguments`].
fn parse_args(args: &[String]) -> CliResult<Options> {
    let invalid = |detail: String| CliError::InvalidArguments {
        command: "lang:check".to_string(),
        detail,
    };

    let mut options = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--json" {
            options.json = true;
        } else if arg == "--locale" {
            let value = iter.next().ok_or_else(|| {
                invalid("`--locale` requires a value, e.g. `lang:check --locale en`".into())
            })?;
            options.locale = Some(non_empty("--locale", value, &invalid)?);
        } else if let Some(value) = arg.strip_prefix("--locale=") {
            options.locale = Some(non_empty("--locale", value, &invalid)?);
        } else if arg == "--path" {
            let value = iter.next().ok_or_else(|| {
                invalid("`--path` requires a value, e.g. `lang:check --path resources/lang`".into())
            })?;
            options.path = Some(PathBuf::from(non_empty("--path", value, &invalid)?));
        } else if let Some(value) = arg.strip_prefix("--path=") {
            options.path = Some(PathBuf::from(non_empty("--path", value, &invalid)?));
        } else if arg.starts_with('-') {
            return Err(invalid(format!("unrecognized flag `{arg}`")));
        }
    }
    Ok(options)
}

/// Validate that a flag value is non-empty, returning it trimmed-free as-is.
fn non_empty(flag: &str, value: &str, invalid: &impl Fn(String) -> CliError) -> CliResult<String> {
    if value.is_empty() {
        return Err(invalid(format!("`{flag}` requires a non-empty value")));
    }
    Ok(value.to_string())
}

/// Resolve the reference locale from `config/app.toml`, then the default.
///
/// Mirrors the `app_env` precedence used by the migration commands: the
/// configured `app_locale` wins, and [`rustasea_foundation::config::DEFAULT_LOCALE`]
/// is the fallback when config is absent or blank.
fn resolve_locale() -> String {
    if let Ok(loader) = rustasea_config::ConfigLoader::load_from(&["config/app"]) {
        if let Ok(config) = rustasea_foundation::AppConfig::from_loader(&loader) {
            let locale = config.locale.trim();
            if !locale.is_empty() {
                return locale.to_string();
            }
        }
    }
    rustasea_foundation::config::DEFAULT_LOCALE.to_string()
}

/// Build the scan roots (`crates`, `app`, `routes`) under the project root.
///
/// Falls back to the current directory when no `Cargo.toml` can be found; a
/// non-existent root is skipped by the scanner.
fn scan_roots() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = project_root(&cwd).unwrap_or(cwd);
    SCAN_DIRS.iter().map(|dir| root.join(dir)).collect()
}

/// Load every locale under `base` and assemble a [`CheckReport`].
///
/// Returns [`Outcome::NoBase`] / [`Outcome::NoLocales`] for the tolerated empty
/// states, and a typed [`CliError::Domain`] when a dictionary cannot be read or
/// parsed.
fn build_report(base: &Path, reference: &str, scan_roots: &[PathBuf]) -> CliResult<Outcome> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(_) => return Ok(Outcome::NoBase),
    };

    let mut locales: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    locales.sort();
    if locales.is_empty() {
        return Ok(Outcome::NoLocales);
    }

    let loader = TranslationLoader::new(base);
    let mut dictionaries: Vec<(String, Translations)> = Vec::with_capacity(locales.len());
    for locale in &locales {
        let translations = loader
            .load_locale(locale)
            .map_err(|error| CliError::Domain(error.to_string()))?;
        dictionaries.push((locale.clone(), translations));
    }

    let mut union: BTreeSet<String> = BTreeSet::new();
    let mut duplicates: Vec<DuplicateGroup> = Vec::new();
    for (locale, translations) in &dictionaries {
        for (key, _) in translations.iter() {
            union.insert(key.to_string());
        }
        duplicates.extend(duplicate_groups(locale, translations));
    }
    duplicates.sort_by(|a, b| (&a.locale, &a.value).cmp(&(&b.locale, &b.value)));

    let missing = missing_keys(reference, &dictionaries);

    let referenced = scan::referenced_keys(scan_roots);
    let unused: Vec<String> = union
        .into_iter()
        .filter(|key| !referenced.contains(key))
        .collect();

    Ok(Outcome::Report(Box::new(CheckReport {
        locale: reference.to_string(),
        missing,
        unused,
        duplicates,
    })))
}

/// Compute the keys missing from each non-reference locale, sorted.
///
/// When the reference locale has no directory the result is a single sentinel
/// entry so the run still fails with a clear report.
fn missing_keys(reference: &str, dictionaries: &[(String, Translations)]) -> Vec<MissingEntry> {
    let Some((_, reference_dict)) = dictionaries.iter().find(|(locale, _)| locale == reference)
    else {
        return vec![MissingEntry {
            key: REFERENCE_ABSENT_KEY.to_string(),
            locale: reference.to_string(),
        }];
    };

    let mut missing = Vec::new();
    for (locale, translations) in dictionaries {
        if locale == reference {
            continue;
        }
        for (key, _) in reference_dict.iter() {
            if translations.get(key).is_none() {
                missing.push(MissingEntry {
                    key: key.to_string(),
                    locale: locale.clone(),
                });
            }
        }
    }
    missing.sort_by(|a, b| (&a.locale, &a.key).cmp(&(&b.locale, &b.key)));
    missing
}

/// Group the keys of one locale by message value, keeping groups of 2+.
fn duplicate_groups(locale: &str, translations: &Translations) -> Vec<DuplicateGroup> {
    let mut by_value: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, value) in translations.iter() {
        by_value
            .entry(value.to_string())
            .or_default()
            .push(key.to_string());
    }
    by_value
        .into_iter()
        .filter(|(_, keys)| keys.len() > 1)
        .map(|(value, mut keys)| {
            keys.sort();
            DuplicateGroup {
                locale: locale.to_string(),
                value,
                keys,
            }
        })
        .collect()
}

/// Render the report and map failing findings onto the exit-code error.
fn render(report: CheckReport, json: bool, io: &mut Io) -> CliResult<()> {
    if json {
        io.line(serde_json::to_string(&report)?);
    } else {
        render_human(&report, io);
    }

    if report.has_failures() {
        return Err(CliError::TranslationCheckFailed {
            missing: report.missing.len(),
            duplicate: report.duplicates.len(),
        });
    }
    Ok(())
}

/// Render the human-readable sections of a report.
fn render_human(report: &CheckReport, io: &mut Io) {
    if report.missing.is_empty() && report.duplicates.is_empty() {
        io.line(format!(
            "Translations OK for reference locale `{}`.",
            report.locale
        ));
    }

    if !report.missing.is_empty() {
        let mut rows = vec![vec!["Missing key".to_string(), "Locale".to_string()]];
        for entry in &report.missing {
            rows.push(vec![entry.key.clone(), entry.locale.clone()]);
        }
        io.line(format!("Missing keys ({}):", report.missing.len()));
        io.line(output::table(rows).trim_end());
    }

    if !report.duplicates.is_empty() {
        let mut rows = vec![vec![
            "Locale".to_string(),
            "Value".to_string(),
            "Keys".to_string(),
        ]];
        for group in &report.duplicates {
            rows.push(vec![
                group.locale.clone(),
                group.value.clone(),
                group.keys.join(", "),
            ]);
        }
        io.line(format!("Duplicate values ({}):", report.duplicates.len()));
        io.line(output::table(rows).trim_end());
    }

    if !report.unused.is_empty() {
        io.line(format!("Unused keys ({}):", report.unused.len()));
        for key in &report.unused {
            io.line(format!("  {key}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse: both flag spellings, the `--json` toggle and a flag error.
    #[test]
    fn parses_flags_and_rejects_unknown() {
        let options = parse_args(&[
            "--locale".into(),
            "fr".into(),
            "--path=tmp/lang".into(),
            "--json".into(),
        ])
        .expect("valid args");
        assert_eq!(options.locale.as_deref(), Some("fr"));
        assert_eq!(options.path.as_deref(), Some(Path::new("tmp/lang")));
        assert!(options.json);

        let equals = parse_args(&["--locale=de".into()]).expect("valid args");
        assert_eq!(equals.locale.as_deref(), Some("de"));

        let err = parse_args(&["--bogus".into()]).expect_err("unknown flag must fail");
        assert!(matches!(err, CliError::InvalidArguments { .. }), "{err:?}");

        let missing = parse_args(&["--locale".into()]).expect_err("missing value");
        assert!(matches!(missing, CliError::InvalidArguments { .. }));
    }

    /// Duplicate grouping keeps only values shared by two or more keys.
    #[test]
    fn groups_duplicate_values_only() {
        let mut translations = Translations::new();
        translations.insert("auth.a", "same");
        translations.insert("auth.b", "same");
        translations.insert("auth.c", "unique");

        let groups = duplicate_groups("en", &translations);
        assert_eq!(groups.len(), 1, "groups: {groups:?}");
        assert_eq!(groups[0].locale, "en");
        assert_eq!(groups[0].value, "same");
        assert_eq!(groups[0].keys, vec!["auth.a", "auth.b"]);
    }

    /// A missing reference locale yields the sentinel failing entry.
    #[test]
    fn missing_reference_locale_is_a_finding() {
        let mut french = Translations::new();
        french.insert("auth.failed", "non");
        let dictionaries = vec![("fr".to_string(), french)];

        let missing = missing_keys("en", &dictionaries);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].key, REFERENCE_ABSENT_KEY);
        assert_eq!(missing[0].locale, "en");
    }
}
