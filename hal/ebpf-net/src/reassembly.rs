//! Chunks back into a block, within fixed memory.
//!
//! Every chunk reaching here has already opened under a peer's key, so the
//! sender is known and cannot be impersonated. That changes what a bad chunk
//! means. Two chunks at one index with different bytes, or two chunks of one
//! block disagreeing about its length, are *misbehaviour* by that peer — there
//! is no honest way to produce either. Running out of room is not: it is
//! refused, counted, and blames nobody, because an honest peer relaying a big
//! block while others do the same is not an offence.
//!
//! # Bounds
//!
//! - A sender holds at most [`Limits::pending_per_sender`] incomplete blocks and
//!   [`Limits::pending_bytes_per_sender`] of reassembly memory.
//! - All senders together hold at most [`Limits::pending_bytes`].
//! - An incomplete block is discarded after [`Limits::timeout`].
//!
//! Memory is committed when a block's first chunk arrives, at the length its
//! header declares, and that length is capped by the header itself at
//! `MAX_BODY_LEN`. A peer that declares large blocks and never finishes them
//! occupies its own allowance until the timeout and no more.
//!
//! # Per sender, deliberately
//!
//! Reassembly is keyed by `(sender, block_id)`, and a block is remembered as
//! delivered per sender too. Remembering it across senders would let one peer
//! relay a garbage body under a real block id and so suppress every honest
//! relay of that block: the garbage is only found out later, when the node
//! checks the body against `tx_root`. Deduplication across senders belongs
//! after that check, which is the node's.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::time::{Duration, Instant};

use maya_ebpf_net_common::header::RelayHeader;

/// Memory and time bounds on reassembly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Incomplete blocks one sender may have in flight.
    pub pending_per_sender: usize,
    /// Reassembly bytes one sender may occupy.
    pub pending_bytes_per_sender: usize,
    /// Reassembly bytes all senders together may occupy.
    pub pending_bytes: usize,
    /// How long an incomplete block is kept.
    pub timeout: Duration,
    /// Completed `(sender, block)` pairs remembered, so late duplicate chunks
    /// do not start a second copy.
    pub remembered: usize,
}

impl Limits {
    /// Room for two maximal blocks per sender, and sixteen senders' worth of
    /// that at once. A block interval is fifteen seconds; ten seconds of
    /// patience for the rest of a block is already generous for chunks sent in
    /// one burst.
    pub const DEFAULT: Self = Self {
        pending_per_sender: 4,
        pending_bytes_per_sender: 16 * 1024 * 1024,
        pending_bytes: 256 * 1024 * 1024,
        timeout: Duration::from_secs(10),
        remembered: 1_024,
    };
}

/// What a chunk did.
#[derive(Debug, PartialEq, Eq)]
pub enum Absorbed {
    /// Stored; the block is not complete yet.
    Partial {
        /// Chunks held.
        received: u16,
        /// Chunks needed.
        of: u16,
    },
    /// The same bytes at an index already held. Normal after a resend.
    Duplicate,
    /// A chunk of a block this sender already delivered.
    AlreadyDelivered,
    /// The last chunk: here is the body.
    Complete(Vec<u8>),
}

/// A chunk refused for want of room. Blames nobody.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// The sender already has its allowance of blocks or bytes in flight.
    #[error("the sender's reassembly allowance is full")]
    SenderBusy,
    /// All senders together have filled reassembly memory.
    #[error("reassembly memory is full")]
    MemoryFull,
}

/// Something an authenticated sender cannot do honestly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Misbehaviour {
    /// Two chunks at one index carry different bytes.
    #[error("two chunks at one index carry different bytes")]
    ConflictingChunk,
    /// Chunks of one block disagree about its length.
    #[error("chunks of one block disagree about its length")]
    InconsistentLength,
}

/// Why a chunk was not absorbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbsorbError {
    /// No room. Blames nobody.
    Refused(Refused),
    /// The sender misbehaved.
    Misbehaviour(Misbehaviour),
}

