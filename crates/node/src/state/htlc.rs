//! Lattice HTLC records in the state, and their place in the root.
//!
//! Invariant 25: one prefix in `RECORD_LAYERS`, one
//! [`StateLayer`](crate::state::proof::StateLayer), and the undo journal covers
//! everything written through `put_record`. The layer folds only when
//! non-empty, so a chain with no locks keeps the root it had before HTLC-L.
//!
//! | Prefix | Holds |
//! |---|---|
//! | `h:lk:<lock id>` | one [`LockRecord`], including the opening once claimed |
//!
//! One record per lock rather than a record plus a revelation index: a lock
//! changes exactly once after it is written, and the watcher always knows the
//! lock id it is waiting on.

use maya_htlc_lattice::LockRecord;

use crate::core::htlc_payload::LockId;
use crate::error::{NodeError, Result};
use crate::state::db::{Overlay, StateDB};

/// Prefix shared by every record this subsystem owns.
pub const HTLC_PREFIX: &[u8] = b"h:";

/// Prefix for a lock.
pub(crate) const LOCK_PREFIX: &[u8] = b"h:lk:";

// A sub-prefix outside the one the fold knows about is state outside the root.
const _: () = assert!(LOCK_PREFIX[0] == HTLC_PREFIX[0] && LOCK_PREFIX[1] == HTLC_PREFIX[1]);

/// Domain for deriving a lock id.
const LOCK_ID_DOMAIN: &str = "maya2c htlc-l lock id v1";

/// Storage key for a lock.
#[must_use]
pub fn lock_key(id: &LockId) -> Vec<u8> {
    let mut key = Vec::with_capacity(LOCK_PREFIX.len() + id.len());
    key.extend_from_slice(LOCK_PREFIX);
    key.extend_from_slice(id);
    key
}

/// The id a lock from `sender` at `nonce` receives.
///
/// Derived rather than chosen, so a sender cannot pick an id that collides with
/// somebody else's lock, and computable before submission, so the counterparty
/// can be told which lock to check before it locks anything of its own.
#[must_use]
pub fn derive_lock_id(sender: &[u8; 32], nonce: u64) -> LockId {
    let mut hasher = blake3::Hasher::new_derive_key(LOCK_ID_DOMAIN);
    hasher.update(sender);
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}

impl StateDB {
    /// A lock, through the overlay.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub(crate) fn htlc_record(&self, overlay: &Overlay, id: &LockId) -> Result<Option<LockRecord>> {
        self.record(overlay, &lock_key(id))?
            .map(|bytes| decode(&bytes))
            .transpose()
    }

    /// A lock from committed state alone.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub fn stored_htlc_lock(&self, id: &LockId) -> Result<Option<LockRecord>> {
        self.raw_get(&lock_key(id))?
            .map(|bytes| decode(&bytes))
            .transpose()
    }
}

/// Decodes a stored lock.
pub(crate) fn decode(bytes: &[u8]) -> Result<LockRecord> {
    LockRecord::decode(bytes).map_err(|error| NodeError::Decode(format!("htlc: {error}")))
}
