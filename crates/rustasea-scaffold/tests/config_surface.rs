//! Generated-config integration tests.
//!
//! The scaffolder emits the Laravel-parity `config/*.toml` surface plus the
//! `.env.example` mirror of it. These tests assert each file is present and
//! carries the load-bearing defaults a generated app parses at boot.

use rustasea_scaffold::{Scaffold, StarterKitVariant};

/// The Laravel-parity config files every variant must emit (CFG-010, CFG-011),
/// plus the framework's typed `broadcasting` and `cors` surfaces.
/// `config/mongo.toml` (standalone) and `inertia.toml` (variant-specific) are
/// asserted separately below.
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
    "config/broadcasting.toml",
    "config/cors.toml",
];

/// Assert the generated TOML config files exist, parse as TOML, and carry their
/// key defaults.
///
/// Each required file is parsed with the `toml` crate (a dev-dependency) so a
/// malformed template fails here rather than in a generated app, then the
/// load-bearing substrings a generated app relies on are asserted.
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
        file.contents
            .parse::<toml::Value>()
            .unwrap_or_else(|error| panic!("{path} is not valid TOML ({variant}): {error}"));
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

    // Broadcasting: the parsed default selector must be the in-process hub.
    let broadcasting = find("config/broadcasting.toml")
        .parse::<toml::Value>()
        .unwrap_or_else(|error| panic!("config/broadcasting.toml is not valid TOML: {error}"));
    assert_eq!(
        broadcasting
            .get("broadcasting")
            .and_then(|table| table.get("default"))
            .and_then(toml::Value::as_str),
        Some("hub"),
        "broadcasting default must be `hub` ({variant})"
    );

    // CORS: restrictive by default (empty origins, no credentials).
    let cors = find("config/cors.toml")
        .parse::<toml::Value>()
        .unwrap_or_else(|error| panic!("config/cors.toml is not valid TOML: {error}"));
    let cors_table = cors
        .get("cors")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("config/cors.toml missing [cors] table ({variant})"));
    assert_eq!(
        cors_table
            .get("allowed_origins")
            .and_then(toml::Value::as_array)
            .map(Vec::len),
        Some(0),
        "cors allowed_origins must be empty by default ({variant})"
    );
    assert_eq!(
        cors_table
            .get("allow_credentials")
            .and_then(toml::Value::as_bool),
        Some(false),
        "cors allow_credentials must default to false ({variant})"
    );
}

#[test]
fn every_variant_emits_all_fourteen_configs() {
    for variant in StarterKitVariant::ALL {
        let files = Scaffold::new("my-app", variant).render().expect("render");
        assert_config_surface(&files, variant.as_str());

        // Fourteen `config/*.toml` files, plus `inertia.toml` for react/vue/svelte.
        let config_count = files
            .iter()
            .filter(|file| file.path.starts_with("config/") && file.path.ends_with(".toml"))
            .count();
        let expected = if variant.uses_inertia() { 15 } else { 14 };
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
        "BROADCAST_CONNECTION=",
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
