//! Configuration and mailer-factory tests for `rustasea-mail`.
//!
//! These cover the Laravel 13.x `mail.php`-shaped `[mail]` table: parsing the
//! shipped `config/mail.toml`, building mailers from it, applying the
//! `[mail.from]` sender, `failover` fallthrough, and the typed error paths.

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use rustasea_config::ConfigLoader;
use rustasea_mail::{
    mailer_from_config, ArrayMailer, FailoverMailer, FromConfig, FromMailer, LogMailer,
    MailAddress, MailConfig, MailConfigError, MailError, MailMessage, Mailer, MailerConfig,
};

/// Serializes tests that read or mutate process-global `MAIL_*` variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Path to the repository's `config/` directory.
fn config_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config")
}

/// Load the shipped `[mail]` table from `config/mail.toml`.
fn shipped_config() -> MailConfig {
    let loader = ConfigLoader::load_from_dir(config_dir()).expect("load config dir");
    MailConfig::from_loader(&loader).expect("parse [mail]")
}

/// A mailer that always fails, used to exercise `failover` fallthrough.
struct FailingMailer;

#[async_trait]
impl Mailer for FailingMailer {
    /// Always report a transport failure.
    async fn send(&self, _message: MailMessage) -> rustasea_mail::Result<()> {
        Err(MailError::Transport("primary unavailable".into()))
    }
}

#[test]
fn shipped_mail_toml_parses_the_full_shape() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let config = shipped_config();

    assert_eq!(config.default, "log");
    for name in ["smtp", "log", "array", "failover"] {
        assert!(config.mailers.contains_key(name), "missing mailer {name}");
    }

    let smtp = &config.mailers["smtp"];
    assert_eq!(smtp.resolve_transport("smtp"), "smtp");
    assert_eq!(smtp.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(smtp.port, Some(2525));

    let failover = &config.mailers["failover"];
    assert_eq!(failover.mailers, vec!["smtp", "log"]);
    assert_eq!(failover.retry_after, Some(60));

    assert_eq!(config.from.address, "hello@example.com");
    assert_eq!(config.from.name.as_deref(), Some("RustaSea"));
    let from = config.from_address().expect("from address");
    assert_eq!(from.email, "hello@example.com");
    assert_eq!(from.name.as_deref(), Some("RustaSea"));
}

#[tokio::test]
async fn factory_builds_array_mailer_and_applies_from() {
    let config = MailConfig::new("array")
        .with_mailer("array", MailerConfig::new("array"))
        .with_from(FromConfig {
            address: "hello@example.com".into(),
            name: Some("RustaSea".into()),
        });

    let mailer = mailer_from_config(&config).expect("build array mailer");
    let from_mailer = mailer
        .downcast_ref::<FromMailer>()
        .expect("from wrapper applied");
    assert_eq!(from_mailer.from().email, "hello@example.com");
    let array = from_mailer
        .inner()
        .downcast_ref::<ArrayMailer>()
        .expect("array transport");

    // Send through the factory-produced wrapper so `[mail.from]` is stamped.
    mailer
        .send(MailMessage::new("Hi").to(MailAddress::from_email("ada@example.com")))
        .await
        .expect("deliver");

    let sent = array.last().expect("captured message");
    let from = sent.from.expect("from stamped from config");
    assert_eq!(from.email, "hello@example.com");
    assert_eq!(from.name.as_deref(), Some("RustaSea"));
}

#[tokio::test]
async fn factory_builds_log_mailer() {
    let config = MailConfig::new("log").with_mailer("log", MailerConfig::new("log"));
    let mailer = mailer_from_config(&config).expect("build log mailer");
    assert!(
        mailer.downcast_ref::<LogMailer>().is_some(),
        "expected a LogMailer"
    );
}

#[tokio::test]
async fn explicit_from_on_message_is_preserved() {
    let config = MailConfig::new("array")
        .with_mailer("array", MailerConfig::new("array"))
        .with_from(FromConfig {
            address: "hello@example.com".into(),
            name: None,
        });
    let mailer = mailer_from_config(&config).expect("build array mailer");
    let from_mailer = mailer.downcast_ref::<FromMailer>().expect("from wrapper");
    let array = from_mailer
        .inner()
        .downcast_ref::<ArrayMailer>()
        .expect("array transport");

    let message = MailMessage::new("Hi")
        .to(MailAddress::from_email("ada@example.com"))
        .sender(MailAddress::from_email("sender@example.com"));
    mailer.send(message).await.expect("deliver");

    let sent = array.last().expect("captured message");
    assert_eq!(
        sent.from.expect("from").email,
        "sender@example.com",
        "message-level sender must win over [mail.from]"
    );
}

