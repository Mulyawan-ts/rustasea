//! Secret scrubbing for captured Sentry events (ADOPT-004).
//!
//! [`scrub_event`] is the `before_send` hook installed by [`super::init`]. It
//! strips credentials from request headers/body/query/cookies and from the
//! free-form `extra`, `contexts` and breadcrumb `data` maps before an event
//! leaves the process. It is deliberately structure-preserving: keys are kept
//! and only their values are replaced with [`REDACTED`], so events stay
//! debuggable without leaking secrets.

/// The placeholder written in place of a scrubbed value.
pub(super) const REDACTED: &str = "[Filtered]";

/// Request header names removed by [`scrub_event`] (matched case-insensitively).
///
/// Headers whose *name* merely contains a credential fragment (e.g.
/// `x-auth-token`, `x-csrf-token`) are also removed by [`is_sensitive_header`];
/// this list holds the names that do not contain one and would otherwise slip
/// through the substring match.
const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "x-api-key",
];

/// Key fragments that mark a value as a credential (matched case-insensitively).
const SENSITIVE_KEY_FRAGMENTS: &[&str] = &[
    "password", "secret", "token", "api_key", "apikey", "app_key",
];

/// `before_send` hook that removes credentials from an event before it is sent.
///
/// Scrubbing is best-effort and never drops the event (always returns
/// `Some(event)`). It:
///
/// * removes sensitive request headers (explicit list + credential-looking
///   names) and sensitive `env` keys;
/// * redacts credentials in the request body — a JSON body is walked
///   recursively, anything else falls back to form-encoded `key=value`;
/// * redacts sensitive query-string parameters;
/// * drops the request cookie jar wholesale;
/// * redacts sensitive keys in `extra`, `contexts` (`Context::Other` maps) and
///   breadcrumb `data` maps.
pub fn scrub_event(
    mut event: sentry::protocol::Event<'static>,
) -> Option<sentry::protocol::Event<'static>> {
    if let Some(request) = event.request.as_mut() {
        request.headers.retain(|name, _| !is_sensitive_header(name));
        request.env.retain(|name, _| !is_sensitive_key(name));
        if let Some(body) = request.data.as_deref() {
            request.data = Some(scrub_body(body));
        }
        if let Some(query) = request.query_string.as_deref() {
            request.query_string = Some(scrub_form_body(query));
        }
        // The cookie jar carries session/CSRF credentials with no useful
        // diagnostic value, so it is dropped rather than redacted key by key.
        request.cookies = None;
    }

    for (key, value) in event.extra.iter_mut() {
        if is_sensitive_key(key) {
            *value = sentry::protocol::Value::from(REDACTED);
        }
    }

    // Context names are not credentials, but the free-form `Other` payload can
    // carry arbitrary key/value pairs (e.g. `request` context data).
    event.contexts.retain(|name, _| !is_sensitive_key(name));
    for context in event.contexts.values_mut() {
        if let sentry::protocol::Context::Other(map) = context {
            for (key, value) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *value = sentry::protocol::Value::from(REDACTED);
                }
            }
        }
    }

    for breadcrumb in event.breadcrumbs.iter_mut() {
        for (key, value) in breadcrumb.data.iter_mut() {
            if is_sensitive_key(key) {
                *value = sentry::protocol::Value::from(REDACTED);
            }
        }
    }

    Some(event)
}

/// True when `name` is a sensitive request header.
///
/// Matches the explicit [`SENSITIVE_HEADERS`] list, and — because credentials
/// often appear in compound names such as `x-auth-token` — any header whose
/// name looks like a credential via [`is_sensitive_key`].
fn is_sensitive_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SENSITIVE_HEADERS.contains(&name.as_str()) || is_sensitive_key(&name)
}

/// True when `key` looks like a credential (contains a sensitive fragment).
fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SENSITIVE_KEY_FRAGMENTS
        .iter()
        .any(|fragment| key.contains(fragment))
}

/// Redact credentials in a request body.
///
/// A body that parses as JSON is walked recursively (see [`scrub_json`]); any
/// other body is treated as form-encoded and scrubbed by [`scrub_form_body`].
/// This keeps the form path as the fallback so plain `k=v&…` bodies (which are
/// not valid JSON) still work.
fn scrub_body(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(mut value) => {
            scrub_json(&mut value);
            serde_json::to_string(&value).unwrap_or_else(|_| scrub_form_body(body))
        }
        Err(_) => scrub_form_body(body),
    }
}

/// Recursively redact sensitive keys inside a JSON value, preserving structure.
///
/// Object values whose key is sensitive are replaced with [`REDACTED`]; arrays
/// and nested objects are visited in place.
fn scrub_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *entry = serde_json::Value::from(REDACTED);
                } else {
                    scrub_json(entry);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                scrub_json(item);
            }
        }
        _ => {}
    }
}

