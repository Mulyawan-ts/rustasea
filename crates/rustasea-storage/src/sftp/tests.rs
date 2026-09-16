//! Hermetic unit tests for the SFTP disk configuration (no network).

use std::sync::Mutex;

use super::config::{
    SftpDiskConfig, DEFAULT_SFTP_PORT, DEFAULT_SFTP_ROOT, DEFAULT_SFTP_TIMEOUT_SECS,
};
use crate::error::StorageError;

/// Serializes environment-mutating tests (precedent: broadcast `manager/tests.rs`).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Run `body` with the given env vars set, restoring prior values afterwards.
fn with_env<F: FnOnce()>(vars: &[(&str, &str)], body: F) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let prior: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
        .collect();
    for (key, value) in vars {
        std::env::set_var(key, value);
    }
    body();
    for (key, value) in prior {
        match value {
            Some(value) => std::env::set_var(&key, value),
            None => std::env::remove_var(&key),
        }
    }
}

#[test]
fn deserializes_from_toml_with_driver_tag() {
    let toml = r#"
[storage]
default = "remote"

[storage.disks.remote]
driver = "sftp"
host = "files.example.com"
port = 2222
username = "deploy"
password = "s3cret"
private_key_path = "/etc/rustasea/id_ed25519"
root = "/srv/upload"
timeout = 45
host_key = "SHA256:abc"
"#;
    let config = crate::StorageFacadeConfig::from_toml(toml).unwrap();
    let disk = config.disks.get("remote").unwrap();
    let sftp = match disk {
        crate::DiskDefinition::Sftp(sftp) => sftp,
        other => panic!("expected sftp disk, got {other:?}"),
    };
    assert_eq!(sftp.host, "files.example.com");
    assert_eq!(sftp.port, Some(2222));
    assert_eq!(sftp.username, "deploy");
    assert_eq!(sftp.password.as_deref(), Some("s3cret"));
    assert_eq!(
        sftp.private_key_path.as_deref(),
        Some("/etc/rustasea/id_ed25519")
    );
    assert_eq!(sftp.root.as_deref(), Some("/srv/upload"));
    assert_eq!(sftp.timeout, Some(45));
    assert_eq!(sftp.host_key.as_deref(), Some("SHA256:abc"));
}

#[test]
fn defaults_apply_when_optional_keys_absent() {
    let toml = r#"
[storage]
default = "remote"

[storage.disks.remote]
driver = "sftp"
host = "files.example.com"
username = "deploy"
"#;
    let config = crate::StorageFacadeConfig::from_toml(toml).unwrap();
    let sftp = match config.disks.get("remote").unwrap() {
        crate::DiskDefinition::Sftp(sftp) => sftp,
        other => panic!("expected sftp disk, got {other:?}"),
    };
    assert_eq!(sftp.port(), DEFAULT_SFTP_PORT);
    assert_eq!(sftp.root(), DEFAULT_SFTP_ROOT);
    assert_eq!(sftp.timeout_secs(), DEFAULT_SFTP_TIMEOUT_SECS);
    assert!(sftp.password.is_none());
    assert!(sftp.host_key.is_none());
}

#[test]
fn debug_redacts_password() {
    let config = SftpDiskConfig {
        host: "files.example.com".into(),
        username: "deploy".into(),
        password: Some("super-secret-value".into()),
        ..empty_config()
    };
    let rendered = format!("{config:?}");
    assert!(!rendered.contains("super-secret-value"));
    assert!(rendered.contains("[REDACTED]"));
    assert!(rendered.contains("files.example.com"));
    assert!(rendered.contains("deploy"));
}

#[test]
fn apply_env_overlays_documented_variables() {
    let mut config = SftpDiskConfig {
        host: "file.example.com".into(),
        username: "fileuser".into(),
        ..empty_config()
    };
    with_env(
        &[
            ("SFTP_HOST", "env.example.com"),
            ("SFTP_PORT", "2022"),
            ("SFTP_USERNAME", "envuser"),
            ("SFTP_PASSWORD", "envpass"),
            ("SFTP_PRIVATE_KEY", "/env/key"),
            ("SFTP_ROOT", "/env/root"),
            ("SFTP_TIMEOUT", "99"),
        ],
        || config.apply_env(),
    );
    assert_eq!(config.host, "env.example.com");
    assert_eq!(config.port, Some(2022));
    assert_eq!(config.username, "envuser");
    assert_eq!(config.password.as_deref(), Some("envpass"));
    assert_eq!(config.private_key_path.as_deref(), Some("/env/key"));
    assert_eq!(config.root.as_deref(), Some("/env/root"));
    assert_eq!(config.timeout, Some(99));
}

