//! End-to-end `log:show` command tests.
//!
//! Each case builds an isolated project root under a unique temp directory —
//! `config/logging.toml` plus a `storage/logs/` fixture — and enters it as the
//! process working directory (the same [`IsolatedCwd`] RAII guard the `tinker`
//! tests use). `log:show` resolves both its config and its log path relative to
//! the working directory, so this keeps the run hermetic: the repository tree is
//! never read or written.
//!
//! The process working directory is global, so every test serializes through
//! [`LOCK`] (which also serializes the process-wide command registry touched by
//! [`rustasea_cli::load_default_commands`]).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rustasea_cli::Artisan;

/// Serializes tests that mutate the process working directory / registry.
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Isolate the process working directory in a fresh project fixture.
///
/// On drop the original directory is restored and the temp tree removed, even
/// on panic, so sibling tests never observe the temporary cwd.
struct IsolatedProject {
    original: PathBuf,
    root: PathBuf,
}

impl IsolatedProject {
    /// Create the fixture (`config/logging.toml` + a log file) and enter it.
    ///
    /// `log_contents` is written to `storage/logs/rustasea.log`; the channel
    /// config points the `daily` and default channels at that path.
    fn enter(log_contents: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rustasea-cli-logshow-{}-{}",
            std::process::id(),
            unique
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).expect("config dir");
        std::fs::create_dir_all(root.join("storage/logs")).expect("logs dir");

        std::fs::write(
            root.join("config/logging.toml"),
            "[logging]\ndefault = \"daily\"\n\n\
             [logging.channels.daily]\ndriver = \"daily\"\n\
             path = \"storage/logs/rustasea.log\"\nlevel = \"debug\"\nmax_files = 14\n",
        )
        .expect("write config");
        std::fs::write(root.join("storage/logs/rustasea.log"), log_contents).expect("write log");

        let original = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(&root).expect("enter fixture root");
        Self { original, root }
    }

    /// Remove the fixture log file (to exercise the missing-file path).
    fn remove_log(&self) {
        let _ = std::fs::remove_file(self.root.join("storage/logs/rustasea.log"));
    }
}

impl Drop for IsolatedProject {
    /// Restore the original working directory and remove the fixture tree.
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A mixed-level fixture with a distinguishable message on each line.
const FIXTURE: &str = "2026-09-15T00:00:00Z INFO app::boot: server started\n\
                       2026-09-15T01:00:00Z ERROR app::db: connection refused\n\
                       2026-09-15T02:00:00Z WARN app::cache: miss for key\n\
                       2026-09-15T03:00:00Z ERROR app::db: deadlock detected\n\
                       this line is malformed\n";

/// `--level=error` keeps only the ERROR rows.
#[tokio::test]
async fn level_filter_shows_only_errors() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);

    let out = Artisan::call("log:show", vec!["--level=error".to_string()])
        .await
        .expect("call");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("connection refused"),
        "stdout: {}",
        out.stdout
    );
    assert!(
        out.stdout.contains("deadlock detected"),
        "stdout: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("server started"),
        "stdout: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("miss for key"),
        "stdout: {}",
        out.stdout
    );
    drop(project);
}

/// `--grep` matches a case-insensitive substring over message and target.
#[tokio::test]
async fn grep_filter_matches_substring() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);

    let out = Artisan::call("log:show", vec!["--grep=DEADLOCK".to_string()])
        .await
        .expect("call");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("deadlock detected"),
        "stdout: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("connection refused"),
        "stdout: {}",
        out.stdout
    );
    drop(project);
}

/// `--json` emits one parseable object per line.
#[tokio::test]
async fn json_emits_parseable_lines() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);

    let out = Artisan::call(
        "log:show",
        vec!["--json".to_string(), "--level=error".to_string()],
    )
    .await
    .expect("call");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    let lines: Vec<&str> = out
        .stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 2, "stdout: {}", out.stdout);
    for line in lines {
        let parsed: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
        assert_eq!(parsed["level"], "ERROR");
    }
    drop(project);
}

/// A missing log file exits non-zero with an actionable message.
#[tokio::test]
async fn missing_file_exits_nonzero_with_message() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);
    project.remove_log();

    let out = Artisan::call("log:show", Vec::new()).await.expect("call");
    assert_ne!(out.exit_code, 0, "stdout: {}", out.stdout);
    assert!(
        out.stderr.contains("log file not found"),
        "stderr: {}",
        out.stderr
    );
    drop(project);
}

/// A malformed line is skipped, not fatal.
#[tokio::test]
async fn malformed_lines_are_skipped() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);

    let out = Artisan::call("log:show", Vec::new()).await.expect("call");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    // Four well-formed lines render; the malformed one never appears.
    assert!(
        !out.stdout.contains("this line is malformed"),
        "stdout: {}",
        out.stdout
    );
    assert!(
        out.stdout.contains("server started"),
        "stdout: {}",
        out.stdout
    );
    drop(project);
}

/// A rotated-only channel (bare file absent) resolves the newest sibling.
#[tokio::test]
async fn rotated_file_is_resolved_when_bare_missing() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);
    project.remove_log();
    std::fs::write(
        project.root.join("storage/logs/rustasea-.2026-09-15.log"),
        "2026-09-15T00:00:00Z ERROR app: from rotation\n",
    )
    .expect("write rotated");

    let out = Artisan::call("log:show", Vec::new()).await.expect("call");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("from rotation"),
        "stdout: {}",
        out.stdout
    );
    drop(project);
}

/// `--channel` selects a named channel; an unknown one is a typed error.
#[tokio::test]
async fn channel_selection_and_unknown_channel() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);

    let known = Artisan::call("log:show", vec!["--channel=daily".to_string()])
        .await
        .expect("call");
    assert_eq!(known.exit_code, 0, "stderr: {}", known.stderr);

    let unknown = Artisan::call("log:show", vec!["--channel=nope".to_string()])
        .await
        .expect("call");
    assert_ne!(unknown.exit_code, 0);
    assert!(
        unknown.stderr.contains("is not defined"),
        "stderr: {}",
        unknown.stderr
    );
    drop(project);
}

/// `--limit` keeps the newest entries.
#[tokio::test]
async fn limit_keeps_newest_entries() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);

    let out = Artisan::call("log:show", vec!["--limit=1".to_string()])
        .await
        .expect("call");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("deadlock detected"),
        "stdout: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("server started"),
        "stdout: {}",
        out.stdout
    );
    drop(project);
}

/// `--since` bounds the result set by timestamp.
#[tokio::test]
async fn since_bounds_by_timestamp() {
    let _guard = LOCK.lock().await;
    rustasea_cli::load_default_commands();
    let project = IsolatedProject::enter(FIXTURE);

    let out = Artisan::call("log:show", vec!["--since=2026-09-15T01:30:00Z".to_string()])
        .await
        .expect("call");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("miss for key"),
        "stdout: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("server started"),
        "stdout: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("connection refused"),
        "stdout: {}",
        out.stdout
    );
    drop(project);
}
