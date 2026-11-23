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
//! The budget is also split twice (audit P2P-N1): no gossip author may hold
//! more than an eighth of the bytes or of the slots, and no connected relay
//! more than half. The author share alone was not enough: with strict gossip
//! signing the author is any keypair that signed the message, so one link
//! could sign each frame with a fresh key and fill the window anyway. A relay
//! is a connection, which costs a handshake and counts against its source's
//! connection cap (`network::ip_limits`); half leaves an honest peer that
//! carries most of a small mesh room to do so.
//!
//! Gossip also holds its verdict on a frame until the BFT loop has checked
//! its signatures (`NodeHandle::settle_frame`), so a forged frame is never
//! relayed and only ever costs the nodes its sender is connected to.
//!
//! Also open: the BFT loop can still lag under a flood of *frames* (it verifies
//! them under the chain lock); the loop counts the loss and relies on the
//! engine to re-gossip. A fetch carries no signature, so a fetch from a
//! member's id is relayed on trust, as before.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use libp2p::PeerId;
use libp2p::gossipsub::MessageId;
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

/// Parts of the window one connected relay may hold: 128 MiB and 256 frames.
const RELAY_SHARES: usize = 2;

/// A DAG-BFT frame as gossip delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The peer that signed the gossip message. Any keypair can sign one, so
    /// it is never blamed for a forged frame unless it is also the relay.
    pub author: Option<PeerId>,
    /// The connected peer that delivered it, which the verdict is reported
    /// against.
    pub relay: PeerId,
    /// Gossipsub's id for the message, held unrelayed until it is settled.
    pub id: MessageId,
    /// The frame's bytes.
    pub bytes: Vec<u8>,
}

/// What the BFT loop found when it checked a held frame's signatures, which
/// decides whether gossip relays it (`NodeHandle::settle_frame`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameCheck {
    /// Every signature it carries verifies against the committee, or it
    /// carries none to check (a fetch from a member's id). Relayed.
    Authentic,
    /// A signature fails or names a signer outside the committee. Dropped.
    Forged,
    /// For an epoch this node holds no committee for, so nothing was checked.
    /// Dropped, not relayed unchecked, and nobody is blamed.
    Unjudged,
}

/// Least time between two warnings about frames refused for the byte budget:
/// a peer can trigger refusals at will, so a line per frame would be a
/// remote-triggered log flood.
const DROP_WARNING_INTERVAL: Duration = Duration::from_secs(10);

/// Why a frame was refused, counted apart so a refusal under load can be told
/// from one under attack when it is measured (audit P2P-N1, item 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The whole window is over its bytes.
    Budget = 0,
    /// The frame's gossip author is over its share.
    AuthorShare = 1,
    /// The connected relay that delivered it is over its share.
    RelayShare = 2,
}

/// Bytes and frames each key holds in the channel. Looked up, never iterated.
struct Shares<K> {
    held: HashMap<K, (usize, usize)>,
    parts: usize,
}

impl<K: Hash + Eq + Copy> Shares<K> {
    fn new(parts: usize) -> Self {
        Self {
            held: HashMap::new(),
            parts: parts.max(1),
        }
    }

    /// Whether `key` may add a frame of `len` bytes, counting `freed` (the
    /// frame an admission would evict) only if it is `key`'s own.
    fn fits(
        &self,
        key: K,
        len: usize,
        freed: Option<(K, usize)>,
        max_bytes: usize,
        capacity: usize,
    ) -> bool {
        let (mut bytes, mut frames) = self.held.get(&key).copied().unwrap_or((0, 0));
        if let Some((owner, n)) = freed
            && owner == key
        {
            bytes -= n;
            frames -= 1;
        }
        bytes + len <= max_bytes / self.parts && frames < (capacity / self.parts).max(1)
    }

    fn add(&mut self, key: K, len: usize) {
        let entry = self.held.entry(key).or_insert((0, 0));
        entry.0 += len;
        entry.1 += 1;
    }

    fn remove(&mut self, key: K, len: usize) {
        if let Some(entry) = self.held.get_mut(&key) {
            entry.0 -= len;
            entry.1 -= 1;
            if entry.1 == 0 {
                self.held.remove(&key);
            }
        }
    }
}

/// One frame still in the channel.
#[derive(Clone, Copy)]
struct Held {
    author: Option<PeerId>,
    relay: PeerId,
    len: usize,
}

