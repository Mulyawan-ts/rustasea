//! Generated-config integration tests.
//!
//! The scaffolder emits the Laravel-parity `config/*.toml` surface plus the
//! `.env.example` mirror of it. These tests assert each file is present and
//! carries the load-bearing defaults a generated app parses at boot.

use rustasea_scaffold::{Scaffold, StarterKitVariant};

/// The eleven Laravel-parity config files every variant must emit (CFG-010, CFG-011).
/// `config/mongo.toml` (standalone) and `inertia.toml` (variant-specific) are separate.
const REQUIRED_CONFIGS: &[&str] = &[
    "config/app.toml",
    "config/auth.toml",
    "config/cache.toml",
    "config/database.toml",
    "config/queue.toml",
    "config/session.toml",
    "config/logging.toml",
    "config/mail.toml",
    "config/services.toml",
    "config/storage.toml",
    "config/fortify.toml",
];

/// Assert the generated TOML config files exist and carry their key defaults.
///
/// The test harness has no TOML parser available (the scaffold crate depends on
/// `serde` only), so this asserts the load-bearing substrings each file must
/// contain: the defaults a generated app parses at boot.
fn assert_config_surface(files: &[rustasea_scaffold::RenderedFile], variant: &str) {
    for path in REQUIRED_CONFIGS {
        let file = files
            .iter()
            .find(|file| file.path == *path)
            .unwrap_or_else(|| panic!("missing config file {path} ({variant})"));
        assert!(
            !file.contents.trim().is_empty(),
            "empty config file {path} ({variant})"
        );
        assert!(
            !file.contents.contains("@@"),
            "unsubstituted placeholder in {path} ({variant})"
        );
    }

    // Spot-check the defaults a generated app relies on.
    let find = |path: &str| {
        files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("missing {path} ({variant})"))
            .contents
            .as_str()
    };
    assert!(find("config/app.toml").contains("app_name = \"my-app\""));
    assert!(find("config/cache.toml").contains("rustasea-cache-"));
    assert!(find("config/session.toml").contains("cookie = \"rustasea-session\""));
    assert!(find("config/queue.toml").contains("default = \"database\""));
    assert!(find("config/logging.toml").contains("default = \"stack\""));
    assert!(find("config/mail.toml").contains("default = \"log\""));
    assert!(find("config/database.toml").contains("driver = \"sqlite\""));
    assert!(find("config/storage.toml").contains("default = \"local\""));
    assert!(find("config/fortify.toml").contains("home = \"/dashboard\""));
    assert!(find("config/mongo.toml").contains("uri = \"mongodb://localhost:27017\""));
}

#[test]
fn every_variant_emits_all_twelve_configs() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        assert_config_surface(&files, variant.as_str());

        // Twelve `config/*.toml` files, plus `inertia.toml` for react/vue.
        let config_count = files
            .iter()
            .filter(|file| file.path.starts_with("config/") && file.path.ends_with(".toml"))
            .count();
        let expected = if variant.uses_inertia() { 13 } else { 12 };
        assert_eq!(config_count, expected, "bad config count ({variant})");
    }
}

#[test]
fn inertia_variants_add_inertia_config_only() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        let has_inertia = files.iter().any(|file| file.path == "config/inertia.toml");
        assert_eq!(
            has_inertia,
            variant.uses_inertia(),
            "inertia.toml presence must match uses_inertia ({variant})"
        );
    }
}

#[test]
fn generated_env_example_covers_the_config_surface() {
    let files = Scaffold::new("my-app", StarterKitVariant::Blade)
        .render()
        .expect("render");
    let env = files
        .iter()
        .find(|file| file.path == ".env.example")
        .expect(".env.example present")
        .contents
        .as_str();
    for needle in [
        "APP_NAME=",
        "APP_KEY=",
        "LOG_CHANNEL=",
        "MAIL_MAILER=",
        "CACHE_PREFIX=",
        "QUEUE_CONNECTION=",
        "SESSION_DRIVER=",
        "SESSION_SECURE_COOKIE=",
        "SESSION_EXPIRE_ON_CLOSE=",
        "SESSION_ENCRYPT=",
        "SESSION_PARTITIONED_COOKIE=",
        "SESSION_HTTP_ONLY=",
        "SESSION_CONNECTION=",
        "SESSION_TABLE=",
        "SESSION_STORE=",
        "SESSION_PATH=",
        "SESSION_DOMAIN=",
        "AUTH_GUARD=",
        "AUTH_PASSWORD_BROKER=",
        "AUTH_MODEL=",
        "AUTH_PASSWORD_RESET_TOKEN_TABLE=",
        "AUTH_PASSWORD_TIMEOUT=",
        "PASSKEYS_USER_HANDLE_SECRET=",
        "DB_CONNECTION=",
        "DATABASE_URL=",
        "REDIS_URL=",
        "MONGODB_URI=",
        "AWS_ACCESS_KEY_ID=",
        "POSTMARK_API_KEY=",
        "RESEND_API_KEY=",
        "SLACK_BOT_USER_OAUTH_TOKEN=",
    ] {
        assert!(
            env.contains(needle),
            "generated .env.example missing {needle}"
        );
    }
}
