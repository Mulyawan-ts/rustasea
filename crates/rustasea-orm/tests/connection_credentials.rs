//! Credential percent-encoding tests for the connection-URL builders.
//!
//! Verifies that `username`/`password` values containing RFC 3986 reserved
//! characters (`@`, `:`, `/`, `#`, `?`, `%`, …) are percent-encoded before they
//! are interpolated into a `postgres://`/`mysql://` URL or a `mongodb://` URI,
//! so the assembled string stays parseable and the authority is not corrupted.
//! Plain credentials must round-trip unchanged.
//!
//! Note: the `url` crate exposes `username()`/`password()` as percent-encoded
//! ASCII, so the assertions compare against the encoded forms — which is exactly
//! what proves the reserved characters were escaped rather than emitted raw.

use rustasea_orm::ConnectionConfig;
use url::Url;

/// A password exercising every reserved delimiter the finding calls out.
const TRICKY_PASSWORD: &str = "p@ss:w/rd#1%";

/// The expected percent-encoding of [`TRICKY_PASSWORD`].
const ENCODED_PASSWORD: &str = "p%40ss%3Aw%2Frd%231%25";

/// A username containing an `@` (a classic authority-corrupting character).
const TRICKY_USERNAME: &str = "user@corp";

/// The expected percent-encoding of [`TRICKY_USERNAME`].
const ENCODED_USERNAME: &str = "user%40corp";

/// Build a Postgres connection config from granular fields.
fn postgres(username: &str, password: &str) -> ConnectionConfig {
    ConnectionConfig {
        driver: "postgres".into(),
        host: Some("db.example".into()),
        port: Some(5432),
        database: Some("app".into()),
        username: Some(username.into()),
        password: Some(password.into()),
        ..ConnectionConfig::default()
    }
}

/// A Postgres URL with reserved characters parses and round-trips credentials.
#[test]
fn network_url_encodes_reserved_password_characters() {
    let url = postgres(TRICKY_USERNAME, TRICKY_PASSWORD)
        .build_url()
        .expect("build postgres url");

    assert_eq!(
        url,
        format!("postgres://{ENCODED_USERNAME}:{ENCODED_PASSWORD}@db.example:5432/app")
    );

    // The `url` crate parses the encoded credentials back to their raw form.
    let parsed = Url::parse(&url).expect("assembled URL must parse");
    assert_eq!(parsed.host_str(), Some("db.example"));
    assert_eq!(parsed.port(), Some(5432));
    assert_eq!(parsed.path(), "/app");
    assert_eq!(parsed.username(), ENCODED_USERNAME);
    assert_eq!(parsed.password(), Some(ENCODED_PASSWORD));
}

/// A MySQL URL percent-encodes credentials the same way as Postgres.
#[test]
fn mysql_url_encodes_reserved_password_characters() {
    let config = ConnectionConfig {
        driver: "mysql".into(),
        host: Some("db.example".into()),
        database: Some("app".into()),
        username: Some(TRICKY_USERNAME.into()),
        password: Some(TRICKY_PASSWORD.into()),
        ..ConnectionConfig::default()
    };
    let url = config.build_url().expect("build mysql url");
    let parsed = Url::parse(&url).expect("assembled URL must parse");
    assert_eq!(parsed.host_str(), Some("db.example"));
    assert_eq!(parsed.username(), ENCODED_USERNAME);
    assert_eq!(parsed.password(), Some(ENCODED_PASSWORD));
}

/// Plain (unreserved) credentials are left byte-for-byte unchanged.
#[test]
fn plain_credentials_are_unchanged() {
    let url = postgres("rustasea", "secret")
        .build_url()
        .expect("build postgres url");
    assert_eq!(url, "postgres://rustasea:secret@db.example:5432/app");
}

/// A Mongo URI with reserved characters parses and round-trips credentials.
#[test]
fn mongo_uri_encodes_reserved_password_characters() {
    let config = ConnectionConfig {
        driver: "mongodb".into(),
        host: Some("db.example".into()),
        port: Some(27017),
        database: Some("app".into()),
        username: Some(TRICKY_USERNAME.into()),
        password: Some(TRICKY_PASSWORD.into()),
        ..ConnectionConfig::default()
    };
    let uri = config.build_mongo_uri().expect("build mongodb uri");

    assert_eq!(
        uri,
        format!("mongodb://{ENCODED_USERNAME}:{ENCODED_PASSWORD}@db.example:27017/app")
    );

    let parsed = Url::parse(&uri).expect("assembled URI must parse");
    assert_eq!(parsed.scheme(), "mongodb");
    assert_eq!(parsed.host_str(), Some("db.example"));
    assert_eq!(parsed.port(), Some(27017));
    assert_eq!(parsed.username(), ENCODED_USERNAME);
    assert_eq!(parsed.password(), Some(ENCODED_PASSWORD));
}

/// A Mongo URI with plain credentials is left byte-for-byte unchanged.
#[test]
fn mongo_plain_credentials_are_unchanged() {
    let config = ConnectionConfig {
        driver: "mongodb".into(),
        host: Some("db.example".into()),
        database: Some("app".into()),
        username: Some("rustasea".into()),
        password: Some("secret".into()),
        ..ConnectionConfig::default()
    };
    assert_eq!(
        config.build_mongo_uri().unwrap(),
        "mongodb://rustasea:secret@db.example:27017/app"
    );
}

/// A username with no password still percent-encodes and parses cleanly.
#[test]
fn username_only_with_reserved_characters_parses() {
    let config = ConnectionConfig {
        driver: "postgres".into(),
        host: Some("db.example".into()),
        database: Some("app".into()),
        username: Some(TRICKY_USERNAME.into()),
        ..ConnectionConfig::default()
    };
    let url = config.build_url().expect("build postgres url");
    assert_eq!(
        url,
        format!("postgres://{ENCODED_USERNAME}@db.example:5432/app")
    );
    let parsed = Url::parse(&url).expect("assembled URL must parse");
    assert_eq!(parsed.username(), ENCODED_USERNAME);
    assert_eq!(parsed.password(), None);
}
