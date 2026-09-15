//! The key book, and what happens to one relay datagram in user space.
//!
//! ```text
//! datagram ─► RelayHeader::split ─► key book[key_id] ─► open ─► reassemble
//!                  │ malformed           │ unknown        │ forged    │ refused
//!                  └──────────────── dropped, nobody blamed ──────────┘
//!                                                           │ conflict
//!                                                           └─► Misbehaved { peer }
//! ```
//!
//! A key id is public — it rides in every header — so a datagram that names a
//! real key and then fails authentication proves nothing about the key's
//! owner. Only what happens *after* a chunk opens is attributable.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use maya_ebpf_net_common::HeaderError;
use maya_ebpf_net_common::header::{CHUNK_LEN, RelayHeader};

use crate::reassembly::{AbsorbError, Absorbed, Limits, Misbehaviour, Reassembler, Refused};
use crate::seal::RelayKey;

/// How often [`RelayReceiver::ingest`] discards expired reassemblies.
const SWEEP_INTERVAL: Duration = Duration::from_secs(1);

/// Inbound relay keys, by key id.
#[derive(Debug)]
pub struct KeyBook<P> {
    keys: HashMap<u64, (P, Arc<RelayKey>)>,
}

impl<P> Default for KeyBook<P> {
    fn default() -> Self {
        Self {
            keys: HashMap::new(),
        }
    }
}

/// A key book shared between the node, which fills it, and receive workers.
pub type SharedKeyBook<P> = Arc<RwLock<KeyBook<P>>>;

impl<P: Copy + Eq> KeyBook<P> {
    /// An empty book.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty book, shared.
    #[must_use]
    pub fn shared() -> SharedKeyBook<P> {
        Arc::new(RwLock::new(Self::new()))
    }

    /// Records `peer`'s inbound key. A peer may hold several — one per
    /// connection — and each has its own id.
    pub fn insert(&mut self, peer: P, key: RelayKey) -> u64 {
        let id = key.id();
        self.keys.insert(id, (peer, Arc::new(key)));
        id
    }

    /// Forgets every key belonging to `peer`.
    pub fn remove_peer(&mut self, peer: P) {
        self.keys.retain(|_, (owner, _)| *owner != peer);
    }

    /// The owner and key for `id`.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<(P, Arc<RelayKey>)> {
        self.keys.get(&id).map(|(peer, key)| (*peer, Arc::clone(key)))
    }

    /// Keys held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether no keys are held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// What a datagram produced.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayEvent<P> {
    /// A whole block body from `peer`. Not yet validated as a block.
    Block {
        /// The authenticated sender.
        peer: P,
        /// The id its headers named. The node recomputes it from the body.
        block_id: [u8; 32],
        /// The block's bytes.
        body: Vec<u8>,
    },
    /// `peer` sent something it could not have sent honestly.
    Misbehaved {
        /// The authenticated sender.
        peer: P,
        /// What it did.
        what: Misbehaviour,
    },
}

/// Why a datagram produced nothing. None of these blame anyone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// Structurally invalid. Normally the kernel drops these first.
    Header(HeaderError),
    /// Names no key this node holds.
    UnknownKey,
    /// Failed authentication.
    Forged,
    /// No room to reassemble.
    Refused(Refused),
}

/// Running counts, for metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReceiverStats {
    /// Chunks that opened and were stored.
    pub accepted: u64,
    /// Chunks already held, or of blocks already delivered.
    pub duplicates: u64,
    /// Structurally invalid datagrams.
    pub malformed: u64,
    /// Datagrams naming no known key.
    pub unknown_key: u64,
    /// Datagrams failing authentication.
    pub forged: u64,
    /// Chunks refused for want of room.
    pub refused: u64,
    /// Blocks delivered.
    pub delivered: u64,
    /// Misbehaviour reported.
    pub misbehaved: u64,
    /// Incomplete blocks discarded on timeout.
    pub expired: u64,
}

