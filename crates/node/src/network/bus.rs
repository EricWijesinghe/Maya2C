//! Where the node's events go.
//!
//! Consensus frames used to share one 256-slot broadcast channel with every
//! other event. A flood of cheap events (6,000 gossiped transactions, each
//! `TransactionAccepted` or `TransactionRejected`) overflowed it faster than
//! the BFT loop could drain it between frames, the loop was told it had
//! `Lagged`, and the votes it lost stalled finality for about eight minutes
//! (`reports/bench/2026-10-25-tps-series.md`, 3000 senders x 2). Frames now
//! have a channel of their own, so nothing a transaction flood does can push a
//! vote out of the buffer; the flood is still free to lag the subscribers that
//! only want transaction events, which can afford it.
//!
//! The frame buffer is not free: a gossip message may be 8 MiB
//! (`MAX_GOSSIP_MESSAGE_BYTES`), so the worst case is that size times the
//! capacity. Real frames are far smaller; a peer sending maximum-size frames
//! is rate-limited and scored like any other (`PeerHealth`).

use std::sync::Arc;

use tokio::sync::broadcast;

use super::NodeEvent;

/// Slots in the general event channel.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Slots in the consensus-frame channel: twice the old shared channel, which
/// is what a BFT loop that is busy under the chain lock needs to ride out a
/// burst of frames without losing one.
const FRAME_CHANNEL_CAPACITY: usize = 512;

/// The node's two broadcast channels.
#[derive(Clone)]
pub(super) struct Bus {
    events: broadcast::Sender<NodeEvent>,
    frames: broadcast::Sender<Arc<Vec<u8>>>,
}

impl Bus {
    pub(super) fn new() -> Self {
        Self::with_capacity(EVENT_CHANNEL_CAPACITY, FRAME_CHANNEL_CAPACITY)
    }

    fn with_capacity(events: usize, frames: usize) -> Self {
        Self {
            events: broadcast::channel(events).0,
            frames: broadcast::channel(frames).0,
        }
    }

    /// A send failure only means nobody is subscribed, which is not an error.
    pub(super) fn emit(&self, event: NodeEvent) {
        let _ = self.events.send(event);
    }

    /// As [`Bus::emit`], for a DAG-BFT frame that arrived on the consensus
    /// topic. Decoded and verified by `consensus::bft::BftDriver`, not here.
    pub(super) fn emit_frame(&self, frame: Arc<Vec<u8>>) {
        let _ = self.frames.send(frame);
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.events.subscribe()
    }

    pub(super) fn subscribe_frames(&self) -> broadcast::Receiver<Arc<Vec<u8>>> {
        self.frames.subscribe()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use libp2p::PeerId;
    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;

    #[test]
    fn a_flood_of_events_cannot_push_a_consensus_frame_out() {
        let bus = Bus::with_capacity(4, 4);
        let mut events = bus.subscribe();
        let mut frames = bus.subscribe_frames();

        bus.emit_frame(Arc::new(vec![1, 2, 3]));
        for _ in 0..100 {
            bus.emit(NodeEvent::PeerConnected(PeerId::random()));
        }

        // The general channel overflowed, as a transaction flood makes it do...
        assert!(matches!(events.try_recv(), Err(TryRecvError::Lagged(_))));
        // ...and the frame is still there, in order.
        assert_eq!(*frames.try_recv().expect("frame kept"), vec![1, 2, 3]);
    }

    #[test]
    fn frames_flooding_themselves_still_report_what_was_lost() {
        let bus = Bus::with_capacity(4, 2);
        let mut frames = bus.subscribe_frames();
        for n in 0..5u8 {
            bus.emit_frame(Arc::new(vec![n]));
        }
        // Loss on the frame channel stays visible to the loop that counts it.
        assert!(matches!(frames.try_recv(), Err(TryRecvError::Lagged(3))));
    }
}
