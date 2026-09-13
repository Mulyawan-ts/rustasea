//! Facade tests: config parsing and config-keyed disk construction.

use std::path::PathBuf;

use rustasea_storage::{StorageError, StorageFacadeConfig, StorageManager, Visibility};

/// Unique scratch directory for a test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustasea-facade-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

#[test]
fn parses_workspace_storage_config() {
    let config = StorageFacadeConfig::from_toml(include_str!("../../../config/storage.toml"))
        .expect("config/storage.toml must parse");
    assert_eq!(config.default.as_deref(), Some("local"));
    assert!(config.disks.contains_key("local"));
    assert!(config.disks.contains_key("public"));
    assert!(config.disks.contains_key("archive"));
    assert!(config.read_through.is_none());
    assert_eq!(
        config.links.get("public/storage").map(String::as_str),
        Some("storage/app/public")
    );
}

#[test]
fn parses_laravel_parity_disk_settings() {
    let config = StorageFacadeConfig::from_toml(include_str!("../../../config/storage.toml"))
        .expect("config/storage.toml must parse");
    let local = config.disks.get("local").unwrap().settings();
    assert_eq!(local.serve, Some(true));
    assert_eq!(local.visibility, Some(Visibility::Local));
    assert_eq!(local.throw, Some(false));
    assert_eq!(local.report, Some(false));

    let public = config.disks.get("public").unwrap().settings();
    assert_eq!(public.visibility, Some(Visibility::Public));
    assert_eq!(public.serve, None);
}

#[test]
fn absent_settings_deserialize_to_none() {
    let toml = r#"
[storage]
default = "local"

[storage.disks.local]
driver = "local"
root = "/tmp/rustasea-facade-absent"
"#;
    let config = StorageFacadeConfig::from_toml(toml).unwrap();
    let settings = config.disks.get("local").unwrap().settings();
    assert_eq!(settings.serve, None);
    assert_eq!(settings.visibility, None);
    assert_eq!(settings.throw, None);
    assert_eq!(settings.report, None);
}

#[test]
fn invalid_visibility_is_a_config_error() {
    let toml = r#"
[storage]
default = "local"

[storage.disks.local]
driver = "local"
root = "/tmp/rustasea-facade-badvis"
visibility = "world-readable"
"#;
    let err = StorageFacadeConfig::from_toml(toml).expect_err("invalid visibility must fail");
    assert!(matches!(err, StorageError::Config(_)), "got {err:?}");
}

#[test]
fn create_links_materializes_symlink_and_is_idempotent() {
    let base = scratch("links");
    std::fs::create_dir_all(&base).unwrap();
    let target = base.join("storage/app/public");
    std::fs::create_dir_all(&target).unwrap();
    let link = base.join("public/storage");

    let manager = StorageManager::new("local", "local", false)
        .with_local("local", rustasea_storage::LocalDisk::new(&target))
        .with_links(
            [(link.display().to_string(), target.display().to_string())]
                .into_iter()
                .collect(),
        );

    let rt = tokio::runtime::Runtime::new().unwrap();
    let created = rt.block_on(manager.create_links()).unwrap();
    assert_eq!(created, vec![link.clone()]);
    assert!(link.symlink_metadata().is_ok(), "symlink must exist");

    // Idempotent: a second run creates nothing.
    let again = rt.block_on(manager.create_links()).unwrap();
    assert!(again.is_empty(), "existing links must be skipped");

    std::fs::remove_dir_all(&base).ok();
}

/// Build a manager whose `links` map holds a single entry.
fn manager_with_link(link: &str, target: &str) -> StorageManager {
    StorageManager::new("local", "local", false)
        .with_local(
            "local",
            rustasea_storage::LocalDisk::new(std::env::temp_dir()),
        )
        .with_links(
            [(link.to_string(), target.to_string())]
                .into_iter()
                .collect(),
        )
}

#[test]
fn create_links_rejects_traversal_in_link_path() {
    let manager = manager_with_link("../escaped/storage", "storage/app/public");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(manager.create_links())
        .expect_err("traversal link path must be rejected");
    assert!(matches!(err, StorageError::PathTraversal(_)), "got {err:?}");
}

#[test]
fn create_links_rejects_escaping_target() {
    let manager = manager_with_link("public/storage", "../../etc/passwd");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(manager.create_links())
        .expect_err("escaping target must be rejected");
    assert!(matches!(err, StorageError::PathTraversal(_)), "got {err:?}");
}