/// Redact sensitive `key=value` pairs in a form-encoded body or query string.
///
/// The input is split on `&`; each pair is split on the first `=` and, when the
/// key is sensitive, its value is replaced with [`REDACTED`]. A pair without a
/// separator is returned unchanged.
fn scrub_form_body(body: &str) -> String {
    body.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_sensitive_key(key) => format!("{key}={REDACTED}"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an event carrying a single `extra` entry.
    fn event_with_extra(key: &str, value: &str) -> sentry::protocol::Event<'static> {
        let mut event = sentry::protocol::Event::default();
        event.extra.insert(
            key.to_string(),
            sentry::protocol::Value::from(value.to_string()),
        );
        event
    }

    /// Scrub an event carrying `request` and return the surviving request.
    fn scrubbed_request(request: sentry::protocol::Request) -> sentry::protocol::Request {
        let event = sentry::protocol::Event {
            request: Some(request),
            ..Default::default()
        };
        scrub_event(event)
            .expect("event survives scrubbing")
            .request
            .expect("request kept")
    }

    #[test]
    fn scrub_removes_authorization_header() {
        let mut request = sentry::protocol::Request::default();
        request.headers.insert(
            "Authorization".to_string(),
            "Bearer super-secret".to_string(),
        );
        request
            .headers
            .insert("X-Request-Id".to_string(), "abc".to_string());

        let headers = scrubbed_request(request).headers;
        assert!(
            !headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("authorization")),
            "authorization header must be removed: {headers:?}"
        );
        assert_eq!(headers.get("X-Request-Id").map(String::as_str), Some("abc"));
    }

    #[test]
    fn scrub_drops_compound_auth_headers() {
        let mut request = sentry::protocol::Request::default();
        request
            .headers
            .insert("X-Auth-Token".to_string(), "secret".to_string());
        request
            .headers
            .insert("Proxy-Authorization".to_string(), "Basic xyz".to_string());
        request
            .headers
            .insert("X-Request-Id".to_string(), "abc".to_string());
        let headers = scrubbed_request(request).headers;
        assert!(!headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("x-auth-token")));
        assert!(!headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("proxy-authorization")));
        assert_eq!(headers.get("X-Request-Id").map(String::as_str), Some("abc"));
    }

    #[test]
    fn scrub_filters_password_keyed_extra() {
        let event = event_with_extra("user_password", "hunter2");
        let scrubbed = scrub_event(event).expect("event survives scrubbing");
        let value = scrubbed.extra.get("user_password").expect("key kept");
        assert_eq!(value, &sentry::protocol::Value::from(REDACTED));
    }

    #[test]
    fn scrub_keeps_non_sensitive_extra() {
        let event = event_with_extra("route", "/dashboard");
        let scrubbed = scrub_event(event).expect("event survives scrubbing");
        let value = scrubbed.extra.get("route").expect("key kept");
        assert_eq!(value, &sentry::protocol::Value::from("/dashboard"));
    }

    #[test]
    fn scrub_redacts_form_body_secrets() {
        let request = sentry::protocol::Request {
            data: Some("username=alice&password=hunter2&token=abc".to_string()),
            ..Default::default()
        };
        let body = scrubbed_request(request).data.expect("body kept");
        assert_eq!(body, "username=alice&password=[Filtered]&token=[Filtered]");
    }

    #[test]
    fn scrub_redacts_nested_json_body_secrets() {
        let request = sentry::protocol::Request {
            data: Some(
                r#"{"user":{"name":"alice","password":"hunter2"},"token":"abc","tags":["a","b"]}"#
                    .to_string(),
            ),
            ..Default::default()
        };
        let body = scrubbed_request(request).data.expect("body kept");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid json kept");
        assert_eq!(value["user"]["name"], serde_json::json!("alice"));
        assert_eq!(value["user"]["password"], serde_json::json!(REDACTED));
        assert_eq!(value["token"], serde_json::json!(REDACTED));
        assert_eq!(value["tags"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn scrub_redacts_query_string_and_drops_cookies() {
        let request = sentry::protocol::Request {
            query_string: Some("page=2&token=abc&filter=recent".to_string()),
            cookies: Some("session=secret; csrf=xyz".to_string()),
            ..Default::default()
        };
        let request = scrubbed_request(request);
        assert_eq!(
            request.query_string.as_deref(),
            Some("page=2&token=[Filtered]&filter=recent")
        );
        assert!(request.cookies.is_none(), "cookie jar must be dropped");
    }

    #[test]
    fn scrub_redacts_breadcrumb_data_secrets() {
        let mut breadcrumb = sentry::protocol::Breadcrumb::default();
        breadcrumb
            .data
            .insert("password".to_string(), "hunter2".into());
        breadcrumb.data.insert("route".to_string(), "/login".into());
        let event = sentry::protocol::Event {
            breadcrumbs: vec![breadcrumb].into(),
            ..Default::default()
        };

        let scrubbed = scrub_event(event).expect("event survives scrubbing");
        let data = &scrubbed.breadcrumbs.values[0].data;
        assert_eq!(data.get("password"), Some(&REDACTED.into()));
        assert_eq!(data.get("route"), Some(&"/login".into()));
    }
}
