//! End-to-end `lang:check` command tests.
//!
//! Each case builds an isolated `resources/lang`-style tree under a unique
//! temp directory and drives the command through [`Artisan::call`] with an
//! explicit `--path`, so the repository tree is never read or written and the
//! check is independent of the process working directory's own dictionaries.
//!
//! The scan roots for the "unused" finding are resolved from the crate's
//! working directory; fixture keys are deliberately namespaced under `checker.`
//! so they are never accidentally matched by a real helper call elsewhere.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rustasea_cli::Artisan;

/// Serializes tests that touch the process-wide command registry.
static REGISTRY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A temporary language directory removed on drop.
struct TempLang {
    root: PathBuf,
}

impl TempLang {
    /// Create a fresh temp directory with a process-unique name.
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rustasea-cli-langcheck-{}-{}",
            std::process::id(),
            unique
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp lang dir");
        Self { root }
    }

    /// Write `contents` to `{locale}/{name}`.
    fn write(&self, locale: &str, name: &str, contents: &str) {
        let dir = self.root.join(locale);
        fs::create_dir_all(&dir).expect("create locale dir");
        fs::write(dir.join(name), contents).expect("write dictionary");
    }

    /// The base path to pass to `--path`.
    fn base(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempLang {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Build the raw argument vector for a check rooted at `lang`.
fn args(lang: &TempLang, extra: &[&str]) -> Vec<String> {
    let mut args = vec!["--path".to_string(), lang.base().display().to_string()];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

/// A missing key in a non-reference locale fails with exit code 7.
#[tokio::test]
async fn missing_key_fails_with_exit_code_7() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let lang = TempLang::new();
    lang.write(
        "en",
        "auth.toml",
        "failed = \"These credentials do not match.\"\n",
    );
    lang.write("fr", "auth.toml", "throttle = \"Trop de tentatives.\"\n");

    let out = Artisan::call("lang:check", args(&lang, &["--locale", "en"]))
        .await
        .expect("call");
    assert_eq!(
        out.exit_code, 7,
        "stdout: {} stderr: {}",
        out.stdout, out.stderr
    );
    assert!(out.stdout.contains("auth.failed"), "stdout: {}", out.stdout);
    assert!(out.stderr.contains("missing"), "stderr: {}", out.stderr);
}

/// Identical dictionaries are clean: exit 0 and no findings.
#[tokio::test]
async fn identical_locales_are_clean() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let lang = TempLang::new();
    let body = "failed = \"Same message.\"\n";
    lang.write("en", "auth.toml", body);
    lang.write("fr", "auth.toml", body);

    let out = Artisan::call("lang:check", args(&lang, &["--locale", "en"]))
        .await
        .expect("call");
    assert!(out.is_success(), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("Translations OK"),
        "stdout: {}",
        out.stdout
    );
}

/// A duplicate message value under two keys fails with exit code 7.
#[tokio::test]
async fn duplicate_values_fail() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let lang = TempLang::new();
    lang.write(
        "en",
        "auth.toml",
        "a = \"same\"\nb = \"same\"\nc = \"other\"\n",
    );

    let out = Artisan::call("lang:check", args(&lang, &["--locale", "en"]))
        .await
        .expect("call");
    assert_eq!(
        out.exit_code, 7,
        "stdout: {} stderr: {}",
        out.stdout, out.stderr
    );
    assert!(out.stdout.contains("Duplicate"), "stdout: {}", out.stdout);
    assert!(out.stdout.contains("auth.a"), "stdout: {}", out.stdout);
    assert!(out.stdout.contains("auth.b"), "stdout: {}", out.stdout);
}

/// Unused keys are informational and do not fail the command.
#[tokio::test]
async fn unused_keys_are_informational() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let lang = TempLang::new();
    lang.write("en", "checker.toml", "alpha = \"A\"\nbeta = \"B\"\n");

    let out = Artisan::call("lang:check", args(&lang, &["--locale", "en"]))
        .await
        .expect("call");
    assert!(out.is_success(), "stderr: {}", out.stderr);
    assert!(out.stdout.contains("Unused keys"), "stdout: {}", out.stdout);
    assert!(
        out.stdout.contains("checker.alpha"),
        "stdout: {}",
        out.stdout
    );
}

/// A malformed dictionary is a typed domain error (exit 1).
#[tokio::test]
async fn malformed_dictionary_is_a_typed_error() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let lang = TempLang::new();
    lang.write("en", "bad.toml", "failed = \n");

    let out = Artisan::call("lang:check", args(&lang, &["--locale", "en"]))
        .await
        .expect("call");
    assert_eq!(
        out.exit_code, 1,
        "stdout: {} stderr: {}",
        out.stdout, out.stderr
    );
    assert!(
        out.stderr.to_lowercase().contains("malformed"),
        "stderr: {}",
        out.stderr
    );
}

/// A reference locale absent among the existing locales is a failing finding.
#[tokio::test]
async fn missing_reference_locale_is_non_zero() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let lang = TempLang::new();
    lang.write("fr", "auth.toml", "failed = \"Non.\"\n");

    let out = Artisan::call("lang:check", args(&lang, &["--locale", "en"]))
        .await
        .expect("call");
    assert!(!out.is_success(), "stdout: {}", out.stdout);
    assert_eq!(out.exit_code, 7, "stderr: {}", out.stderr);
}

/// A base directory with no locale sub-directories exits 0 with a notice.
#[tokio::test]
async fn empty_base_is_a_notice() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let lang = TempLang::new();

    let out = Artisan::call("lang:check", args(&lang, &["--locale", "en"]))
        .await
        .expect("call");
    assert!(out.is_success(), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("No translation locales"),
        "stdout: {}",
        out.stdout
    );
}

/// A missing base directory exits 0 with a notice (tolerant, like the loader).
#[tokio::test]
async fn missing_base_is_tolerated() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let missing = std::env::temp_dir().join(format!(
        "rustasea-cli-langcheck-absent-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let args = vec!["--path".to_string(), missing.display().to_string()];

    let out = Artisan::call("lang:check", args).await.expect("call");
    assert!(out.is_success(), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("No translation directory"),
        "stdout: {}",
        out.stdout
    );
}

/// `--json` emits a structured report with the documented fields.
#[tokio::test]
async fn json_report_has_expected_fields() {
    let _guard = REGISTRY_LOCK.lock().await;
    rustasea_cli::load_default_commands();

    let lang = TempLang::new();
    lang.write("en", "auth.toml", "failed = \"English.\"\n");
    lang.write("fr", "auth.toml", "throttle = \"Francais.\"\n");

    let out = Artisan::call("lang:check", args(&lang, &["--locale", "en", "--json"]))
        .await
        .expect("call");
    let parsed: serde_json::Value =
        serde_json::from_str(out.stdout.trim()).expect("valid JSON report");

    assert_eq!(parsed["locale"], "en");
    assert_eq!(parsed["missing"][0]["key"], "auth.failed");
    assert_eq!(parsed["missing"][0]["locale"], "fr");
    assert!(parsed["unused"].is_array(), "unused: {}", parsed["unused"]);
    assert!(
        parsed["duplicates"].is_array(),
        "duplicates: {}",
        parsed["duplicates"]
    );
}
