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
//! The frame buffer is bounded in bytes as well as slots: 512 slots of 5 MiB
//! frames is 2.5 GiB, and a frame that decodes is not yet a frame whose
//! signature checks. [`FrameWindow`] tracks the frames the channel can still
//! hold and refuses one that would take them past [`MAX_HELD_FRAME_BYTES`], so
//! a flood of large frames costs frames, which the engine re-fetches, never
//! the node's memory (pre-audit S-003).
//!
//! Also open: the BFT loop can still lag under a flood of *frames* (it verifies
//! them under the chain lock); the loop counts the loss and relies on the
//! engine to re-gossip.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

use super::NodeEvent;

/// Slots in the general event channel.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Slots in the consensus-frame channel: twice the old shared channel, which
/// is what a BFT loop that is busy under the chain lock needs to ride out a
/// burst of frames without losing one.
const FRAME_CHANNEL_CAPACITY: usize = 512;

/// Most frame bytes the channel may hold: 64 proposals with a full 4 MiB
/// batch, far above a round's worth from any real committee.
const MAX_HELD_FRAME_BYTES: usize = 256 * 1024 * 1024;

/// Least time between two warnings about frames refused for the byte budget:
/// a peer can trigger refusals at will, so a line per frame would be a
/// remote-triggered log flood.
const DROP_WARNING_INTERVAL: Duration = Duration::from_secs(10);

/// Sizes of the frames still in the channel, oldest first. The broadcast
/// channel keeps exactly the last `capacity` frames sent, so this mirrors it.
struct FrameWindow {
    sizes: VecDeque<usize>,
    held: usize,
    capacity: usize,
    max_bytes: usize,
    /// Frames refused since the last warning.
    dropped: u64,
    last_warning: Option<Instant>,
}

impl FrameWindow {
    /// Forgets the oldest sizes until only `queued` remain: frames every
    /// receiver has read no longer hold memory. Without this a read frame
    /// left the budget only when 512 newer ones pushed it out, so under load
    /// the last few hundred megabytes of read proposals refused every new
    /// proposal and certificate, and the DAG stalled for good with no error
    /// (bench-tps 1000 x 4, 2026-11-08).
    fn trim(&mut self, queued: usize) {
        while self.sizes.len() > queued {
            self.held -= self.sizes.pop_front().unwrap_or(0);
        }
    }

    /// Counts a refused frame. Returns how many to report when a warning is
    /// due (the first refusal, then at most one per [`DROP_WARNING_INTERVAL`]),
    /// and starts the next count.
    fn refused(&mut self, now: Instant) -> Option<u64> {
        self.dropped += 1;
        if self
            .last_warning
            .is_some_and(|at| now.duration_since(at) < DROP_WARNING_INTERVAL)
        {
            return None;
        }
        self.last_warning = Some(now);
        Some(std::mem::take(&mut self.dropped))
    }

    /// Admits a frame of `len` bytes, or refuses it if keeping it would take
    /// the channel past `max_bytes`. Admitting a frame into a full channel
    /// evicts the oldest, exactly as the broadcast channel does.
    fn admit(&mut self, len: usize) -> bool {
        let evicted = if self.sizes.len() == self.capacity {
            self.sizes.front().copied().unwrap_or(0)
        } else {
            0
        };
        if self.held - evicted + len > self.max_bytes {
            return false;
        }
        if self.sizes.len() == self.capacity {
            self.sizes.pop_front();
            self.held -= evicted;
        }
        self.sizes.push_back(len);
        self.held += len;
        true
    }
}

/// The node's two broadcast channels.
#[derive(Clone)]
pub(super) struct Bus {
    events: broadcast::Sender<NodeEvent>,
    frames: broadcast::Sender<Arc<Vec<u8>>>,
    window: Arc<Mutex<FrameWindow>>,
}

impl Bus {
    pub(super) fn new() -> Self {
        Self::with_limits(
            EVENT_CHANNEL_CAPACITY,
            FRAME_CHANNEL_CAPACITY,
            MAX_HELD_FRAME_BYTES,
        )
    }

    #[cfg(test)]
    fn with_capacity(events: usize, frames: usize) -> Self {
        Self::with_limits(events, frames, MAX_HELD_FRAME_BYTES)
    }

    fn with_limits(events: usize, frames: usize, max_bytes: usize) -> Self {
        Self {
            events: broadcast::channel(events).0,
            frames: broadcast::channel(frames).0,
            window: Arc::new(Mutex::new(FrameWindow {
                sizes: VecDeque::with_capacity(frames),
                held: 0,
                // The channel rounds its capacity up the same way; the window
                // must evict exactly when the channel does.
                capacity: frames.next_power_of_two(),
                max_bytes,
                dropped: 0,
                last_warning: None,
            })),
        }
    }

    /// A send failure only means nobody is subscribed, which is not an error.
    pub(super) fn emit(&self, event: NodeEvent) {
        let _ = self.events.send(event);
    }

