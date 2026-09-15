//! Broadcasting authorization surface — `POST /broadcasting/auth` (ADOPT-022).
//!
//! Parity target: Laravel's `Broadcasting::auth` endpoint. A Pusher client (or
//! any private/presence channel client) posts `socket_id` + `channel_name`; the
//! handler resolves the session identity, runs the configured channel gate, and
//! returns the signed `{ "auth": "...", "channel_data": ... }` payload the
//! client uses to subscribe.
//!
//! Gated twice:
//!
//! * **Auth** — the route carries the `auth` middleware, so an unauthenticated
//!   request is redirected to `/login` (never reaching the handler).
//! * **Config** — a private/presence request with no configured Pusher driver
//!   answers `404` (nothing to authorize against); public channels always
//!   authorize.
//!
//! The `POST` route is covered by the global CSRF allow-list (see
//! [`crate::routes::helpers::csrf_protected`]). The module is only compiled with
//! the `broadcasting` feature.

use axum::body::Bytes;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rustasea::auth::AuthUser;
use rustasea::broadcast::{BroadcastError, Subscriber};
use rustasea::router::Router as RouteTable;

use crate::routes::helpers::{field, json_error, parse_form};
use crate::routes::AUTH;

/// Register the broadcasting routes onto `table`.
pub fn register(table: &mut RouteTable) {
    table.group(|group| {
        group
            .middleware(AUTH)
            .post_action("/broadcasting/auth", authorize);
    });
}

/// The parsed `socket_id` + `channel_name` from a form or JSON body.
struct AuthRequest {
    socket_id: String,
    channel_name: String,
}

/// Parse the request body, accepting both form-urlencoded and JSON.
///
/// A missing or blank field yields `None`, so the handler answers `400` rather
/// than signing an empty request.
fn parse_request(body: &Bytes) -> Option<AuthRequest> {
    // JSON first (a Pusher client sends `application/json`).
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        if let (Some(socket_id), Some(channel_name)) = (
            value.get("socket_id").and_then(|v| v.as_str()),
            value.get("channel_name").and_then(|v| v.as_str()),
        ) {
            if !socket_id.trim().is_empty() && !channel_name.trim().is_empty() {
                return Some(AuthRequest {
                    socket_id: socket_id.to_string(),
                    channel_name: channel_name.to_string(),
                });
            }
        }
    }
    // Fall back to form-urlencoded.
    let fields = parse_form(body);
    let socket_id = field(&fields, "socket_id")?;
    let channel_name = field(&fields, "channel_name")?;
    if socket_id.trim().is_empty() || channel_name.trim().is_empty() {
        return None;
    }
    Some(AuthRequest {
        socket_id: socket_id.to_string(),
        channel_name: channel_name.to_string(),
    })
}

/// Whether `channel_name` is a private/presence channel (needs signing).
fn requires_signing(channel_name: &str) -> bool {
    channel_name.starts_with("private-") || channel_name.starts_with("presence-")
}

/// POST /broadcasting/auth — authorize and sign a channel subscription.
async fn authorize(user: Option<Extension<AuthUser>>, body: Bytes) -> Response {
    let Some(Extension(user)) = user else {
        // The `auth` middleware normally redirects; this is a defensive fallback.
        return json_error(
            StatusCode::UNAUTHORIZED,
            "Unauthenticated",
            "authentication is required to authorize a broadcast channel",
        );
    };
    let Some(request) = parse_request(&body) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "InvalidBroadcastAuthRequest",
            "socket_id and channel_name are required",
        );
    };

    let Some(manager) = rustasea::broadcast::broadcast_manager() else {
        return json_error(
            StatusCode::NOT_FOUND,
            "BroadcastingNotConfigured",
            "no broadcast configuration is installed",
        );
    };

    let subscriber = Subscriber {
        id: user.id.clone(),
        name: user.email.clone(),
    };

    // Public channels authorize without signing.
    if !requires_signing(&request.channel_name) {
        return axum::Json(serde_json::json!({ "auth": "" })).into_response();
    }

    let channel_data = presence_channel_data(&request.channel_name, &subscriber);
    match manager
        .pusher_channel_auth(
            &request.socket_id,
            &request.channel_name,
            &subscriber,
            channel_data.as_deref(),
        )
        .await
    {
        Ok(auth) => axum::Json(auth).into_response(),
        Err(error) => error_response(error, &request.channel_name),
    }
}

/// Build the presence `channel_data` JSON string for a presence channel.
///
/// Returns `None` for private channels; for `presence-` channels it carries the
/// member id and (when known) the display name.
fn presence_channel_data(channel_name: &str, subscriber: &Subscriber) -> Option<String> {
    if !channel_name.starts_with("presence-") {
        return None;
    }
    let user_info = match &subscriber.name {
        Some(name) => serde_json::json!({ "name": name }),
        None => serde_json::json!({}),
    };
    Some(
        serde_json::json!({
            "user_id": subscriber.id,
            "user_info": user_info,
        })
        .to_string(),
    )
}

/// Map a broadcast error to an HTTP response.
fn error_response(error: BroadcastError, channel: &str) -> Response {
    match error {
        BroadcastError::Unauthorized { .. } | BroadcastError::Forbidden { .. } => json_error(
            StatusCode::FORBIDDEN,
            "BroadcastForbidden",
            &format!("not authorized for channel {channel}"),
        ),
        BroadcastError::Unauthenticated { .. } => json_error(
            StatusCode::UNAUTHORIZED,
            "BroadcastUnauthenticated",
            &format!("authentication required for channel {channel}"),
        ),
        BroadcastError::NotConfigured { connection } => json_error(
            StatusCode::NOT_FOUND,
            "BroadcastingNotConfigured",
            &format!("broadcast connection {connection} is not configured"),
        ),
        other => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "BroadcastAuthFailed",
            &other.to_string(),
        ),
    }
}
