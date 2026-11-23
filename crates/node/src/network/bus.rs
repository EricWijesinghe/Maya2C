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
//! The budget is also split by gossip author (audit P2P-N1): no author may
//! hold more than an eighth of the bytes or of the slots. A frame that decodes
//! is relayed before its signature is checked, so one identity used to be able
//! to fill the whole window with frames that would fail ML-DSA, and every
//! honest frame was refused until the engine re-fetched it. Each frame now
//! carries its author, so the engine's verdict can be blamed on it
//! (`NodeHandle::report_offence`).
//!
//! Also open: the BFT loop can still lag under a flood of *frames* (it verifies
//! them under the chain lock); the loop counts the loss and relies on the
//! engine to re-gossip. Eight fresh identities can still fill the window
//! together until each is quarantined; reserving a share for the committee
//! needs a peer-id-to-validator map, which does not exist yet.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use libp2p::PeerId;
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

/// Parts of the window one gossip author may hold, in bytes and in slots:
/// 32 MiB, or eight proposals with a full batch, and 64 frames.
const AUTHOR_SHARES: usize = 8;

/// A DAG-BFT frame as gossip delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The peer that signed the gossip message, which is who a frame that
    /// fails verification is blamed on. Never the relay: an honest relay
    /// forwards every frame that decodes (`docs/peer-health.md`).
    pub author: Option<PeerId>,
    /// The frame's bytes.
    pub bytes: Vec<u8>,
}

/// Least time between two warnings about frames refused for the byte budget:
/// a peer can trigger refusals at will, so a line per frame would be a
/// remote-triggered log flood.
const DROP_WARNING_INTERVAL: Duration = Duration::from_secs(10);

