//! The driver's half of the peer guard: gossip verdicts, quarantine, and block
//! sync.
//!
//! What is scored, and why lateness and double proposals are not, is in
//! [`crate::network::peer_health`]. The fetch protocol and its bounds are in
//! [`crate::network::sync`]. Split from `node.rs` to keep that file within the
//! project's size limit; it is a child module so it can reach the driver's
//! private state.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use libp2p::PeerId;
use libp2p::gossipsub::{Message as GossipMessage, MessageId};
use libp2p::request_response::{self, Message};
use tokio::sync::oneshot;

use super::{NodeDriver, NodeEvent};
use crate::core::{Block, Transaction};
use crate::error::{NodeError, Result};
use crate::network::evidence_tap;
use crate::network::peer_health::{
    Action, Offence, Verdict, classify_block_frame, classify_transaction,
};
use crate::network::sync::{self, BlockRequest, BlockResponse, MAX_BLOCKS_PER_REQUEST};
use crate::network::topics::{bft_topic, blocks_topic, txs_topic};

impl NodeDriver {
    /// Validates one gossiped message and tells gossipsub what to do with it.
    ///
    /// Every path ends in [`NodeDriver::report`]: with validation on, a message
    /// nobody reports is never forwarded and sits in gossipsub's cache until
    /// it ages out.
    pub(super) fn handle_gossip_message(
        &mut self,
        source: PeerId,
        id: &MessageId,
        message: &GossipMessage,
    ) {
        // Every delivered message went through `EvidenceTap`, which put the
        // author's signature in front of the frame.
        let Some((signature, data)) = evidence_tap::split(&message.data) else {
            self.report(source, id, Verdict::Ignore);
            return;
        };
        let author = message.source;
        let now = Instant::now();
        if self.health.is_quarantined(&source, now) {
            // Gossipsub drops a blacklisted peer's messages before they get
            // here. This covers the moment between the verdict and the close.
            self.health.note_dropped(&source);
            self.report(source, id, Verdict::Ignore);
            return;
        }
        let verdict = if message.topic == txs_topic().hash() {
            self.gossiped_transaction(data)
        } else if message.topic == blocks_topic().hash() {
            self.gossiped_block(source, author, data, now)
        } else if message.topic == bft_topic().hash() {
            self.gossiped_bft(data)
        } else {
            Verdict::Ignore
        };
        self.report(source, id, verdict);
        self.attest(verdict, author, message.sequence_number, signature, data);
    }

    /// Relays a DAG-BFT frame that has the right version byte and hands it to
    /// whoever runs consensus. Signatures are the engine's to check; the gossip
    /// layer only refuses what cannot be a frame (`consensus::bft` module note).
    fn gossiped_bft(&mut self, data: &[u8]) -> Verdict {
        if data.first() != Some(&crate::consensus::bft::wire::WIRE_VERSION) {
            return Verdict::Ignore;
        }
        self.emit(NodeEvent::BftFrame(std::sync::Arc::new(data.to_vec())));
        Verdict::Accept
    }

    fn gossiped_transaction(&mut self, data: &[u8]) -> Verdict {
        let result = self.mempool.insert_encoded(data);
        let verdict = classify_transaction(&result);
        match result {
            // The hash is recomputed from the decoded transaction rather than
            // taken from anything the sender supplied.
            Ok(true) => match Transaction::from_bytes(data) {
                Ok(tx) => self.emit(NodeEvent::TransactionAccepted(tx.txid())),
                Err(e) => self.emit(NodeEvent::TransactionRejected(e.to_string())),
            },
            // Already pooled: normal in a gossip mesh, not worth an event.
            Ok(false) => {}
            Err(e) => self.emit(NodeEvent::TransactionRejected(e.to_string())),
        }
        verdict
    }

    pub(super) fn gossiped_block(
        &mut self,
        source: PeerId,
        author: Option<PeerId>,
        data: &[u8],
        now: Instant,
    ) -> Verdict {
        let decoded = Block::from_bytes(data);
        let verdict = classify_block_frame(&decoded);
        match decoded {
            Ok(block) if verdict == Verdict::Accept => {
                // Monitoring only. The timestamp is the miner's claim and the
                // clock is ours, so this is skew plus propagation, never scored.
                let age = unix_now().saturating_sub(block.header.timestamp);
                self.health
                    .observe_latency(source, Duration::from_secs(age), now);
                self.emit(NodeEvent::BlockReceived {
                    block: Box::new(block),
                    source,
                    author,
                });
            }
            Ok(_) => self.emit(NodeEvent::TransactionRejected(
                "gossiped block body does not match its tx_root".to_string(),
            )),
            Err(e) => self.emit(NodeEvent::TransactionRejected(e.to_string())),
        }
        verdict
    }

