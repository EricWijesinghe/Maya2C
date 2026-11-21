//! Block sync: fetching a missing parent from the peer that relayed its child.
//!
//! The one repair a peer can need that proof of work does not already give.
//! A node's state cannot be corrupted by a peer — every block is validated and
//! its state root checked (invariant 24) — but a node that received a child
//! before its parent holds an orphan until the parent arrives, and gossip does
//! not resend history. So the importer asks the relay for the missing ids, and
//! whatever comes back goes through `Chain::insert_block` like any other block:
//! the heavier valid chain wins, and a lie is refused and scored.
//!
//! There are no "consensus proofs" beyond that. A set of honest nodes vouching
//! for a state would need a known committee, and on an open proof-of-work
//! network a vote is free to forge. Work is the proof.
//!
//! # Bounds
//!
//! Requests carry at most [`MAX_BLOCKS_PER_REQUEST`] ids. A response stops
//! adding blocks at [`MAX_RESPONSE_PAYLOAD`] bytes and is refused on the wire
//! past [`MAX_RESPONSE_BYTES`]. Each peer may ask [`REQUESTS_PER_WINDOW`] times
//! per [`REQUEST_WINDOW`]; beyond that it is answered with nothing.
//!
//! Per-peer limits alone do not bound a node: peer identities cost one
//! keygen. So every window also has a budget per peer in bytes
//! ([`PEER_RESPONSE_BYTES_PER_WINDOW`]) and one shared by all peers, in
//! requests ([`GLOBAL_REQUESTS_PER_WINDOW`]) and in bytes
//! ([`GLOBAL_RESPONSE_BYTES_PER_WINDOW`]). Serving runs on the swarm task, so
//! the shared budget is also what bounds the block-store reads a burst of
//! requests can put in front of gossip and BFT frames. A response is cut to
//! what is left of either byte budget; with nothing left it is empty, which a
//! requester reads as "the relay lacks it".

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

use libp2p::{PeerId, StreamProtocol};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::core::Block;
use crate::network::peer_health::Offence;

/// Protocol name.
pub const BLOCK_SYNC_PROTOCOL: StreamProtocol = StreamProtocol::new("/maya/blocks/1.0.0");

/// Ids one request may name.
pub const MAX_BLOCKS_PER_REQUEST: usize = 16;

/// Largest request on the wire: the ids plus CBOR framing.
pub const MAX_REQUEST_BYTES: u64 = 4 * 1024;

/// Block bytes a response stops adding at.
pub const MAX_RESPONSE_PAYLOAD: usize = 24 * 1024 * 1024;

/// Largest response on the wire. Above the payload cap by one gossip-sized
/// block, so a response that stopped at the cap still fits.
pub const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// Requests a peer may make per window.
pub const REQUESTS_PER_WINDOW: u32 = 32;

/// The rate-limit window.
pub const REQUEST_WINDOW: Duration = Duration::from_secs(10);

/// Response bytes one peer may be sent per window: two full responses. An
/// honest ancestor walk asks for one block at a time, so this is far above
/// what it needs, and far below what [`REQUESTS_PER_WINDOW`] full responses
/// would be.
pub const PEER_RESPONSE_BYTES_PER_WINDOW: usize = 2 * MAX_RESPONSE_PAYLOAD;

/// Requests answered per window across all peers. Each costs up to
/// [`MAX_BLOCKS_PER_REQUEST`] block-store reads on the swarm task.
pub const GLOBAL_REQUESTS_PER_WINDOW: u32 = 1_024;

/// Response bytes sent per window across all peers: 256 MiB, about 26 MiB/s.
pub const GLOBAL_RESPONSE_BYTES_PER_WINDOW: usize = 256 * 1024 * 1024;

/// Inbound plus outbound sync streams one connection may hold at once. An
/// honest node runs at most a few ancestor walks, each one request at a time;
/// without a cap one connection could pipeline the request-response default of
/// 100.
pub const MAX_SYNC_STREAMS_PER_CONNECTION: usize = 8;

/// Peers the rate limiter tracks before it forgets the oldest window.
const MAX_LIMITED_PEERS: usize = 4_096;

/// "Send me these blocks."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRequest {
    /// Header ids wanted.
    pub ids: Vec<[u8; 32]>,
}

/// The blocks the responder holds, in no particular order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockResponse {
    /// Encoded blocks.
    pub blocks: Vec<BlockBytes>,
}

