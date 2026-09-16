//! Service-account credentials (`google/auth` parity).
//!
//! [`ServiceAccount`] is the JSON key Google Cloud issues for a service
//! account. It is parsed from the raw JSON (or a file), validated for the
//! fields the assertion flow actually needs (`client_email`, `private_key`,
//! `token_uri`), and never rendered with its private key in `Debug` output.

use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::error::{GoogleAuthError, Result};

/// Google account type a service-account key must declare.
const SERVICE_ACCOUNT_TYPE: &str = "service_account";

/// A parsed Google service-account key.
///
/// Fields are private: construction goes through [`ServiceAccount::from_json`],
/// [`ServiceAccount::from_file`], or [`ServiceAccount::new`], each of which
/// validates the required fields. `Debug` redacts `private_key`.
#[derive(Clone, Deserialize)]
pub struct ServiceAccount {
    /// Account kind (`"service_account"` for a service-account key).
    #[serde(rename = "type", default)]
    account_type: Option<String>,
    /// Google Cloud project the account belongs to.
    #[serde(default)]
    project_id: Option<String>,
    /// Identifier of the key (not secret); useful for rotation logs.
    #[serde(default)]
    private_key_id: Option<String>,
    /// PEM-encoded RSA private key used to sign assertions.
    #[serde(default)]
    private_key: String,
    /// Service-account email, used as the JWT `iss` claim.
    #[serde(default)]
    client_email: String,
    /// Numeric OAuth client id.
    #[serde(default)]
    client_id: Option<String>,
    /// Token endpoint the assertion is exchanged at (`aud` claim).
    #[serde(default)]
    token_uri: String,
    /// Google Cloud universe domain (e.g. `googleapis.com`).
    #[serde(default)]
    universe_domain: Option<String>,
}

impl ServiceAccount {
    /// Build an account from its three required parts.
    ///
    /// # Errors
    ///
    /// [`GoogleAuthError::InvalidCredentials`] when any part is blank.
    pub fn new(
        client_email: impl Into<String>,
        private_key: impl Into<String>,
        token_uri: impl Into<String>,
    ) -> Result<Self> {
        let account = Self {
            account_type: Some(SERVICE_ACCOUNT_TYPE.to_string()),
            project_id: None,
            private_key_id: None,
            private_key: private_key.into(),
            client_email: client_email.into(),
            client_id: None,
            token_uri: token_uri.into(),
            universe_domain: None,
        };
        account.validate()?;
        Ok(account)
    }

    /// Parse a service-account key from its JSON document.
    ///
    /// # Errors
    ///
    /// [`GoogleAuthError::InvalidCredentials`] when the JSON is malformed, a
    /// field has the wrong type, or a required field is blank; the message
    /// names the field and never echoes the private key.
    pub fn from_json(json: &str) -> Result<Self> {
        let account: Self = serde_json::from_str(json)
            .map_err(|error| invalid(format!("malformed JSON ({error})")))?;
        account.validate()?;
        Ok(account)
    }

    /// Read and parse a service-account key from a file.
    ///
    /// # Errors
    ///
    /// [`GoogleAuthError::CredentialsFile`] when the file cannot be read, or
    /// the same errors as [`ServiceAccount::from_json`] when it is malformed.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let contents = std::fs::read_to_string(path.as_ref()).map_err(|error| {
            GoogleAuthError::CredentialsFile {
                message: error.to_string(),
            }
        })?;
        Self::from_json(&contents)
    }

    /// Load credentials, preferring inline JSON over a file path.
    ///
    /// Blank values are treated as absent, matching the configuration layer's
    /// "empty means unset" policy.
    ///
    /// # Errors
    ///
    /// [`GoogleAuthError::InvalidCredentials`] when neither source is
    /// supplied, or when the chosen source fails to parse.
    pub fn load(json: Option<&str>, path: Option<&str>) -> Result<Self> {
        if let Some(json) = non_blank(json) {
            return Self::from_json(json);
        }
        match non_blank(path) {
            Some(path) => Self::from_file(path),
            None => Err(invalid(
                "no service-account source supplied (expected inline JSON or a file path)"
                    .to_string(),
            )),
        }
    }

    /// The service-account email (`iss` claim).
    #[must_use]
    pub fn client_email(&self) -> &str {
        &self.client_email
    }

    /// The token endpoint URL (`aud` claim).
    #[must_use]
    pub fn token_uri(&self) -> &str {
        &self.token_uri
    }

    /// The Google Cloud project id, when present.
    #[must_use]
    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    /// The key identifier, when present.
    #[must_use]
    pub fn private_key_id(&self) -> Option<&str> {
        self.private_key_id.as_deref()
    }

    /// The numeric OAuth client id, when present.
    #[must_use]
    pub fn client_id(&self) -> Option<&str> {
        self.client_id.as_deref()
    }

    /// The declared universe domain, when present.
    #[must_use]
    pub fn universe_domain(&self) -> Option<&str> {
        self.universe_domain.as_deref()
    }

    /// The PEM private key, visible only inside the crate (never public).
    pub(crate) fn private_key(&self) -> &str {
        &self.private_key
    }

    /// Validate the fields the assertion flow requires.
    fn validate(&self) -> Result<()> {
        if let Some(kind) = self.account_type.as_deref() {
            if kind != SERVICE_ACCOUNT_TYPE {
                return Err(invalid(format!(
                    "`type` is `{kind}`, expected `{SERVICE_ACCOUNT_TYPE}`"
                )));
            }
        }
        for (field, value) in [
            ("client_email", &self.client_email),
            ("private_key", &self.private_key),
            ("token_uri", &self.token_uri),
        ] {
            if value.trim().is_empty() {
                return Err(invalid(format!("`{field}` is empty")));
            }
        }
        Ok(())
    }
}

