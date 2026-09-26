//! The parallel, verifying downloader (sans-IO).
//!
//! Keeps up to `per_peer` requests in flight to every peer that is not
//! banned, verifies every chunk on arrival, bans a peer on its first bad
//! chunk and re-queues whatever it still owed. Chunks are handed out lowest
//! index first, so a restart can resume from a contiguous prefix.

use std::collections::{BTreeSet, VecDeque};

use crate::manifest::ChunkProof;

/// A request to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// Peer to ask.
    pub peer: u16,
    /// Chunk wanted.
    pub chunk: u32,
}

/// Something that happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A chunk verified.
    Verified(u32),
    /// A peer sent a bad chunk and is banned.
    Banned(u16),
    /// Every chunk has verified.
    Complete,
}

/// Download state.
#[derive(Debug)]
pub struct Downloader {
    root: [u8; 32],
    chunks: u32,
    per_peer: usize,
    queue: VecDeque<u32>,
    in_flight: BTreeSet<(u16, u32)>,
    peers: BTreeSet<u16>,
    banned: BTreeSet<u16>,
    done: BTreeSet<u32>,
    /// Bytes received, good and bad.
    pub bytes: u64,
    /// Bytes received in chunks that failed verification.
    pub wasted: u64,
}

impl Downloader {
    /// A download of `chunks` chunks under `root` from `peers`.
    pub fn new(
        root: [u8; 32],
        chunks: u32,
        peers: impl IntoIterator<Item = u16>,
        per_peer: usize,
    ) -> Self {
        Self {
            root,
            chunks,
            per_peer: per_peer.max(1),
            queue: (0..chunks).collect(),
            in_flight: BTreeSet::new(),
            peers: peers.into_iter().collect(),
            banned: BTreeSet::new(),
            done: BTreeSet::new(),
            bytes: 0,
            wasted: 0,
        }
    }

    /// Peers banned so far.
    pub fn banned(&self) -> &BTreeSet<u16> {
        &self.banned
    }

    /// Whether every chunk verified.
    pub fn is_complete(&self) -> bool {
        self.done.len() as u64 == u64::from(self.chunks)
    }

    /// Requests to send now to keep every honest peer busy.
    pub fn poll(&mut self) -> Vec<Request> {
        let mut out = Vec::new();
        let peers: Vec<u16> = self.peers.difference(&self.banned).copied().collect();
        if peers.is_empty() {
            return out;
        }
        'fill: loop {
            let mut progressed = false;
            for &p in &peers {
                let load = self.in_flight.iter().filter(|(q, _)| *q == p).count();
                if load >= self.per_peer {
                    continue;
                }
                let Some(c) = self.queue.pop_front() else {
                    break 'fill;
                };
                self.in_flight.insert((p, c));
                out.push(Request { peer: p, chunk: c });
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
        out
    }

    /// A response arrived. Verifies it and returns what happened.
    pub fn receive(
        &mut self,
        peer: u16,
        chunk: u32,
        bytes: &[u8],
        proof: &ChunkProof,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        if !self.in_flight.remove(&(peer, chunk)) || self.banned.contains(&peer) {
            return events; // unsolicited or from a banned peer: ignore
        }
        self.bytes += bytes.len() as u64;
        if proof.index == chunk && proof.verify(&self.root, self.chunks, bytes) {
            if self.done.insert(chunk) {
                events.push(Event::Verified(chunk));
            }
        } else {
            self.wasted += bytes.len() as u64;
            self.banned.insert(peer);
            events.push(Event::Banned(peer));
            // Everything the liar still owed goes back to the front.
            let owed: Vec<u32> = self
                .in_flight
                .iter()
                .filter(|(p, _)| *p == peer)
                .map(|(_, c)| *c)
                .collect();
            for c in owed.into_iter().chain([chunk]).rev() {
                self.in_flight.remove(&(peer, c));
                self.queue.push_front(c);
            }
        }
        if self.is_complete() {
            events.push(Event::Complete);
        }
        events
    }
}
