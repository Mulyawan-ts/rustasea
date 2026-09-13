/// Session hardening policy and the deserialization allow-list contract.
///
/// Extracted from `session` to keep each module under the 500-line limit.
/// `SessionPolicy` encodes the Laravel-13 hardening defaults (JSON
/// serialization, hyphenated `-session-`/`-cache-` key prefixes, and a
/// `serializable_classes` allow-list checked before any deserialization) and
/// derives the session keys the guard reads and writes.
use crate::error::{AuthError, Result};

/// Hardening policy for session/cache serialization (FR-303/FR-304).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPolicy {
    /// Serialization format; `json` is the only supported default.
    pub serialization: String,
    /// Key prefix — must contain the hyphenated `-session-` marker.
    pub prefix: String,
    /// Fully-qualified type names allowed to deserialize.
    pub serializable_classes: Vec<String>,
}

impl Default for SessionPolicy {
    /// Laravel-13 defaults: JSON, hyphenated prefix, empty allow-list.
    fn default() -> Self {
        Self {
            serialization: "json".to_string(),
            prefix: "rustasea-session-".to_string(),
            serializable_classes: Vec::new(),
        }
    }
}

impl SessionPolicy {
    /// Create a policy with an explicit allow-list.
    pub fn with_classes(classes: Vec<String>) -> Self {
        Self {
            serializable_classes: classes,
            ..Self::default()
        }
    }

    /// Verify the prefix uses hyphens, not underscores (`-session-`).
    pub fn validate_prefix(&self) -> Result<()> {
        if self.serialization != "json" {
            return Err(AuthError::Disabled(format!(
                "unsupported session serialization {:?} (only \"json\" is allowed)",
                self.serialization
            )));
        }
        if !self.prefix.contains("-session-") {
            return Err(AuthError::Disabled(format!(
                "session prefix {:?} must contain -session-",
                self.prefix
            )));
        }
        Ok(())
    }

    /// Verify a cache prefix uses the hyphenated `-cache-` marker.
    ///
    /// Separate from [`SessionPolicy::validate_prefix`] because cache keys
    /// and session keys use different markers (FS-M3-03, TC-M3-07). A prefix
    /// with the underscore variant (`_cache_`) is rejected, matching the
    /// Laravel-13 hyphenation rule (#12).
    pub fn validate_cache_prefix(prefix: &str) -> Result<()> {
        if !prefix.contains("-cache-") {
            return Err(AuthError::Disabled(format!(
                "cache prefix {prefix:?} must contain -cache-"
            )));
        }
        Ok(())
    }

    /// Gate a type name against the allow-list before deserialization.
    pub fn allow(
        &self,
        type_name: &str,
    ) -> std::result::Result<(), crate::error::SerializationError> {
        if self.serializable_classes.iter().any(|c| c == type_name) {
            Ok(())
        } else {
            Err(crate::error::SerializationError::NotAllowed {
                type_name: type_name.to_string(),
            })
        }
    }

    /// Session key under which the guard stores the authenticated user.
    ///
    /// Derived from [`SessionPolicy::prefix`] so the `-session-` marker is
    /// always present; the concrete [`crate::session::SessionUser`] type is
    /// stored directly, so no `serializable_classes` entry is needed (the
    /// allow-list continues to gate polymorphic cache values, never weakened).
    pub fn user_key(&self) -> String {
        format!("{}user", self.prefix)
    }

    /// Session key under which the password-confirmation timestamp is stored.
    ///
    /// Mirrors [`SessionPolicy::user_key`] and Laravel's session key
    /// `auth.password_confirmed_at`; derived from the same `-session-` prefix so
    /// the key participates in the same hardening rule.
    pub fn password_confirmed_key(&self) -> String {
        format!("{}password_confirmed_at", self.prefix)
    }
}

/// Allow-list contract for safe deserialization (FR-304, TC-M3-06).
///
/// Any store that deserializes values from untrusted bytes (sessions,
/// caches) must gate the concrete type name against an explicit allow-list
/// *before* `from_str`, so a poisoned cache entry can never decode into an
/// attacker-chosen gadget type. Session and cache policies share this trait;
/// both surface `SerializationError::NotAllowed { type_name }` on a miss.
pub trait DeserializationAllowList {
    /// Reject deserializing `type_name` unless it is allow-listed.
    fn allow(&self, type_name: &str) -> std::result::Result<(), crate::error::SerializationError>;
}

impl DeserializationAllowList for SessionPolicy {
    /// Delegates to [`SessionPolicy::allow`].
    fn allow(&self, type_name: &str) -> std::result::Result<(), crate::error::SerializationError> {
        SessionPolicy::allow(self, type_name)
    }
}
