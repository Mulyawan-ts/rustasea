//! Tests for [`CacheConfig`] parsing/validation and [`CacheManager`] wiring.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::*;
use crate::repository::CacheManager;

/// Serializes tests that read or mutate process-global `CACHE_*` variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Unique temp config dir; removed on drop.
struct TempConfigDir {
    path: std::path::PathBuf,
}

impl TempConfigDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustasea-cache-config-{}-{}",
            std::process::id(),
            unique
        ));
        std::fs::create_dir_all(&path).expect("create temp config dir");
        Self { path }
    }

    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.path.join(name), contents).expect("write config file");
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempConfigDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const CACHE_TOML: &str = r#"
[cache]
default = "memory"
prefix = "rustasea-cache-"
serializable_classes = []

[cache.stores.memory]
driver = "memory"
serialize = false

[cache.stores.redis]
driver = "redis"
connection = "redis://127.0.0.1:6379"
lock_connection = "redis://127.0.0.1:6379"
"#;

fn loader_with(dir: &TempConfigDir) -> ConfigLoader {
    ConfigLoader::load_from_dir(dir.path()).expect("load config")
}

#[test]
fn parses_two_store_config() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write("cache.toml", CACHE_TOML);
    let config = CacheConfig::from_loader(&loader_with(&dir)).expect("parse config");

    assert_eq!(config.default_store(), "memory");
    assert_eq!(config.prefix(), "rustasea-cache-");
    assert_eq!(config.stores.len(), 2);
    assert_eq!(
        config
            .store_config("memory")
            .unwrap()
            .driver_kind()
            .unwrap(),
        CacheDriver::Memory
    );
    assert_eq!(
        config.store_config("redis").unwrap().driver_kind().unwrap(),
        CacheDriver::Redis
    );
    assert_eq!(
        config
            .store_config("redis")
            .unwrap()
            .resolved_url()
            .as_deref(),
        Some("redis://127.0.0.1:6379")
    );
}

#[test]
fn missing_table_yields_defaults() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write("app.toml", "[app]\nname = \"rustasea\"\n");
    let config = CacheConfig::from_loader(&loader_with(&dir)).expect("parse config");
    assert_eq!(config, CacheConfig::default());
    assert_eq!(config.default_store(), DEFAULT_STORE);
    assert_eq!(config.prefix(), DEFAULT_CACHE_PREFIX);
}

#[test]
fn array_driver_maps_to_memory() {
    let store = StoreConfig {
        driver: "array".into(),
        ..StoreConfig::default()
    };
    assert_eq!(store.driver_kind().unwrap(), CacheDriver::Memory);
}

#[test]
fn manager_registers_memory_and_resolves_default() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write("cache.toml", CACHE_TOML);
    let config = CacheConfig::from_loader(&loader_with(&dir)).expect("parse config");
    let manager = CacheManager::from_config(&config).expect("build manager");

    assert_eq!(manager.default_store(), "memory");
    assert_eq!(manager.prefix(), "rustasea-cache-");
    // `memory` is always registered and is the default.
    assert!(manager.store("memory").is_ok());
    let repository = manager.repository().expect("default repository");
    assert_eq!(repository.prefix(), "rustasea-cache-");
}

#[test]
fn manager_registers_redis_only_when_url_and_feature() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write("cache.toml", CACHE_TOML);
    let config = CacheConfig::from_loader(&loader_with(&dir)).expect("parse config");
    let manager = CacheManager::from_config(&config).expect("build manager");

    if cfg!(feature = "redis") {
        assert!(
            manager.store("redis").is_ok(),
            "redis store should register"
        );
    } else {
        assert!(
            manager.store("redis").is_err(),
            "redis store must stay unregistered without the feature"
        );
    }
}

