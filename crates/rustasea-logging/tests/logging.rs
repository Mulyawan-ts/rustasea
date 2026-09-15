//! Integration tests for `rustasea-logging`.
//!
//! File-output tests install the built subscriber as a *thread-local* default
//! ([`tracing::subscriber::with_default`]) so they never contend for the
//! process-global slot; the global-install tests are isolated and assert only
//! the deterministic idempotency contract.

use std::path::Path;
use std::sync::Mutex;

use rustasea_config::ConfigLoader;
use rustasea_logging::config::{ChannelConfig, Driver};
#[cfg(feature = "sentry")]
use rustasea_logging::SentryConfig;
use rustasea_logging::{build, init, init_from_config, LoggingConfig, LoggingError};
use tempfile::TempDir;

/// Serializes tests that mutate process-global environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Create a fresh temporary directory (removed when dropped).
fn tempdir() -> TempDir {
    tempfile::tempdir().expect("create temp dir")
}

/// Read the contents of `path` (empty string when the file is absent).
fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Read the single file in `dir` whose name starts with `prefix`.
fn read_prefixed(dir: &Path, prefix: &str) -> String {
    let mut found = String::new();
    for entry in std::fs::read_dir(dir).expect("read log dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(prefix) {
            found.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
    }
    found
}

#[test]
fn single_driver_writes_a_line_to_a_temp_file() {
    let dir = tempdir();
    let log_path = dir.path().join("rustasea.log");
    let config = LoggingConfig::new("single").with_channel(
        "single",
        ChannelConfig::new("single")
            .with_path(log_path.to_str().unwrap())
            .with_level("info"),
    );

    let (subscriber, guard) = build(&config).expect("build subscriber");
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("hello from the single driver");
    });
    drop(guard); // Flush and stop the appender worker.

    let contents = read(&log_path);
    assert!(
        contents.contains("hello from the single driver"),
        "expected the log line, got: {contents:?}"
    );
}

#[test]
fn level_filtering_suppresses_lower_priority_events() {
    let dir = tempdir();
    let log_path = dir.path().join("rustasea.log");
    let config = LoggingConfig::new("single").with_channel(
        "single",
        ChannelConfig::new("single")
            .with_path(log_path.to_str().unwrap())
            .with_level("info"),
    );

    let (subscriber, guard) = build(&config).expect("build subscriber");
    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!("this should be filtered out");
        tracing::info!("this should be kept");
    });
    drop(guard);

    let contents = read(&log_path);
    assert!(contents.contains("this should be kept"), "info must pass");
    assert!(
        !contents.contains("this should be filtered out"),
        "debug must be filtered: {contents:?}"
    );
}

#[test]
fn daily_driver_creates_a_dated_file() {
    let dir = tempdir();
    let log_path = dir.path().join("rustasea.log");
    let config = LoggingConfig::new("daily").with_channel(
        "daily",
        ChannelConfig::new("daily")
            .with_path(log_path.to_str().unwrap())
            .with_level("info")
            .with_max_files(7),
    );

    let (subscriber, guard) = build(&config).expect("build subscriber");
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("rotated line");
    });
    drop(guard);

    let contents = read_prefixed(dir.path(), "rustasea-");
    assert!(
        contents.contains("rotated line"),
        "expected a dated log file with the line, got: {contents:?}"
    );
}

#[test]
fn stack_channel_composes_members() {
    let dir = tempdir();
    let first = dir.path().join("first.log");
    let second = dir.path().join("second.log");
    let config = LoggingConfig::new("stack")
        .with_channel(
            "stack",
            ChannelConfig::new("stack").with_channels(["first", "second"]),
        )
        .with_channel(
            "first",
            ChannelConfig::new("single")
                .with_path(first.to_str().unwrap())
                .with_level("info"),
        )
        .with_channel(
            "second",
            ChannelConfig::new("single")
                .with_path(second.to_str().unwrap())
                .with_level("info"),
        );

    let (subscriber, guard) = build(&config).expect("build subscriber");
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("composed line");
    });
    drop(guard);

    assert!(read(&first).contains("composed line"), "first member wrote");
    assert!(
        read(&second).contains("composed line"),
        "second member wrote"
    );
}

#[test]
fn unknown_driver_is_a_typed_error() {
    let config =
        LoggingConfig::new("bogus").with_channel("bogus", ChannelConfig::new("not-a-driver"));
    let error = build(&config).err().expect("unknown driver must fail");
    assert!(
        error.is_unknown_driver(),
        "expected UnknownDriver, got {error}"
    );
    assert!(matches!(error, LoggingError::UnknownDriver(name) if name == "not-a-driver"));
}