#[tokio::test]
async fn failover_falls_through_to_the_next_mailer() {
    let array = std::sync::Arc::new(ArrayMailer::new());
    let failover = FailoverMailer::new(vec![std::sync::Arc::new(FailingMailer), array.clone()]);

    failover
        .send(MailMessage::new("Hi").to(MailAddress::from_email("ada@example.com")))
        .await
        .expect("secondary mailer must succeed");

    assert_eq!(array.count(), 1, "fallback mailer received the message");
}

#[tokio::test]
async fn factory_builds_failover_from_config() {
    let config = MailConfig::new("failover")
        .with_mailer("array", MailerConfig::new("array"))
        .with_mailer("log", MailerConfig::new("log"))
        .with_mailer(
            "failover",
            MailerConfig::new("failover").with_mailers(["array", "log"]),
        );

    let mailer = mailer_from_config(&config).expect("build failover mailer");
    let failover = mailer
        .downcast_ref::<FailoverMailer>()
        .expect("expected a FailoverMailer");
    assert_eq!(failover.mailers().len(), 2);
}

#[test]
fn unknown_transport_is_a_typed_error() {
    let config = MailConfig::new("pigeon").with_mailer("pigeon", MailerConfig::new("carrier"));
    let error = mailer_from_config(&config)
        .err()
        .expect("unknown transport must fail");
    assert!(
        matches!(&error, MailConfigError::UnknownTransport(name) if name == "carrier"),
        "got {error}"
    );
}

#[test]
fn unsupported_transport_is_a_typed_error() {
    let config = MailConfig::new("ses").with_mailer("ses", MailerConfig::new("ses"));
    let error = mailer_from_config(&config)
        .err()
        .expect("ses must be unsupported");
    assert!(
        matches!(&error, MailConfigError::UnsupportedTransport(name) if name == "ses"),
        "got {error}"
    );
}

#[test]
fn missing_smtp_host_is_a_typed_error() {
    let config = MailConfig::new("smtp").with_mailer("smtp", MailerConfig::new("smtp"));
    let error = mailer_from_config(&config)
        .err()
        .expect("smtp without host must fail");
    assert!(
        matches!(&error, MailConfigError::MissingSmtpHost(name) if name == "smtp"),
        "got {error}"
    );
}

#[test]
fn unknown_default_mailer_is_a_typed_error() {
    let config = MailConfig::new("ghost");
    let error = mailer_from_config(&config)
        .err()
        .expect("undefined default must fail");
    assert!(
        matches!(&error, MailConfigError::UnknownDefaultMailer(name) if name == "ghost"),
        "got {error}"
    );
}

#[test]
fn env_overrides_win_over_the_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("MAIL_MAILER", "array");
    std::env::set_var("MAIL_HOST", "smtp.example.test");
    std::env::set_var("MAIL_PORT", "587");
    std::env::set_var("MAIL_USERNAME", "robot");
    std::env::set_var("MAIL_PASSWORD", "secret");
    std::env::set_var("MAIL_FROM_ADDRESS", "override@example.com");
    std::env::set_var("MAIL_FROM_NAME", "Override");

    let config = shipped_config();

    std::env::remove_var("MAIL_MAILER");
    std::env::remove_var("MAIL_HOST");
    std::env::remove_var("MAIL_PORT");
    std::env::remove_var("MAIL_USERNAME");
    std::env::remove_var("MAIL_PASSWORD");
    std::env::remove_var("MAIL_FROM_ADDRESS");
    std::env::remove_var("MAIL_FROM_NAME");

    assert_eq!(config.default, "array");
    let smtp = &config.mailers["smtp"];
    assert_eq!(smtp.host.as_deref(), Some("smtp.example.test"));
    assert_eq!(smtp.port, Some(587));
    assert_eq!(smtp.username.as_deref(), Some("robot"));
    assert_eq!(smtp.password.as_deref(), Some("secret"));
    assert_eq!(config.from.address, "override@example.com");
    assert_eq!(config.from.name.as_deref(), Some("Override"));
}

#[cfg(feature = "smtp")]
#[test]
fn smtp_factory_builds_when_the_feature_is_enabled() {
    let config = MailConfig::new("smtp").with_mailer(
        "smtp",
        MailerConfig::new("smtp")
            .with_host("127.0.0.1")
            .with_port(2525),
    );
    let mailer = mailer_from_config(&config).expect("build smtp mailer");
    assert!(
        mailer.downcast_ref::<rustasea_mail::SmtpMailer>().is_some(),
        "expected an SmtpMailer"
    );
}

#[cfg(not(feature = "smtp"))]
#[test]
fn smtp_factory_requires_the_feature() {
    let config = MailConfig::new("smtp").with_mailer(
        "smtp",
        MailerConfig::new("smtp")
            .with_host("127.0.0.1")
            .with_port(2525),
    );
    let error = mailer_from_config(&config)
        .err()
        .expect("smtp without the feature must fail");
    assert!(
        matches!(error, MailConfigError::MissingSmtpFeature),
        "got {error}"
    );
}
