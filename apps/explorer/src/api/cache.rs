//! TTL cache with single-flight fetches.
//!
//! The node's live answers are cheap for one caller and expensive under load:
//! `get_supply` scans every account. When many requests miss at once, the
//! first one fetches and the rest wait on its result rather than each asking
//! the node again.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Hard cap on cached keys. A stream of distinct addresses cannot grow the
/// table past it; a key that arrives while the table is full and nothing has
/// expired is fetched without being cached.
pub const MAX_SLOTS: usize = 1024;

/// One key's value and when it was fetched. Its own lock is the single-flight
/// gate: holders of the lock are the only ones fetching that key.
type Slot<V> = Arc<Mutex<Option<(Instant, V)>>>;

struct Table<V> {
    slots: HashMap<String, Slot<V>>,
    /// When a full table was last swept. A sweep is O(n) under the table
    /// lock, so a full table is swept at most once per TTL, not per new key.
    swept: Option<Instant>,
}

/// A cache of `V` by string key, each entry valid for a fixed time.
pub struct TtlCache<V> {
    table: Mutex<Table<V>>,
}

impl<V: Clone + Send> Default for TtlCache<V> {
    fn default() -> Self {
        Self {
            table: Mutex::new(Table {
                slots: HashMap::new(),
                swept: None,
            }),
        }
    }
}

/// Whether a value came from the cache or from a fresh fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Served from a fresh entry.
    Hit,
    /// Fetched now.
    Miss,
}

impl<V: Clone + Send> TtlCache<V> {
    /// Returns the cached value for `key` if it is younger than `ttl`,
    /// otherwise runs `fetch` once for all concurrent callers.
    ///
    /// Errors are not cached: the next caller retries the node.
    ///
    /// # Errors
    ///
    /// Whatever `fetch` returns.
    pub async fn get_or_fetch<E, F, Fut>(
        &self,
        key: &str,
        ttl: Duration,
        fetch: F,
    ) -> Result<(V, Source), E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, E>>,
    {
        let slot = self.slot(key, ttl).await;
        let mut entry = slot.lock().await;
        if let Some((at, value)) = entry.as_ref()
            && at.elapsed() < ttl
        {
            return Ok((value.clone(), Source::Hit));
        }
        let value = fetch().await?;
        *entry = Some((Instant::now(), value.clone()));
        Ok((value, Source::Miss))
    }

    /// The slot for `key`, created if absent. A full table is swept of expired
    /// slots (never one being fetched right now); if it is still full, the
    /// caller gets a private slot that is not cached.
    async fn slot(&self, key: &str, ttl: Duration) -> Slot<V> {
        let mut table = self.table.lock().await;
        if let Some(slot) = table.slots.get(key) {
            return Arc::clone(slot);
        }
        if table.slots.len() >= MAX_SLOTS {
            if table.swept.is_none_or(|at| at.elapsed() >= ttl) {
                table.slots.retain(|_, slot| {
                    slot.try_lock().map_or(true, |entry| {
                        entry.as_ref().is_some_and(|(at, _)| at.elapsed() < ttl)
                    })
                });
                table.swept = Some(Instant::now());
            }
            if table.slots.len() >= MAX_SLOTS {
                return Slot::default();
            }
        }
        Arc::clone(table.slots.entry(key.to_string()).or_default())
    }

    #[cfg(test)]
    async fn cached_keys(&self) -> usize {
        self.table.lock().await.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn fresh_value_is_served_without_a_fetch() {
        let cache = TtlCache::<u32>::default();
        let calls = AtomicUsize::new(0);
        for _ in 0..3 {
            let fetch = || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(5)
            };
            cache
                .get_or_fetch("k", Duration::from_secs(60), fetch)
                .await
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_misses_share_one_fetch() {
        let cache = Arc::new(TtlCache::<u32>::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..50 {
            let (cache, calls) = (Arc::clone(&cache), Arc::clone(&calls));
            tasks.push(tokio::spawn(async move {
                let fetch = || async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>(9)
                };
                cache
                    .get_or_fetch("supply", Duration::from_secs(30), fetch)
                    .await
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap().0, 9);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn errors_are_not_cached() {
        let cache = TtlCache::<u32>::default();
        let failed = cache
            .get_or_fetch("k", Duration::from_secs(60), || async {
                Err::<u32, _>("down")
            })
            .await;
        assert!(failed.is_err());
        let (value, source) = cache
            .get_or_fetch("k", Duration::from_secs(60), || async { Ok::<_, &str>(3) })
            .await
            .unwrap();
        assert_eq!((value, source), (3, Source::Miss));
    }

    #[tokio::test]
    async fn a_full_table_stops_growing_and_still_answers() {
        let cache = TtlCache::<usize>::default();
        let long = Duration::from_secs(60);
        for n in 0..MAX_SLOTS + 50 {
            let (value, _) = cache
                .get_or_fetch(&format!("addr{n}"), long, || async { Ok::<_, ()>(n) })
                .await
                .unwrap();
            assert_eq!(value, n);
        }
        assert_eq!(cache.cached_keys().await, MAX_SLOTS);
        // A key cached before the table filled is still served from cache.
        let (_, source) = cache
            .get_or_fetch("addr0", long, || async { Ok::<_, ()>(0) })
            .await
            .unwrap();
        assert_eq!(source, Source::Hit);
        // Once entries expire, the next new key sweeps them and is cached.
        let (_, source) = cache
            .get_or_fetch("fresh", Duration::ZERO, || async { Ok::<_, ()>(1) })
            .await
            .unwrap();
        assert_eq!(source, Source::Miss);
        assert_eq!(cache.cached_keys().await, 1);
    }

    #[tokio::test]
    async fn expired_value_is_refetched() {
        let cache = TtlCache::<u32>::default();
        cache
            .get_or_fetch("k", Duration::ZERO, || async { Ok::<_, ()>(1) })
            .await
            .unwrap();
        let (value, source) = cache
            .get_or_fetch("k", Duration::ZERO, || async { Ok::<_, ()>(2) })
            .await
            .unwrap();
        assert_eq!((value, source), (2, Source::Miss));
    }
}
