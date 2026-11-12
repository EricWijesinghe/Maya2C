//! A value recomputed only when the chain tip moves, so an RPC that reads
//! committed state (`get_next_upgrade`) stays cheap under load.
//!
//! Keyed on the tip, not the height alone: a proof-of-work reorg can replace
//! the block at the same height, and with it the schedule.

use std::sync::{Mutex, PoisonError};

/// The last `(key, value)` computed.
#[derive(Debug)]
pub struct TipCache<K, V> {
    slot: Mutex<Option<(K, V)>>,
}

impl<K, V> Default for TipCache<K, V> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }
}

impl<K: PartialEq, V: Clone> TipCache<K, V> {
    /// The value for `key`: the cached one when `key` matches the last call,
    /// otherwise `compute`'s, which then replaces it. An error is not cached.
    ///
    /// # Errors
    ///
    /// Whatever `compute` returns.
    pub fn get_or_compute<E>(
        &self,
        key: K,
        compute: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E> {
        // A panic elsewhere cannot leave a half-written entry: the slot is
        // only ever replaced whole.
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((cached, value)) = slot.as_ref()
            && *cached == key
        {
            return Ok(value.clone());
        }
        let value = compute()?;
        *slot = Some((key, value.clone()));
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::TipCache;
    use std::cell::Cell;

    #[test]
    fn a_second_call_at_the_same_tip_does_not_recompute() {
        let cache = TipCache::<(u64, [u8; 32]), u32>::default();
        let computed = Cell::new(0);
        let count = || {
            computed.set(computed.get() + 1);
            Ok::<_, ()>(computed.get())
        };
        assert_eq!(cache.get_or_compute((7, [1; 32]), count), Ok(1));
        assert_eq!(cache.get_or_compute((7, [1; 32]), count), Ok(1));
        assert_eq!(computed.get(), 1, "same tip: cached");
        assert_eq!(cache.get_or_compute((8, [2; 32]), count), Ok(2));
        // A reorg to another block at the same height recomputes too.
        assert_eq!(cache.get_or_compute((8, [3; 32]), count), Ok(3));
    }

    #[test]
    fn an_error_is_not_cached() {
        let cache = TipCache::<u64, u32>::default();
        assert_eq!(cache.get_or_compute(1, || Err("db")), Err("db"));
        assert_eq!(cache.get_or_compute(1, || Ok::<_, &str>(5)), Ok(5));
    }
}