#[test]
fn apply_env_ignores_blank_and_unparseable_values() {
    let mut config = SftpDiskConfig {
        host: "file.example.com".into(),
        username: "fileuser".into(),
        port: Some(2222),
        timeout: Some(45),
        ..empty_config()
    };
    with_env(
        &[
            ("SFTP_HOST", "   "),
            ("SFTP_PORT", "not-a-port"),
            ("SFTP_TIMEOUT", "not-a-number"),
        ],
        || config.apply_env(),
    );
    assert_eq!(config.host, "file.example.com");
    assert_eq!(config.port, Some(2222));
    assert_eq!(config.timeout, Some(45));
}

#[test]
fn validate_rejects_blank_host_and_username() {
    let missing_host = SftpDiskConfig {
        username: "deploy".into(),
        ..empty_config()
    };
    assert!(matches!(
        missing_host.validate().unwrap_err(),
        StorageError::Config(_)
    ));

    let missing_user = SftpDiskConfig {
        host: "files.example.com".into(),
        ..empty_config()
    };
    assert!(matches!(
        missing_user.validate().unwrap_err(),
        StorageError::Config(_)
    ));
}

#[test]
fn validate_accepts_complete_config() {
    let config = SftpDiskConfig {
        host: "files.example.com".into(),
        username: "deploy".into(),
        ..empty_config()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn key_confinement_rejects_traversal_and_absolute() {
    for key in ["../evil", "a/../b", "/absolute", "a//b", "", "a/./b"] {
        let err = SftpDiskConfig::validate_key(key).unwrap_err();
        assert!(
            matches!(err, StorageError::PathTraversal(_)),
            "key {key:?}: got {err:?}"
        );
    }
}

#[test]
fn key_confinement_accepts_nested_keys() {
    assert!(SftpDiskConfig::validate_key("a/b/c.txt").is_ok());
    assert!(SftpDiskConfig::validate_key("file.txt").is_ok());
}

#[test]
fn remote_path_joins_under_root() {
    let config = SftpDiskConfig {
        host: "h".into(),
        username: "u".into(),
        root: Some("/srv/upload/".into()),
        ..empty_config()
    };
    assert_eq!(
        config.remote_path("a/b.txt").unwrap(),
        "/srv/upload/a/b.txt"
    );

    let slash_root = SftpDiskConfig {
        root: Some("/".into()),
        ..config
    };
    assert_eq!(slash_root.remote_path("a.txt").unwrap(), "/a.txt");
}

#[test]
fn remote_path_rejects_escaping_key() {
    let config = SftpDiskConfig {
        host: "h".into(),
        username: "u".into(),
        root: Some("/srv".into()),
        ..empty_config()
    };
    let err = config.remote_path("../etc/passwd").unwrap_err();
    assert!(matches!(err, StorageError::PathTraversal(_)));
}

#[test]
fn list_dir_maps_prefix_to_remote_directory() {
    let config = SftpDiskConfig {
        host: "h".into(),
        username: "u".into(),
        root: Some("/srv/upload".into()),
        ..empty_config()
    };
    assert_eq!(config.list_dir("").unwrap(), "/srv/upload");
    assert_eq!(config.list_dir("/").unwrap(), "/srv/upload");
    assert_eq!(config.list_dir("a/b/").unwrap(), "/srv/upload/a/b");
    assert_eq!(SftpDiskConfig::relative_prefix("/a/b/"), "a/b");
}

/// A config with every field at its default (blank host/username included).
fn empty_config() -> SftpDiskConfig {
    SftpDiskConfig {
        host: String::new(),
        port: None,
        username: String::new(),
        password: None,
        private_key_path: None,
        root: None,
        timeout: None,
        host_key: None,
        settings: Default::default(),
    }
}