impl BlockResponse {
    /// Encoded block bytes carried, the figure the serve budgets count.
    #[must_use]
    pub fn payload_len(&self) -> usize {
        self.blocks.iter().map(|b| b.0.len()).sum()
    }
}

/// An encoded block, serialized as a CBOR byte string rather than an array of
/// integers, which would nearly double its size.
#[derive(Clone, PartialEq, Eq)]
pub struct BlockBytes(pub Vec<u8>);

impl fmt::Debug for BlockBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlockBytes({} bytes)", self.0.len())
    }
}

impl Serialize for BlockBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for BlockBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BytesVisitor;

        impl Visitor<'_> for BytesVisitor {
            type Value = BlockBytes;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a byte string")
            }

            fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<BlockBytes, E> {
                Ok(BlockBytes(bytes.to_vec()))
            }

            fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<BlockBytes, E> {
                Ok(BlockBytes(bytes))
            }
        }

        deserializer.deserialize_byte_buf(BytesVisitor)
    }
}

/// Answers `request` from `lookup`, within the bounds.
///
/// A request over [`MAX_BLOCKS_PER_REQUEST`] is answered with nothing rather
/// than truncated: a truncated answer would look like a responder that lacks
/// the rest. Lookup failures are skipped — a responder's storage trouble is
/// not the requester's.
pub fn serve<F, E>(request: &BlockRequest, lookup: F) -> BlockResponse
where
    F: FnMut(&[u8; 32]) -> Result<Option<Vec<u8>>, E>,
{
    serve_within(request, MAX_RESPONSE_PAYLOAD, lookup)
}

/// [`serve`], stopping at `max_payload` bytes (never above
/// [`MAX_RESPONSE_PAYLOAD`]): what is left of the budgets
/// [`RateLimiter::admit`] returned.
pub fn serve_within<F, E>(
    request: &BlockRequest,
    max_payload: usize,
    mut lookup: F,
) -> BlockResponse
where
    F: FnMut(&[u8; 32]) -> Result<Option<Vec<u8>>, E>,
{
    let max_payload = max_payload.min(MAX_RESPONSE_PAYLOAD);
    let mut blocks = Vec::new();
    if request.ids.len() > MAX_BLOCKS_PER_REQUEST {
        return BlockResponse { blocks };
    }
    let mut payload = 0usize;
    let mut seen = HashSet::new();
    for id in &request.ids {
        if !seen.insert(*id) {
            continue;
        }
        let Ok(Some(bytes)) = lookup(id) else {
            continue;
        };
        if payload.saturating_add(bytes.len()) > max_payload {
            break;
        }
        payload += bytes.len();
        blocks.push(BlockBytes(bytes));
    }
    BlockResponse { blocks }
}

/// Checks a response against the request it answers.
///
/// # Errors
///
/// [`Offence::BadSyncResponse`] for a block that does not decode, one nobody
/// asked for, or the same block twice. Missing blocks are not an error: a
/// responder may simply not have them.
pub fn accept_response(
    requested: &[[u8; 32]],
    response: BlockResponse,
) -> Result<Vec<Block>, Offence> {
    let wanted: HashSet<[u8; 32]> = requested.iter().copied().collect();
    if response.blocks.len() > requested.len() {
        return Err(Offence::BadSyncResponse);
    }
    let mut delivered = HashSet::new();
    let mut blocks = Vec::with_capacity(response.blocks.len());
    for BlockBytes(bytes) in response.blocks {
        let block = Block::from_bytes(&bytes).map_err(|_| Offence::BadSyncResponse)?;
        let id = block.header.id();
        if !wanted.contains(&id) || !delivered.insert(id) {
            return Err(Offence::BadSyncResponse);
        }
        blocks.push(block);
    }
    Ok(blocks)
}

/// Requests and response bytes counted in one fixed window.
#[derive(Clone, Copy, Debug)]
struct Window {
    start: Instant,
    requests: u32,
    bytes: usize,
}

impl Window {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            requests: 0,
            bytes: 0,
        }
    }

    /// Starts a fresh window once this one has run its length.
    fn roll(&mut self, now: Instant) {
        if now.saturating_duration_since(self.start) >= REQUEST_WINDOW {
            *self = Self::new(now);
        }
    }
}

/// Fixed-window request and byte budgets, per peer and across all peers.
///
/// The per-peer budgets bound one identity; the global ones bound them all,
/// because identities are free and the table forgets its oldest window to
/// admit a newcomer.
#[derive(Debug, Default)]
pub struct RateLimiter {
    windows: HashMap<PeerId, Window>,
    global: Option<Window>,
}

