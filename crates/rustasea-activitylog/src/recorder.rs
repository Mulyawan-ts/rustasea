//! `ActivityLogger` — the recorder implementation and repository facade.
//!
//! `ActivityLogger` owns a [`DbPool`] and implements the ORM's
//! [`rustasea_orm::activity::ActivityRecorder`] trait, so a single instance can
//! be installed process-wide with
//! [`rustasea_orm::activity::register_activity_recorder`] and also serve the
//! read/query API. Writes use the ORM runtime API only.

use rustasea_orm::activity::{ActivityEvent, ActivityLogError, ActivityRecorder};
use rustasea_orm::{DbPool, Value};
use uuid::Uuid;

use crate::error::{ActivityError, Result};
use crate::model::{timestamp_bind, Activity, ActivityQuery};

/// Recorder + repository over the `audit_log` table.
#[derive(Debug, Clone)]
pub struct ActivityLogger {
    pool: DbPool,
    table: String,
}

impl ActivityLogger {
    /// Create a logger over `pool` using the default `audit_log` table.
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            table: crate::AUDIT_LOG_TABLE.to_string(),
        }
    }

    /// Create a logger over `pool` with a custom table name.
    pub fn with_table(pool: DbPool, table: impl Into<String>) -> Self {
        Self {
            pool,
            table: table.into(),
        }
    }

    /// The underlying pool.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// The backing table name.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Persist one activity row and return it as read back.
    ///
    /// The `log_name` defaults to the subject table; `description` is the
    /// operation name. `properties` is the Laravel-parity
    /// `{ "old": …, "new": …, "changed": […] }` payload.
    pub async fn record_event(&self, event: &ActivityEvent) -> Result<Activity> {
        let id = Uuid::new_v4();
        let properties = build_properties(event);
        let stamp = timestamp_bind(&self.pool);
        let batch = match event.batch_uuid.as_deref() {
            Some(text) => Some(Uuid::parse_str(text).map_err(|_| {
                ActivityError::InvalidArgument(format!(
                    "batch_uuid must be a valid UUID, got `{text}`"
                ))
            })?),
            None => None,
        };
        let subject_id = event
            .model_id
            .as_deref()
            .and_then(|text| Uuid::parse_str(text).ok());

        let sql = format!(
            "INSERT INTO {} \
             (id, batch_uuid, log_name, description, subject_type, subject_id, \
              causer_type, causer_id, properties, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
            self.table
        );
        self.pool
            .execute_bind(
                &sql,
                &[
                    Value::Uuid(id),
                    batch.map_or(Value::Null, Value::Uuid),
                    Value::Text(event.table.clone()),
                    Value::Text(event.operation.as_str().to_string()),
                    Value::Text(event.model_type.clone()),
                    subject_id.map_or(Value::Null, Value::Uuid),
                    Value::Null,
                    event.causer_id.clone().map_or(Value::Null, Value::Text),
                    Value::Json(properties),
                    stamp.clone(),
                ],
            )
            .await?;

        self.find(id)
            .await?
            .ok_or_else(|| ActivityError::Decode("recorded row vanished".to_string()))
    }

    /// Load a single activity row by id.
    pub async fn find(&self, id: Uuid) -> Result<Option<Activity>> {
        let sql = format!("SELECT * FROM {} WHERE id = $1", self.table);
        let rows = self.pool.fetch_json(&sql, &[Value::Uuid(id)]).await?;
        match rows.first() {
            Some(row) => Ok(Some(crate::model::activity_from_row(row)?)),
            None => Ok(None),
        }
    }

    /// Query rows for a subject, newest-first.
    pub async fn for_subject(&self, subject_type: &str, subject_id: Uuid) -> Result<Vec<Activity>> {
        self.query()
            .for_subject(subject_type, subject_id)
            .get()
            .await
    }

    /// Query rows caused by `causer_id`, newest-first.
    pub async fn caused_by(&self, causer_id: &str) -> Result<Vec<Activity>> {
        self.query().caused_by(causer_id).get().await
    }

    /// Query rows in a logical log channel, newest-first.
    pub async fn in_log(&self, log_name: &str) -> Result<Vec<Activity>> {
        self.query().in_log(log_name).get().await
    }

    /// Query the most recent `limit` rows.
    pub async fn latest(&self, limit: u64) -> Result<Vec<Activity>> {
        self.query().limit(limit).get().await
    }

    /// Query rows in a batch, newest-first.
    pub async fn for_batch(&self, batch_uuid: Uuid) -> Result<Vec<Activity>> {
        self.query().for_batch(batch_uuid).get().await
    }

    /// Start a query over the logger's table.
    pub fn query(&self) -> ActivityQuery<'_> {
        ActivityQuery::new(&self.pool).with_table(self.table.clone())
    }
}

/// Build the Laravel-parity properties payload for an event.
///
/// Shape: `{ "old": <old|omit>, "new": <new|omit>, "changed": [columns] }`.
/// `old`/`new` are omitted when absent so the payload never stores a spurious
/// `null` diff side.
pub(crate) fn build_properties(event: &ActivityEvent) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if let Some(old) = &event.old {
        map.insert("old".to_string(), old.clone());
    }
    if let Some(new) = &event.new {
        map.insert("new".to_string(), new.clone());
    }
    map.insert(
        "changed".to_string(),
        serde_json::Value::Array(
            event
                .changed
                .iter()
                .map(|column| serde_json::Value::String(column.clone()))
                .collect(),
        ),
    );
    serde_json::Value::Object(map)
}

#[async_trait::async_trait]
impl ActivityRecorder for ActivityLogger {
    /// Persist the event, mapping failures to the erased ORM error channel.
    async fn record(&self, event: ActivityEvent) -> std::result::Result<(), ActivityLogError> {
        self.record_event(&event)
            .await
            .map(|_| ())
            .map_err(|error| Box::new(error) as ActivityLogError)
    }
}
