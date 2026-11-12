//! Per-client request budget for the HTTP registry.

use std::collections::BTreeMap;

/// Fixed-window counter per client key.
/// shortcut: keys are never evicted, so memory grows with distinct clients per process; bound it with an LRU before any public exposure.
#[derive(Debug)]
pub struct FixedWindow {
    window_secs: u64,
    max: u32,
    /// key -> (window start, requests admitted in that window)
    counts: BTreeMap<String, (u64, u32)>,
}

impl FixedWindow {
    pub fn new(window_secs: u64, max: u32) -> Self {
        Self {
            window_secs: window_secs.max(1),
            max,
            counts: BTreeMap::new(),
        }
    }

    /// Counts one request from `key` at `now` (Unix seconds). False when over budget.
    pub fn admit(&mut self, key: &str, now: u64) -> bool {
        let start = now - now % self.window_secs;
        let entry = self.counts.entry(key.to_owned()).or_insert((start, 0));
        if entry.0 != start {
            *entry = (start, 0);
        }
        if entry.1 >= self.max {
            return false;
        }
        entry.1 += 1;
        true
    }
}
