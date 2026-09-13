//! MongoDB client wrapper and typed collection access.
//!
//! [`MongoClient`] owns the driver [`mongodb::Client`] and the resolved
//! database handle. [`MongoClient::collection`] returns a typed
//! [`Collection<T>`] whose CRUD helpers map driver failures onto
//! [`crate::MongoError`].

use bson::{Bson, Document as BsonDocument};
use mongodb::options::ClientOptions;
use mongodb::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::MongoConfig;
use crate::document::Document;
use crate::error::{map_driver_error, MongoError, Result};
use crate::filter::Filter;
use crate::update::Update;

/// Connected MongoDB client bound to a single database.
#[derive(Debug, Clone)]
pub struct MongoClient {
    /// Underlying driver client (cheap to clone — shares the pool).
    client: Client,
    /// Database name resolved from the configuration.
    database: String,
}

impl MongoClient {
    /// Connect using a validated [`MongoConfig`].
    ///
    /// Parses the URI into [`ClientOptions`], applies pool tuning, and opens
    /// the shared connection pool. Does not perform a round-trip; call
    /// [`MongoClient::ping`] to verify reachability.
    pub async fn connect(config: &MongoConfig) -> Result<Self> {
        config.validate()?;
        let mut options = ClientOptions::parse(&config.uri)
            .await
            .map_err(map_driver_error)?;
        if let Some(pool) = &config.pool {
            pool.apply(&mut options);
        }
        let client = Client::with_options(options).map_err(map_driver_error)?;
        Ok(Self {
            client,
            database: config.database.clone(),
        })
    }

    /// Connect from `MONGODB_URI` (+ optional `MONGODB_DATABASE`).
    pub async fn from_env() -> Result<Self> {
        let config = MongoConfig::from_env()?;
        Self::connect(&config).await
    }

    /// Borrow the underlying driver client.
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Name of the database this client is bound to.
    pub fn database_name(&self) -> &str {
        &self.database
    }

    /// Borrow the driver [`mongodb::Database`] handle.
    pub fn database(&self) -> mongodb::Database {
        self.client.database(&self.database)
    }

    /// Run the `ping` command to verify the cluster is reachable.
    pub async fn ping(&self) -> Result<()> {
        let command = bson::doc! { "ping": 1 };
        self.database()
            .run_command(command)
            .await
            .map_err(map_driver_error)?;
        Ok(())
    }

    /// Get a typed collection handle for `T`.
    pub fn collection<T: Document>(&self) -> Collection<T> {
        Collection {
            inner: self
                .client
                .database(&self.database)
                .collection(T::COLLECTION),
            _marker: std::marker::PhantomData,
        }
    }

    /// Get an untyped collection handle by name.
    pub fn raw_collection(&self, name: &str) -> Collection<BsonDocument> {
        Collection {
            inner: self.client.database(&self.database).collection(name),
            _marker: std::marker::PhantomData,
        }
    }
}

/// Typed handle over a MongoDB collection of `T`.
///
/// Construct via [`MongoClient::collection`]. Every method maps driver errors
/// through [`crate::MongoError`]; BSON encode/decode failures surface as
/// [`MongoError::Serialization`].
pub struct Collection<T: Send + Sync> {
    /// Underlying driver collection.
    inner: mongodb::Collection<T>,
    /// Pins the document type.
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T: Send + Sync> Clone for Collection<T> {
    /// Clone the handle (shares the driver's underlying connection pool).
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T: Document> Collection<T> {
    /// Insert a single document.
    ///
    /// Returns the `_id` the server assigned (or echoed).
    pub async fn insert_one(&self, doc: &T) -> Result<Bson> {
        let result = self.inner.insert_one(doc).await.map_err(map_driver_error)?;
        Ok(result.inserted_id)
    }

    /// Insert many documents.
    ///
    /// Returns the number of inserted documents.
    pub async fn insert_many(&self, docs: &[T]) -> Result<usize> {
        let result = self
            .inner
            .insert_many(docs)
            .await
            .map_err(map_driver_error)?;
        Ok(result.inserted_ids.len())
    }

    /// Find documents matching `filter`.
    pub async fn find(&self, filter: &Filter) -> Result<Vec<T>> {
        let mut cursor = self
            .inner
            .find(filter.as_document().clone())
            .await
            .map_err(map_driver_error)?;
        let mut docs = Vec::new();
        while cursor.advance().await.map_err(map_driver_error)? {
            docs.push(cursor.deserialize_current().map_err(map_driver_error)?);
        }
        Ok(docs)
    }

    /// Find the first document matching `filter`.
    pub async fn find_one(&self, filter: &Filter) -> Result<Option<T>> {
        self.inner
            .find_one(filter.as_document().clone())
            .await
            .map_err(map_driver_error)
    }

    /// Find a document by its string `_id`.
    pub async fn find_by_id(&self, id: &str) -> Result<Option<T>> {
        let filter = Filter::new().eq("_id", id);
        self.find_one(&filter).await
    }

    /// Apply `update` to the first document matching `filter`.
    ///
    /// Returns `(matched, modified)` counts.
    pub async fn update_one(&self, filter: &Filter, update: &Update) -> Result<(u64, u64)> {
        let result = self
            .inner
            .update_one(filter.as_document().clone(), update.as_document().clone())
            .await
            .map_err(map_driver_error)?;
        Ok((result.matched_count, result.modified_count))
    }

    /// Replace the first document matching `filter`.
    ///
    /// Returns `(matched, modified)` counts.
    pub async fn replace_one(&self, filter: &Filter, replacement: &T) -> Result<(u64, u64)> {
        let result = self
            .inner
            .replace_one(filter.as_document().clone(), replacement)
            .await
            .map_err(map_driver_error)?;
        Ok((result.matched_count, result.modified_count))
    }

    /// Delete the first document matching `filter`.
    ///
    /// Returns the number of deleted documents.
    pub async fn delete_one(&self, filter: &Filter) -> Result<u64> {
        let result = self
            .inner
            .delete_one(filter.as_document().clone())
            .await
            .map_err(map_driver_error)?;
        Ok(result.deleted_count)
    }

    /// Count documents matching `filter`.
    pub async fn count(&self, filter: &Filter) -> Result<u64> {
        self.inner
            .count_documents(filter.as_document().clone())
            .await
            .map_err(map_driver_error)
    }

    /// Count every document in the collection.
    pub async fn count_all(&self) -> Result<u64> {
        self.count(&Filter::new()).await
    }

    /// Serialize `value` to a BSON document.
    ///
    /// Exposed so callers can reuse the crate's typed error mapping.
    pub fn to_document<V: Serialize>(value: &V) -> Result<BsonDocument> {
        bson::serialize_to_document(value).map_err(|e| MongoError::Serialization(e.to_string()))
    }

    /// Deserialize a BSON document into `V`.
    pub fn from_document<V: DeserializeOwned>(doc: BsonDocument) -> Result<V> {
        bson::deserialize_from_document(doc).map_err(|e| MongoError::Serialization(e.to_string()))
    }
}
