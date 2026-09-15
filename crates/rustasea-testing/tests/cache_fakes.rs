//! Integration tests for [`FakeCache`] (ADOPT-013 residual scope).
//!
//! Each test owns its own [`FakeCache`] instance, so — unlike the queue route
//! registry and the process-wide mailer — there is no global state to isolate
//! and the tests are safe to run in parallel.
//!
//! Positive tests prove the recording + assertion contract; negative tests pin
//! the failure text with `#[should_panic(expected = ...)]` so a regression that
//! drops the "what was recorded" detail fails loudly.

use std::time::Duration;

use rustasea_cache::{CacheManager, Store};
use rustasea_testing::{FakeCache, Op, RecordedOp};

/// Positive: a `put` is readable through the store and recorded in the log.
#[tokio::test]
async fn fake_cache_put_records_and_stores() {
    let cache = FakeCache::new();

    cache
        .put("user:1", b"ada".to_vec(), Duration::ZERO)
        .await
        .expect("put");

    cache.assert_has("user:1").await;
    cache.assert_put("user:1");
    cache.assert_missing("user:2").await;

    let value = cache.get("user:1").await.expect("get").expect("present");
    assert_eq!(value, b"ada".to_vec());

    assert_eq!(cache.keys(), vec!["user:1".to_string()]);
    // `put` + `get` were recorded (order preserved).
    assert_eq!(cache.count(), 2);
}

/// Positive: `assert_missing` passes for a key that was never written, and the
/// op-log write assertions stay silent.
#[tokio::test]
async fn fake_cache_missing_key_passes() {
    let cache = FakeCache::new();

    cache.assert_missing("never:written").await;
    cache.assert_nothing_stored();
}

/// Negative: `assert_has` on a missing key panics with the convention prefix.
#[tokio::test]
#[should_panic(expected = "expected key `missing` to have been stored, but it was not.")]
async fn fake_cache_assert_has_panics_on_missing_key() {
    let cache = FakeCache::new();

    cache.assert_has("missing").await;
}

/// Negative: `assert_put` on a never-put key panics with the convention prefix.
#[tokio::test]
#[should_panic(expected = "expected key `never:put` to have been put, but it was not.")]
async fn fake_cache_assert_put_panics_on_never_put_key() {
    let cache = FakeCache::new();
    cache.get("read-only").await.expect("get");

    cache.assert_put("never:put");
}

/// TTL recording: the requested TTL is captured on the recorded `put`.
#[tokio::test]
async fn fake_cache_records_put_ttl() {
    let cache = FakeCache::new();
    let ttl = Duration::from_secs(60);

    cache
        .put("session:1", b"token".to_vec(), ttl)
        .await
        .expect("put");

    let ops = cache.operations();
    assert_eq!(ops.len(), 1);
    match ops[0].op {
        Op::Put { len, ttl: recorded } => {
            assert_eq!(len, 5);
            assert_eq!(recorded, ttl);
        }
        other => panic!("expected a put op, got {other:?}"),
    }
    assert_eq!(ops[0].key, "session:1");
}

/// `put_if_absent` returns `true` on first write and `false` on the second.
#[tokio::test]
async fn fake_cache_put_if_absent_is_first_write_wins() {
    let cache = FakeCache::new();

    let first = cache
        .put_if_absent("lock:1", b"owner-a".to_vec(), Duration::ZERO)
        .await
        .expect("first");
    let second = cache
        .put_if_absent("lock:1", b"owner-b".to_vec(), Duration::ZERO)
        .await
        .expect("second");

    assert!(first);
    assert!(!second);

    let value = cache.get("lock:1").await.expect("get").expect("present");
    assert_eq!(value, b"owner-a".to_vec());

    cache.assert_put("lock:1");
    // Both attempts are recorded as `PutIfAbsent` with the captured arguments.
    let attempts = cache
        .operations()
        .into_iter()
        .filter(|r| matches!(r.op, Op::PutIfAbsent { .. }))
        .count();
    assert_eq!(attempts, 2);
}

/// `compare_and_delete` returns `true` on a matching value, `false` otherwise.
#[tokio::test]
async fn fake_cache_compare_and_delete_matches_value() {
    let cache = FakeCache::new();
    cache
        .put("lease", b"abc".to_vec(), Duration::ZERO)
        .await
        .expect("put");

    let mismatch = cache
        .compare_and_delete("lease", b"xyz")
        .await
        .expect("mismatch");
    let matched = cache
        .compare_and_delete("lease", b"abc")
        .await
        .expect("match");

    assert!(!mismatch);
    assert!(matched);
    cache.assert_missing("lease").await;
}

