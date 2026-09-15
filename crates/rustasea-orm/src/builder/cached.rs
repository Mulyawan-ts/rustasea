//! Opt-in caching modifiers on [`QueryBuilder`] (ADOPT-019).
//!
//! A builder with a [`CacheMode`] set stores its decoded rows through the
//! process-wide [`crate::cache::QueryCacheStore`] (when one is installed) and
//! serves the next identical query from cache. `.cache(ttl)` sets an expiry;
//! `.cache_forever()` never expires on its own (a table write still invalidates
//! it); `.without_cache()` clears any mode, forcing a database round-trip.

use std::time::Duration;

use super::QueryBuilder;

/// The caching policy attached to a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Cache with a time-to-live; the entry expires after `ttl`.
    Ttl(Duration),
    /// Cache with no expiry (invalidated only by a table write).
    Forever,
}

impl CacheMode {
    /// The store TTL for this mode (`None` = forever).
    pub(crate) fn ttl(self) -> Option<Duration> {
        match self {
            CacheMode::Ttl(ttl) => Some(ttl),
            CacheMode::Forever => None,
        }
    }
}

impl QueryBuilder {
    /// Cache this query's rows for `ttl` seconds (ADOPT-019).
    ///
    /// A zero duration means "forever" — the entry is invalidated only by a
    /// write to the table, matching Laravel's `cacheForever` shorthand.
    pub fn cache(mut self, ttl: Duration) -> Self {
        self.cache_mode = Some(if ttl.is_zero() {
            CacheMode::Forever
        } else {
            CacheMode::Ttl(ttl)
        });
        self
    }

    /// Cache this query's rows with no expiry.
    pub fn cache_forever(mut self) -> Self {
        self.cache_mode = Some(CacheMode::Forever);
        self
    }

    /// Bypass the cache for this query (`refresh()` semantics).
    pub fn without_cache(mut self) -> Self {
        self.cache_mode = None;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.cache(d)` records a TTL mode.
    #[test]
    fn cache_sets_ttl_mode() {
        let builder = QueryBuilder::table("users").cache(Duration::from_secs(30));
        assert_eq!(
            builder.cache_mode(),
            Some(CacheMode::Ttl(Duration::from_secs(30)))
        );
    }

    /// A zero TTL means forever.
    #[test]
    fn cache_zero_ttl_is_forever() {
        let builder = QueryBuilder::table("users").cache(Duration::ZERO);
        assert_eq!(builder.cache_mode(), Some(CacheMode::Forever));
    }

    /// `.cache_forever()` sets the forever mode.
    #[test]
    fn cache_forever_sets_mode() {
        let builder = QueryBuilder::table("users").cache_forever();
        assert_eq!(builder.cache_mode(), Some(CacheMode::Forever));
    }

    /// `.without_cache()` clears any mode.
    #[test]
    fn without_cache_clears_mode() {
        let builder = QueryBuilder::table("users")
            .cache(Duration::from_secs(5))
            .without_cache();
        assert_eq!(builder.cache_mode(), None);
    }

    /// The TTL accessor maps forever to `None`.
    #[test]
    fn ttl_maps_forever_to_none() {
        assert_eq!(CacheMode::Forever.ttl(), None);
        assert_eq!(
            CacheMode::Ttl(Duration::from_secs(7)).ttl(),
            Some(Duration::from_secs(7))
        );
    }
}