impl RateLimiter {
    /// Whether `peer` may make another request at `now`, counting it if so,
    /// and the most response bytes it may then be sent. `None` when a request
    /// budget is spent or nothing is left of a byte budget.
    pub fn admit(&mut self, peer: PeerId, now: Instant) -> Option<usize> {
        if self.windows.len() >= MAX_LIMITED_PEERS && !self.windows.contains_key(&peer) {
            self.windows
                .retain(|_, w| now.saturating_duration_since(w.start) < REQUEST_WINDOW);
            if self.windows.len() >= MAX_LIMITED_PEERS {
                self.forget_oldest();
            }
        }
        let global = self.global.get_or_insert_with(|| Window::new(now));
        global.roll(now);
        let window = self.windows.entry(peer).or_insert_with(|| Window::new(now));
        window.roll(now);
        if window.requests >= REQUESTS_PER_WINDOW || global.requests >= GLOBAL_REQUESTS_PER_WINDOW {
            return None;
        }
        let budget = MAX_RESPONSE_PAYLOAD
            .min(PEER_RESPONSE_BYTES_PER_WINDOW.saturating_sub(window.bytes))
            .min(GLOBAL_RESPONSE_BYTES_PER_WINDOW.saturating_sub(global.bytes));
        if budget == 0 {
            return None;
        }
        window.requests += 1;
        global.requests += 1;
        Some(budget)
    }

    /// Counts `bytes` sent to `peer` against both byte budgets.
    pub fn spend(&mut self, peer: PeerId, bytes: usize) {
        if let Some(window) = self.windows.get_mut(&peer) {
            window.bytes = window.bytes.saturating_add(bytes);
        }
        if let Some(global) = self.global.as_mut() {
            global.bytes = global.bytes.saturating_add(bytes);
        }
    }

