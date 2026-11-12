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

/// Entries above this count trigger a sweep of expired ones, so a stream of
/// distinct addresses cannot grow the table without bound.
const SWEEP_AT: usize = 1024;

/// One key's value and when it was fetched. Its own lock is the single-flight
/// gate: holders of the lock are the only ones fetching that key.
type Slot<V> = Arc<Mutex<Option<(Instant, V)>>>;

/// A cache of `V` by string key, each entry valid for a fixed time.
pub struct TtlCache<V> {
    slots: Mutex<HashMap<String, Slot<V>>>,
}

impl<V: Clone + Send> Default for TtlCache<V> {
    fn default() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
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

    /// The slot for `key`, created if absent. Sweeps expired slots when the
    /// table is large; a slot being fetched right now is never swept.
    async fn slot(&self, key: &str, ttl: Duration) -> Slot<V> {
        let mut slots = self.slots.lock().await;
        if slots.len() >= SWEEP_AT && !slots.contains_key(key) {
            slots.retain(|_, slot| {
                slot.try_lock().map_or(true, |entry| {
                    entry.as_ref().is_some_and(|(at, _)| at.elapsed() < ttl)
                })
            });
        }
        Arc::clone(slots.entry(key.to_string()).or_default())
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