impl fmt::Debug for ServiceAccount {
    /// Render the account with the private key masked, so a debug dump or log
    /// line can never leak the signing credential.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceAccount")
            .field("account_type", &self.account_type)
            .field("project_id", &self.project_id)
            .field("private_key_id", &self.private_key_id)
            .field("private_key", &"[REDACTED]")
            .field("client_email", &self.client_email)
            .field("client_id", &self.client_id)
            .field("token_uri", &self.token_uri)
            .field("universe_domain", &self.universe_domain)
            .finish()
    }
}

/// Build an `InvalidCredentials` error.
fn invalid(message: String) -> GoogleAuthError {
    GoogleAuthError::InvalidCredentials { message }
}

/// Return `Some(value)` when `value` is non-blank, else `None`.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{key_pair, service_account_json};

    /// A full service-account JSON parses into its typed fields.
    #[test]
    fn deserializes_service_account_fields() {
        let key = key_pair();
        let json = service_account_json(&key.private_pem);
        let account = ServiceAccount::from_json(&json).expect("parse service account");

        assert_eq!(
            account.client_email(),
            "test@test-project.iam.gserviceaccount.com"
        );
        assert_eq!(account.token_uri(), "https://oauth2.googleapis.com/token");
        assert_eq!(account.project_id(), Some("test-project"));
        assert_eq!(account.private_key_id(), Some("abc123"));
        assert_eq!(account.client_id(), Some("1234567890"));
        assert!(account.private_key().contains("BEGIN PRIVATE KEY"));
    }

    /// Missing required fields surface a typed error naming the field.
    #[test]
    fn missing_required_field_is_invalid_credentials() {
        let json = r#"{"type":"service_account","client_email":"a@b.iam.gserviceaccount.com"}"#;
        let error = ServiceAccount::from_json(json).expect_err("missing key must fail");
        match error {
            GoogleAuthError::InvalidCredentials { message } => {
                assert!(message.contains("private_key") || message.contains("token_uri"));
            }
            other => panic!("expected InvalidCredentials, got {other:?}"),
        }
    }

    /// A non-service-account `type` is rejected.
    #[test]
    fn wrong_account_type_is_rejected() {
        let json = r#"{"type":"authorized_user","client_email":"a@b","private_key":"k","token_uri":"https://t"}"#;
        let error = ServiceAccount::from_json(json).expect_err("wrong type must fail");
        assert!(error.to_string().contains("authorized_user"));
    }

    /// Malformed JSON is a typed error and never echoes the payload.
    #[test]
    fn malformed_json_is_typed_error() {
        let error = ServiceAccount::from_json("{\"private_key\": ").expect_err("malformed JSON");
        assert!(matches!(error, GoogleAuthError::InvalidCredentials { .. }));
    }

    /// `Debug` masks the private key while keeping the identifying fields.
    #[test]
    fn debug_redacts_private_key() {
        let key = key_pair();
        let account = ServiceAccount::from_json(&service_account_json(&key.private_pem))
            .expect("parse service account");
        let rendered = format!("{account:?}");
        assert!(!rendered.contains("PRIVATE KEY"), "key leaked: {rendered}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(rendered.contains("test@test-project.iam.gserviceaccount.com"));
    }

    /// `from_file` reads the JSON from disk.
    #[test]
    fn from_file_reads_json() {
        let key = key_pair();
        let json = service_account_json(&key.private_pem);
        let path = std::env::temp_dir().join(format!(
            "rustasea-google-{}-{}.json",
            std::process::id(),
            line!()
        ));
        std::fs::write(&path, &json).expect("write temp credentials");
        let account = ServiceAccount::from_file(&path).expect("read service account");
        let _ = std::fs::remove_file(&path);
        assert_eq!(account.project_id(), Some("test-project"));
    }

    /// `load` prefers inline JSON and falls back to the path.
    #[test]
    fn load_prefers_inline_json() {
        let key = key_pair();
        let inline = service_account_json(&key.private_pem);
        let account =
            ServiceAccount::load(Some(&inline), Some("/does/not/exist.json")).expect("inline wins");
        assert_eq!(account.token_uri(), "https://oauth2.googleapis.com/token");

        let error = ServiceAccount::load(None, None).expect_err("no source must fail");
        assert!(matches!(error, GoogleAuthError::InvalidCredentials { .. }));
    }
}