    /// As [`Bus::emit`] (a send failure only means nobody is subscribed), for a
    /// DAG-BFT frame that arrived on the consensus topic. Decoded and verified
    /// by `consensus::bft::BftDriver`, not here. Returns `false` when the
    /// frame was refused for the byte budget.
    pub(super) fn emit_frame(&self, frame: Arc<Vec<u8>>) -> bool {
        // The lock covers the send, so the window and the channel agree on order.
        let mut window = self.window.lock().unwrap_or_else(PoisonError::into_inner);
        // FIFO: what `len` no longer counts is the oldest, read by everyone.
        window.trim(self.frames.len());
        if !window.admit(frame.len()) {
            // Silent, this looked like lost transactions (a DAG stalled with
            // nothing in the log, bench-tps 2026-11-08).
            if let Some(n) = window.refused(Instant::now()) {
                eprintln!(
                    "bft: refused {n} consensus frame(s) over the {} MiB frame budget ({} bytes held unread); the engine re-fetches them",
                    window.max_bytes / (1024 * 1024),
                    window.held
                );
            }
            return false;
        }
        let _ = self.frames.send(frame);
        true
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

    #[test]
    fn large_frames_cannot_hold_more_than_the_byte_budget() {
        let bus = Bus::with_limits(4, 8, 10);
        let mut frames = bus.subscribe_frames();
        assert!(bus.emit_frame(Arc::new(vec![0; 6])));
        // 6 + 6 would hold 12 bytes: refused, and the first frame survives.
        assert!(!bus.emit_frame(Arc::new(vec![1; 6])));
        assert!(bus.emit_frame(Arc::new(vec![2; 4])));
        assert_eq!(frames.try_recv().unwrap().len(), 6);
        assert_eq!(frames.try_recv().unwrap().len(), 4);
        assert!(frames.try_recv().is_err());
    }

    #[test]
    fn a_read_frame_frees_its_bytes() {
        // A slot capacity far above what is sent, so only reading can free bytes.
        let bus = Bus::with_limits(4, 64, 10);
        let mut frames = bus.subscribe_frames();
        for n in 0..20u8 {
            assert!(bus.emit_frame(Arc::new(vec![n; 6])), "frame {n} refused");
            assert_eq!(frames.try_recv().unwrap().len(), 6);
        }
        // Unread frames still count: 6 held, 6 more would pass 10.
        assert!(bus.emit_frame(Arc::new(vec![0; 6])));
        assert!(!bus.emit_frame(Arc::new(vec![0; 6])));
    }

    #[test]
    fn a_reader_that_never_reads_still_caps_the_held_bytes() {
        // S-003: the bound exists for a subscriber that stalls; reading
        // nothing must keep every byte counted, up to the budget.
        let bus = Bus::with_limits(4, 64, 10);
        let _stalled = bus.subscribe_frames();
        assert!(bus.emit_frame(Arc::new(vec![0; 6])));
        assert!(bus.emit_frame(Arc::new(vec![0; 4])));
        assert!(!bus.emit_frame(Arc::new(vec![0; 1])));
    }

    #[test]
    fn a_lagged_reader_is_still_counted_for_what_the_channel_holds() {
        let bus = Bus::with_limits(4, 2, 100);
        let mut frames = bus.subscribe_frames();
        for _ in 0..5 {
            assert!(bus.emit_frame(Arc::new(vec![0; 5])));
        }
        let held = || {
            let mut window = bus.window.lock().unwrap();
            window.trim(bus.frames.len());
            window.held
        };
        assert!(matches!(frames.try_recv(), Err(TryRecvError::Lagged(3))));
        // After the lag both frames the channel still holds are counted.
        assert_eq!(held(), 10);
        frames.try_recv().unwrap();
        // One read frees its 5 bytes, and only those.
        assert_eq!(held(), 5);
        frames.try_recv().unwrap();
        assert_eq!(held(), 0);
    }

    #[test]
    fn refusals_are_reported_at_most_once_per_interval() {
        let bus = Bus::with_limits(4, 2, 10);
        let mut window = bus.window.lock().unwrap();
        let start = Instant::now();
        assert_eq!(window.refused(start), Some(1));
        assert_eq!(window.refused(start + Duration::from_secs(1)), None);
        assert_eq!(window.refused(start + Duration::from_secs(9)), None);
        assert_eq!(window.refused(start + DROP_WARNING_INTERVAL), Some(3));
        assert_eq!(window.refused(start + DROP_WARNING_INTERVAL), None);
    }

    #[test]
    fn an_evicted_frame_frees_its_bytes() {
        let bus = Bus::with_limits(4, 2, 10);
        // A receiver that never reads: with none, nothing is held at all.
        let _stalled = bus.subscribe_frames();
        assert!(bus.emit_frame(Arc::new(vec![0; 5])));
        assert!(bus.emit_frame(Arc::new(vec![0; 5])));
        // The channel is full; the next frame evicts a 5-byte one, so 10 fit.
        assert!(bus.emit_frame(Arc::new(vec![0; 5])));
        assert!(!bus.emit_frame(Arc::new(vec![0; 6])));
    }
}
