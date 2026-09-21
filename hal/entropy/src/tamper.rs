//! The tamper line: one event wipes every registered key store.
//!
//! Master Prompt 8's firmware guard calls [`TamperLine::trip`] when an
//! enclosure switch, a voltage glitch detector or a mesh break fires. Tripping
//! zeroizes every store registered with the line, latches, and from then on
//! the entropy pool refuses to produce output. It never un-trips: recovery
//! from a physical tamper is a new device, not a reset.
//!
//! Stores are held weakly, so registering one does not keep it alive after
//! its owner drops it (at which point `ZeroizeOnDrop` has already wiped it).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use zeroize::Zeroize;

type Store = Weak<Mutex<dyn Zeroize + Send>>;

#[derive(Default)]
struct Inner {
    tripped: AtomicBool,
    stores: Mutex<Vec<Store>>,
}

/// A shared tamper line. Clones observe and trip the same line.
#[derive(Clone, Default)]
pub struct TamperLine {
    inner: Arc<Inner>,
}

impl core::fmt::Debug for TamperLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TamperLine")
            .field("tripped", &self.is_tripped())
            .finish()
    }
}

impl TamperLine {
    /// A fresh, untripped line.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a key store to be wiped on a trip. Registering after a trip
    /// wipes the store immediately: a tampered device must not accept keys.
    pub fn register<Z: Zeroize + Send + 'static>(&self, store: &Arc<Mutex<Z>>) {
        let dynamic: Arc<Mutex<dyn Zeroize + Send>> = store.clone();
        if self.is_tripped() {
            wipe(&dynamic);
            return;
        }
        self.inner
            .stores
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::downgrade(&dynamic));
    }

    /// Whether the line has tripped.
    #[must_use]
    pub fn is_tripped(&self) -> bool {
        self.inner.tripped.load(Ordering::SeqCst)
    }

    /// Trips the line: latch first, then wipe every live store. Returns how
    /// many stores were wiped.
    ///
    /// A poisoned store lock is wiped anyway — the panic that poisoned it is
    /// no reason to leave its keys in memory.
    pub fn trip(&self) -> usize {
        self.inner.tripped.store(true, Ordering::SeqCst);
        tracing::error!("tamper line tripped: zeroizing registered key stores");
        let stores = std::mem::take(
            &mut *self
                .inner
                .stores
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        stores
            .iter()
            .filter_map(Weak::upgrade)
            .map(|s| wipe(&s))
            .count()
    }
}

fn wipe(store: &Arc<Mutex<dyn Zeroize + Send>>) {
    store
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .zeroize();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trip_wipes_every_registered_store_and_latches() {
        let line = TamperLine::new();
        let a = Arc::new(Mutex::new(vec![0xAAu8; 32]));
        let b = Arc::new(Mutex::new([0xBBu8; 64]));
        line.register(&a);
        line.register(&b);

        assert_eq!(line.clone().trip(), 2);
        assert!(line.is_tripped());
        assert!(a.lock().expect("lock").is_empty(), "Vec zeroize clears it");
        assert!(b.lock().expect("lock").iter().all(|&x| x == 0));
    }

    #[test]
    fn a_store_registered_after_the_trip_is_wiped_at_once() {
        let line = TamperLine::new();
        line.trip();
        let late = Arc::new(Mutex::new([0xCCu8; 16]));
        line.register(&late);
        assert!(late.lock().expect("lock").iter().all(|&x| x == 0));
    }

    #[test]
    fn a_dropped_store_is_skipped_not_resurrected() {
        let line = TamperLine::new();
        {
            let gone = Arc::new(Mutex::new([1u8; 8]));
            line.register(&gone);
        }
        assert_eq!(line.trip(), 0);
    }

    #[test]
    fn a_poisoned_store_is_still_wiped() {
        let line = TamperLine::new();
        let store = Arc::new(Mutex::new([0xDDu8; 16]));
        line.register(&store);
        let clone = Arc::clone(&store);
        let _ = std::thread::spawn(move || {
            let _guard = clone.lock().expect("lock");
            panic!("poison the lock");
        })
        .join();
        assert_eq!(line.trip(), 1);
        let wiped = store.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(wiped.iter().all(|&x| x == 0));
    }
}
