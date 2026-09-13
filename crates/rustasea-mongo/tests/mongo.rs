//! Unit tests for the Mongo document store that need no live server.
//!
//! The single live integration test is marked `#[ignore = "requires MONGODB_URI"]`
//! and only runs when a cluster is available.

use bson::{doc, Bson, Document as BsonDocument};
use rustasea_mongo::{
    Document, Filter, MongoConfig, MongoError, MongoPoolConfig, Update, DEFAULT_DATABASE,
};
use serde::{Deserialize, Serialize};

/// Sample document used across the serde and filter tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct User {
    /// MongoDB primary key, omitted when `None` so the server assigns one.
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    /// Display name.
    name: String,
    /// Account status.
    status: String,
    /// Age in years.
    age: i32,
    /// Login count.
    logins: i64,
}

impl Document for User {
    /// Collection backing the `User` document.
    const COLLECTION: &'static str = "users";

    /// Expose the string `_id`.
    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

/// Build a fully-populated fixture user.
fn fixture() -> User {
    User {
        id: Some("u-1".into()),
        name: "Ada".into(),
        status: "active".into(),
        age: 36,
        logins: 4,
    }
}

#[test]
fn document_trait_exposes_collection_and_id() {
    let user = fixture();
    assert_eq!(User::COLLECTION, "users");
    assert_eq!(user.id(), Some("u-1"));

    let anonymous = User { id: None, ..user };
    assert_eq!(anonymous.id(), None);
}

#[test]
fn bson_serde_round_trip() {
    let user = fixture();
    let document = bson::serialize_to_document(&user).expect("serialize user");
    assert_eq!(document.get_str("_id").unwrap(), "u-1");
    assert_eq!(document.get_str("name").unwrap(), "Ada");
    assert_eq!(document.get_i32("age").unwrap(), 36);

    let restored: User = bson::deserialize_from_document(document).expect("deserialize user");
    assert_eq!(restored, user);
}

#[test]
fn bson_serde_omits_none_id() {
    let user = User {
        id: None,
        ..fixture()
    };
    let document = bson::serialize_to_document(&user).expect("serialize user");
    assert!(!document.contains_key("_id"), "None id must be omitted");
}

#[test]
fn filter_equality_serializes() {
    let filter = Filter::new().eq("status", "active");
    assert_eq!(filter.into_document(), doc! { "status": "active" });
}

#[test]
fn filter_comparison_operators_serialize() {
    let filter = Filter::new()
        .eq("status", "active")
        .ne("role", "banned")
        .gt("age", 18)
        .gte("age", 21)
        .lt("age", 65)
        .lte("age", 64);
    let document = filter.into_document();

    // `eq` is stored flat; `ne` and comparison operators nest under `$op`.
    assert_eq!(document.get_str("status").unwrap(), "active");
    let role = document
        .get_document("role")
        .expect("role operator document");
    assert_eq!(role.get_str("$ne").unwrap(), "banned");
    let age = document.get_document("age").expect("age operator document");
    assert_eq!(age.get_i32("$gt").unwrap(), 18);
    assert_eq!(age.get_i32("$gte").unwrap(), 21);
    assert_eq!(age.get_i32("$lt").unwrap(), 65);
    assert_eq!(age.get_i32("$lte").unwrap(), 64);
}

#[test]
fn filter_in_and_nin_serialize_arrays() {
    let filter = Filter::new()
        .is_in("status", vec!["active", "pending"])
        .not_in("role", vec!["admin"]);
    let document = filter.into_document();

    let status = document.get_document("status").expect("status operators");
    assert_eq!(
        status.get_array("$in").unwrap(),
        &vec![
            Bson::String("active".into()),
            Bson::String("pending".into())
        ]
    );
    let role = document.get_document("role").expect("role operators");
    assert_eq!(
        role.get_array("$nin").unwrap(),
        &vec![Bson::String("admin".into())]
    );
}

#[test]
fn filter_exists_serializes() {
    let document = Filter::new().exists("email", true).into_document();
    assert_eq!(document, doc! { "email": { "$exists": true } });
}

#[test]
fn filter_regex_serializes() {
    let document = Filter::new()
        .regex("name", "^a", "i")
        .expect("regex builds")
        .into_document();
    let regex = document.get("name").expect("regex field");
    assert!(
        matches!(regex, Bson::RegularExpression(_)),
        "regex must serialize as BSON regex"
    );
}

#[test]
fn filter_regex_rejects_nul_byte() {
    let error = Filter::new()
        .regex("name", "bad\0pattern", "")
        .expect_err("NUL byte must be rejected");
    assert!(error.is_configuration(), "expected configuration error");
}

#[test]
fn filter_and_or_compose() {
    let document = Filter::new()
        .eq("tenant", "t1")
        .and(Filter::new().gt("age", 18))
        .or(Filter::new().eq("role", "admin"))
        .into_document();

    assert_eq!(document.get_str("tenant").unwrap(), "t1");
    let and = document.get_array("$and").expect("$and array");
    assert_eq!(and.len(), 1);
    let or = document.get_array("$or").expect("$or array");
    assert_eq!(or.len(), 1);
}

#[test]
fn filter_and_folds_into_single_clause() {
    let document = Filter::new()
        .and(Filter::new().eq("a", 1))
        .and(Filter::new().eq("b", 2))
        .into_document();
    let and = document.get_array("$and").expect("$and array");
    assert_eq!(and.len(), 2, "successive and() calls fold into one $and");
}

#[test]
fn empty_filter_matches_all() {
    let filter = Filter::new();
    assert!(filter.is_empty());
    assert_eq!(filter.into_document(), BsonDocument::new());
}

#[test]
fn update_operators_serialize() {
    let document = Update::new()
        .set("name", "Ada")
        .inc("logins", 1_i64)
        .unset("temp")
        .into_document();

    assert_eq!(
        document
            .get_document("$set")
            .unwrap()
            .get_str("name")
            .unwrap(),
        "Ada"
    );
    assert_eq!(
        document
            .get_document("$inc")
            .unwrap()
            .get_i64("logins")
            .unwrap(),
        1
    );
    assert!(document
        .get_document("$unset")
        .unwrap()
        .contains_key("temp"));
}

#[test]
fn update_push_and_pull_serialize() {
    let document = Update::new()
        .push("tags", "rust")
        .pull("tags", "old")
        .into_document();
    let push = document.get_document("$push").expect("$push");
    assert_eq!(push.get_str("tags").unwrap(), "rust");
    let pull = document.get_document("$pull").expect("$pull");
    assert_eq!(pull.get_str("tags").unwrap(), "old");
}

#[test]
fn config_from_toml_parses_uri_and_database() {
    let toml = r#"
        [mongo]
        uri = "mongodb://localhost:27017"
        database = "rustasea"

        [mongo.pool]
        min = 1
        max = 10
        idle_timeout = 600
        app_name = "rustasea-test"
    "#;

    let config = MongoConfig::from_toml(toml).expect("parse config");
    assert_eq!(config.uri, "mongodb://localhost:27017");
    assert_eq!(config.database, "rustasea");
    let pool = config.pool.expect("pool present");
    assert_eq!(pool.min, Some(1));
    assert_eq!(pool.max, Some(10));
    assert_eq!(pool.idle_timeout, Some(600));
    assert_eq!(pool.app_name.as_deref(), Some("rustasea-test"));
}

#[test]
fn config_from_toml_requires_uri() {
    let error = MongoConfig::from_toml("[mongo]\ndatabase = \"rustasea\"\n")
        .expect_err("missing uri must fail");
    assert!(error.is_configuration());
}

#[test]
fn config_rejects_bad_scheme() {
    let error = MongoConfig::new("postgres://localhost/db", "rustasea")
        .validate()
        .expect_err("wrong scheme must fail");
    assert!(error.is_configuration());
    assert!(error.to_string().contains("mongodb://"));
}

#[test]
fn config_rejects_empty_database() {
    let error = MongoConfig::new("mongodb://localhost:27017", "  ")
        .validate()
        .expect_err("empty database must fail");
    assert!(error.is_configuration());
}

#[test]
fn config_accepts_srv_scheme() {
    MongoConfig::new("mongodb+srv://cluster.example.com", "rustasea")
        .validate()
        .expect("srv scheme is valid");
}

#[test]
fn config_builder_attaches_pool() {
    let config =
        MongoConfig::new("mongodb://localhost:27017", "rustasea").with_pool(MongoPoolConfig {
            max: Some(20),
            ..MongoPoolConfig::default()
        });
    assert_eq!(config.pool.unwrap().max, Some(20));
}

#[test]
fn config_default_database_constant() {
    assert_eq!(DEFAULT_DATABASE, "rustasea");
}

#[test]
fn error_classification_helpers() {
    let connection = MongoError::Connection("down".into());
    assert!(connection.is_connection());
    assert!(!connection.is_configuration());

    let config = MongoError::Configuration("bad".into());
    assert!(config.is_configuration());
    assert!(!config.is_connection());
}

#[test]
fn invalid_uri_surfaces_typed_error_without_server() {
    // A malformed URI is rejected by the driver's parser before any network
    // round-trip, so this needs no live server.
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let error = runtime
        .block_on(rustasea_mongo::MongoClient::connect(&MongoConfig::new(
            "mongodb://[invalid",
            "rustasea",
        )))
        .expect_err("malformed uri must fail");
    assert!(
        matches!(
            error,
            MongoError::Configuration(_) | MongoError::Connection(_)
        ),
        "malformed uri must map to a typed error, got {error:?}"
    );
}

/// Live integration test — requires a reachable cluster via `MONGODB_URI`.
///
/// Run with `cargo test -p rustasea-mongo -- --ignored` after exporting
/// `MONGODB_URI` (and optionally `MONGODB_DATABASE`).
#[tokio::test]
#[ignore = "requires MONGODB_URI"]
async fn live_crud_round_trip() {
    let config = MongoConfig::from_env().expect("MONGODB_URI must be set");
    let client = rustasea_mongo::MongoClient::connect(&config)
        .await
        .expect("connect");
    client.ping().await.expect("ping");

    let users = client.collection::<User>();
    let id = format!("it-{}", uuid_like());
    let user = User {
        id: Some(id.clone()),
        name: "Integration".into(),
        status: "active".into(),
        age: 30,
        logins: 0,
    };

    users.insert_one(&user).await.expect("insert");
    let found = users.find_by_id(&id).await.expect("find").expect("present");
    assert_eq!(found.name, "Integration");

    let updated = users
        .update_one(
            &Filter::new().eq("_id", &id),
            &Update::new().inc("logins", 1),
        )
        .await
        .expect("update");
    assert_eq!(updated.0, 1);

    let count = users
        .count(&Filter::new().eq("_id", &id))
        .await
        .expect("count");
    assert_eq!(count, 1);

    let deleted = users
        .delete_one(&Filter::new().eq("_id", &id))
        .await
        .expect("delete");
    assert_eq!(deleted, 1);
}

/// Build a process-unique suffix without pulling in a UUID dependency.
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", std::process::id(), nanos)
}
