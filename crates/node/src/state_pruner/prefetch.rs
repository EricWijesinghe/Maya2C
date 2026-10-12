//! Fetching a run of blocks with several requests in flight.
//!
//! A joining node used to download every block body one request at a time:
//! about 0.35 s a round trip through the public bootstrap endpoint, so the
//! 3,600-7,200 bodies above a snapshot took 20-40 minutes before the node
//! showed a single block (measured 2026-10-12 on three CI runners and two
//! local installs). The time is round trips, not bandwidth or the endpoint's
//! per-client limit, so a few requests in flight cut it by about that factor.
//!
//! Only the *fetching* is concurrent. Results come back in height order and
//! every caller still links and validates them one by one exactly as before,
//! so a source that answers out of order or lies gains nothing from this.

use crate::error::Result;

/// Requests in flight at once. The public bootstrap endpoint allows 10 a
/// second per client with a burst of 20; four ~0.35 s round trips in flight
/// stay just under that, and a 429 is retried with backoff anyway.
pub const PREFETCH_WORKERS: usize = 4;

/// Heights fetched before the caller consumes them. Bounds memory: a window
/// is held whole, and the next one starts only after the caller is done.
pub const PREFETCH_WINDOW: u64 = 64;

/// Fetches `from..=to` with up to `workers` calls of `fetch` running at once
/// and returns the results in height order. The first error, in height
/// order, is returned and the rest of the run is discarded.
///
/// `fetch` runs on scoped OS threads, so it may block (the RPC sources do,
/// through their runtime handle) and must not need the caller's thread.
///
/// # Errors
///
/// Whatever `fetch` returns for the lowest failing height.
pub fn fetch_ordered<T, F>(from: u64, to: u64, workers: usize, fetch: &F) -> Result<Vec<T>>
where
    T: Send,
    F: Fn(u64) -> Result<T> + Sync,
{
    if to < from {
        return Ok(Vec::new());
    }
    let count = usize::try_from(to - from + 1).unwrap_or(usize::MAX);
    let workers = workers.clamp(1, count);
    if workers == 1 {
        return (from..=to).map(fetch).collect();
    }
    // Worker `w` takes heights from + w, from + w + workers, ...; each keeps
    // its own results, so nothing is shared but the closure.
    let lanes: Vec<Vec<(u64, Result<T>)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|w| {
                scope.spawn(move || {
                    (from + w as u64..=to)
                        .step_by(workers)
                        .map(|h| (h, fetch(h)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    let mut all: Vec<(u64, Result<T>)> = lanes.into_iter().flatten().collect();
    all.sort_unstable_by_key(|(h, _)| *h);
    all.into_iter().map(|(_, r)| r).collect()
}

/// [`fetch_ordered`] over `from..=to` in windows of [`PREFETCH_WINDOW`],
/// handing each result to `consume` in height order before the next window
/// is fetched.
///
/// # Errors
///
/// The first error from `fetch` or `consume`, in height order.
pub fn for_each_prefetched<T, F, C>(from: u64, to: u64, fetch: &F, mut consume: C) -> Result<()>
where
    T: Send,
    F: Fn(u64) -> Result<T> + Sync,
    C: FnMut(T) -> Result<()>,
{
    let mut start = from;
    while start <= to {
        let end = to.min(start.saturating_add(PREFETCH_WINDOW - 1));
        for item in fetch_ordered(start, end, PREFETCH_WORKERS, fetch)? {
            consume(item)?;
        }
        let Some(next) = end.checked_add(1) else {
            break;
        };
        start = next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::NodeError;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn results_come_back_in_height_order() {
        let got = fetch_ordered(10, 40, 4, &|h| Ok(h * 2)).expect("fetch");
        assert_eq!(got, (10..=40).map(|h| h * 2).collect::<Vec<_>>());
    }

    #[test]
    fn every_height_is_fetched_exactly_once() {
        let asked = Mutex::new(Vec::new());
        fetch_ordered(1, 100, 4, &|h| {
            asked.lock().expect("lock").push(h);
            Ok(())
        })
        .expect("fetch");
        let mut asked = asked.into_inner().expect("lock");
        asked.sort_unstable();
        assert_eq!(asked, (1..=100).collect::<Vec<_>>());
    }

    #[test]
    fn requests_really_overlap() {
        let in_flight = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        fetch_ordered(1, 16, 4, &|_| {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(20));
            in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        })
        .expect("fetch");
        assert!(peak.load(Ordering::SeqCst) > 1, "fetches ran one at a time");
    }

    #[test]
    fn the_lowest_failing_height_is_reported() {
        let result = fetch_ordered(1, 50, 4, &|h| {
            if h == 17 || h == 33 {
                Err(NodeError::Network(format!("no block {h}")))
            } else {
                Ok(h)
            }
        });
        let Err(NodeError::Network(message)) = result else {
            panic!("expected a network error, got {result:?}");
        };
        assert_eq!(message, "no block 17");
    }

    #[test]
    fn empty_and_single_ranges() {
        assert!(
            fetch_ordered(5, 4, 4, &|h| Ok(h))
                .expect("empty")
                .is_empty()
        );
        assert_eq!(fetch_ordered(7, 7, 4, &|h| Ok(h)).expect("one"), vec![7]);
    }

    #[test]
    fn windows_are_consumed_in_order_and_stop_on_error() {
        let mut seen = Vec::new();
        for_each_prefetched(1, 200, &|h| Ok(h), |h| {
            seen.push(h);
            Ok(())
        })
        .expect("all");
        assert_eq!(seen, (1..=200).collect::<Vec<_>>());

        let mut consumed = 0;
        let result = for_each_prefetched(1, 200, &|h| Ok(h), |h| {
            consumed += 1;
            if h == 70 {
                Err(NodeError::Network("bad".into()))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(consumed, 70, "nothing after the failing block is consumed");
    }

    #[test]
    fn a_range_ending_at_u64_max_terminates() {
        let mut seen = 0;
        for_each_prefetched(u64::MAX - 2, u64::MAX, &|h| Ok(h), |_| {
            seen += 1;
            Ok(())
        })
        .expect("fetch");
        assert_eq!(seen, 3);
    }
}
