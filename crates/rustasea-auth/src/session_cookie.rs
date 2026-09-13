/// Hardened session-cookie configuration.
///
/// Encodes the browser defaults the session guard expects: `HttpOnly` (the
/// cookie is never readable from JavaScript, mitigating XSS token theft),
/// `Secure` (never sent over plaintext HTTP), and `SameSite=Lax` (the cookie
/// is withheld on cross-site sub-requests, blocking the classic cross-site
/// form POST). `Path=/` scopes the cookie to the whole application.
///
/// # CSRF
///
/// `SameSite=Lax` is defense-in-depth, **not** a CSRF control on its own:
/// top-level navigations and same-site sub-requests still carry the cookie.
/// Every state-changing request must therefore also carry the origin-aware
/// token checked by [`crate::csrf::PreventRequestForgery`] (`Sec-Fetch-Site`
/// plus the `X-CSRF-TOKEN`/`_token` value). The session guard never treats a
/// valid cookie as proof of user intent.
use tower_sessions::cookie::{Cookie, SameSite};

/// Cookie attributes applied to the session cookie.
///
/// Construct with [`SessionCookieConfig::default`] (hardened) and override
/// only for local development, e.g. `secure = false` on `http://localhost`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCookieConfig {
    /// Cookie name; the `rustasea-session` prefix matches the policy marker.
    pub name: String,
    /// `HttpOnly` flag — hidden from `document.cookie`.
    pub http_only: bool,
    /// `Secure` flag — HTTPS-only transport.
    pub secure: bool,
    /// `SameSite` policy — `Lax` by default.
    pub same_site: SameSite,
    /// Cookie path scope.
    pub path: String,
    /// Optional cookie domain; `None` means host-only (the browser default).
    pub domain: Option<String>,
}

impl Default for SessionCookieConfig {
    /// Hardened defaults: HttpOnly, Secure, SameSite=Lax, Path=/, host-only.
    fn default() -> Self {
        Self {
            name: "rustasea-session".to_string(),
            http_only: true,
            secure: true,
            same_site: SameSite::Lax,
            path: "/".to_string(),
            domain: None,
        }
    }
}

impl SessionCookieConfig {
    /// Build the outgoing session cookie with the configured hardening flags.
    ///
    /// The returned cookie is what the `SessionManagerLayer` (M5 HTTP wiring)
    /// serialises onto the response; it is exposed here so the flags are
    /// testable without an HTTP stack. A configured [`SessionCookieConfig::domain`]
    /// is applied as the cookie `Domain` attribute; `None` leaves it host-only.
    pub fn build_cookie(&self, value: impl Into<String>) -> Cookie<'static> {
        let mut builder = Cookie::build((self.name.clone(), value.into()))
            .http_only(self.http_only)
            .secure(self.secure)
            .same_site(self.same_site)
            .path(self.path.clone());
        if let Some(domain) = self.domain.clone() {
            builder = builder.domain(domain);
        }
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default cookie carries every hardening flag.
    #[test]
    fn default_cookie_is_hardened() {
        let config = SessionCookieConfig::default();
        assert!(config.http_only);
        assert!(config.secure);
        assert_eq!(config.same_site, SameSite::Lax);
        assert_eq!(config.domain, None);

        let cookie = config.build_cookie("sid-value");
        assert_eq!(cookie.name(), "rustasea-session");
        assert_eq!(cookie.value(), "sid-value");
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.domain(), None);
    }

    /// A configured domain is emitted on the cookie; `None` stays host-only.
    #[test]
    fn configured_domain_is_applied() {
        let config = SessionCookieConfig {
            domain: Some("example.com".to_string()),
            ..SessionCookieConfig::default()
        };
        let cookie = config.build_cookie("sid-value");
        assert_eq!(cookie.domain(), Some("example.com"));
    }
}
