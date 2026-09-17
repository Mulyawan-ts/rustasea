//! `posts` model and its in-memory repository.
//!
//! The [`Post`] struct and its hand-written [`Model`] impl mirror the scaffold's
//! `app/models/user.rs`: a plain data struct with the ORM contract written
//! against the umbrella's `rustasea::orm::Model` path, so the app depends on the
//! single `rustasea` crate.
//!
//! # Swapping the in-memory store for the real ORM
//!
//! [`PostRepository`] keeps rows in a process-wide `Mutex<Vec<Post>>` so the
//! example routes work with no database. To use the real query builder, replace
//! the bodies with calls such as `Post::query().get()` and
//! `Post::query().where_eq("id", Value::Uuid(id))`, mirroring the ORM examples
//! in `rustasea-orm`. The struct, the [`Model`] impl, and every controller stay
//! unchanged, so only this file moves.

use std::sync::{Mutex, MutexGuard, OnceLock};

use rustasea::orm::{Model, Timestamps};
use uuid::Uuid;

/// A blog post.
///
/// Timestamps travel in the ORM's [`Timestamps`] value so the model matches the
/// scaffold shape and the `uses_timestamps` contract.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Post {
    /// Primary key.
    pub id: Uuid,
    /// Post headline.
    pub title: String,
    /// Post body text.
    pub body: String,
    /// Whether the post is visible to readers.
    pub published: bool,
    /// Created/updated timestamps.
    pub timestamps: Timestamps,
}

impl Model for Post {
    /// Type name driving the default table derivation.
    fn type_name() -> &'static str {
        "Post"
    }

    /// Explicit table name (`posts`).
    fn table_name() -> String {
        "posts".to_string()
    }

    /// Primary key value.
    fn primary_key(&self) -> Uuid {
        self.id
    }

    /// Assign a fresh client-generated UUID (v7) before persistence.
    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }

    /// Posts are hard-deleted in this example.
    fn uses_soft_deletes() -> bool {
        false
    }

    /// Timestamps are maintained via the `timestamps` column.
    fn uses_timestamps() -> bool {
        true
    }
}

/// Process-wide in-memory row store.
///
/// Initialized empty on first use. A poisoned lock is recovered with
/// [`std::sync::PoisonError::into_inner`] rather than panicking, so a panic in
/// one request never takes the whole store down for later requests.
fn store() -> &'static Mutex<Vec<Post>> {
    static STORE: OnceLock<Mutex<Vec<Post>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Lock the store, recovering from a poisoned mutex instead of panicking.
fn lock() -> MutexGuard<'static, Vec<Post>> {
    match store().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Process-wide serialization lock shared by every test that mutates the store.
///
/// The in-memory store is process-wide, so tests in different modules (this
/// module's repository tests and the route/action tests under `crate::app` and
/// `crate::routes`) must share one lock or they race under libtest's parallel
/// runner. A `tokio` mutex is used because the guard is held across an `.await`
/// in the async tests and must stay `Send`; the [`LazyLock`](std::sync::LazyLock)
/// wrapper is needed because `tokio::sync::Mutex::new` is not a `const fn`.
#[cfg(test)]
pub(crate) fn test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    &LOCK
}

/// In-memory repository for [`Post`] rows.
///
/// The associated functions are intentionally stateless: the rows live in the
/// process-wide store, so the controller can call them without threading a
/// handle through the router. This is the direct analogue of a stateless
/// Eloquent model calling into the shared connection.
pub struct PostRepository;

impl PostRepository {
    /// Return every post, newest first.
    pub fn list() -> Vec<Post> {
        let mut posts = lock().clone();
        posts.sort_by_key(|post| std::cmp::Reverse(post.timestamps.created_at));
        posts
    }

    /// Find a post by primary key.
    pub fn find(id: Uuid) -> Option<Post> {
        lock().iter().find(|post| post.id == id).cloned()
    }

    /// Create a post, assigning a fresh UUID and timestamps.
    pub fn create(title: String, body: String, published: bool) -> Post {
        let mut post = Post {
            id: Uuid::nil(),
            title,
            body,
            published,
            timestamps: Timestamps::default(),
        };
        post.assign_id();
        lock().push(post.clone());
        post
    }

    /// Update an existing post, returning the new value or `None` if absent.
    pub fn update(id: Uuid, title: String, body: String, published: bool) -> Option<Post> {
        let mut posts = lock();
        let post = posts.iter_mut().find(|post| post.id == id)?;
        post.title = title;
        post.body = body;
        post.published = published;
        post.timestamps.updated_at = chrono::Utc::now();
        Some(post.clone())
    }

    /// Delete a post, returning whether a row was removed.
    pub fn delete(id: Uuid) -> bool {
        let mut posts = lock();
        let before = posts.len();
        posts.retain(|post| post.id != id);
        posts.len() != before
    }

    /// Remove every post (used by tests to isolate cases).
    #[cfg(test)]
    pub fn reset() {
        lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes repository tests: the store is process-wide, so cases that
    /// mutate it must not interleave under libtest's parallel runner.
    ///
    /// The tests are synchronous, so the shared `tokio` mutex is acquired with
    /// [`tokio::sync::Mutex::blocking_lock`] (safe here: no async runtime is
    /// active in a plain `#[test]`).
    fn lock_tests() -> tokio::sync::MutexGuard<'static, ()> {
        super::test_lock().blocking_lock()
    }

    /// `table_name` derives from the explicit override.
    #[test]
    fn table_name_is_posts() {
        assert_eq!(Post::table_name(), "posts");
        assert_eq!(Post::type_name(), "Post");
    }

    /// CRUD round-trip: create, list, find, update, delete.
    #[test]
    fn repository_crud_round_trip() {
        let _guard = lock_tests();
        PostRepository::reset();

        let created = PostRepository::create("Hello".to_string(), "World".to_string(), false);
        assert_eq!(PostRepository::list().len(), 1);
        assert_eq!(
            PostRepository::find(created.id).map(|post| post.title),
            Some("Hello".to_string())
        );

        let updated = PostRepository::update(
            created.id,
            "Hello again".to_string(),
            "World".to_string(),
            true,
        )
        .expect("the created post must exist");
        assert_eq!(updated.title, "Hello again");
        assert!(updated.published);

        assert!(PostRepository::delete(created.id));
        assert!(PostRepository::find(created.id).is_none());
        assert!(PostRepository::list().is_empty());
    }

    /// Updating or deleting an unknown id is a no-op, never a panic.
    #[test]
    fn unknown_id_is_a_no_op() {
        let _guard = lock_tests();
        PostRepository::reset();

        let missing = Uuid::now_v7();
        assert!(PostRepository::update(missing, "t".to_string(), "b".to_string(), true).is_none());
        assert!(!PostRepository::delete(missing));
    }
}