#[test]
fn create_links_rejects_traversal_before_any_fs_mutation() {
    let base = scratch("links-no-mutation");
    std::fs::create_dir_all(&base).unwrap();
    let link = base.join("public/storage");

    let manager = manager_with_link(&link.display().to_string(), "../outside");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt.block_on(manager.create_links()).unwrap_err();
    assert!(matches!(err, StorageError::PathTraversal(_)), "got {err:?}");
    assert!(
        link.symlink_metadata().is_err(),
        "no symlink may be created on rejection"
    );
    assert!(
        !link.parent().unwrap().exists(),
        "no parent directory may be created on rejection"
    );

    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn builds_local_disks_and_round_trips() {
    let root = scratch("roundtrip");
    let archive = scratch("roundtrip-archive");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&archive).unwrap();
    let toml = format!(
        r#"
[storage]
default = "local"

[storage.disks.local]
driver = "local"
root = "{root}"

[storage.disks.archive]
driver = "local"
root = "{archive}"
"#,
        root = root.display(),
        archive = archive.display(),
    );

    let manager = StorageManager::from_toml(&toml).unwrap();
    assert!(manager.disk("archive").is_ok());

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        manager.put("a/b.txt", b"hello").await.unwrap();
        assert!(manager.exists("a/b.txt").await.unwrap());
        assert_eq!(manager.get("a/b.txt").await.unwrap(), b"hello".to_vec());
        manager.delete("a/b.txt").await.unwrap();
        assert!(!manager.exists("a/b.txt").await.unwrap());
    });
}

#[test]
fn from_toml_file_reads_document() {
    let root = scratch("file");
    std::fs::create_dir_all(&root).unwrap();
    let path = scratch("file-manifest").with_extension("toml");
    std::fs::write(
        &path,
        format!(
            "[storage]\ndefault = \"local\"\n\n[storage.disks.local]\ndriver = \"local\"\nroot = \"{}\"\n",
            root.display()
        ),
    )
    .unwrap();

    let manager = StorageManager::from_toml_file(&path).unwrap();
    assert!(manager.disk("local").is_ok());
    std::fs::remove_file(&path).ok();
}

#[test]
fn unknown_driver_is_a_config_error() {
    let toml = r#"
[storage]
default = "local"

[storage.disks.local]
driver = "ftp"
host = "example.com"
"#;
    let err = StorageManager::from_toml(toml)
        .err()
        .expect("unknown driver must fail");
    assert!(matches!(err, StorageError::Config(_)), "got {err:?}");
}

#[test]
fn missing_default_and_read_through_is_rejected() {
    let toml = r#"
[storage.disks.local]
driver = "local"
root = "/tmp/rustasea-facade-none"
"#;
    let err = StorageManager::from_toml(toml)
        .err()
        .expect("unknown driver must fail");
    assert!(matches!(err, StorageError::Config(_)), "got {err:?}");
}

#[test]
fn read_through_primary_must_exist() {
    let root = scratch("missing-primary");
    std::fs::create_dir_all(&root).unwrap();
    let toml = format!(
        r#"
[storage]
default = "local"

[storage.read_through]
primary = "s3"
fallback = "local"
copy_back = false

[storage.disks.local]
driver = "local"
root = "{root}"
"#,
        root = root.display(),
    );
    let err = StorageManager::from_toml(&toml)
        .err()
        .expect("config must be rejected");
    assert!(matches!(err, StorageError::Config(_)), "got {err:?}");
}

#[cfg(not(any(feature = "aws", feature = "gcp", feature = "azure")))]
#[test]
fn cloud_drivers_require_their_feature() {
    for (driver, feature) in [("s3", "aws"), ("gcs", "gcp"), ("azure", "azure")] {
        let toml = format!(
            r#"
[storage]
default = "local"

[storage.disks.local]
driver = "local"
root = "/tmp/rustasea-facade-cloud"

[storage.disks.cloud]
driver = "{driver}"
bucket = "example"
account = "example"
container = "example"
"#
        );
        let err = StorageManager::from_toml(&toml)
            .err()
            .expect("config must be rejected");
        assert!(
            matches!(err, StorageError::StoreUnavailable(_)),
            "driver {driver}: got {err:?}"
        );
        assert!(
            err.to_string().contains(feature),
            "driver {driver}: error must name the `{feature}` feature: {err}"
        );
    }
}

#[cfg(all(feature = "aws", feature = "gcp", feature = "azure"))]
#[test]
fn cloud_disk_definitions_parse_with_features() {
    let toml = r#"
[storage]
default = "local"

[storage.disks.local]
driver = "local"
root = "/tmp/rustasea-facade-cloud"

[storage.disks.s3]
driver = "s3"
bucket = "example"
region = "us-east-1"

[storage.disks.gcs]
driver = "gcs"
bucket = "example"

[storage.disks.azure]
driver = "azure"
account = "example"
container = "example"
"#;
    let config = StorageFacadeConfig::from_toml(toml).unwrap();
    assert_eq!(config.disks.len(), 4);
}