#[test]
fn unsupported_driver_is_a_typed_error() {
    let config = LoggingConfig::new("slack").with_channel("slack", ChannelConfig::new("slack"));
    let error = build(&config).err().expect("slack must be unsupported");
    assert!(
        error.is_unsupported_driver(),
        "expected UnsupportedDriver, got {error}"
    );
    assert!(matches!(error, LoggingError::UnsupportedDriver(name) if name == "slack"));
}

#[test]
fn null_driver_builds_without_writing() {
    let config = LoggingConfig::new("null").with_channel("null", ChannelConfig::new("null"));
    let (_subscriber, guard) = build(&config).expect("null builds");
    assert_eq!(guard.worker_count(), 0, "null owns no workers");
}

#[test]
fn emergency_channel_infers_driver_from_path() {
    // Laravel's `emergency` channel declares only a `path`.
    let dir = tempdir();
    let log_path = dir.path().join("emergency.log");
    let mut channel = ChannelConfig::new("");
    channel.path = Some(log_path.to_str().unwrap().to_string());
    let config = LoggingConfig::new("emergency").with_channel("emergency", channel);

    let (subscriber, guard) = build(&config).expect("emergency builds");
    tracing::subscriber::with_default(subscriber, || {
        tracing::error!("emergency line");
    });
    drop(guard);

    assert!(read(&log_path).contains("emergency line"));
}

#[test]
fn cyclic_stack_is_rejected() {
    let config = LoggingConfig::new("a")
        .with_channel("a", ChannelConfig::new("stack").with_channels(["b"]))
        .with_channel("b", ChannelConfig::new("stack").with_channels(["a"]));
    let error = build(&config).err().expect("cycle must fail");
    assert!(matches!(error, LoggingError::CyclicStack(_)), "got {error}");
}

#[test]
fn missing_stack_member_is_a_typed_error() {
    let config = LoggingConfig::new("stack").with_channel(
        "stack",
        ChannelConfig::new("stack").with_channels(["ghost"]),
    );
    let error = build(&config).err().expect("missing member must fail");
    assert!(
        matches!(&error, LoggingError::UnknownChannel(name) if name == "ghost"),
        "got {error}"
    );
}

#[test]
fn invalid_level_is_a_typed_error() {
    let config = LoggingConfig::new("single")
        .with_channel("single", ChannelConfig::new("single").with_level("loud"));
    let error = build(&config).err().expect("invalid level must fail");
    assert!(
        matches!(error, LoggingError::InvalidConfig(_)),
        "got {error}"
    );
}

#[test]
fn double_init_is_safe_and_idempotent() {
    let config = LoggingConfig::new("null").with_channel("null", ChannelConfig::new("null"));
    let _first = init(&config).expect("first init must not panic");
    let second = init(&config).expect("second init must not panic");
    // The second call can never win the global slot, so it is always a no-op.
    assert!(!second.is_installed(), "second init must be a no-op");
}

#[test]
fn from_loader_parses_the_shipped_logging_toml() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let loader = ConfigLoader::load_from_dir(&dir).expect("load config dir");
    let config = LoggingConfig::from_loader(&loader).expect("parse [logging]");

    assert_eq!(config.default, "stack");
    for name in [
        "stack",
        "single",
        "daily",
        "monthly",
        "stderr",
        "stdout",
        "null",
        "emergency",
        "errorlog",
        "slack",
        "papertrail",
        "syslog",
    ] {
        assert!(config.channels.contains_key(name), "missing channel {name}");
    }
    assert_eq!(config.channels["daily"].max_files, Some(14));
    assert_eq!(config.channels["monthly"].max_files, Some(3));
    assert_eq!(config.channels["stack"].channels, vec!["daily", "stderr"]);
}

#[test]
fn shipped_logging_toml_drivers_resolve() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let loader = ConfigLoader::load_from_dir(&dir).expect("load config dir");
    let config = LoggingConfig::from_loader(&loader).expect("parse [logging]");

    assert_eq!(
        config.channel("daily").unwrap().resolve_driver().unwrap(),
        Driver::Daily
    );
    assert_eq!(
        config.channel("monthly").unwrap().resolve_driver().unwrap(),
        Driver::Monthly
    );
    assert_eq!(
        config.channel("single").unwrap().resolve_driver().unwrap(),
        Driver::Single
    );
    assert_eq!(
        config.channel("stderr").unwrap().resolve_driver().unwrap(),
        Driver::Stderr
    );
    assert_eq!(
        config.channel("stdout").unwrap().resolve_driver().unwrap(),
        Driver::Stdout
    );
    assert_eq!(
        config.channel("null").unwrap().resolve_driver().unwrap(),
        Driver::Null
    );
    assert_eq!(
        config
            .channel("emergency")
            .unwrap()
            .resolve_driver()
            .unwrap(),
        Driver::Emergency
    );
    assert_eq!(
        config
            .channel("errorlog")
            .unwrap()
            .resolve_driver()
            .unwrap(),
        Driver::Errorlog
    );
    assert_eq!(
        config.channel("slack").unwrap().resolve_driver().unwrap(),
        Driver::Slack
    );
}

