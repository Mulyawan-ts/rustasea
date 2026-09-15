//! Application error type and the dev/prod error renderers (ADOPT-010).
//!
//! [`AppError`] is the *request-handling* error surface — it is deliberately
//! distinct from [`crate::HttpError`], which models failures of the outgoing
//! HTTP client. A handler returns `AppError` through `?` / `IntoResponse`; the
//! app-layer middleware (see `rustasea-app`'s `routes::errors`) inspects the
//! [`ErrorDetail`] marker this module attaches to decide between the rich dev
//! page and the prod JSON envelope.
//!
//! # Prod safety
//!
//! For any `5xx` the *client-facing* `detail` is always the generic string
//! "An unexpected error occurred." — never the error message, the source chain,
//! or a backtrace. The real diagnostics travel only in the [`ErrorDetail`]
//! request extension, which is read by the dev renderer and is **never**
//! serialized to the client. `4xx` details are client-facing by design (a
//! validation or not-found message is meant to be shown).
//!
//! # Envelope convention
//!
//! The JSON shape matches the rest of the workspace
//! (`{"errors":[{status,code,title,detail}]}`) with a top-level `request_id`
//! added. The id is `null` at [`IntoResponse`] time (the error does not know
//! it); the middleware fills it in.

use std::error::Error as StdError;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Maximum length (bytes) of any single interpolated field in the dev page.
///
/// Caps a pathological message/source so a hostile or runaway error cannot
/// produce a multi-megabyte page.
const MAX_FIELD_LEN: usize = 8 * 1024;

/// Structured, server-side-only diagnostics attached to an error response.
///
/// Inserted as a response extension by [`AppError::into_response`] and read by
/// the app-layer error middleware. It never serializes to the client: the dev
/// renderer consumes it, the prod path discards everything but `type_name`.
#[derive(Debug, Clone)]
pub struct ErrorDetail {
    /// Short stable identifier for the error kind (e.g. `AppError::Internal`).
    pub type_name: String,
    /// The real, unredacted message (dev only).
    pub message: String,
    /// `Display` of each link in the source chain, outermost first.
    pub source_chain: Vec<String>,
}

impl ErrorDetail {
    /// Build a generic detail for a bare `5xx` with no typed error attached.
    pub fn generic(type_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            message: message.into(),
            source_chain: Vec::new(),
        }
    }
}

/// Request metadata rendered into the dev error page.
///
/// `headers` is a **whitelist** only (content-type, accept, user-agent,
/// x-request-id) — never `authorization` or `cookie`, so the page cannot leak a
/// bearer token or a session id.
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    /// HTTP method.
    pub method: String,
    /// Request path (no query string).
    pub path: String,
    /// Resolved request id (incoming header or generated).
    pub request_id: Option<String>,
    /// Application environment name, when known.
    pub environment: Option<String>,
    /// Authenticated principal description (`id` or `id <email>`), when known.
    pub user: Option<String>,
    /// Whitelisted request headers.
    pub headers: Vec<(String, String)>,
}

