//! MongoDB support for the named-connections resolver.
//!
//! A `[database.connections.<name>]` entry with `driver = "mongodb"` is
//! parseable and resolvable alongside the SQL connections. URI assembly is
//! always available (a full `url`/`uri` or granular `host`/`port`/`database`);
//! connecting to the cluster — and therefore the [`DatabaseConnection::Mongo`]
//! variant — is gated behind the `mongodb` feature so an SQL-only build never
//! links the driver.
//!
//! Without the feature, resolving a MongoDB connection returns a typed
//! [`ConnectionError::UnsupportedDriver`] whose message names the feature; it
//! never panics.

use super::{scheme_of, ConnectionConfig, ConnectionPair, ConnectionResolver};
use crate::db::encode_userinfo;
use crate::error::{ConnectionError, OrmError, Result};

/// Default MongoDB port used when a granular connection omits `port`.
const MONGO_DEFAULT_PORT: u16 = 27017;

/// Driver string surfaced when a `mongodb` connection is resolved without the
/// `mongodb` feature; it names the feature so the remedy is obvious.
#[cfg(not(feature = "mongodb"))]
const MONGO_FEATURE_HINT: &str = "mongodb (enable the `mongodb` feature on rustasea-orm)";

impl ConnectionConfig {
    /// Whether this connection declares the MongoDB driver.
    ///
    /// Accepts `mongodb` (canonical) and `mongo` (alias), case-insensitively.
    pub fn is_mongo(&self) -> bool {
        let driver = self.driver.trim();
        driver.eq_ignore_ascii_case("mongodb") || driver.eq_ignore_ascii_case("mongo")
    }

    /// Build a `mongodb://` URI for a MongoDB connection.
    ///
    /// A full `url` (or its `uri` alias) is returned after its scheme is
    /// validated against the driver; otherwise the URI is assembled from the
    /// granular `host`/`port`/`database` fields (plus optional credentials),
    /// defaulting the port to 27017 and omitting the database path when unset.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::DriverMismatch`] when a declared URL is not a MongoDB
    /// scheme, or [`ConnectionError::MissingField`] (`uri`) when neither a URL
    /// nor a `host` is present.
    pub fn build_mongo_uri(&self) -> Result<String> {
        if let Some(url) = self.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            let scheme = scheme_of(url);
            if scheme.eq_ignore_ascii_case("mongodb") || scheme.eq_ignore_ascii_case("mongodb+srv")
            {
                return Ok(url.to_string());
            }
            return Err(OrmError::Connection(ConnectionError::DriverMismatch {
                driver: "mongodb".to_string(),
                scheme: scheme.to_string(),
            }));
        }

        let host = self
            .host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| {
                OrmError::Connection(ConnectionError::MissingField {
                    field: "uri".to_string(),
                })
            })?;
        let port = self.port.unwrap_or(MONGO_DEFAULT_PORT);

        let mut uri = String::from("mongodb://");
        if let Some(username) = self.username.as_deref().filter(|u| !u.is_empty()) {
            uri.push_str(&encode_userinfo(username));
            if let Some(password) = self.password.as_deref().filter(|p| !p.is_empty()) {
                uri.push(':');
                uri.push_str(&encode_userinfo(password));
            }
            uri.push('@');
        }
        uri.push_str(host);
        uri.push(':');
        uri.push_str(&port.to_string());
        if let Some(database) = self.mongo_database() {
            uri.push('/');
            uri.push_str(database);
        }
        Ok(uri)
    }

    /// The MongoDB database name for this connection, when declared and non-blank.
    pub fn mongo_database(&self) -> Option<&str> {
        self.database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
    }
}

/// A resolved named connection: an SQL read/write pair or a MongoDB client.
///
/// Returned by [`ConnectionResolver::resolve_any`]. The SQL variant always
/// exists; the MongoDB variant is present only with the `mongodb` feature.
#[derive(Debug, Clone)]
pub enum DatabaseConnection {
    /// An SQL connection, carrying its read/write pool pair.
    Sql(ConnectionPair),
    /// A MongoDB client bound to one database (feature `mongodb`).
    #[cfg(feature = "mongodb")]
    Mongo(rustasea_mongo::MongoClient),
}

impl DatabaseConnection {
    /// Whether this is an SQL connection.
    pub fn is_sql(&self) -> bool {
        matches!(self, DatabaseConnection::Sql(_))
    }

    /// Whether this is a MongoDB connection (always `false` without the feature).
    pub fn is_mongo(&self) -> bool {
        #[cfg(feature = "mongodb")]
        {
            matches!(self, DatabaseConnection::Mongo(_))
        }
        #[cfg(not(feature = "mongodb"))]
        {
            false
        }
    }

    /// Borrow the SQL read/write pair, when this is an SQL connection.
    pub fn as_sql(&self) -> Option<&ConnectionPair> {
        match self {
            DatabaseConnection::Sql(pair) => Some(pair),
            #[cfg(feature = "mongodb")]
            DatabaseConnection::Mongo(_) => None,
        }
    }

