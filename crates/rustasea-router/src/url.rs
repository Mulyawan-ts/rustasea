//! Reverse URL resolution — name → path with parameter substitution.
//!
//! A [`NamedRoutes`] index maps a registered route name onto the path it was
//! registered with. [`NamedRoutes::resolve`] substitutes the router's own
//! placeholder syntax — both the Laravel-style `{id}` / `{user:slug}` and the
//! Axum-style `:id` — so callers can build links from names instead of
//! hard-coding paths.

use std::collections::HashMap;

use crate::route::RouteEntry;
use crate::router::Router;

/// Reverse index from route name to the path it was registered under.
///
/// Built from a route table via [`NamedRoutes::from_routes`]; the first entry
/// wins when several rows share a name (e.g. an `any()` route expands to six
/// methods, or a resource `update` spans `PUT` + `PATCH`).
#[derive(Debug, Clone, Default)]
pub struct NamedRoutes {
    paths: HashMap<String, String>,
    domains: HashMap<String, Option<String>>,
}

impl NamedRoutes {
    /// Build an index from a slice of route entries.
    pub fn from_routes(routes: &[RouteEntry]) -> Self {
        let mut index = Self::default();
        for route in routes {
            let Some(name) = &route.name else {
                continue;
            };
            index
                .paths
                .entry(name.clone())
                .or_insert_with(|| route.path.clone());
            index
                .domains
                .entry(name.clone())
                .or_insert_with(|| route.domain.clone());
        }
        index
    }

    /// Whether any named route is registered.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Return the raw registered path for `name`, or `None` when unknown.
    ///
    /// No parameter substitution is performed; use [`NamedRoutes::resolve`] to
    /// fill placeholders.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.paths.get(name).map(String::as_str)
    }

    /// Resolve `name` to a URL, substituting placeholders from `params`.
    ///
    /// Returns `None` when the name is unknown, or when the path contains a
    /// placeholder for which no value was supplied. The returned string is the
    /// registered path (no host) unless the route carries a domain, in which
    /// case the domain is prepended as a protocol-relative host
    /// (`//{domain}{path}`).
    pub fn resolve(&self, name: &str, params: &[(&str, &str)]) -> Option<String> {
        let path = self.paths.get(name)?;
        let substituted = substitute(path, params)?;
        match self.domains.get(name).and_then(Option::as_deref) {
            Some(domain) => Some(format!("//{domain}{substituted}")),
            None => Some(substituted),
        }
    }
}

/// Substitute `{param}` / `{param:field}` and `:param` placeholders.
///
/// Returns `None` if any placeholder has no matching entry in `params`. Text
/// outside placeholders — including an unterminated `{` — passes through
/// verbatim.
fn substitute(path: &str, params: &[(&str, &str)]) -> Option<String> {
    let lookup = |key: &str| params.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let mut out = String::with_capacity(path.len());
    let chars: Vec<char> = path.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                let Some(close) = chars[i + 1..].iter().position(|c| *c == '}') else {
                    out.push('{');
                    i += 1;
                    continue;
                };
                let inner: String = chars[i + 1..i + 1 + close].iter().collect();
                let param = inner.split(':').next().unwrap_or(&inner);
                out.push_str(lookup(param)?);
                i += close + 2;
            }
            ':' if i + 1 < chars.len() && is_ident_start(chars[i + 1]) => {
                let start = i + 1;
                let mut end = start;
                while end < chars.len() && is_ident_char(chars[end]) {
                    end += 1;
                }
                let param: String = chars[start..end].iter().collect();
                out.push_str(lookup(&param)?);
                i = end;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    Some(out)
}

/// Whether `c` may start a placeholder identifier.
fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// Whether `c` may continue a placeholder identifier.
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

impl Router {
    /// Build a reverse index over the registered named routes.
    ///
    /// Includes plain, resource, action-bound, and redirect routes that were
    /// given a name (via [`Router::named`] or [`Router::resource`]); the group
    /// name prefix is already folded into each entry's name.
    pub fn named_routes(&self) -> NamedRoutes {
        NamedRoutes::from_routes(&self.routes)
    }

    /// Resolve a named route to a URL, substituting path parameters.
    ///
    /// Returns `None` when `name` is unknown or when a placeholder in the
    /// registered path has no matching entry in `params`. The result is the
    /// registered path (no host) unless the route carries a domain, in which
    /// case it is prefixed as a protocol-relative host (`//{domain}{path}`).
    ///
    /// ```rust
    /// # use rustasea_router::Router;
    /// let mut router = Router::new();
    /// router.get("/users/{id}").named("users.show");
    /// assert_eq!(
    ///     router.url("users.show", &[("id", "42")]).as_deref(),
    ///     Some("/users/42")
    /// );
    /// assert_eq!(router.url("missing", &[]), None);
    /// ```
    pub fn url(&self, name: &str, params: &[(&str, &str)]) -> Option<String> {
        self.named_routes().resolve(name, params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a single named entry for resolver tests.
    fn entry(name: &str, path: &str) -> RouteEntry {
        RouteEntry {
            method: "GET".to_string(),
            path: path.to_string(),
            name: Some(name.to_string()),
            middleware: Vec::new(),
            domain: None,
            binding_fields: Vec::new(),
            controller: None,
            handler: None,
        }
    }

    /// A brace placeholder is substituted from the supplied params.
    #[test]
    fn brace_placeholder_is_substituted() {
        let index = NamedRoutes::from_routes(&[entry("users.show", "/users/{id}")]);
        assert_eq!(
            index.resolve("users.show", &[("id", "42")]),
            Some("/users/42".to_string())
        );
    }

    /// A typed brace placeholder uses the field name before the colon.
    #[test]
    fn typed_placeholder_is_substituted() {
        let index = NamedRoutes::from_routes(&[entry("users.show", "/users/{user:slug}")]);
        assert_eq!(
            index.resolve("users.show", &[("user", "ada")]),
            Some("/users/ada".to_string())
        );
    }

    /// An Axum-style colon placeholder is substituted too.
    #[test]
    fn colon_placeholder_is_substituted() {
        let index =
            NamedRoutes::from_routes(&[entry("teams.show", "/teams/:team/members/:member")]);
        assert_eq!(
            index.resolve("teams.show", &[("team", "7"), ("member", "ada")]),
            Some("/teams/7/members/ada".to_string())
        );
    }

    /// Unknown names and missing params both resolve to `None`.
    #[test]
    fn unknown_name_and_missing_param_return_none() {
        let index = NamedRoutes::from_routes(&[entry("users.show", "/users/{id}")]);
        assert_eq!(index.resolve("missing", &[]), None);
        assert_eq!(index.resolve("users.show", &[]), None);
    }

    /// A domain-bearing route resolves to a protocol-relative host + path.
    #[test]
    fn domain_is_prepended_when_set() {
        let mut route = entry("api.home", "/home");
        route.domain = Some("api.example.com".to_string());
        let index = NamedRoutes::from_routes(&[route]);
        assert_eq!(
            index.resolve("api.home", &[]),
            Some("//api.example.com/home".to_string())
        );
    }
}
