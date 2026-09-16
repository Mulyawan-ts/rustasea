//! Docker-backed SFTP integration tests (ADOPT-025).
//!
//! These exercise the real `SftpDisk` against an `atmoz/sftp` container. They
//! are `#[ignore]` because they require a Docker daemon; run them explicitly:
//!
//! ```text
//! cargo test -p rustasea-storage --features sftp --test sftp_container -- --ignored --nocapture
//! ```

#![cfg(feature = "sftp")]

use std::time::Duration;

use rustasea_storage::{SftpDisk, SftpDiskConfig, Storage, StorageError};
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// Remote root the test user is confined to inside the container.
const REMOTE_ROOT: &str = "/home/testuser/upload";
/// Startup budget: generous so a cold image pull still fits.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);

/// Start an `atmoz/sftp` container and return it with the mapped SSH port.
async fn start_sftp() -> (ContainerAsync<GenericImage>, u16) {
    let container = GenericImage::new("atmoz/sftp", "alpine")
        .with_exposed_port(ContainerPort::Tcp(22))
        .with_wait_for(WaitFor::message_on_stderr("Server listening on"))
        .with_env_var("SFTP_USERS", "testuser:testpass:1001:100:upload")
        .with_startup_timeout(STARTUP_TIMEOUT)
        .start()
        .await
        .expect("atmoz/sftp container must start");
    let port = container
        .get_host_port_ipv4(22u16)
        .await
        .expect("SSH port must be mapped");
    (container, port)
}

/// A config pointing at the container's mapped SSH port.
fn config_for(port: u16, password: &str) -> SftpDiskConfig {
    SftpDiskConfig {
        host: "127.0.0.1".to_string(),
        port: Some(port),
        username: "testuser".to_string(),
        password: Some(password.to_string()),
        private_key_path: None,
        root: Some(REMOTE_ROOT.to_string()),
        timeout: Some(30),
        host_key: None,
        settings: Default::default(),
    }
}

#[tokio::test]
#[ignore = "requires docker"]
async fn put_get_exists_delete_and_list_round_trip() {
    let (_container, port) = start_sftp().await;
    let disk = SftpDisk::connect(config_for(port, "testpass"))
        .await
        .expect("connection must succeed");

    disk.put("a/b/hello.txt", b"hello sftp").await.unwrap();
    assert!(disk.exists("a/b/hello.txt").await.unwrap());
    assert_eq!(disk.get("a/b/hello.txt").await.unwrap(), b"hello sftp");

    // `list` returns keys relative to the root (directories suffixed "/").
    let keys = disk.list("").await.unwrap();
    assert!(keys.contains(&"a/".to_string()), "listing: {keys:?}");
    let nested = disk.list("a/b").await.unwrap();
    assert!(
        nested.contains(&"a/b/hello.txt".to_string()),
        "listing: {nested:?}"
    );

    disk.delete("a/b/hello.txt").await.unwrap();
    assert!(!disk.exists("a/b/hello.txt").await.unwrap());
    // Deleting an absent file is a no-op.
    disk.delete("a/b/hello.txt").await.unwrap();
}

#[tokio::test]
#[ignore = "requires docker"]
async fn wrong_credentials_surface_connection_failed() {
    let (_container, port) = start_sftp().await;
    let err = SftpDisk::connect(config_for(port, "wrong-password"))
        .await
        .expect_err("bad credentials must fail");
    assert!(
        matches!(err, StorageError::ConnectionFailed(_)),
        "got {err:?}"
    );
}

#[tokio::test]
#[ignore = "requires docker"]
async fn path_traversal_is_rejected() {
    let (_container, port) = start_sftp().await;
    let disk = SftpDisk::connect(config_for(port, "testpass"))
        .await
        .expect("connection must succeed");

    let err = disk.get("../../etc/passwd").await.unwrap_err();
    assert!(matches!(err, StorageError::PathTraversal(_)), "got {err:?}");
    assert!(matches!(
        disk.put("/absolute.txt", b"x").await.unwrap_err(),
        StorageError::PathTraversal(_)
    ));
}