    /// Drops the window that started longest ago, so a new peer is admitted
    /// even when the table is full. Trade-off: a peer at its quota whose window
    /// is oldest regains a full per-peer quota once enough newer peers have
    /// cycled through the table; the global budgets still hold.
    fn forget_oldest(&mut self) {
        let oldest = self
            .windows
            .iter()
            .min_by_key(|entry| entry.1.start)
            .map(|(peer, _)| *peer);
        if let Some(peer) = oldest {
            self.windows.remove(&peer);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::core::block::BlockHeader;
    use crate::crypto::pow::target_from_leading_zero_bits;

    fn block(nonce: u64) -> Block {
        Block::new(
            BlockHeader {
                version: 0,
                prev_hash: [0; 32],
                state_root: [0; 32],
                timestamp: 1,
                nonce,
                difficulty_target: target_from_leading_zero_bits(0),
                tx_root: [0; 32],
            },
            vec![],
        )
    }

    #[test]
    fn serve_skips_unknown_and_repeated_ids_and_refuses_oversized_requests() {
        let held = block(1);
        let id = held.header.id();
        let lookup =
            |wanted: &[u8; 32]| -> Result<_, ()> { Ok((*wanted == id).then(|| held.to_bytes())) };
        let response = serve(
            &BlockRequest {
                ids: vec![id, [9; 32], id],
            },
            lookup,
        );
        assert_eq!(response.blocks, vec![BlockBytes(held.to_bytes())]);

        let oversized = BlockRequest {
            ids: vec![id; MAX_BLOCKS_PER_REQUEST + 1],
        };
        assert!(serve(&oversized, lookup).blocks.is_empty());
    }

    #[test]
    fn a_response_may_omit_blocks_but_not_add_or_repeat_them() {
        let (a, b) = (block(1), block(2));
        let requested = [a.header.id()];
        let partial = BlockResponse { blocks: vec![] };
        assert_eq!(accept_response(&requested, partial), Ok(vec![]));

        let honest = BlockResponse {
            blocks: vec![BlockBytes(a.to_bytes())],
        };
        assert_eq!(accept_response(&requested, honest).map(|v| v.len()), Ok(1));

        let unasked = BlockResponse {
            blocks: vec![BlockBytes(b.to_bytes())],
        };
        assert_eq!(
            accept_response(&requested, unasked),
            Err(Offence::BadSyncResponse)
        );

        let garbage = BlockResponse {
            blocks: vec![BlockBytes(vec![1, 2, 3])],
        };
        assert_eq!(
            accept_response(&requested, garbage),
            Err(Offence::BadSyncResponse)
        );

        let both = [a.header.id(), b.header.id()];
        let doubled = BlockResponse {
            blocks: vec![BlockBytes(a.to_bytes()), BlockBytes(a.to_bytes())],
        };
        assert_eq!(
            accept_response(&both, doubled),
            Err(Offence::BadSyncResponse)
        );
    }

    #[test]
    fn a_peer_gets_its_quota_per_window_and_no_more() {
        let mut limiter = RateLimiter::default();
        let (peer, now) = (PeerId::random(), Instant::now());
        for _ in 0..REQUESTS_PER_WINDOW {
            assert!(limiter.admit(peer, now).is_some());
        }
        assert!(limiter.admit(peer, now).is_none());
        assert!(limiter.admit(PeerId::random(), now).is_some());
        assert!(limiter.admit(peer, now + REQUEST_WINDOW).is_some());
    }

    #[test]
    fn a_peer_is_sent_at_most_its_byte_budget_per_window() {
        let mut limiter = RateLimiter::default();
        let (peer, now) = (PeerId::random(), Instant::now());
        assert_eq!(limiter.admit(peer, now), Some(MAX_RESPONSE_PAYLOAD));
        limiter.spend(peer, PEER_RESPONSE_BYTES_PER_WINDOW - 1);
        assert_eq!(limiter.admit(peer, now), Some(1));
        limiter.spend(peer, 1);
        assert_eq!(limiter.admit(peer, now), None);
        assert_eq!(
            limiter.admit(peer, now + REQUEST_WINDOW),
            Some(MAX_RESPONSE_PAYLOAD)
        );
    }

    #[test]
    fn fresh_identities_share_one_global_budget() {
        // Identities are free, so the bound that matters is the shared one: a
        // flood of new peers never gets more than the global budgets.
        let mut limiter = RateLimiter::default();
        let now = Instant::now();
        let (mut admitted, mut sent) = (0u32, 0usize);
        for _ in 0..(GLOBAL_REQUESTS_PER_WINDOW + 300) {
            let peer = PeerId::random();
            if let Some(budget) = limiter.admit(peer, now) {
                admitted += 1;
                sent += budget;
                limiter.spend(peer, budget);
            }
        }
        assert!(admitted <= GLOBAL_REQUESTS_PER_WINDOW);
        assert_eq!(sent, GLOBAL_RESPONSE_BYTES_PER_WINDOW);
        assert!(limiter.admit(PeerId::random(), now).is_none());

        let mut requests_only = RateLimiter::default();
        let admitted = (0..(GLOBAL_REQUESTS_PER_WINDOW + 300))
            .filter(|_| requests_only.admit(PeerId::random(), now).is_some())
            .count();
        assert_eq!(admitted, GLOBAL_REQUESTS_PER_WINDOW as usize);
        assert!(
            requests_only
                .admit(PeerId::random(), now + REQUEST_WINDOW)
                .is_some()
        );
    }

    #[test]
    fn serve_within_stops_at_the_budget_it_is_given() {
        let held = [block(1), block(2)];
        let (a, b) = (&held[0], &held[1]);
        let lookup = |wanted: &[u8; 32]| -> Result<_, ()> {
            Ok(held
                .iter()
                .find(|blk| blk.header.id() == *wanted)
                .map(Block::to_bytes))
        };
        let request = BlockRequest {
            ids: vec![a.header.id(), b.header.id()],
        };
        let one = a.to_bytes().len();
        let cut = serve_within(&request, one, lookup);
        assert_eq!(cut.blocks, vec![BlockBytes(a.to_bytes())]);
        assert_eq!(cut.payload_len(), one);
        assert!(serve_within(&request, one - 1, lookup).blocks.is_empty());
        assert_eq!(serve_within(&request, usize::MAX, lookup).blocks.len(), 2);
    }

    #[test]
    fn a_newcomer_is_admitted_when_the_table_is_full() {
        // Filled directly: admitting 4,096 peers through `admit` would spend
        // the global request budget first.
        let mut limiter = RateLimiter::default();
        let now = Instant::now();
        for _ in 0..MAX_LIMITED_PEERS {
            limiter.windows.insert(PeerId::random(), Window::new(now));
        }
        assert!(
            limiter.admit(PeerId::random(), now).is_some(),
            "a full table must not refuse a newcomer"
        );
        assert_eq!(limiter.windows.len(), MAX_LIMITED_PEERS);
    }
}
