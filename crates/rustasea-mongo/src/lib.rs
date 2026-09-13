//! RustaSea Mongo — MongoDB document store for the framework.
//!
//! This crate is a thin, non-ORM layer over the official `mongodb` driver. It
//! provides:
//!
//! - [`MongoConfig`] — connection settings loaded from `MONGODB_URI` /
//!   `MONGODB_DATABASE` or from a `config/mongo.toml` document.
//! - [`MongoClient`] — a connected client bound to one database, with
//!   [`MongoClient::ping`] for reachability checks.
//! - [`Document`] — the contract binding a `serde` type to a collection.
//! - [`Collection`] — typed CRUD helpers (`insert_one`, `insert_many`, `find`,
//!   `find_one`, `update_one`, `delete_one`, `count`, ...).
//! - [`Filter`] and [`Update`] — pragmatic builders that serialize to
//!   [`bson::Document`].
//! - [`MongoError`] — a four-variant typed error over driver failures.
//!
//! ```no_run
//! use rustasea_mongo::{Document, Filter, MongoClient, MongoConfig};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Debug, Serialize, Deserialize)]
//! struct User {
//!     #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
//!     id: Option<String>,
//!     name: String,
//! }
//!
//! impl Document for User {
//!     const COLLECTION: &'static str = "users";
//!     fn id(&self) -> Option<&str> { self.id.as_deref() }
//! }
//!
//! # async fn run() -> rustasea_mongo::Result<()> {
//! let client = MongoClient::connect(&MongoConfig::new(
//!     "mongodb://localhost:27017",
//!     "rustasea",
//! )).await?;
//! client.ping().await?;
//!
//! let users = client.collection::<User>();
//! users.insert_one(&User { id: None, name: "Ada".into() }).await?;
//! let active = users.find(&Filter::new().eq("name", "Ada")).await?;
//! assert_eq!(active.len(), 1);
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod config;
pub mod document;
pub mod error;
pub mod filter;
pub mod update;

pub use client::{Collection, MongoClient};
pub use config::{MongoConfig, MongoPoolConfig, DEFAULT_DATABASE, ENV_DATABASE, ENV_URI};
pub use document::Document;
pub use error::{map_driver_error, MongoError, Result};
pub use filter::Filter;
pub use update::Update;