/// A reassembler several receivers share, so that its memory bounds hold
/// across all of them rather than per receiver.
pub type SharedReassembler<P> = Arc<Mutex<Reassembler<P>>>;

/// Opens and reassembles relay datagrams.
#[derive(Debug)]
pub struct RelayReceiver<P> {
    keys: SharedKeyBook<P>,
    reassembler: SharedReassembler<P>,
    scratch: Box<[u8; CHUNK_LEN]>,
    stats: ReceiverStats,
    last_sweep: Option<Instant>,
}

impl<P: Copy + Eq + Hash> RelayReceiver<P> {
    /// A receiver reading keys from `keys`, with a reassembler of its own.
    #[must_use]
    pub fn new(keys: SharedKeyBook<P>, limits: Limits) -> Self {
        Self::with_reassembler(keys, Arc::new(Mutex::new(Reassembler::new(limits))))
    }

    /// A receiver sharing `reassembler` with others.
    ///
    /// Required wherever several receivers serve one node — one per AF_XDP
    /// queue. A sender can spread its chunks across queues by varying its UDP
    /// source port, and separate reassemblers would give it a separate memory
    /// allowance on each. Opening happens outside the lock; only the copy into
    /// the reassembly buffer is inside it.
    #[must_use]
    pub fn with_reassembler(keys: SharedKeyBook<P>, reassembler: SharedReassembler<P>) -> Self {
        Self {
            keys,
            reassembler,
            scratch: Box::new([0; CHUNK_LEN]),
            stats: ReceiverStats::default(),
            last_sweep: None,
        }
    }

    /// Counts so far.
    #[must_use]
    pub const fn stats(&self) -> ReceiverStats {
        self.stats
    }

    /// Processes one UDP payload.
    ///
    /// # Errors
    ///
    /// [`Dropped`] for a datagram that produced nothing and blames nobody.
    pub fn ingest(
        &mut self,
        datagram: &[u8],
        now: Instant,
    ) -> Result<Option<RelayEvent<P>>, Dropped> {
        self.sweep(now);

        let (header, sealed) = RelayHeader::split(datagram).map_err(|error| {
            self.stats.malformed += 1;
            Dropped::Header(error)
        })?;
        let found = self
            .keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(header.key_id());
        let Some((peer, key)) = found else {
            self.stats.unknown_key += 1;
            return Err(Dropped::UnknownKey);
        };

        let plaintext = &mut self.scratch[..header.plaintext_len()];
        if key.open(&header, sealed, plaintext).is_err() {
            self.stats.forged += 1;
            return Err(Dropped::Forged);
        }

        let absorbed = self
            .reassembler
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .absorb(peer, &header, plaintext, now);
        match absorbed {
            Ok(Absorbed::Partial { .. }) => {
                self.stats.accepted += 1;
                Ok(None)
            }
            Ok(Absorbed::Duplicate | Absorbed::AlreadyDelivered) => {
                self.stats.duplicates += 1;
                Ok(None)
            }
            Ok(Absorbed::Complete(body)) => {
                self.stats.accepted += 1;
                self.stats.delivered += 1;
                Ok(Some(RelayEvent::Block {
                    peer,
                    block_id: *header.block_id(),
                    body,
                }))
            }
            Err(AbsorbError::Refused(refused)) => {
                self.stats.refused += 1;
                Err(Dropped::Refused(refused))
            }
            Err(AbsorbError::Misbehaviour(what)) => {
                self.stats.misbehaved += 1;
                Ok(Some(RelayEvent::Misbehaved { peer, what }))
            }
        }
    }

    fn sweep(&mut self, now: Instant) {
        let due = self
            .last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= SWEEP_INTERVAL);
        if due {
            let expired = self
                .reassembler
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .expire(now);
            self.stats.expired += expired as u64;
            self.last_sweep = Some(now);
        }
    }
}