    /// Consume the connection, yielding the SQL pair when applicable.
    pub fn into_sql(self) -> Option<ConnectionPair> {
        match self {
            DatabaseConnection::Sql(pair) => Some(pair),
            #[cfg(feature = "mongodb")]
            DatabaseConnection::Mongo(_) => None,
        }
    }

    /// Borrow the MongoDB client, when this is a MongoDB connection.
    #[cfg(feature = "mongodb")]
    pub fn as_mongo(&self) -> Option<&rustasea_mongo::MongoClient> {
        match self {
            DatabaseConnection::Mongo(client) => Some(client),
            DatabaseConnection::Sql(_) => None,
        }
    }

    /// Consume the connection, yielding the MongoDB client when applicable.
    #[cfg(feature = "mongodb")]
    pub fn into_mongo(self) -> Option<rustasea_mongo::MongoClient> {
        match self {
            DatabaseConnection::Mongo(client) => Some(client),
            DatabaseConnection::Sql(_) => None,
        }
    }
}

impl ConnectionResolver {
    /// Resolve `name` (or the configured default) to a typed [`DatabaseConnection`].
    ///
    /// SQL connections resolve through [`ConnectionResolver::pair`] and yield
    /// [`DatabaseConnection::Sql`]. A `driver = "mongodb"` connection yields
    /// [`DatabaseConnection::Mongo`] (feature `mongodb`), connecting lazily and
    /// caching the client per name. Without the feature a MongoDB connection
    /// returns a typed [`ConnectionError::UnsupportedDriver`] naming the feature
    /// — never a panic.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::NotConfigured`] when no default exists,
    /// [`ConnectionError::UnknownConnection`] for an undeclared name, a typed
    /// [`ConnectionError`] from URL/config assembly, or a driver-level failure
    /// from connecting the SQL pair or the MongoDB client.
    pub async fn resolve_any(&self, name: Option<&str>) -> Result<DatabaseConnection> {
        let name = self.resolve_name(name)?;
        let connection = self.config.connections.get(&name).ok_or_else(|| {
            OrmError::Connection(ConnectionError::UnknownConnection(name.clone()))
        })?;

        if connection.is_mongo() {
            #[cfg(feature = "mongodb")]
            {
                let mut cache = self.mongo_cache.lock().await;
                if let Some(client) = cache.get(&name) {
                    return Ok(DatabaseConnection::Mongo(client.clone()));
                }
                let config = rustasea_mongo::MongoConfig::try_from(connection)?;
                let client = rustasea_mongo::MongoClient::connect(&config)
                    .await
                    .map_err(map_mongo_error)?;
                cache.insert(name, client.clone());
                return Ok(DatabaseConnection::Mongo(client));
            }
            #[cfg(not(feature = "mongodb"))]
            {
                return Err(OrmError::Connection(ConnectionError::UnsupportedDriver {
                    driver: MONGO_FEATURE_HINT.to_string(),
                }));
            }
        }

        Ok(DatabaseConnection::Sql(self.pair(Some(&name)).await?))
    }
}

/// Build a [`rustasea_mongo::MongoConfig`] from a named-connection entry.
///
/// The interop seam for [`ConnectionConfig`]: the URI is assembled by
/// [`ConnectionConfig::build_mongo_uri`] and the database taken from the
/// connection. **Environment precedence** is applied by
/// [`rustasea_mongo::MongoConfig::from_parts`] — `MONGODB_URI` overrides the
/// connection URI and `MONGODB_DATABASE` overrides its database. A
/// per-connection `[….pool]` (`min`/`max`/`idle_timeout`) is forwarded as
/// [`rustasea_mongo::MongoPoolConfig`].
#[cfg(feature = "mongodb")]
impl TryFrom<&ConnectionConfig> for rustasea_mongo::MongoConfig {
    type Error = OrmError;

    fn try_from(connection: &ConnectionConfig) -> Result<Self> {
        let uri = connection.build_mongo_uri()?;
        let mut config = rustasea_mongo::MongoConfig::from_parts(
            Some(uri.as_str()),
            connection.mongo_database(),
        )
        .map_err(map_mongo_error)?;
        if let Some(pool) = &connection.pool {
            config = config.with_pool(rustasea_mongo::MongoPoolConfig {
                min: pool.min,
                max: pool.max,
                idle_timeout: pool.idle_timeout,
                app_name: None,
            });
        }
        Ok(config)
    }
}

/// Map a [`rustasea_mongo::MongoError`] onto the ORM error taxonomy.
///
/// Configuration problems become a typed
/// [`ConnectionError::InvalidConfig`]; connection failures become
/// [`OrmError::Pool`]; serialization/operation failures become
/// [`OrmError::Storage`].
#[cfg(feature = "mongodb")]
fn map_mongo_error(error: rustasea_mongo::MongoError) -> OrmError {
    use rustasea_mongo::MongoError;
    match error {
        MongoError::Configuration(message) => {
            OrmError::Connection(ConnectionError::InvalidConfig(message))
        }
        MongoError::Connection(message) => OrmError::Pool(message),
        MongoError::Serialization(message) => OrmError::Storage(message),
        MongoError::Operation(message) => OrmError::Storage(message),
    }
}

#[cfg(test)]
mod tests;