/// Sizes of the frames still in the channel, oldest first. The broadcast
/// channel keeps exactly the last `capacity` frames sent, so this mirrors it.
struct FrameWindow {
    sizes: VecDeque<(Option<PeerId>, usize)>,
    held: usize,
    /// Bytes and frames each author has in the channel. Looked up, never
    /// iterated.
    by_author: HashMap<Option<PeerId>, (usize, usize)>,
    capacity: usize,
    max_bytes: usize,
    shares: usize,
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
            self.pop_oldest();
        }
    }

    fn pop_oldest(&mut self) {
        let Some((author, len)) = self.sizes.pop_front() else {
            return;
        };
        self.held -= len;
        if let Some(entry) = self.by_author.get_mut(&author) {
            entry.0 -= len;
            entry.1 -= 1;
            if entry.1 == 0 {
                self.by_author.remove(&author);
            }
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

    /// Admits a frame of `len` bytes from `author`, or refuses it if keeping
    /// it would take the channel past `max_bytes`, or the author past its
    /// share of the bytes or the slots. Admitting a frame into a full channel
    /// evicts the oldest, exactly as the broadcast channel does.
    fn admit(&mut self, author: Option<PeerId>, len: usize) -> bool {
        let full = self.sizes.len() == self.capacity;
        let oldest = self.sizes.front().copied().filter(|_| full);
        let evicted = oldest.map_or(0, |(_, n)| n);
        if self.held - evicted + len > self.max_bytes {
            return false;
        }
        let (bytes, frames) = self.by_author.get(&author).copied().unwrap_or((0, 0));
        // What the eviction frees counts only if it is this author's frame.
        let (bytes, frames) = match oldest {
            Some((a, n)) if a == author => (bytes - n, frames - 1),
            _ => (bytes, frames),
        };
        let byte_share = self.max_bytes / self.shares;
        let slot_share = (self.capacity / self.shares).max(1);
        if bytes + len > byte_share || frames + 1 > slot_share {
            return false;
        }
        if full {
            self.pop_oldest();
        }
        self.sizes.push_back((author, len));
        self.held += len;
        let entry = self.by_author.entry(author).or_insert((0, 0));
        entry.0 += len;
        entry.1 += 1;
        true
    }
}

/// The node's two broadcast channels.
#[derive(Clone)]
pub(super) struct Bus {
    events: broadcast::Sender<NodeEvent>,
    frames: broadcast::Sender<Arc<Frame>>,
    window: Arc<Mutex<FrameWindow>>,
}

impl Bus {
    pub(super) fn new() -> Self {
        Self::with_limits(
            EVENT_CHANNEL_CAPACITY,
            FRAME_CHANNEL_CAPACITY,
            MAX_HELD_FRAME_BYTES,
            AUTHOR_SHARES,
        )
    }

    #[cfg(test)]
    fn with_capacity(events: usize, frames: usize) -> Self {
        Self::with_limits(events, frames, MAX_HELD_FRAME_BYTES, 1)
    }

    fn with_limits(events: usize, frames: usize, max_bytes: usize, shares: usize) -> Self {
        Self {
            events: broadcast::channel(events).0,
            frames: broadcast::channel(frames).0,
            window: Arc::new(Mutex::new(FrameWindow {
                sizes: VecDeque::with_capacity(frames),
                held: 0,
                by_author: HashMap::new(),
                // The channel rounds its capacity up the same way; the window
                // must evict exactly when the channel does.
                capacity: frames.next_power_of_two(),
                max_bytes,
                shares: shares.max(1),
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
    /// frame was refused for the byte budget or its author's share of it.
    pub(super) fn emit_frame(&self, frame: Arc<Frame>) -> bool {
        // The lock covers the send, so the window and the channel agree on order.
        let mut window = self.window.lock().unwrap_or_else(PoisonError::into_inner);
        // FIFO: what `len` no longer counts is the oldest, read by everyone.
        window.trim(self.frames.len());
        if !window.admit(frame.author, frame.bytes.len()) {
            // Silent, this looked like lost transactions (a DAG stalled with
            // nothing in the log, bench-tps 2026-11-08).
            if let Some(n) = window.refused(Instant::now()) {
                eprintln!(
                    "bft: refused {n} consensus frame(s) over the {} MiB frame budget or one author's share of it ({} bytes held unread); the engine re-fetches them",
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

    pub(super) fn subscribe_frames(&self) -> broadcast::Receiver<Arc<Frame>> {
        self.frames.subscribe()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;

    fn frame(author: Option<PeerId>, bytes: Vec<u8>) -> Arc<Frame> {
        Arc::new(Frame { author, bytes })
    }

    #[test]
    fn one_author_flooding_frames_cannot_crowd_out_another() {
        // Audit P2P-N1: frames that decode are held before any signature is
        // checked, so one identity could fill the window and every honest
        // frame after it was refused. 8 shares of 80 bytes and 16 slots.
        let bus = Bus::with_limits(4, 16, 80, 8);
        let _stalled = bus.subscribe_frames();
        let attacker = Some(PeerId::random());
        let refused = (0..100)
            .filter(|_| !bus.emit_frame(frame(attacker, vec![0; 5])))
            .count();
        // Two frames (its slot share) are all it may hold.
        assert_eq!(refused, 98);
        let honest = Some(PeerId::random());
        assert!(
            bus.emit_frame(frame(honest, vec![1; 10])),
            "honest frame refused"
        );
        // Its byte share is ten: one more byte is refused, its own fault.
        assert!(!bus.emit_frame(frame(honest, vec![1; 1])));
    }

    #[test]
    fn a_frame_carries_its_author_to_the_engine() {
        let bus = Bus::new();
        let mut frames = bus.subscribe_frames();
        let author = Some(PeerId::random());
        assert!(bus.emit_frame(frame(author, vec![7])));
        assert_eq!(frames.try_recv().unwrap().author, author);
    }

    #[test]
    fn a_read_frame_returns_its_authors_share() {
        let bus = Bus::with_limits(4, 16, 80, 8);
        let mut frames = bus.subscribe_frames();
        let author = Some(PeerId::random());
        for _ in 0..10 {
            assert!(bus.emit_frame(frame(author, vec![0; 10])));
            frames.try_recv().unwrap();
        }
    }

    #[test]
    fn a_flood_of_events_cannot_push_a_consensus_frame_out() {
        let bus = Bus::with_capacity(4, 4);
        let mut events = bus.subscribe();
        let mut frames = bus.subscribe_frames();

        bus.emit_frame(frame(None, vec![1, 2, 3]));
        for _ in 0..100 {
            bus.emit(NodeEvent::PeerConnected(PeerId::random()));
        }

        // The general channel overflowed, as a transaction flood makes it do...
        assert!(matches!(events.try_recv(), Err(TryRecvError::Lagged(_))));
        // ...and the frame is still there, in order.
        assert_eq!(frames.try_recv().expect("frame kept").bytes, vec![1, 2, 3]);
    }

    #[test]
    fn frames_flooding_themselves_still_report_what_was_lost() {
        let bus = Bus::with_capacity(4, 2);
        let mut frames = bus.subscribe_frames();
        for n in 0..5u8 {
            bus.emit_frame(frame(None, vec![n]));
        }
        // Loss on the frame channel stays visible to the loop that counts it.
        assert!(matches!(frames.try_recv(), Err(TryRecvError::Lagged(3))));
    }

    #[test]
    fn large_frames_cannot_hold_more_than_the_byte_budget() {
        let bus = Bus::with_limits(4, 8, 10, 1);
        let mut frames = bus.subscribe_frames();
        assert!(bus.emit_frame(frame(None, vec![0; 6])));
        // 6 + 6 would hold 12 bytes: refused, and the first frame survives.
        assert!(!bus.emit_frame(frame(None, vec![1; 6])));
        assert!(bus.emit_frame(frame(None, vec![2; 4])));
        assert_eq!(frames.try_recv().unwrap().bytes.len(), 6);
        assert_eq!(frames.try_recv().unwrap().bytes.len(), 4);
        assert!(frames.try_recv().is_err());
    }

    #[test]
    fn a_read_frame_frees_its_bytes() {
        // A slot capacity far above what is sent, so only reading can free bytes.
        let bus = Bus::with_limits(4, 64, 10, 1);
        let mut frames = bus.subscribe_frames();
        for n in 0..20u8 {
            assert!(bus.emit_frame(frame(None, vec![n; 6])), "frame {n} refused");
            assert_eq!(frames.try_recv().unwrap().bytes.len(), 6);
        }
        // Unread frames still count: 6 held, 6 more would pass 10.
        assert!(bus.emit_frame(frame(None, vec![0; 6])));
        assert!(!bus.emit_frame(frame(None, vec![0; 6])));
    }

    #[test]
    fn a_reader_that_never_reads_still_caps_the_held_bytes() {
        // S-003: the bound exists for a subscriber that stalls; reading
        // nothing must keep every byte counted, up to the budget.
        let bus = Bus::with_limits(4, 64, 10, 1);
        let _stalled = bus.subscribe_frames();
        assert!(bus.emit_frame(frame(None, vec![0; 6])));
        assert!(bus.emit_frame(frame(None, vec![0; 4])));
        assert!(!bus.emit_frame(frame(None, vec![0; 1])));
    }

    #[test]
    fn a_lagged_reader_is_still_counted_for_what_the_channel_holds() {
        let bus = Bus::with_limits(4, 2, 100, 1);
        let mut frames = bus.subscribe_frames();
        for _ in 0..5 {
            assert!(bus.emit_frame(frame(None, vec![0; 5])));
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
        let bus = Bus::with_limits(4, 2, 10, 1);
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
        let bus = Bus::with_limits(4, 2, 10, 1);
        // A receiver that never reads: with none, nothing is held at all.
        let _stalled = bus.subscribe_frames();
        assert!(bus.emit_frame(frame(None, vec![0; 5])));
        assert!(bus.emit_frame(frame(None, vec![0; 5])));
        // The channel is full; the next frame evicts a 5-byte one, so 10 fit.
        assert!(bus.emit_frame(frame(None, vec![0; 5])));
        assert!(!bus.emit_frame(frame(None, vec![0; 6])));
    }
}