/// Sizes of the frames still in the channel, oldest first. The broadcast
/// channel keeps exactly the last `capacity` frames sent, so this mirrors it.
struct FrameWindow {
    sizes: VecDeque<Held>,
    held: usize,
    authors: Shares<Option<PeerId>>,
    relays: Shares<PeerId>,
    capacity: usize,
    max_bytes: usize,
    /// Frames refused since the last warning, by [`Refusal`].
    dropped: [u64; 3],
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
        let Some(frame) = self.sizes.pop_front() else {
            return;
        };
        self.held -= frame.len;
        self.authors.remove(frame.author, frame.len);
        self.relays.remove(frame.relay, frame.len);
    }

    /// Counts a refused frame. Returns the counts to report when a warning is
    /// due (the first refusal, then at most one per [`DROP_WARNING_INTERVAL`]),
    /// and starts the next count.
    fn refused(&mut self, why: Refusal, now: Instant) -> Option<[u64; 3]> {
        self.dropped[why as usize] += 1;
        if self
            .last_warning
            .is_some_and(|at| now.duration_since(at) < DROP_WARNING_INTERVAL)
        {
            return None;
        }
        self.last_warning = Some(now);
        Some(std::mem::take(&mut self.dropped))
    }

    /// Admits a frame, or refuses it if keeping it would take the channel
    /// past `max_bytes`, or its author or relay past a share of the bytes or
    /// the slots. Admitting a frame into a full channel evicts the oldest,
    /// exactly as the broadcast channel does.
    fn admit(&mut self, frame: Held) -> Result<(), Refusal> {
        let full = self.sizes.len() == self.capacity;
        let oldest = self.sizes.front().copied().filter(|_| full);
        let evicted = oldest.map_or(0, |o| o.len);
        if self.held - evicted + frame.len > self.max_bytes {
            return Err(Refusal::Budget);
        }
        let (max, slots) = (self.max_bytes, self.capacity);
        let by_author = oldest.map(|o| (o.author, o.len));
        if !self
            .authors
            .fits(frame.author, frame.len, by_author, max, slots)
        {
            return Err(Refusal::AuthorShare);
        }
        let by_relay = oldest.map(|o| (o.relay, o.len));
        if !self
            .relays
            .fits(frame.relay, frame.len, by_relay, max, slots)
        {
            return Err(Refusal::RelayShare);
        }
        if full {
            self.pop_oldest();
        }
        self.sizes.push_back(frame);
        self.held += frame.len;
        self.authors.add(frame.author, frame.len);
        self.relays.add(frame.relay, frame.len);
        Ok(())
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
            (AUTHOR_SHARES, RELAY_SHARES),
        )
    }

    #[cfg(test)]
    fn with_capacity(events: usize, frames: usize) -> Self {
        Self::with_limits(events, frames, MAX_HELD_FRAME_BYTES, (1, 1))
    }

    /// `shares` is the parts of the window per author and per relay.
    fn with_limits(
        events: usize,
        frames: usize,
        max_bytes: usize,
        (author_shares, relay_shares): (usize, usize),
    ) -> Self {
        Self {
            events: broadcast::channel(events).0,
            frames: broadcast::channel(frames).0,
            window: Arc::new(Mutex::new(FrameWindow {
                sizes: VecDeque::with_capacity(frames),
                held: 0,
                authors: Shares::new(author_shares),
                relays: Shares::new(relay_shares),
                // The channel rounds its capacity up the same way; the window
                // must evict exactly when the channel does.
                capacity: frames.next_power_of_two(),
                max_bytes,
                dropped: [0; 3],
                last_warning: None,
            })),
        }
    }

    /// A send failure only means nobody is subscribed, which is not an error.
    pub(super) fn emit(&self, event: NodeEvent) {
        let _ = self.events.send(event);
    }

    /// Hands a DAG-BFT frame from the consensus topic to the BFT loop, which
    /// checks its signatures and settles its gossip verdict. Returns `false`
    /// when nothing will: nobody is subscribed (a chain that does not run
    /// DAG-BFT), or the frame was refused for the byte budget or a share of it.
    pub(super) fn emit_frame(&self, frame: Arc<Frame>) -> bool {
        if self.frames.receiver_count() == 0 {
            return false;
        }
        // The lock covers the send, so the window and the channel agree on order.
        let mut window = self.window.lock().unwrap_or_else(PoisonError::into_inner);
        // FIFO: what `len` no longer counts is the oldest, read by everyone.
        window.trim(self.frames.len());
        let held = Held {
            author: frame.author,
            relay: frame.relay,
            len: frame.bytes.len(),
        };
        if let Err(why) = window.admit(held) {
            // Silent, this looked like lost transactions (a DAG stalled with
            // nothing in the log, bench-tps 2026-11-08).
            if let Some([budget, author, relay]) = window.refused(why, Instant::now()) {
                eprintln!(
                    "bft: refused consensus frames: {budget} over the {} MiB frame budget, {author} over one author's share, {relay} over one relay's share ({} bytes held unread); the engine re-fetches them",
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

    fn relayed(author: Option<PeerId>, relay: PeerId, bytes: Vec<u8>) -> Arc<Frame> {
        Arc::new(Frame {
            author,
            relay,
            id: MessageId::new(b"test"),
            bytes,
        })
    }

    /// A frame from a relay of its own.
    fn frame(author: Option<PeerId>, bytes: Vec<u8>) -> Arc<Frame> {
        relayed(author, PeerId::random(), bytes)
    }

    #[test]
    fn one_relay_signing_each_frame_with_a_fresh_key_cannot_fill_the_window() {
        // The author is whatever keypair signed the gossip message, so one
        // link could give every frame a new one and pass every author share.
        // 8 author shares and 2 relay shares of 80 bytes and 16 slots.
        let bus = Bus::with_limits(4, 16, 80, (8, 2));
        let _stalled = bus.subscribe_frames();
        let link = PeerId::random();
        let refused = (0..100)
            .filter(|_| !bus.emit_frame(relayed(Some(PeerId::random()), link, vec![0; 5])))
            .count();
        // Eight frames, its relay share of the slots and of the bytes.
        assert_eq!(refused, 92);
        let honest = PeerId::random();
        assert!(
            bus.emit_frame(relayed(Some(honest), honest, vec![1; 5])),
            "a frame over another link was refused"
        );
    }

    #[test]
    fn a_frame_nobody_will_check_is_refused() {
        // No BFT loop subscribed, so nothing would ever settle it: gossip
        // must drop it at once rather than hold it unrelayed, or relay it
        // unchecked.
        let bus = Bus::new();
        assert!(!bus.emit_frame(frame(Some(PeerId::random()), vec![7])));
    }

    #[test]
    fn one_author_flooding_frames_cannot_crowd_out_another() {
        // Audit P2P-N1: frames that decode are held before any signature is
        // checked, so one identity could fill the window and every honest
        // frame after it was refused. 8 shares of 80 bytes and 16 slots.
        let bus = Bus::with_limits(4, 16, 80, (8, 1));
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
        let bus = Bus::with_limits(4, 16, 80, (8, 1));
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
        let bus = Bus::with_limits(4, 8, 10, (1, 1));
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
        let bus = Bus::with_limits(4, 64, 10, (1, 1));
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
        let bus = Bus::with_limits(4, 64, 10, (1, 1));
        let _stalled = bus.subscribe_frames();
        assert!(bus.emit_frame(frame(None, vec![0; 6])));
        assert!(bus.emit_frame(frame(None, vec![0; 4])));
        assert!(!bus.emit_frame(frame(None, vec![0; 1])));
    }

    #[test]
    fn a_lagged_reader_is_still_counted_for_what_the_channel_holds() {
        let bus = Bus::with_limits(4, 2, 100, (1, 1));
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
        let bus = Bus::with_limits(4, 2, 10, (1, 1));
        let mut window = bus.window.lock().unwrap();
        let start = Instant::now();
        let later = |s| start + Duration::from_secs(s);
        assert_eq!(window.refused(Refusal::Budget, start), Some([1, 0, 0]));
        assert_eq!(window.refused(Refusal::Budget, later(1)), None);
        assert_eq!(window.refused(Refusal::RelayShare, later(9)), None);
        let due = start + DROP_WARNING_INTERVAL;
        assert_eq!(window.refused(Refusal::AuthorShare, due), Some([1, 1, 1]));
        assert_eq!(window.refused(Refusal::Budget, due), None);
    }

    #[test]
    fn an_evicted_frame_frees_its_bytes() {
        let bus = Bus::with_limits(4, 2, 10, (1, 1));
        // A receiver that never reads: with none, nothing is held at all.
        let _stalled = bus.subscribe_frames();
        assert!(bus.emit_frame(frame(None, vec![0; 5])));
        assert!(bus.emit_frame(frame(None, vec![0; 5])));
        // The channel is full; the next frame evicts a 5-byte one, so 10 fit.
        assert!(bus.emit_frame(frame(None, vec![0; 5])));
        assert!(!bus.emit_frame(frame(None, vec![0; 6])));
    }
}
