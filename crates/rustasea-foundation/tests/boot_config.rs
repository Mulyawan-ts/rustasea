//! Integration tests for boot-time `ConfigLoader` mounting.
//!
//! [`Application::boot`] mounts a layered `ConfigLoader` under
//! [`CONFIG_LOADER_KEY`] before providers register, so service providers and app
//! services resolve the same configuration source. These tests cover a mounted
//! loader resolving file values, a missing directory degrading to an
//! environment-only loader, and a caller's explicit binding being preserved.

use std::sync::Arc;

use rustasea_config::ConfigLoader;
use rustasea_foundation::{Application, CONFIG_LOADER_KEY};

/// Key used by the temp config files; unique so process env cannot collide.
const TEST_KEY: &str = "rustasea_foundation_boot_config_name";

/// Booting mounts a loader that resolves keys from the configured directory.
#[test]
fn boot_mounts_config_loader_from_configured_dir() {
    let dir = tempfile::tempdir().expect("create temp dir");
    std::fs::write(
        dir.path().join("app.toml"),
        format!("{TEST_KEY} = \"Mounted\"\n"),
    )
    .expect("write config");

    let mut app = Application::new();
    app.set_config_dir(dir.path());
    app.boot().expect("boot succeeds");

    assert!(app.container.bound(CONFIG_LOADER_KEY));
    let name: String = app
        .config()
        .expect("config loader mounted")
        .get_key(TEST_KEY)
        .expect("key present");
    assert_eq!(name, "Mounted");
}

/// A missing config directory still mounts an environment-only loader.
#[test]
fn missing_config_dir_mounts_env_only_loader() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let missing = dir.path().join("does-not-exist");

    let mut app = Application::new();
    app.set_config_dir(missing);
    app.boot().expect("boot tolerates a missing config dir");

    let value: Option<String> = app
        .config()
        .expect("config loader mounted")
        .get_key(TEST_KEY)
        .ok();
    assert!(value.is_none(), "absent key must resolve to None");
}

/// A caller's explicit pre-boot binding under the key is preserved by boot.
#[test]
fn explicit_config_binding_is_preserved() {
    let boot_dir = tempfile::tempdir().expect("create boot dir");
    std::fs::write(
        boot_dir.path().join("app.toml"),
        format!("{TEST_KEY} = \"FromBootDir\"\n"),
    )
    .expect("write boot config");

    let custom_dir = tempfile::tempdir().expect("create custom dir");
    std::fs::write(
        custom_dir.path().join("app.toml"),
        format!("{TEST_KEY} = \"Custom\"\n"),
    )
    .expect("write custom config");

    let custom_path = custom_dir.path().join("app");
    let custom = ConfigLoader::load_from(&[custom_path.to_str().expect("utf-8 path")])
        .expect("load custom loader");

    let mut app = Application::new();
    app.set_config_dir(boot_dir.path());
    app.container.instance(CONFIG_LOADER_KEY, Arc::new(custom));
    app.boot().expect("boot succeeds");

    let name: String = app
        .config()
        .expect("config loader mounted")
        .get_key(TEST_KEY)
        .expect("key present");
    assert_eq!(name, "Custom", "explicit binding must win over boot mount");
}