#[derive(Debug)]
struct Pending {
    body: Vec<u8>,
    held: Vec<bool>,
    received: u16,
    started: Instant,
}

#[derive(Debug, Default)]
struct Usage {
    blocks: usize,
    bytes: usize,
}

/// Reassembles blocks from authenticated chunks.
#[derive(Debug)]
pub struct Reassembler<S> {
    limits: Limits,
    pending: HashMap<(S, [u8; 32]), Pending>,
    usage: HashMap<S, Usage>,
    bytes: usize,
    delivered: HashSet<(S, [u8; 32])>,
    delivered_order: VecDeque<(S, [u8; 32])>,
}

impl<S: Copy + Eq + Hash> Reassembler<S> {
    /// An empty reassembler.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            pending: HashMap::new(),
            usage: HashMap::new(),
            bytes: 0,
            delivered: HashSet::new(),
            delivered_order: VecDeque::new(),
        }
    }

    /// Bytes committed to incomplete blocks.
    #[must_use]
    pub const fn pending_bytes(&self) -> usize {
        self.bytes
    }

    /// Incomplete blocks held.
    #[must_use]
    pub fn pending_blocks(&self) -> usize {
        self.pending.len()
    }

    /// Absorbs one opened chunk from `sender`.
    ///
    /// # Errors
    ///
    /// [`AbsorbError::Refused`] when out of room, and
    /// [`AbsorbError::Misbehaviour`] when the chunk contradicts one already
    /// held. A contradicted block is discarded: nothing from that sender for
    /// that block can be trusted to assemble into anything.
    pub fn absorb(
        &mut self,
        sender: S,
        header: &RelayHeader,
        plaintext: &[u8],
        now: Instant,
    ) -> Result<Absorbed, AbsorbError> {
        let key = (sender, *header.block_id());
        if self.delivered.contains(&key) {
            return Ok(Absorbed::AlreadyDelivered);
        }
        if !self.pending.contains_key(&key) {
            self.start(key, header, now)?;
        }
        let Some(pending) = self.pending.get_mut(&key) else {
            return Err(AbsorbError::Refused(Refused::MemoryFull));
        };

        let start = header.body_offset();
        let range = start..start + plaintext.len();
        let slot = usize::from(header.chunk_index());
        let fits = pending.body.len() == header.body_len() as usize
            && plaintext.len() == header.plaintext_len()
            && slot < pending.held.len()
            && range.end <= pending.body.len();
        if !fits {
            self.discard(&key);
            return Err(AbsorbError::Misbehaviour(Misbehaviour::InconsistentLength));
        }

        if pending.held[slot] {
            if pending.body[range] == *plaintext {
                return Ok(Absorbed::Duplicate);
            }
            self.discard(&key);
            return Err(AbsorbError::Misbehaviour(Misbehaviour::ConflictingChunk));
        }
        pending.body[range].copy_from_slice(plaintext);
        pending.held[slot] = true;
        pending.received += 1;
        if pending.received < header.chunk_count() {
            return Ok(Absorbed::Partial {
                received: pending.received,
                of: header.chunk_count(),
            });
        }

        let body = self.discard(&key).map(|done| done.body).unwrap_or_default();
        self.remember(key);
        Ok(Absorbed::Complete(body))
    }

    /// Discards incomplete blocks older than the timeout. Returns how many.
    pub fn expire(&mut self, now: Instant) -> usize {
        let timeout = self.limits.timeout;
        let stale: Vec<(S, [u8; 32])> = self
            .pending
            .iter()
            .filter(|(_, pending)| now.saturating_duration_since(pending.started) >= timeout)
            .map(|(key, _)| *key)
            .collect();
        for key in &stale {
            self.discard(key);
        }
        stale.len()
    }

    fn start(
        &mut self,
        key: (S, [u8; 32]),
        header: &RelayHeader,
        now: Instant,
    ) -> Result<(), AbsorbError> {
        let len = header.body_len() as usize;
        let usage = self.usage.get(&key.0);
        let (blocks, bytes) = usage.map_or((0, 0), |u| (u.blocks, u.bytes));
        if blocks >= self.limits.pending_per_sender
            || bytes + len > self.limits.pending_bytes_per_sender
        {
            return Err(AbsorbError::Refused(Refused::SenderBusy));
        }
        if self.bytes + len > self.limits.pending_bytes {
            return Err(AbsorbError::Refused(Refused::MemoryFull));
        }
        let usage = self.usage.entry(key.0).or_default();
        usage.blocks += 1;
        usage.bytes += len;
        self.bytes += len;
        self.pending.insert(
            key,
            Pending {
                body: vec![0; len],
                held: vec![false; usize::from(header.chunk_count())],
                received: 0,
                started: now,
            },
        );
        Ok(())
    }

    fn discard(&mut self, key: &(S, [u8; 32])) -> Option<Pending> {
        let pending = self.pending.remove(key)?;
        let len = pending.body.len();
        self.bytes -= len;
        if let Some(usage) = self.usage.get_mut(&key.0) {
            usage.blocks -= 1;
            usage.bytes -= len;
            if usage.blocks == 0 {
                self.usage.remove(&key.0);
            }
        }
        Some(pending)
    }

    fn remember(&mut self, key: (S, [u8; 32])) {
        if self.limits.remembered == 0 {
            return;
        }
        if self.delivered.insert(key) {
            self.delivered_order.push_back(key);
        }
        while self.delivered_order.len() > self.limits.remembered {
            if let Some(oldest) = self.delivered_order.pop_front() {
                self.delivered.remove(&oldest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use maya_ebpf_net_common::header::CHUNK_LEN;

    use super::*;

    const BLOCK: [u8; 32] = [6; 32];

    fn chunks(body: &[u8]) -> Vec<(RelayHeader, Vec<u8>)> {
        let len = body.len() as u32;
        let first = RelayHeader::new(0, BLOCK, 0, len).unwrap();
        (0..first.chunk_count())
            .map(|i| {
                let header = RelayHeader::new(0, BLOCK, i, len).unwrap();
                let start = header.body_offset();
                (header, body[start..start + header.plaintext_len()].to_vec())
            })
            .collect()
    }

    #[test]
    fn chunks_in_any_order_with_duplicates_reassemble_exactly_once() {
        let body: Vec<u8> = (0..CHUNK_LEN * 3 + 5).map(|i| i as u8).collect();
        let mut parts = chunks(&body);
        parts.reverse();
        let mut reassembler = Reassembler::new(Limits::DEFAULT);
        let now = Instant::now();
        let mut outcomes = Vec::new();
        for (header, plain) in parts.iter().chain(parts.iter().take(1)) {
            outcomes.push(reassembler.absorb(1u8, header, plain, now).unwrap());
        }
        assert!(matches!(
            outcomes[0],
            Absorbed::Partial { received: 1, of: 4 }
        ));
        assert_eq!(outcomes[3], Absorbed::Complete(body));
        assert_eq!(outcomes[4], Absorbed::AlreadyDelivered);
        assert_eq!(reassembler.pending_bytes(), 0);
    }

    #[test]
    fn a_conflicting_chunk_is_misbehaviour_and_discards_the_block() {
        let body = vec![1u8; CHUNK_LEN + 1];
        let parts = chunks(&body);
        let mut reassembler = Reassembler::new(Limits::DEFAULT);
        let now = Instant::now();
        let (header, plain) = &parts[0];
        reassembler.absorb(1u8, header, plain, now).unwrap();
        assert_eq!(
            reassembler.absorb(1u8, header, plain, now),
            Ok(Absorbed::Duplicate)
        );
        let mut other = plain.clone();
        other[0] ^= 1;
        assert_eq!(
            reassembler.absorb(1u8, header, &other, now),
            Err(AbsorbError::Misbehaviour(Misbehaviour::ConflictingChunk))
        );
        assert_eq!(reassembler.pending_blocks(), 0);
    }

    #[test]
    fn chunks_disagreeing_about_the_body_length_are_misbehaviour() {
        let mut reassembler = Reassembler::new(Limits::DEFAULT);
        let now = Instant::now();
        let long = RelayHeader::new(0, BLOCK, 0, (CHUNK_LEN * 2) as u32).unwrap();
        reassembler
            .absorb(1u8, &long, &vec![0; CHUNK_LEN], now)
            .unwrap();
        let short = RelayHeader::new(0, BLOCK, 0, (CHUNK_LEN + 1) as u32).unwrap();
        assert_eq!(
            reassembler.absorb(1u8, &short, &vec![0; CHUNK_LEN], now),
            Err(AbsorbError::Misbehaviour(Misbehaviour::InconsistentLength))
        );
        assert_eq!(reassembler.pending_bytes(), 0);
    }

    #[test]
    fn one_senders_allowance_does_not_consume_anothers() {
        let limits = Limits {
            pending_per_sender: 1,
            pending_bytes_per_sender: 1 << 20,
            pending_bytes: 1 << 20,
            ..Limits::DEFAULT
        };
        let mut reassembler = Reassembler::new(limits);
        let now = Instant::now();
        let header =
            |block: u8| RelayHeader::new(0, [block; 32], 0, (CHUNK_LEN * 2) as u32).unwrap();
        let plain = vec![0u8; CHUNK_LEN];
        reassembler.absorb(1u8, &header(1), &plain, now).unwrap();
        assert_eq!(
            reassembler.absorb(1u8, &header(2), &plain, now),
            Err(AbsorbError::Refused(Refused::SenderBusy))
        );
        assert!(reassembler.absorb(2u8, &header(2), &plain, now).is_ok());
    }

    #[test]
    fn total_memory_is_bounded_across_senders() {
        let len = (CHUNK_LEN * 2) as u32;
        let limits = Limits {
            pending_bytes: len as usize,
            ..Limits::DEFAULT
        };
        let mut reassembler = Reassembler::new(limits);
        let now = Instant::now();
        let header = RelayHeader::new(0, BLOCK, 0, len).unwrap();
        let plain = vec![0u8; CHUNK_LEN];
        reassembler.absorb(1u8, &header, &plain, now).unwrap();
        assert_eq!(
            reassembler.absorb(2u8, &header, &plain, now),
            Err(AbsorbError::Refused(Refused::MemoryFull))
        );
    }

    #[test]
    fn an_incomplete_block_expires_and_releases_its_memory() {
        let mut reassembler = Reassembler::new(Limits::DEFAULT);
        let start = Instant::now();
        let header = RelayHeader::new(0, BLOCK, 0, (CHUNK_LEN * 2) as u32).unwrap();
        reassembler
            .absorb(1u8, &header, &vec![0; CHUNK_LEN], start)
            .unwrap();
        assert_eq!(reassembler.expire(start + Duration::from_secs(9)), 0);
        assert_eq!(reassembler.expire(start + Duration::from_secs(10)), 1);
        assert_eq!(reassembler.pending_bytes(), 0);
    }

    #[test]
    fn the_delivered_memory_is_bounded() {
        let limits = Limits {
            remembered: 2,
            ..Limits::DEFAULT
        };
        let mut reassembler = Reassembler::new(limits);
        let now = Instant::now();
        for block in 0..3u8 {
            let header = RelayHeader::new(0, [block; 32], 0, 1).unwrap();
            assert_eq!(
                reassembler.absorb(1u8, &header, &[block], now),
                Ok(Absorbed::Complete(vec![block]))
            );
        }
        let oldest = RelayHeader::new(0, [0; 32], 0, 1).unwrap();
        assert_eq!(
            reassembler.absorb(1u8, &oldest, &[0], now),
            Ok(Absorbed::Complete(vec![0])),
            "forgotten after two newer deliveries"
        );
        let newest = RelayHeader::new(0, [2; 32], 0, 1).unwrap();
        assert_eq!(
            reassembler.absorb(1u8, &newest, &[2], now),
            Ok(Absorbed::AlreadyDelivered)
        );
    }
}