    /// Reports `verdict` to gossipsub and scores the sender for a rejection.
    fn report(&mut self, source: PeerId, id: &MessageId, verdict: Verdict) {
        // `false` means the message is no longer cached — a duplicate that
        // gossipsub already resolved. Nothing to do either way.
        let _ = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .report_message_validation_result(id, &source, verdict.acceptance());
        if let Some(offence) = verdict.offence() {
            self.punish(source, offence);
        }
    }

    /// Records `offence` and quarantines `peer` if it crossed the line.
    ///
    /// Quarantine is three things at once: gossipsub drops everything the peer
    /// relays or authored, it stops being an explicit peer that messages are
    /// always pushed to, and the block list closes its connections and refuses
    /// new ones.
    pub(super) fn punish(&mut self, peer: PeerId, offence: Offence) {
        let Action::Quarantine { strikes, until } =
            self.health.record(peer, offence, Instant::now())
        else {
            return;
        };
        let behaviour = self.swarm.behaviour_mut();
        behaviour.gossipsub.blacklist_peer(&peer);
        behaviour.gossipsub.remove_explicit_peer(&peer);
        behaviour.block_list.block_peer(peer);
        self.relay_quarantined(peer, until);
        self.emit(NodeEvent::PeerQuarantined { peer, strikes });
    }

    /// Lifts every quarantine that has run its course.
    ///
    /// A peer also convicted on chain stays refused (`node::threat`): the local
    /// quarantine ending says nothing about the conviction.
    pub(super) fn release_expired(&mut self) {
        for peer in self.health.release_expired(Instant::now()) {
            if !self.convicted.contains(&peer) {
                let behaviour = self.swarm.behaviour_mut();
                behaviour.gossipsub.remove_blacklisted_peer(&peer);
                behaviour.block_list.unblock_peer(peer);
            }
            self.relay_released(peer);
            self.emit(NodeEvent::PeerReleased(peer));
        }
    }

    /// Sends a block-sync request, answering `reply` when it resolves.
    pub(super) fn request_blocks(
        &mut self,
        peer: PeerId,
        ids: Vec<[u8; 32]>,
        reply: oneshot::Sender<Result<Vec<Block>>>,
    ) {
        if ids.is_empty() || ids.len() > MAX_BLOCKS_PER_REQUEST {
            let _ = reply.send(Err(NodeError::Network(format!(
                "a block-sync request names 1 to {MAX_BLOCKS_PER_REQUEST} blocks, not {}",
                ids.len()
            ))));
            return;
        }
        if self.health.is_quarantined(&peer, Instant::now()) {
            let _ = reply.send(Err(NodeError::Network(format!("{peer} is quarantined"))));
            return;
        }
        let request_id = self
            .swarm
            .behaviour_mut()
            .sync
            .send_request(&peer, BlockRequest { ids: ids.clone() });
        self.pending_sync.insert(request_id, (peer, ids, reply));
    }

    pub(super) fn handle_sync_event(
        &mut self,
        event: request_response::Event<BlockRequest, BlockResponse>,
    ) {
        match event {
            request_response::Event::Message {
                peer,
                message:
                    Message::Request {
                        request, channel, ..
                    },
                ..
            } => {
                let now = Instant::now();
                let allowed =
                    !self.health.is_quarantined(&peer, now) && self.sync_limits.allow(peer, now);
                let response = if allowed {
                    sync::serve(&request, |id| self.state.block_bytes(id))
                } else {
                    BlockResponse { blocks: Vec::new() }
                };
                // Fails only if the requester has already gone.
                let _ = self
                    .swarm
                    .behaviour_mut()
                    .sync
                    .send_response(channel, response);
            }
            request_response::Event::Message {
                message:
                    Message::Response {
                        request_id,
                        response,
                    },
                ..
            } => {
                let Some((peer, ids, reply)) = self.pending_sync.remove(&request_id) else {
                    return;
                };
                let result = sync::accept_response(&ids, response).map_err(|offence| {
                    self.punish(peer, offence);
                    NodeError::Network(format!("{peer} sent a {} response", offence.label()))
                });
                let _ = reply.send(result);
            }
            request_response::Event::OutboundFailure {
                request_id, error, ..
            } => {
                if let Some((_, _, reply)) = self.pending_sync.remove(&request_id) {
                    let _ = reply.send(Err(NodeError::Network(format!("block sync: {error}"))));
                }
            }
            _ => {}
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
