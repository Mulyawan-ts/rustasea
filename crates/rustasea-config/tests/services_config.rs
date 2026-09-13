//! Integration tests for the `[services]` credentials surface (CFG-007).
//!
//! Exercises the public API only: a typed [`ServicesConfig`] parsed from a temp
//! `services.toml` exposes the Laravel-style third-party credentials, treats
//! blank values as `None`, applies the documented environment overrides, and
//! surfaces a typed error on malformed input. The shipped repository
//! `config/services.toml` is also parsed to prove the shape matches.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use rustasea_config::{
    ConfigLoader, ServicesConfig, ServicesConfigError, SesCredentials, SlackNotifications,
    DEFAULT_SES_REGION,
};

/// Serializes tests that read or write process-global environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Environment variables consulted by `ServicesConfig::from_loader`.
const SERVICE_ENV_KEYS: [&str; 6] = [
    "POSTMARK_API_KEY",
    "RESEND_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "SLACK_BOT_USER_OAUTH_TOKEN",
    "SLACK_BOT_USER_DEFAULT_CHANNEL",
];

/// Unique temporary directory removed when dropped.
struct TempConfigDir {
    path: PathBuf,
}

impl TempConfigDir {
    /// Create an empty temp directory with a process-unique name.
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustasea-services-{}-{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&path).expect("create temp config dir");
        Self { path }
    }

    /// Write `contents` to `name` inside the temp directory.
    fn write(&self, name: &str, contents: &str) {
        fs::write(self.path.join(name), contents).expect("write temp config file");
    }

    /// Borrow the temp directory path.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempConfigDir {
    /// Remove the temp directory and all of its contents.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Clear the service environment variables so file-only tests are stable.
fn clear_service_env() {
    for key in SERVICE_ENV_KEYS {
        std::env::remove_var(key);
    }
}

/// Path to the workspace-root `config/` directory.
fn workspace_config_dir() -> PathBuf {
    // `CARGO_MANIFEST_DIR` = `<workspace>/crates/rustasea-config`; the workspace
    // root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("config")
}

const FULL_TOML: &str = r##"
[services.postmark]
key = "postmark-token"

[services.resend]
key = "resend-token"

[services.ses]
key = "aws-key"
secret = "aws-secret"
region = "eu-west-1"

[services.slack.notifications]
bot_user_oauth_token = "xoxb-token"
channel = "#alerts"
"##;

/// All four service blocks parse into their typed accessors.
#[test]
fn full_toml_parses_into_typed_accessors() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    let dir = TempConfigDir::new();
    dir.write("services.toml", FULL_TOML);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");

    let config = ServicesConfig::from_loader(&loader).expect("parse full services.toml");

    assert_eq!(config.postmark_key().as_deref(), Some("postmark-token"));
    assert_eq!(config.resend_key().as_deref(), Some("resend-token"));
    assert_eq!(
        config.ses_credentials(),
        Some(SesCredentials {
            key: "aws-key".to_string(),
            secret: "aws-secret".to_string(),
            region: "eu-west-1".to_string(),
        })
    );
    assert_eq!(
        config.slack_notifications(),
        Some(SlackNotifications {
            bot_user_oauth_token: "xoxb-token".to_string(),
            channel: "#alerts".to_string(),
        })
    );
}

/// Blank placeholders are treated as unset.
#[test]
fn blank_values_are_none() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    let dir = TempConfigDir::new();
    dir.write(
        "services.toml",
        r#"
[services.postmark]
key = ""
[services.resend]
key = ""
[services.ses]
key = ""
secret = ""
region = ""
[services.slack.notifications]
bot_user_oauth_token = ""
channel = ""
"#,
    );
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");

    let config = ServicesConfig::from_loader(&loader).expect("parse blank services.toml");

    assert_eq!(config.postmark_key(), None);
    assert_eq!(config.resend_key(), None);
    assert_eq!(config.ses_credentials(), None);
    assert_eq!(config.slack_notifications(), None);
}

/// A missing `[services]` section is tolerated and yields all `None`.
#[test]
fn missing_section_is_all_none() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    let dir = TempConfigDir::new();
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load empty config");

    let config = ServicesConfig::from_loader(&loader).expect("missing services is fine");

    assert_eq!(config, ServicesConfig::default());
}

/// A blank `region` falls back to the Laravel default.
#[test]
fn blank_region_falls_back_to_default() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    let dir = TempConfigDir::new();
    dir.write(
        "services.toml",
        r#"
[services.ses]
key = "aws-key"
secret = "aws-secret"
region = ""
"#,
    );
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");

    let ses = ServicesConfig::from_loader(&loader)
        .expect("parse")
        .ses_credentials()
        .expect("credentials present");
    assert_eq!(ses.region, DEFAULT_SES_REGION);
}

/// The documented environment overrides win over the file.
#[test]
fn env_override_wins_over_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    std::env::set_var("POSTMARK_API_KEY", "env-postmark");
    std::env::set_var("AWS_ACCESS_KEY_ID", "env-aws-key");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "env-aws-secret");

    let dir = TempConfigDir::new();
    dir.write("services.toml", FULL_TOML);
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");
    let config = ServicesConfig::from_loader(&loader).expect("parse with env overrides");

    clear_service_env();
    assert_eq!(config.postmark_key().as_deref(), Some("env-postmark"));
    assert_eq!(
        config.ses_credentials(),
        Some(SesCredentials {
            key: "env-aws-key".to_string(),
            secret: "env-aws-secret".to_string(),
            region: "eu-west-1".to_string(),
        })
    );
}

/// An environment override with no file still creates the block.
#[test]
fn env_override_without_file_creates_block() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    std::env::set_var("SLACK_BOT_USER_OAUTH_TOKEN", "env-xoxb");
    std::env::set_var("SLACK_BOT_USER_DEFAULT_CHANNEL", "#env");

    let dir = TempConfigDir::new();
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load empty config");
    let config = ServicesConfig::from_loader(&loader).expect("parse with env only");

    clear_service_env();
    assert_eq!(
        config.slack_notifications(),
        Some(SlackNotifications {
            bot_user_oauth_token: "env-xoxb".to_string(),
            channel: "#env".to_string(),
        })
    );
}

/// A number where a string is expected is a typed error naming the key.
#[test]
fn malformed_string_type_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    let dir = TempConfigDir::new();
    dir.write("services.toml", "[services.postmark]\nkey = 123\n");
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");

    let error = ServicesConfig::from_loader(&loader).expect_err("number for key must fail");
    match error {
        ServicesConfigError::Invalid(message) => {
            assert!(message.contains("services.postmark.key"), "got: {message}");
        }
    }
}

/// A scalar where a table is expected is a typed error.
#[test]
fn malformed_table_type_is_typed_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    let dir = TempConfigDir::new();
    dir.write("services.toml", "services = 5\n");
    let loader = ConfigLoader::load_from_dir(dir.path()).expect("load config");

    let error = ServicesConfig::from_loader(&loader).expect_err("scalar for services must fail");
    assert!(matches!(error, ServicesConfigError::Invalid(_)));
}

/// The shipped `services.toml` parses and exposes no credentials by default.
#[test]
fn shipped_services_toml_parses_with_empty_placeholders() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_service_env();
    let loader = ConfigLoader::load_from_dir(workspace_config_dir()).expect("load shipped config");
    let config = ServicesConfig::from_loader(&loader).expect("parse shipped services.toml");

    assert_eq!(config.postmark_key(), None);
    assert_eq!(config.resend_key(), None);
    assert_eq!(config.ses_credentials(), None);
    assert_eq!(config.slack_notifications(), None);
}