#[test]
fn env_overrides_replace_channel_level_and_stack() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LOG_LEVEL", "warn");
    std::env::set_var("LOG_CHANNEL", "stderr");
    std::env::set_var("LOG_STACK", "single,daily");
    std::env::set_var("LOG_DAILY_DAYS", "5");

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let loader = ConfigLoader::load_from_dir(&dir).expect("load config dir");
    let config = LoggingConfig::from_loader(&loader).expect("parse [logging]");

    std::env::remove_var("LOG_LEVEL");
    std::env::remove_var("LOG_CHANNEL");
    std::env::remove_var("LOG_STACK");
    std::env::remove_var("LOG_DAILY_DAYS");

    assert_eq!(config.default, "stderr");
    assert_eq!(config.level.as_deref(), Some("warn"));
    assert_eq!(config.channels["stack"].channels, vec!["single", "daily"]);
    assert_eq!(config.channels["daily"].max_files, Some(5));
}

#[test]
fn init_from_config_installs_or_noops_without_panicking() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir();
    std::fs::write(
        dir.path().join("logging.toml"),
        "[logging]\ndefault = \"null\"\n[logging.channels.null]\ndriver = \"null\"\n",
    )
    .expect("write config");
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");
    // Either installs (first caller) or no-ops; both are valid and non-panicking.
    let guard = init_from_config(&loader).expect("init from config");
    drop(guard);
}

/// The shipped `config/logging.toml` has no active `[logging.sentry]` table, so
/// parsing must tolerate the absent section and report Sentry as disabled.
#[cfg(feature = "sentry")]
#[test]
fn sentry_config_tolerates_absent_table() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("SENTRY_DSN");
    std::env::remove_var("SENTRY_TRACES_SAMPLE_RATE");
    std::env::remove_var("SENTRY_ENVIRONMENT");

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let loader = ConfigLoader::load_from_dir(&dir).expect("load config dir");
    let config = SentryConfig::from_loader(&loader).expect("parse [logging.sentry]");

    assert!(config.dsn.is_none());
    assert!(!config.enabled, "no DSN means Sentry stays disabled");
}

/// `SENTRY_*` environment variables override the parsed configuration and a
/// present DSN flips `enabled` on.
#[cfg(feature = "sentry")]
#[test]
fn sentry_env_overrides_drive_enabled_state() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("SENTRY_DSN", "https://public@example.com/42");
    std::env::set_var("SENTRY_TRACES_SAMPLE_RATE", "0.25");
    std::env::set_var("SENTRY_ENVIRONMENT", "staging");

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let loader = ConfigLoader::load_from_dir(&dir).expect("load config dir");
    let config = SentryConfig::from_loader(&loader).expect("parse [logging.sentry]");

    std::env::remove_var("SENTRY_DSN");
    std::env::remove_var("SENTRY_TRACES_SAMPLE_RATE");
    std::env::remove_var("SENTRY_ENVIRONMENT");

    assert_eq!(config.dsn.as_deref(), Some("https://public@example.com/42"));
    assert!((config.traces_sample_rate - 0.25).abs() < f32::EPSILON);
    assert_eq!(config.environment.as_deref(), Some("staging"));
    assert!(config.enabled, "a non-empty DSN enables Sentry");
}

/// A blank `SENTRY_DSN` must not enable Sentry (no init, no overhead).
#[cfg(feature = "sentry")]
#[test]
fn sentry_blank_dsn_is_disabled() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("SENTRY_DSN", "   ");

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    let loader = ConfigLoader::load_from_dir(&dir).expect("load config dir");
    let config = SentryConfig::from_loader(&loader).expect("parse [logging.sentry]");

    std::env::remove_var("SENTRY_DSN");

    assert!(!config.enabled, "a blank DSN must not enable Sentry");
    assert!(config.dsn.is_none(), "a blank DSN is normalised away");
}
