//! Integration tests for the async [`UserProvider`] write seam (AUTH-004).
//!
//! Exercises the new provider surface directly: registration
//! (`create` → `find_by_email`), password rotation (`update_password`), email
//! verification (`set_email_verified_at`), and the fail-closed
//! [`DenyAllProvider`]. The database-backed login path
//! (`SessionGuard::login_with_provider`) is covered in
//! `session_provider_login.rs`.

use rustasea_auth::users::{
    AuthUserRecord, DenyAllProvider, MemoryUserProvider, NewUserRecord, UserLookup, UserProvider,
};
use rustasea_auth::verify::{Argon2Verifier, PasswordVerifier};
use rustasea_auth::AuthError;

/// Real argon2 hasher/verifier (production KDF, no test double).
fn hasher() -> Argon2Verifier {
    Argon2Verifier::new()
}

/// `create` then `find_by_email` returns the stored record with a minted id.
#[tokio::test]
async fn create_then_find_by_email_round_trips() {
    let provider = MemoryUserProvider::default();
    let hash = hasher().hash("s3cr3tPass").expect("hash");

    let created = provider
        .create(NewUserRecord::new(
            "Ada Lovelace",
            "ada@example.com",
            hash.clone(),
        ))
        .await
        .expect("create succeeds");
    assert_eq!(created.email, "ada@example.com");
    assert_eq!(created.password_hash, hash);
    assert_eq!(created.email_verified_at, None);
    assert!(!created.id.is_empty(), "create mints a non-empty id");

    let found = provider
        .find_by_email("ada@example.com")
        .await
        .expect("find succeeds")
        .expect("user is present");
    assert_eq!(found, created);
}

/// `find_by_email` on an unknown address yields `Ok(None)`, not an error.
#[tokio::test]
async fn find_by_email_unknown_is_none() {
    let provider = MemoryUserProvider::default();
    let found = provider
        .find_by_email("nobody@example.com")
        .await
        .expect("find succeeds");
    assert_eq!(found, None);
}

/// `set_email_verified_at` flips the record (and can clear it again).
#[tokio::test]
async fn set_email_verified_at_reflects_on_record() {
    let provider = MemoryUserProvider::default();
    let created = provider
        .create(NewUserRecord::new(
            "Ada",
            "ada@example.com",
            hasher().hash("s3cr3tPass").expect("hash"),
        ))
        .await
        .expect("create");
    assert_eq!(created.email_verified_at, None);

    provider
        .set_email_verified_at(&created.id, Some("2026-01-01T00:00:00Z"))
        .await
        .expect("verify succeeds");
    let verified = provider
        .find_by_email("ada@example.com")
        .await
        .expect("find")
        .expect("present");
    assert_eq!(
        verified.email_verified_at.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );

    // Clearing (email change → re-verification) removes the timestamp.
    provider
        .set_email_verified_at(&created.id, None)
        .await
        .expect("clear succeeds");
    let cleared = provider
        .find_by_email("ada@example.com")
        .await
        .expect("find")
        .expect("present");
    assert_eq!(cleared.email_verified_at, None);
}

/// `create` with a duplicate email returns the typed `UserExists`, not a panic.
#[tokio::test]
async fn create_duplicate_email_is_typed_error() {
    let provider = MemoryUserProvider::default();
    let hash = hasher().hash("s3cr3tPass").expect("hash");
    provider
        .create(NewUserRecord::new("Ada", "ada@example.com", hash.clone()))
        .await
        .expect("first create succeeds");

    let err = provider
        .create(NewUserRecord::new("Impostor", "ada@example.com", hash))
        .await
        .expect_err("duplicate email is rejected");
    assert_eq!(
        err,
        AuthError::UserExists {
            email: "ada@example.com".into()
        }
    );
    assert_eq!(err.code(), "AuthError::UserExists");
}

