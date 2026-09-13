//! End-to-end read/write connection splitting against file-backed SQLite.
//!
//! Verifies the Laravel-style routing contract from the DB-002 task:
//!
//! * no split configured → read and write resolve to the *same* pool instance;
//! * a split (two SQLite files) → reads observe the read file and mutations
//!   land in the write file, proven by seeding each file independently and
//!   asserting which rows each side sees;
//! * invalid read URL and an unreachable read endpoint surface typed errors.

use rustasea_orm::connections::{
    ConnectionConfig, ConnectionPair, ConnectionResolver, DatabaseConfig, EndpointConfig,
};
use rustasea_orm::{ConnectionError, DbPool, OrmError, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

/// A temporary directory holding the primary and replica SQLite files.
struct SplitFixture {
    dir: PathBuf,
    primary: PathBuf,
    replica: PathBuf,
}

impl SplitFixture {
    /// Create a fresh temp directory with unique primary/replica file names.
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rustasea-orm-rw-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let primary = dir.join("primary.sqlite");
        let replica = dir.join("replica.sqlite");
        Self {
            dir,
            primary,
            replica,
        }
    }

    /// A `ConnectionConfig` whose write endpoint is `primary` and read is `replica`.
    fn config(&self) -> ConnectionConfig {
        ConnectionConfig {
            driver: "sqlite".into(),
            database: Some(self.primary.display().to_string()),
            read: Some(EndpointConfig {
                database: Some(self.replica.display().to_string()),
                ..EndpointConfig::default()
            }),
            ..ConnectionConfig::default()
        }
    }
}

impl Drop for SplitFixture {
    /// Remove the temp directory and any WAL/SHM sidecars.
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            for path in [&self.primary, &self.replica] {
                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Create a `markers` table on a pool and insert a single named row.
async fn seed_marker(pool: &DbPool, marker: &str) {
    pool.execute_bind(
        "CREATE TABLE IF NOT EXISTS markers (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
        &[],
    )
    .await
    .expect("create markers table");
    pool.execute_bind(
        "INSERT INTO markers (name) VALUES ($1)",
        &[Value::Text(marker.to_string())],
    )
    .await
    .expect("insert marker");
}

/// Every marker name visible through `pool`, ordered by insertion id.
async fn markers(pool: &DbPool) -> Vec<String> {
    pool.fetch_json("SELECT name FROM markers ORDER BY id", &[])
        .await
        .expect("select markers")
        .into_iter()
        .filter_map(|row| row.get("name").and_then(|v| v.as_str().map(str::to_string)))
        .collect()
}

/// No split: the resolver hands back one pool instance for both roles.
#[tokio::test]
async fn no_split_reuses_single_pool_instance() {
    let resolver = ConnectionResolver::new(DatabaseConfig {
        default: Some("mem".into()),
        connections: BTreeMap::from([(
            "mem".to_string(),
            ConnectionConfig {
                driver: "sqlite".into(),
                url: Some("sqlite::memory:".into()),
                ..ConnectionConfig::default()
            },
        )]),
        ..DatabaseConfig::default()
    });

    let read = resolver.read(None).await.expect("read pool");
    let write = resolver.write(None).await.expect("write pool");
    assert_eq!(
        read.identity(),
        write.identity(),
        "unsplit connection must reuse one pool instance"
    );

    let pair = resolver.pair(None).await.expect("pair");
    assert!(!pair.is_split());
}

/// Split: reads see the replica file, mutations land in the primary file.
#[tokio::test]
async fn split_routes_reads_to_replica_and_writes_to_primary() {
    let fixture = SplitFixture::new();
    let resolver = ConnectionResolver::new(DatabaseConfig {
        default: Some("split".into()),
        connections: BTreeMap::from([("split".to_string(), fixture.config())]),
        ..DatabaseConfig::default()
    });

    let pair: ConnectionPair = resolver.pair(None).await.expect("split pair");
    assert!(pair.is_split(), "two endpoints must yield two pools");

    // Seed each physical file independently: the primary holds `write-only`,
    // the replica holds `read-only`.
    seed_marker(pair.write(), "write-only").await;
    seed_marker(pair.read(), "read-only").await;

    // A read routed through the pair observes only the replica's row.
    let read_rows = pair
        .fetch_json("SELECT name FROM markers ORDER BY id", &[])
        .await
        .expect("pair read");
    let read_names: Vec<&str> = read_rows
        .iter()
        .filter_map(|row| row.get("name").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(
        read_names,
        vec!["read-only"],
        "pair reads must hit the replica"
    );

    // A mutation routed through the pair lands only in the primary.
    pair.execute_bind(
        "INSERT INTO markers (name) VALUES ($1)",
        &[Value::Text("write-two".into())],
    )
    .await
    .expect("pair write");

    let primary_rows = markers(pair.write()).await;
    assert_eq!(
        primary_rows,
        vec!["write-only", "write-two"],
        "writes must land in the primary"
    );
    let replica_rows = markers(pair.read()).await;
    assert_eq!(
        replica_rows,
        vec!["read-only"],
        "writes must not leak into the replica"
    );

    pair.close().await;
}

/// An invalid read URL surfaces a typed `ConnectionError`.
#[tokio::test]
async fn invalid_read_url_is_typed_error() {
    let resolver = ConnectionResolver::new(DatabaseConfig {
        default: Some("bad".into()),
        connections: BTreeMap::from([(
            "bad".to_string(),
            ConnectionConfig {
                driver: "sqlite".into(),
                database: Some(":memory:".into()),
                read: Some(EndpointConfig {
                    // Scheme disagrees with the declared `sqlite` driver.
                    url: Some("mysql://replica/app".into()),
                    ..EndpointConfig::default()
                }),
                ..ConnectionConfig::default()
            },
        )]),
        ..DatabaseConfig::default()
    });

    let error = resolver.read(None).await.unwrap_err();
    assert!(
        matches!(
            error,
            OrmError::Connection(ConnectionError::DriverMismatch { ref driver, ref scheme })
                if driver == "sqlite" && scheme == "mysql"
        ),
        "got {error:?}"
    );
}

/// A read endpoint that cannot be opened surfaces a typed pool error.
#[tokio::test]
async fn unreachable_read_endpoint_is_typed_error() {
    let dir = std::env::temp_dir().join(format!("rustasea-orm-rw-missing-{}", Uuid::now_v7()));
    let bad_path = dir.join("nested/does-not-exist/replica.sqlite");

    let resolver = ConnectionResolver::new(DatabaseConfig {
        default: Some("broken".into()),
        connections: BTreeMap::from([(
            "broken".to_string(),
            ConnectionConfig {
                driver: "sqlite".into(),
                database: Some(":memory:".into()),
                read: Some(EndpointConfig {
                    database: Some(bad_path.display().to_string()),
                    ..EndpointConfig::default()
                }),
                ..ConnectionConfig::default()
            },
        )]),
        ..DatabaseConfig::default()
    });

    let error = resolver.read(None).await.unwrap_err();
    assert!(matches!(error, OrmError::Pool(_)), "got {error:?}");

    let _ = std::fs::remove_dir_all(&dir);
}
