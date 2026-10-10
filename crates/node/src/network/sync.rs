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
pub fn serve<F, E>(request: &BlockRequest, mut lookup: F) -> BlockResponse
where
    F: FnMut(&[u8; 32]) -> Result<Option<Vec<u8>>, E>,
{
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
        if payload.saturating_add(bytes.len()) > MAX_RESPONSE_PAYLOAD {
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

/// Fixed-window request counter per peer.
#[derive(Debug, Default)]
pub struct RateLimiter {
    windows: HashMap<PeerId, (Instant, u32)>,
}

impl RateLimiter {
    /// Whether `peer` may make another request at `now`, counting it if so.
    pub fn allow(&mut self, peer: PeerId, now: Instant) -> bool {
        if self.windows.len() >= MAX_LIMITED_PEERS && !self.windows.contains_key(&peer) {
            self.windows
                .retain(|_, (start, _)| now.saturating_duration_since(*start) < REQUEST_WINDOW);
            if self.windows.len() >= MAX_LIMITED_PEERS {
                return false;
            }
        }
        let window = self.windows.entry(peer).or_insert((now, 0));
        if now.saturating_duration_since(window.0) >= REQUEST_WINDOW {
            *window = (now, 0);
        }
        if window.1 >= REQUESTS_PER_WINDOW {
            return false;
        }
        window.1 += 1;
        true
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
            assert!(limiter.allow(peer, now));
        }
        assert!(!limiter.allow(peer, now));
        assert!(limiter.allow(PeerId::random(), now));
        assert!(limiter.allow(peer, now + REQUEST_WINDOW));
    }
}