/// Typed application error returned by request handlers.
///
/// Variants carry only what the client may see; diagnostics live in the
/// [`ErrorDetail`] this type produces. `5xx` variants never expose their
/// message to the client (see the module docs).
#[derive(Debug)]
pub enum AppError {
    /// `422 Unprocessable Entity` — a client-facing validation failure.
    Validation {
        /// Stable machine code for the failure.
        code: String,
        /// Human-readable, client-facing explanation.
        detail: String,
    },
    /// `404 Not Found` — a client-facing missing resource.
    NotFound {
        /// Human-readable, client-facing explanation.
        detail: String,
    },
    /// `500 Internal Server Error` — a server-side failure.
    Internal {
        /// The real message (dev only; never sent to the client on 5xx).
        message: String,
        /// The originating error, when there is one.
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    /// `500 Internal Server Error` produced by a caught panic.
    Panic {
        /// The panic payload rendered as a string (dev only).
        message: String,
    },
    /// Arbitrary status with explicit envelope fields.
    Generic {
        /// HTTP status code.
        status: u16,
        /// Stable machine code.
        code: String,
        /// Human-readable title.
        title: String,
        /// Client-facing detail.
        detail: String,
    },
}

impl AppError {
    /// Build a `500` with a message and no source.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            source: None,
        }
    }

    /// Build a `500` with a message and an originating error.
    pub fn internal_with_source(
        message: impl Into<String>,
        source: Box<dyn StdError + Send + Sync>,
    ) -> Self {
        Self::Internal {
            message: message.into(),
            source: Some(source),
        }
    }

    /// Build a `404` with a client-facing detail.
    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::NotFound {
            detail: detail.into(),
        }
    }

    /// Build a `422` with a machine code and a client-facing detail.
    pub fn validation(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Validation {
            code: code.into(),
            detail: detail.into(),
        }
    }

    /// Build a `500` from a caught panic payload.
    pub fn panic(message: impl Into<String>) -> Self {
        Self::Panic {
            message: message.into(),
        }
    }

    /// The HTTP status this error maps to.
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::Validation { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::NotFound { .. } => StatusCode::NOT_FOUND,
            AppError::Internal { .. } | AppError::Panic { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Generic { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }

    /// The stable machine code for the envelope.
    pub fn code(&self) -> String {
        match self {
            AppError::Validation { code, .. } => code.clone(),
            AppError::NotFound { .. } => "NotFound".to_string(),
            AppError::Internal { .. } => "AppError::Internal".to_string(),
            AppError::Panic { .. } => "AppError::Panic".to_string(),
            AppError::Generic { code, .. } => code.clone(),
        }
    }

    /// The client-facing `detail` string (prod-safe for `5xx`).
    pub fn client_detail(&self) -> String {
        match self {
            AppError::Validation { detail, .. } => detail.clone(),
            AppError::NotFound { detail, .. } => detail.clone(),
            AppError::Internal { .. } | AppError::Panic { .. } => {
                "An unexpected error occurred.".to_string()
            }
            AppError::Generic { detail, .. } => detail.clone(),
        }
    }

    /// The server-side diagnostics for this error (never sent to the client).
    pub fn debug_detail(&self) -> ErrorDetail {
        match self {
            AppError::Validation { code, detail } => ErrorDetail {
                type_name: code.clone(),
                message: detail.clone(),
                source_chain: Vec::new(),
            },
            AppError::NotFound { detail } => ErrorDetail {
                type_name: "NotFound".to_string(),
                message: detail.clone(),
                source_chain: Vec::new(),
            },
            AppError::Internal { message, source } => ErrorDetail {
                type_name: "AppError::Internal".to_string(),
                message: message.clone(),
                source_chain: source_chain(source.as_deref()),
            },
            AppError::Panic { message } => ErrorDetail {
                type_name: "AppError::Panic".to_string(),
                message: message.clone(),
                source_chain: Vec::new(),
            },
            AppError::Generic { code, detail, .. } => ErrorDetail {
                type_name: code.clone(),
                message: detail.clone(),
                source_chain: Vec::new(),
            },
        }
    }
}

impl From<Box<dyn StdError + Send + Sync>> for AppError {
    fn from(source: Box<dyn StdError + Send + Sync>) -> Self {
        let message = source.to_string();
        AppError::Internal {
            message,
            source: Some(source),
        }
    }
}

impl IntoResponse for AppError {
    /// Render the prod-safe envelope and attach the [`ErrorDetail`] marker.
    ///
    /// The body is always the generic envelope; for a `5xx` the `detail` is the
    /// generic string. The marker extension carries the real diagnostics for the
    /// middleware's dev path and is not part of the serialized body.
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();
        let title = status_title(status.as_u16());
        let detail = self.client_detail();
        let debug = self.debug_detail();

        let body = render_error_json(status.as_u16(), &code, title, &detail, None);
        let mut response = (status, Json(body)).into_response();
        response.extensions_mut().insert(debug);
        response
    }
}