/// `update_password` / `set_email_verified_at` on an unknown id return the
/// typed `UserNotFound`.
#[tokio::test]
async fn mutating_unknown_user_is_typed_error() {
    let provider = MemoryUserProvider::default();
    let err = provider
        .update_password("missing-id", "phc$hash")
        .await
        .expect_err("unknown id rejected");
    assert_eq!(
        err,
        AuthError::UserNotFound {
            id: "missing-id".into()
        }
    );
    let err = provider
        .set_email_verified_at("missing-id", Some("2026-01-01T00:00:00Z"))
        .await
        .expect_err("unknown id rejected");
    assert!(matches!(err, AuthError::UserNotFound { .. }));
}

/// `DenyAllProvider` fails every operation with a typed error — never a panic,
/// never a silent success.
#[tokio::test]
async fn deny_all_provider_fails_closed_with_typed_errors() {
    let provider = DenyAllProvider;
    assert!(matches!(
        provider.find_by_email("ada@example.com").await,
        Err(AuthError::Disabled(_))
    ));
    assert!(matches!(
        provider
            .create(NewUserRecord::new("Ada", "ada@example.com", "phc$hash"))
            .await,
        Err(AuthError::Disabled(_))
    ));
    assert!(matches!(
        provider.update_password("user-1", "phc$hash").await,
        Err(AuthError::Disabled(_))
    ));
    assert!(matches!(
        provider
            .set_email_verified_at("user-1", Some("2026-01-01T00:00:00Z"))
            .await,
        Err(AuthError::Disabled(_))
    ));
}

/// The sync `UserLookup` path still works on the same `MemoryUserProvider`
/// (the write seam did not replace the read seam).
#[tokio::test]
async fn memory_provider_still_satisfies_sync_lookup() {
    let provider = MemoryUserProvider::default();
    let created = provider
        .create(NewUserRecord::new(
            "Ada",
            "ada@example.com",
            hasher().hash("s3cr3tPass").expect("hash"),
        ))
        .await
        .expect("create");

    assert_eq!(
        UserLookup::hash_for_email(&provider, "ada@example.com"),
        Some(created.password_hash.clone())
    );
    assert_eq!(
        UserLookup::id_for_email(&provider, "ada@example.com"),
        Some(created.id.clone())
    );
    assert_eq!(
        UserLookup::email_for_id(&provider, &created.id),
        Some("ada@example.com".to_string())
    );
    assert_eq!(
        UserLookup::email_verified_at_for_id(&provider, &created.id),
        None
    );
}

