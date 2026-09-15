//! Cache-correctness regression tests for the ADOPT-019 review findings.
//!
//! Split out of [`super`] to keep `cache_hooks.rs` within the 500-line cap.
//! Three scenarios, one per finding:
//!
//! 1. A cascade that mutates a child table must bump that table's generation, so
//!    a cached child query misses afterwards (`execute_child_chunk`).
//! 2. A `#[cacheable]` model's `Model::query()` must attach the model's default
//!    cache policy, and `.without_cache()` must still override it.
//! 3. `paginate()`'s COUNT must flow through the cached `count:` path, sharing
//!    its entry with an explicit `count()` on the same filters.

use super::*;

use rustasea_orm::Relation;

/// Build a fresh in-memory pool with every table these scenarios need.
///
/// Separate from [`super::pool_with_users`] because the cascade and cacheable
/// scenarios each need their own tables (`authors`/`books`,
/// `countries`/`currencies`).
async fn pool_with_tables() -> DbPool {
    let pool = DbPool::connect("sqlite::memory:").await.unwrap();
    pool.execute_script(
        "CREATE TABLE countries (id BLOB PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
         CREATE TABLE currencies (id BLOB PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
         CREATE TABLE authors (id BLOB PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
         CREATE TABLE books (id BLOB PRIMARY KEY, author_id BLOB NOT NULL, title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);",
    )
    .await
    .unwrap();
    pool
}

/// A parent model that cascades its soft delete to `books`.
///
/// `relations()` is hand-written (the derive does not emit it), mirroring the
/// cascade fixtures in `cascade_soft_deletes.rs`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Author {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

impl Model for Author {
    fn type_name() -> &'static str {
        "Author"
    }
    fn primary_key(&self) -> Uuid {
        self.id
    }
    fn assign_id(&mut self) -> Uuid {
        self.id = Uuid::now_v7();
        self.id
    }
    fn relations() -> Vec<Relation> {
        vec![Relation::has_many("books", "books", "Author")]
    }
    fn cascade_soft_deletes() -> bool {
        true
    }
    fn cascade_relations() -> &'static [&'static str] {
        &["books"]
    }
}

/// Current time as a fixed-width RFC3339 string (the SQLite datetime shape).
fn now() -> String {
    Utc::now().to_rfc3339()
}

/// Insert an author row.
async fn insert_author(pool: &DbPool, id: Uuid) {
    let s = now();
    pool.execute_bind(
        "INSERT INTO authors (id, name, created_at, updated_at) VALUES ($1, $2, $3, $4)",
        &[
            Value::Uuid(id),
            Value::Text("Ada".into()),
            Value::Text(s.clone()),
            Value::Text(s),
        ],
    )
    .await
    .unwrap();
}

/// Insert a book row owned by `author_id`.
async fn insert_book(pool: &DbPool, id: Uuid, author_id: Uuid) {
    let s = now();
    pool.execute_bind(
        "INSERT INTO books (id, author_id, title, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)",
        &[
            Value::Uuid(id),
            Value::Uuid(author_id),
            Value::Text("notes".into()),
            Value::Text(s.clone()),
            Value::Text(s),
        ],
    )
    .await
    .unwrap();
}

/// An unsaved `Country` with a fresh id.
fn country(name: &str) -> Country {
    let now = Utc::now();
    Country {
        id: Uuid::now_v7(),
        name: name.to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    }
}

/// An unsaved `Currency` with a fresh id.
fn currency(name: &str) -> Currency {
    let now = Utc::now();
    Currency {
        id: Uuid::now_v7(),
        name: name.to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
    }
}

/// Finding 1: a cascade into a child table invalidates that table's cache.
///
/// A cached query on `books` is warmed (one `put`), served from cache on repeat,
/// then the parent `authors` soft delete cascades into `books`. The child table's
/// generation must bump, so the next identical query misses and re-stores.
#[tokio::test]
async fn cascade_invalidates_child_table_cache() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_tables().await;
    let author = Uuid::now_v7();
    insert_author(&pool, author).await;
    insert_book(&pool, Uuid::now_v7(), author).await;
    insert_book(&pool, Uuid::now_v7(), author).await;

    let cached_books = || {
        QueryBuilder::table("books")
            .where_eq("author_id", Value::Uuid(author))
            .cache(Duration::from_secs(60))
    };

    assert_eq!(cached_books().get(&pool).await.unwrap().len(), 2);
    assert_eq!(store.count("put:"), 1, "first child query stores one entry");
    assert_eq!(cached_books().get(&pool).await.unwrap().len(), 2);
    assert_eq!(store.count("put:"), 1, "second child query is a hit");

    // The parent soft delete cascades into `books` (a direct UPDATE).
    assert!(Author::soft_delete(&pool, author).await.unwrap());

    // The child generation bumped, so the same query misses and re-stores.
    assert_eq!(store.count("put:"), 1, "no store write during the cascade");
    let after = cached_books().get(&pool).await.unwrap();
    assert_eq!(after.len(), 2, "rows are still readable");
    assert_eq!(
        store.count("put:"),
        2,
        "cascade must invalidate the cached child query"
    );

    clear_cache_store();
}