#[test]
fn prefix_is_applied_to_keys() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write("cache.toml", CACHE_TOML);
    let config = CacheConfig::from_loader(&loader_with(&dir)).expect("parse config");
    let manager = CacheManager::from_config(&config).expect("build manager");
    let repository = manager.repository().expect("default repository");

    // Custom prefix is applied via the fallible builder.
    let prefixed = repository
        .try_with_prefix("tenant-cache-")
        .expect("valid prefix");
    assert_eq!(prefixed.prefix(), "tenant-cache-");
}

#[test]
fn unknown_driver_is_rejected() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write(
        "cache.toml",
        "[cache]\ndefault = \"memory\"\nprefix = \"rustasea-cache-\"\n\n\
         [cache.stores.memory]\ndriver = \"nope\"\n",
    );
    let error = CacheConfig::from_loader(&loader_with(&dir)).expect_err("must reject");
    assert_eq!(error.code(), "CacheConfigError::UnknownDriver");
}

#[test]
fn unsupported_driver_is_rejected() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write(
        "cache.toml",
        "[cache]\ndefault = \"memory\"\nprefix = \"rustasea-cache-\"\n\n\
         [cache.stores.database]\ndriver = \"database\"\n",
    );
    let error = CacheConfig::from_loader(&loader_with(&dir)).expect_err("must reject");
    assert_eq!(error.code(), "CacheConfigError::UnsupportedDriver");
    assert!(error.to_string().contains("database"));
}

#[test]
fn bad_prefix_is_rejected() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write(
        "cache.toml",
        "[cache]\ndefault = \"memory\"\nprefix = \"nope\"\n\n\
         [cache.stores.memory]\ndriver = \"memory\"\n",
    );
    let error = CacheConfig::from_loader(&loader_with(&dir)).expect_err("must reject");
    assert_eq!(error.code(), "CacheConfigError::InvalidPrefix");
}

#[test]
fn unknown_default_store_is_rejected() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write(
        "cache.toml",
        "[cache]\ndefault = \"ghost\"\nprefix = \"rustasea-cache-\"\n\n\
         [cache.stores.memory]\ndriver = \"memory\"\n",
    );
    let error = CacheConfig::from_loader(&loader_with(&dir)).expect_err("must reject");
    assert_eq!(error.code(), "CacheConfigError::UnknownDefaultStore");
}

#[test]
fn try_with_prefix_rejects_missing_marker() {
    let store: crate::repository::StoreRef = std::sync::Arc::new(crate::memory::MemoryStore::new());
    let error = match crate::repository::Repository::new(store).try_with_prefix("no-marker") {
        Ok(_) => panic!("must reject"),
        Err(error) => error,
    };
    assert!(matches!(error, crate::error::CacheError::Config(_)));
}

#[test]
fn serializable_classes_false_is_empty() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempConfigDir::new();
    dir.write(
        "cache.toml",
        "[cache]\ndefault = \"memory\"\nprefix = \"rustasea-cache-\"\n\
         serializable_classes = false\n\n\
         [cache.stores.memory]\ndriver = \"memory\"\n",
    );
    let config = CacheConfig::from_loader(&loader_with(&dir)).expect("parse config");
    assert!(config.serializable_classes.is_empty());
}

#[test]
fn cache_prefix_env_overrides_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("CACHE_PREFIX", "tenant-cache-");

    let dir = TempConfigDir::new();
    dir.write("cache.toml", CACHE_TOML);
    let config = CacheConfig::from_loader(&loader_with(&dir)).expect("parse config");

    std::env::remove_var("CACHE_PREFIX");
    assert_eq!(config.prefix(), "tenant-cache-");
}

#[test]
fn cache_prefix_blank_env_is_ignored() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("CACHE_PREFIX", "   ");

    let dir = TempConfigDir::new();
    dir.write("cache.toml", CACHE_TOML);
    let config = CacheConfig::from_loader(&loader_with(&dir)).expect("parse config");

    std::env::remove_var("CACHE_PREFIX");
    assert_eq!(config.prefix(), "rustasea-cache-");
}