/// `NewUserRecord::with_email_verified_at` persists a pre-verified record, and
/// `MemoryUserProvider::by_id` resolves it by the minted id.
#[tokio::test]
async fn create_pre_verified_and_resolve_by_id() {
    let provider = MemoryUserProvider::default();
    let created = provider
        .create(
            NewUserRecord::new("Ada", "ada@example.com", "phc$hash")
                .with_email_verified_at(Some("2026-01-01T00:00:00Z")),
        )
        .await
        .expect("create");

    assert_eq!(
        created.email_verified_at.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
    assert_eq!(provider.by_id(&created.id), Some(created.clone()));
}

/// A seeded [`AuthUserRecord`] is visible to both the sync and async reads.
#[tokio::test]
async fn seeded_record_is_lookup_visible() {
    let provider = MemoryUserProvider::default();
    provider.seed(AuthUserRecord {
        id: "user-1".into(),
        email: "ada@example.com".into(),
        password_hash: "phc$hash".into(),
        email_verified_at: None,
    });
    assert_eq!(
        provider.by_email("ada@example.com").map(|r| r.id),
        Some("user-1".to_string())
    );
    let found = provider
        .find_by_email("ada@example.com")
        .await
        .expect("find")
        .expect("present");
    assert_eq!(found.id, "user-1");
}

/// `update_password` stores a genuinely usable argon2 credential: the new hash
/// verifies against the new plaintext and rejects the old one.
#[tokio::test]
async fn updated_password_hash_verifies_with_argon2() {
    let provider = MemoryUserProvider::default();
    let verifier = hasher();
    let created = provider
        .create(NewUserRecord::new(
            "Ada",
            "ada@example.com",
            verifier.hash("old-password").expect("hash"),
        ))
        .await
        .expect("create");

    let new_hash = verifier.hash("new-password").expect("hash");
    provider
        .update_password(&created.id, &new_hash)
        .await
        .expect("update");

    let stored = provider
        .find_by_email("ada@example.com")
        .await
        .expect("find")
        .expect("present");
    assert!(verifier.verify(&stored.password_hash, "new-password"));
    assert!(!verifier.verify(&stored.password_hash, "old-password"));
}

/// `update_profile` persists a new name and email: the old email stops
/// resolving and the new one returns the same user (AUTH-010 unblock).
#[tokio::test]
async fn update_profile_changes_name_and_email() {
    let provider = MemoryUserProvider::default();
    let created = provider
        .create(NewUserRecord::new(
            "Ada Lovelace",
            "ada@example.com",
            hasher().hash("s3cr3tPass").expect("hash"),
        ))
        .await
        .expect("create");
    assert_eq!(
        provider.name_for_id(&created.id).as_deref(),
        Some("Ada Lovelace")
    );

    provider
        .update_profile(&created.id, "Ada King", "ada.king@example.com")
        .await
        .expect("update succeeds");

    // The new email resolves to the SAME user id (proving the email changed).
    let found = provider
        .find_by_email("ada.king@example.com")
        .await
        .expect("find")
        .expect("present under new email");
    assert_eq!(found.id, created.id);
    // The old email no longer resolves.
    assert_eq!(
        provider
            .find_by_email("ada@example.com")
            .await
            .expect("find"),
        None
    );
    assert_eq!(
        provider.name_for_id(&created.id).as_deref(),
        Some("Ada King")
    );
}

/// `update_profile` on an unknown id returns the typed `UserNotFound`.
#[tokio::test]
async fn update_profile_unknown_user_is_typed_error() {
    let provider = MemoryUserProvider::default();
    let err = provider
        .update_profile("missing-id", "Ghost", "ghost@example.com")
        .await
        .expect_err("unknown id rejected");
    assert_eq!(
        err,
        AuthError::UserNotFound {
            id: "missing-id".into()
        }
    );
}

/// `update_profile` to an email owned by ANOTHER user returns the typed
/// `UserExists`; re-asserting the caller's own email is not a collision.
#[tokio::test]
async fn update_profile_to_taken_email_is_typed_error() {
    let provider = MemoryUserProvider::default();
    let hash = hasher().hash("s3cr3tPass").expect("hash");
    let ada = provider
        .create(NewUserRecord::new("Ada", "ada@example.com", hash.clone()))
        .await
        .expect("create ada");
    provider
        .create(NewUserRecord::new("Grace", "grace@example.com", hash))
        .await
        .expect("create grace");

    let err = provider
        .update_profile(&ada.id, "Ada", "grace@example.com")
        .await
        .expect_err("collision rejected");
    assert_eq!(
        err,
        AuthError::UserExists {
            email: "grace@example.com".into()
        }
    );

    // Re-asserting the caller's own email (with a new name) still succeeds.
    provider
        .update_profile(&ada.id, "Ada King", "ada@example.com")
        .await
        .expect("own email is not a collision");
    assert_eq!(provider.name_for_id(&ada.id).as_deref(), Some("Ada King"));
}

/// `DenyAllProvider::update_profile` fails closed with a typed error — never a
/// panic, never a silent success.
#[tokio::test]
async fn deny_all_provider_update_profile_fails_closed() {
    let provider = DenyAllProvider;
    assert!(matches!(
        provider
            .update_profile("user-1", "Ada", "ada@example.com")
            .await,
        Err(AuthError::Disabled(_))
    ));
}