/// Finding 2: `#[cacheable(ttl = "...")]` models cache from `query()` alone.
///
/// The model's default policy is attached by `Model::query()`, so two identical
/// `get()`s hit on the second without an explicit `.cache(...)`; an explicit
/// `.without_cache()` bypasses the store entirely.
#[tokio::test]
async fn cacheable_model_query_caches_by_default() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_tables().await;
    Country::create(&pool, country("Japan")).await.unwrap();

    let query = || Country::query().where_eq("name", "Japan");

    assert_eq!(query().get(&pool).await.unwrap().len(), 1);
    assert_eq!(
        store.count("put:ttl=120:"),
        1,
        "cacheable model caches without an explicit .cache()"
    );
    assert_eq!(query().get(&pool).await.unwrap().len(), 1);
    assert_eq!(
        store.count("put:ttl=120:"),
        1,
        "second query is served from cache"
    );
    assert_eq!(store.count("get:"), 2, "both queries consulted the store");

    // An explicit `.without_cache()` overrides the model default.
    assert_eq!(query().without_cache().get(&pool).await.unwrap().len(), 1);
    assert_eq!(
        store.count("get:"),
        2,
        "without_cache never reads the store"
    );
    assert_eq!(
        store.count("put:"),
        1,
        "without_cache never writes the store"
    );

    clear_cache_store();
}

/// Finding 2 (forever branch): a bare `#[cacheable]` model caches forever.
#[tokio::test]
async fn forever_cacheable_model_query_caches_by_default() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_tables().await;
    Currency::create(&pool, currency("USD")).await.unwrap();

    let query = || Currency::query().where_eq("name", "USD");
    query().get(&pool).await.unwrap();
    query().get(&pool).await.unwrap();

    assert_eq!(
        store.count("put:ttl=forever:"),
        1,
        "a bare #[cacheable] model caches forever by default"
    );

    clear_cache_store();
}

/// Finding 2 (negative): a non-cacheable model's `query()` never touches the store.
#[tokio::test]
async fn non_cacheable_model_query_does_not_touch_store() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;
    User::create(&pool, sample("Ada")).await.unwrap();

    let rows = User::query()
        .where_eq("name", "Ada")
        .get(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(store.count("get:"), 0, "non-cacheable model reads no cache");
    assert_eq!(
        store.count("put:"),
        0,
        "non-cacheable model writes no cache"
    );

    clear_cache_store();
}

/// Finding 3: `paginate()`'s COUNT shares the cached `count:` entry with `count()`.
///
/// Two identical paginations store exactly one COUNT entry and one windowed-SELECT
/// entry (the second page is served from cache). A follow-up `count()` on the same
/// filters is then a cache hit — proving the paginate COUNT used the same `count:`
/// namespace rather than a direct, uncached `fetch_json`.
#[tokio::test]
async fn paginate_count_shares_cached_entry_with_count() {
    let _guard = test_lock().lock().await;
    clear_cache_store();
    let store = install_store();
    let pool = pool_with_users().await;
    User::create(&pool, sample("Ada")).await.unwrap();

    let query = || {
        <User as Model>::query()
            .where_eq("name", "Ada")
            .cache(Duration::from_secs(60))
    };

    let first = query().paginate(&pool, 1, 10).await.unwrap();
    assert_eq!(first.total, 1);
    let second = query().paginate(&pool, 1, 10).await.unwrap();
    assert_eq!(second.total, 1);
    assert_eq!(
        store.count("put:"),
        2,
        "one COUNT entry + one windowed-SELECT entry"
    );

    // The COUNT entry is shared: an explicit count() does not re-store.
    assert_eq!(query().count(&pool).await.unwrap(), 1);
    assert_eq!(
        store.count("put:"),
        2,
        "paginate's COUNT shares the `count:` entry with count()"
    );
    assert_eq!(
        store.count("get:"),
        5,
        "two paginate reads (count+window) + one count() probe"
    );

    clear_cache_store();
}