/// `flush` is recorded and empties the store.
#[tokio::test]
async fn fake_cache_flush_records_and_clears() {
    let cache = FakeCache::new();
    cache
        .put("a", b"1".to_vec(), Duration::ZERO)
        .await
        .expect("put");
    cache
        .put("b", b"2".to_vec(), Duration::ZERO)
        .await
        .expect("put");

    cache.flush().await.expect("flush");

    cache.assert_missing("a").await;
    cache.assert_missing("b").await;
    assert!(cache.operations().iter().any(|r| matches!(r.op, Op::Flush)));
}

/// `increment` is recorded and returns the running value.
#[tokio::test]
async fn fake_cache_increment_records_and_returns() {
    let cache = FakeCache::new();

    let first = cache.increment("hits", 1).await.expect("inc");
    let second = cache.increment("hits", 5).await.expect("inc");

    assert_eq!(first, 1);
    assert_eq!(second, 6);
    let ops = cache.operations();
    let increments: Vec<i64> = ops
        .iter()
        .filter_map(|r| match r.op {
            Op::Increment { amount } => Some(amount),
            _ => None,
        })
        .collect();
    assert_eq!(increments, vec![1, 5]);
}

/// `touch` is recorded and delegates the real TTL extension to the store.
#[tokio::test]
async fn fake_cache_touch_records_and_delegates() {
    let cache = FakeCache::new();
    cache
        .put("session", b"v".to_vec(), Duration::from_secs(1))
        .await
        .expect("put");

    let touched = cache
        .touch("session", Duration::from_secs(300))
        .await
        .expect("touch");

    assert!(touched);
    // The recorded `touch` captured the requested TTL.
    assert!(cache
        .operations()
        .iter()
        .any(|r| matches!(r.op, Op::Touch { ttl } if ttl == Duration::from_secs(300))));
}

/// `forget` is recorded and removes the key.
#[tokio::test]
async fn fake_cache_forget_records_and_removes() {
    let cache = FakeCache::new();
    cache
        .put("temp", b"v".to_vec(), Duration::ZERO)
        .await
        .expect("put");

    cache.forget("temp").await.expect("forget");

    cache.assert_forgotten("temp");
    cache.assert_missing("temp").await;
}

/// `clear` resets both the op log and the backing store.
#[tokio::test]
async fn fake_cache_clear_resets_everything() {
    let cache = FakeCache::new();
    cache
        .put("a", b"1".to_vec(), Duration::ZERO)
        .await
        .expect("put");
    cache.get("a").await.expect("get");

    cache.clear().await;

    assert_eq!(cache.count(), 0);
    assert!(cache.keys().is_empty());
    cache.assert_nothing_stored();
    cache.assert_missing("a").await;
}

/// `RecordedOp`/`Op` are `PartialEq`, so tests can `assert_eq!` on the log.
#[tokio::test]
async fn fake_cache_recorded_ops_compare_by_value() {
    let cache = FakeCache::new();
    cache.get("k").await.expect("get");
    cache
        .put("k", b"value".to_vec(), Duration::from_secs(9))
        .await
        .expect("put");

    let ops = cache.operations();
    assert_eq!(
        ops[0],
        RecordedOp {
            op: Op::Get,
            key: "k".to_string(),
        }
    );
    assert_eq!(
        ops[1],
        RecordedOp {
            op: Op::Put {
                len: 5,
                ttl: Duration::from_secs(9),
            },
            key: "k".to_string(),
        }
    );
}

/// `install` registers the fake with the manager so a repository sees it.
#[tokio::test]
async fn fake_cache_install_routes_through_manager() {
    let manager = CacheManager::new();
    let fake = FakeCache::install(&manager, "test-store");

    // A repository resolved from the manager writes through the fake.
    let repository = manager.store("test-store").expect("store");
    repository
        .put("user:1", &"ada", Duration::ZERO)
        .await
        .expect("put");

    // The fake recorded the prefixed key the repository used and holds the value.
    assert_eq!(fake.keys(), vec!["-cache-user:1".to_string()]);
    fake.assert_has("-cache-user:1").await;
    fake.assert_put("-cache-user:1");
}

/// `decrement` is recorded as an increment with a negative amount.
#[tokio::test]
async fn fake_cache_decrement_records_negative_amount() {
    let cache = FakeCache::new();

    let after_inc = cache.increment("stock", 10).await.expect("inc");
    let after_dec = cache.decrement("stock", 4).await.expect("dec");

    assert_eq!(after_inc, 10);
    assert_eq!(after_dec, 6);

    let amounts: Vec<i64> = cache
        .operations()
        .iter()
        .filter_map(|r| match r.op {
            Op::Increment { amount } => Some(amount),
            _ => None,
        })
        .collect();
    assert_eq!(amounts, vec![10, -4]);
}

/// Negative: `assert_nothing_stored` detects an `increment` as a write.
#[tokio::test]
#[should_panic(expected = "expected nothing to have been stored, but 1 write(s) were recorded.")]
async fn fake_cache_assert_nothing_stored_detects_increment() {
    let cache = FakeCache::new();

    cache.increment("counter", 1).await.expect("inc");
    cache.assert_nothing_stored();
}