/// Walk `source` and collect the `Display` of each link, outermost first.
fn source_chain(source: Option<&(dyn StdError + Send + Sync + 'static)>) -> Vec<String> {
    let mut chain = Vec::new();
    // Upcast the boxed error to the plain `Error` trait object so the `source()`
    // walk can follow the chain (trait upcasting is stable since Rust 1.86).
    let mut current: Option<&(dyn StdError + 'static)> =
        source.map(|error| error as &(dyn StdError + 'static));
    while let Some(error) = current {
        chain.push(error.to_string());
        current = error.source();
    }
    chain
}

/// Build the `{"errors":[...], "request_id": ...}` envelope.
///
/// Shared by [`AppError::into_response`] and the app-layer error middleware so
/// both produce an identical shape.
pub fn render_error_json(
    status: u16,
    code: &str,
    title: &str,
    detail: &str,
    request_id: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "errors": [{
            "status": status.to_string(),
            "code": code,
            "title": title,
            "detail": detail,
        }],
        "request_id": request_id,
    })
}

/// Canonical HTTP reason phrase for `status`, or `"Error"` when unknown.
pub fn status_title(status: u16) -> &'static str {
    StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("Error")
}

/// Render the full dev error page as a standalone HTML document.
///
/// Inline CSS only (no external assets), so the page renders offline. Every
/// interpolated value is HTML-escaped and length-capped. `snippet`, when
/// present, is shown in a `<pre>` block (typically the panic source location).
pub fn render_dev_page(
    error_debug: &ErrorDetail,
    context: &RequestContext,
    snippet: Option<&str>,
) -> String {
    let message = truncate(&error_debug.message, MAX_FIELD_LEN);
    let mut source_rows = String::new();
    for (index, link) in error_debug.source_chain.iter().enumerate() {
        source_rows.push_str(&format!(
            "<li><span class=\"idx\">{}</span> {}</li>",
            index + 1,
            escape_html(&truncate(link, MAX_FIELD_LEN)),
        ));
    }
    let source_section = if source_rows.is_empty() {
        String::new()
    } else {
        format!("<section><h2>Cause chain</h2><ol class=\"chain\">{source_rows}</ol></section>")
    };

    let snippet_section = match snippet {
        Some(code) => format!(
            "<section><h2>Source</h2><pre class=\"snippet\">{}</pre></section>",
            escape_html(code),
        ),
        None => String::new(),
    };

    let mut header_rows = String::new();
    for (name, value) in &context.headers {
        header_rows.push_str(&format!(
            "<tr><th>{}</th><td>{}</td></tr>",
            escape_html(name),
            escape_html(&truncate(value, MAX_FIELD_LEN)),
        ));
    }
    let headers_table = if header_rows.is_empty() {
        "<tr><td colspan=\"2\">—</td></tr>".to_string()
    } else {
        header_rows
    };

    let request_id = context.request_id.as_deref().unwrap_or("—");
    let environment = context.environment.as_deref().unwrap_or("—");
    let user = context.user.as_deref().unwrap_or("—");

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title}</title><style>{css}</style></head><body><main>\
<header class=\"hero\"><p class=\"badge\">{type_name}</p>\
<h1>{message}</h1></header>\
{source_section}{snippet_section}\
<section><h2>Request</h2><table class=\"context\">\
<tr><th>Method</th><td>{method}</td></tr>\
<tr><th>Path</th><td>{path}</td></tr>\
<tr><th>Request ID</th><td>{request_id}</td></tr>\
<tr><th>Environment</th><td>{environment}</td></tr>\
<tr><th>User</th><td>{user}</td></tr>\
{headers_table}</table></section>\
<footer>RustaSea — rendered in debug mode. Not shown in production.</footer>\
</main></body></html>",
        title = escape_html(&truncate(&error_debug.message, 200)),
        css = DEV_PAGE_CSS,
        type_name = escape_html(&truncate(&error_debug.type_name, 200)),
        message = escape_html(&message),
        method = escape_html(&truncate(&context.method, MAX_FIELD_LEN)),
        path = escape_html(&truncate(&context.path, MAX_FIELD_LEN)),
        request_id = escape_html(request_id),
        environment = escape_html(environment),
        user = escape_html(user),
    )
}

/// Inline stylesheet for the dev error page.
const DEV_PAGE_CSS: &str = "body{margin:0;background:#0f172a;color:#e2e8f0;\
font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}\
main{max-width:960px;margin:0 auto;padding:2rem}\
.hero{border-left:4px solid #ef4444;padding-left:1rem;margin-bottom:2rem}\
.badge{display:inline-block;background:#7f1d1d;color:#fecaca;padding:.15rem .5rem;\
border-radius:.25rem;font-size:.8rem;margin:0}\
h1{font-size:1.5rem;line-height:1.4;word-break:break-word}\
h2{font-size:1rem;color:#94a3b8;text-transform:uppercase;letter-spacing:.05em}\
section{margin:1.5rem 0}ol.chain{list-style:none;padding:0}\
ol.chain li{background:#1e293b;margin:.35rem 0;padding:.5rem .75rem;border-radius:.25rem}\
.idx{color:#64748b;margin-right:.5rem}\
pre.snippet{background:#020617;border:1px solid #1e293b;border-radius:.25rem;\
padding:.75rem;overflow:auto;font-size:.85rem}\
table.context{width:100%;border-collapse:collapse;font-size:.85rem}\
table.context th{text-align:left;color:#94a3b8;padding:.35rem .75rem .35rem 0;\
vertical-align:top;white-space:nowrap}\
table.context td{word-break:break-word}\
footer{color:#64748b;font-size:.8rem;margin-top:2rem}";

/// Escape `&`, `<`, `>`, and `"` for safe HTML interpolation.
fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// Truncate `input` to at most `max` bytes on a char boundary.
fn truncate(input: &str, max: usize) -> String {
    if input.len() <= max {
        return input.to_string();
    }
    let mut end = max;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (truncated)", &input[..end])
}
#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
