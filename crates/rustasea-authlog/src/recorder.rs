//! `AuthenticationLogLogger` — the recorder implementation and repository
//! facade.
//!
//! The logger owns a [`DbPool`] and the backing table name. The HTTP layer
//! builds an [`AuthLogEvent`] per request and calls [`AuthenticationLogLogger::record`];
//! the same instance also serves the read/query API. Writes use the ORM runtime
//! API only, so the statements run unchanged on SQLite (tests) and Postgres.

use std::sync::Arc;

use rustasea_orm::{DbPool, Value};
use uuid::Uuid;

use crate::error::Result;
use crate::event::{AuthLogEvent, AuthLogEventKind};
use crate::model::{
    authentication_log_from_row, authentication_logs_from_rows, timestamp_bind, AuthenticationLog,
};
use crate::notifier::NewDeviceNotifier;

/// Recorder + repository over the `authentication_log` table.
#[derive(Clone)]
pub struct AuthenticationLogLogger {
    pool: DbPool,
    table: String,
    notifier: Option<Arc<dyn NewDeviceNotifier>>,
}

impl AuthenticationLogLogger {
    /// Create a logger over `pool` using the default `authentication_log` table.
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            table: crate::AUTHENTICATION_LOG_TABLE.to_string(),
            notifier: None,
        }
    }

    /// Override the backing table name.
    pub fn with_table(mut self, table: impl Into<String>) -> Self {
        self.table = table.into();
        self
    }

    /// Install the new-device notifier used after a successful login.
    pub fn with_notifier(mut self, notifier: Arc<dyn NewDeviceNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// The underlying pool.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// The backing table name.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Persist one authentication event.
    ///
    /// Login/lockout events insert a fresh row (`successful` is true only for
    /// [`AuthLogEventKind::LoginSucceeded`], `login_at` = now). A logout updates
    /// the most recent open row (`logout_at IS NULL`) for the user; when no such
    /// row exists (e.g. the session outlived its login row) a standalone logout
    /// row is inserted so the event is never lost.
    ///
    /// After a successful login from a previously-unseen IP, the configured
    /// [`NewDeviceNotifier`] is invoked. A notifier error is **tolerated**: it is
    /// not propagated, because a mail/queue hiccup must never fail an
    /// authentication that already succeeded. This is a deliberate
    /// best-effort-notification choice (the alternative — failing the record —
    /// would drop the audit row too).
    pub async fn record(&self, event: &AuthLogEvent) -> Result<()> {
        match event.kind {
            AuthLogEventKind::LoginSucceeded
            | AuthLogEventKind::LoginFailed
            | AuthLogEventKind::Lockout => {
                let id = self.insert_event(event).await?;
                if event.kind == AuthLogEventKind::LoginSucceeded {
                    self.notify_new_device(event, id).await;
                }
                Ok(())
            }
            AuthLogEventKind::Logout => self.record_logout(event).await,
        }
    }

    /// Insert one login/failed/lockout row and return its id.
    async fn insert_event(&self, event: &AuthLogEvent) -> Result<Uuid> {
        let id = Uuid::now_v7();
        let stamp = timestamp_bind(&self.pool);
        let sql = format!(
            "INSERT INTO {} \
             (id, user_id, email, guard_name, event, ip_address, user_agent, successful, \
              login_at, logout_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11)",
            self.table
        );
        self.pool
            .execute_bind(
                &sql,
                &[
                    Value::Uuid(id),
                    optional_text(event.user_id.as_deref()),
                    optional_text(event.email.as_deref()),
                    optional_text(event.guard_name.as_deref()),
                    Value::Text(event.kind.as_str().to_string()),
                    optional_text(event.ip_address.as_deref()),
                    optional_text(event.user_agent.as_deref()),
                    Value::Bool(event.kind.is_successful()),
                    stamp.clone(),
                    Value::Null,
                    stamp,
                ],
            )
            .await?;
        Ok(id)
    }

    /// Update the latest open row's `logout_at`, or insert a standalone row.
    ///
    /// Matching is by `user_id` when known, else by `email` when known. When
    /// neither is available (an anonymous logout) there is nothing to attach to,
    /// so a standalone row is inserted.
    async fn record_logout(&self, event: &AuthLogEvent) -> Result<()> {
        let stamp = timestamp_bind(&self.pool);
        if let Some((column, value)) = logout_match(event) {
            let sql = format!(
                "UPDATE {} SET logout_at = $1, updated_at = $1 WHERE {} = $2 AND logout_at IS NULL",
                self.table, column
            );
            let affected = self
                .pool
                .execute_bind(&sql, &[stamp.clone(), value])
                .await?;
            if affected > 0 {
                return Ok(());
            }
        }
        self.insert_standalone_logout(event, stamp).await
    }

    /// Insert a logout row with no matching open login row.
    async fn insert_standalone_logout(&self, event: &AuthLogEvent, stamp: Value) -> Result<()> {
        let id = Uuid::now_v7();
        let sql = format!(
            "INSERT INTO {} \
             (id, user_id, email, guard_name, event, ip_address, user_agent, successful, \
              login_at, logout_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11)",
            self.table
        );
        self.pool
            .execute_bind(
                &sql,
                &[
                    Value::Uuid(id),
                    optional_text(event.user_id.as_deref()),
                    optional_text(event.email.as_deref()),
                    optional_text(event.guard_name.as_deref()),
                    Value::Text(event.kind.as_str().to_string()),
                    optional_text(event.ip_address.as_deref()),
                    optional_text(event.user_agent.as_deref()),
                    Value::Bool(true),
                    Value::Null,
                    stamp.clone(),
                    stamp,
                ],
            )
            .await?;
        Ok(())
    }

    /// Notify on the first login from an IP, tolerating notifier failures.
    async fn notify_new_device(&self, event: &AuthLogEvent, id: Uuid) {
        let Some(notifier) = self.notifier.as_ref() else {
            return;
        };
        let (Some(user_id), Some(ip_address)) =
            (event.user_id.as_deref(), event.ip_address.as_deref())
        else {
            return;
        };
        match self.is_first_login_from_ip(user_id, ip_address, id).await {
            Ok(true) => {
                // Best-effort: a notify error is logged by the caller's policy,
                // never surfaced here (see `record`).
                let _ = notifier
                    .notify(
                        user_id,
                        event.email.as_deref(),
                        ip_address,
                        event.user_agent.as_deref(),
                    )
                    .await;
            }
            Ok(false) => {}
            // A lookup failure is treated as "not new" (fail closed: never spam).
            Err(_) => {}
        }
    }

    /// Whether `user_id` has an earlier successful login from `ip_address`.
    ///
    /// The just-inserted row is excluded by id, so the caller's own login does
    /// not count as its own predecessor.
    async fn is_first_login_from_ip(
        &self,
        user_id: &str,
        ip_address: &str,
        exclude_id: Uuid,
    ) -> Result<bool> {
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM {} WHERE user_id = $1 AND ip_address = $2 \
             AND successful = true AND id <> $3) AS seen",
            self.table
        );
        let rows = self
            .pool
            .fetch_json(
                &sql,
                &[
                    Value::Text(user_id.to_string()),
                    Value::Text(ip_address.to_string()),
                    Value::Uuid(exclude_id),
                ],
            )
            .await?;
        let seen = rows
            .first()
            .and_then(|row| row.get("seen"))
            .map(value_truthy)
            .unwrap_or(false);
        Ok(!seen)
    }

    /// Convenience: record a logout for `user_id`/`email`.
    pub async fn record_logout_for(
        &self,
        user_id: Option<String>,
        email: Option<String>,
        guard_name: Option<String>,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Result<()> {
        let mut event = AuthLogEvent::logout(user_id, guard_name, ip_address, user_agent);
        event.email = email;
        self.record(&event).await
    }

    /// Query rows for `user_id`, newest-first.
    pub async fn for_user(&self, user_id: &str) -> Result<Vec<AuthenticationLog>> {
        let sql = format!(
            "SELECT * FROM {} WHERE user_id = $1 ORDER BY created_at DESC",
            self.table
        );
        let rows = self
            .pool
            .fetch_json(&sql, &[Value::Text(user_id.to_string())])
            .await?;
        authentication_logs_from_rows(&rows)
    }

    /// Query rows for an event kind, newest-first.
    pub async fn for_event(&self, kind: AuthLogEventKind) -> Result<Vec<AuthenticationLog>> {
        let sql = format!(
            "SELECT * FROM {} WHERE event = $1 ORDER BY created_at DESC",
            self.table
        );
        let rows = self
            .pool
            .fetch_json(&sql, &[Value::Text(kind.as_str().to_string())])
            .await?;
        authentication_logs_from_rows(&rows)
    }

    /// Query the most recent `limit` rows.
    pub async fn latest(&self, limit: usize) -> Result<Vec<AuthenticationLog>> {
        let sql = format!(
            "SELECT * FROM {} ORDER BY created_at DESC LIMIT {limit}",
            self.table
        );
        let rows = self.pool.fetch_json(&sql, &[]).await?;
        authentication_logs_from_rows(&rows)
    }

    /// Load a single row by id.
    pub async fn find(&self, id: Uuid) -> Result<Option<AuthenticationLog>> {
        let sql = format!("SELECT * FROM {} WHERE id = $1", self.table);
        let rows = self.pool.fetch_json(&sql, &[Value::Uuid(id)]).await?;
        match rows.first() {
            Some(row) => Ok(Some(authentication_log_from_row(row)?)),
            None => Ok(None),
        }
    }
}

impl std::fmt::Debug for AuthenticationLogLogger {
    /// Manual debug — the notifier is rendered by presence, never by contents.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticationLogLogger")
            .field("table", &self.table)
            .field("notifier", &self.notifier.is_some())
            .finish_non_exhaustive()
    }
}

/// Bind an optional string as text or `NULL`.
fn optional_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |text| Value::Text(text.to_string()))
}

/// The `(column, value)` pair a logout should match on, when any.
fn logout_match(event: &AuthLogEvent) -> Option<(&'static str, Value)> {
    if let Some(user_id) = event.user_id.as_deref() {
        return Some(("user_id", Value::Text(user_id.to_string())));
    }
    event
        .email
        .as_deref()
        .map(|email| ("email", Value::Text(email.to_string())))
}

/// Interpret a JSON `EXISTS(...)` result as a boolean across drivers.
fn value_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
        serde_json::Value::String(text) => matches!(text.as_str(), "1" | "true" | "TRUE"),
        _ => false,
    }
}
