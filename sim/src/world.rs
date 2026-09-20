//! The scheduler: a deterministic discrete-event loop over virtual time.
//!
//! Everything a simulation does goes through here, so that "what happened"
//! is a function of the seed and nothing else. In particular the event queue
//! breaks ties by a monotonically increasing sequence number rather than by
//! whatever order a hash map happened to produce — two events at the same
//! virtual nanosecond must come out in the same order on every machine and
//! every run, or a replay is not a replay.

use crate::clock::{Clock, Duration, Instant};
use crate::disk::{Disk, WriteOutcome};
use crate::net::{Network, NodeId, Verdict};
use crate::rng::SimRng;
use std::collections::BinaryHeap;

/// One thing that happens to a node at a point in virtual time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event<M> {
    /// When it happens.
    pub at: Instant,
    /// Who it happens to.
    pub to: NodeId,
    /// Who caused it. Equal to `to` for a timer.
    pub from: NodeId,
    /// The payload.
    pub payload: M,
}

/// Queue entry. Ordered so that `BinaryHeap` (a max-heap) pops the *earliest*
/// event, and ties break on insertion order.
#[derive(Debug, PartialEq, Eq)]
struct Queued<M> {
    at: Instant,
    seq: u64,
    to: NodeId,
    from: NodeId,
    payload: M,
}

impl<M: Eq> Ord for Queued<M> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reversed: earliest time first, then lowest sequence number.
        other
            .at
            .cmp(&self.at)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

impl<M: Eq> PartialOrd for Queued<M> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// What a `send` did, for a caller that wants to assert on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sent {
    /// Queued for delivery after this delay.
    Queued(Duration),
    /// The network dropped it.
    Lost,
    /// Sender and receiver are partitioned apart.
    Partitioned,
}

/// A running simulation.
///
/// `M` is whatever the caller wants to pass between nodes. The harness never
/// inspects it, so a test can carry real block or transaction types.
#[derive(Debug)]
pub struct World<M> {
    seed: u64,
    clock: Clock,
    net: Network,
    disk: Disk,
    net_rng: SimRng,
    disk_rng: SimRng,
    user_rng: SimRng,
    queue: BinaryHeap<Queued<M>>,
    seq: u64,
    delivered: u64,
    dropped: u64,
}

impl<M: Eq> World<M> {
    /// A world from a seed, with a perfect network and a perfect disk.
    ///
    /// Each model gets its own stream, so adding a draw to one of them does
    /// not shift the others — the change that otherwise silently invalidates
    /// every recorded seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut root = SimRng::new(seed);
        Self {
            seed,
            clock: Clock::new(),
            net: Network::new(),
            disk: Disk::new(),
            net_rng: root.fork(),
            disk_rng: root.fork(),
            user_rng: root.fork(),
            queue: BinaryHeap::new(),
            seq: 0,
            delivered: 0,
            dropped: 0,
        }
    }

    /// The seed this world was built from. Print it on failure.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The current virtual time.
    #[must_use]
    pub const fn now(&self) -> Instant {
        self.clock.now()
    }

    /// The network model, for partitioning or changing the link.
    pub fn net_mut(&mut self) -> &mut Network {
        &mut self.net
    }

    /// The disk model.
    pub fn disk_mut(&mut self) -> &mut Disk {
        &mut self.disk
    }

    /// A stream for the *test's* own choices, kept separate from the models'.
    pub fn rng(&mut self) -> &mut SimRng {
        &mut self.user_rng
    }

    /// A private stream derived from the test's, for a node that wants one.
    pub fn fork_rng(&mut self) -> SimRng {
        self.user_rng.fork()
    }

    /// How many messages have been delivered, and how many the network ate.
    #[must_use]
    pub const fn counts(&self) -> (u64, u64) {
        (self.delivered, self.dropped)
    }

    /// Send `payload` from `from` to `to`, subject to the network model.
    pub fn send(&mut self, from: NodeId, to: NodeId, payload: M) -> Sent {
        match self.net.deliver(from, to, &mut self.net_rng) {
            Verdict::Deliver(delay) => {
                self.push(self.clock.now().saturating_add(delay), to, from, payload);
                Sent::Queued(delay)
            }
            Verdict::Lost => {
                self.dropped += 1;
                Sent::Lost
            }
            Verdict::Partitioned => {
                self.dropped += 1;
                Sent::Partitioned
            }
        }
    }

    /// Deliver `payload` to `to` after `delay`, bypassing the network.
    ///
    /// This is a timer, not a message: it cannot be lost or partitioned,
    /// because a node's own timer does not travel over the network.
    pub fn timer(&mut self, to: NodeId, delay: Duration, payload: M) {
        self.push(self.clock.now().saturating_add(delay), to, to, payload);
    }

    fn push(&mut self, at: Instant, to: NodeId, from: NodeId, payload: M) {
        self.queue.push(Queued {
            at,
            seq: self.seq,
            to,
            from,
            payload,
        });
        self.seq += 1;
    }

    /// Ask the disk model about a write of `len` bytes.
    pub fn write(&mut self, len: usize) -> WriteOutcome {
        self.disk.write(len, &mut self.disk_rng)
    }

    /// Advance to the next event and return it, or `None` when the queue is
    /// empty. Time jumps straight to the event: nothing waits.
    pub fn step(&mut self) -> Option<Event<M>> {
        let q = self.queue.pop()?;
        self.clock.advance_to(q.at);
        self.delivered += 1;
        Some(Event {
            at: q.at,
            to: q.to,
            from: q.from,
            payload: q.payload,
        })
    }

    /// Run until the queue is empty or `deadline` passes, handing each event
    /// to `handler`. The handler may schedule more events.
    ///
    /// `budget` bounds the number of events so a livelock in the code under
    /// test fails the run instead of hanging CI.
    pub fn run_until<F>(&mut self, deadline: Instant, budget: u64, mut handler: F) -> RunOutcome
    where
        F: FnMut(&mut Self, Event<M>),
    {
        let mut steps = 0;
        loop {
            match self.queue.peek() {
                None => return RunOutcome::Drained,
                Some(next) if next.at > deadline => {
                    self.clock.advance_to(deadline);
                    return RunOutcome::DeadlineReached;
                }
                Some(_) => {}
            }
            if steps >= budget {
                return RunOutcome::BudgetExhausted;
            }
            let Some(event) = self.step() else {
                return RunOutcome::Drained;
            };
            handler(self, event);
            steps += 1;
        }
    }
}

/// Why a run stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RunOutcome {
    /// Nothing left to do.
    #[default]
    Drained,
    /// The deadline arrived with events still queued.
    DeadlineReached,
    /// The event budget ran out — usually a livelock in the code under test.
    BudgetExhausted,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::net::LinkModel;
    use std::collections::BTreeSet;

    #[test]
    fn events_come_out_in_time_order() {
        let mut w: World<u32> = World::new(1);
        w.timer(NodeId(0), Duration::from_millis(30), 3);
        w.timer(NodeId(0), Duration::from_millis(10), 1);
        w.timer(NodeId(0), Duration::from_millis(20), 2);
        let order: Vec<u32> = std::iter::from_fn(|| w.step().map(|e| e.payload)).collect();
        assert_eq!(order, vec![1, 2, 3]);
    }

    #[test]
    fn events_at_the_same_instant_keep_insertion_order() {
        let mut w: World<u32> = World::new(2);
        for i in 0..100 {
            w.timer(NodeId(0), Duration::ZERO, i);
        }
        let order: Vec<u32> = std::iter::from_fn(|| w.step().map(|e| e.payload)).collect();
        assert_eq!(order, (0..100).collect::<Vec<u32>>());
    }

    #[test]
    fn the_clock_is_at_the_event_that_was_just_delivered() {
        let mut w: World<()> = World::new(3);
        w.timer(NodeId(0), Duration::from_secs(5), ());
        assert_eq!(w.now(), Instant::START);
        let e = w.step().expect("one event");
        assert_eq!(w.now(), e.at);
        assert_eq!(w.now().since(Instant::START), Duration::from_secs(5));
    }

    #[test]
    fn a_partition_stops_a_send_and_is_counted() {
        let mut w: World<u8> = World::new(4);
        let groups = vec![
            [NodeId(0)].into_iter().collect::<BTreeSet<_>>(),
            [NodeId(1)].into_iter().collect::<BTreeSet<_>>(),
        ];
        w.net_mut().partition(groups);
        assert_eq!(w.send(NodeId(0), NodeId(1), 7), Sent::Partitioned);
        assert!(w.step().is_none());
        assert_eq!(w.counts(), (0, 1));
        w.net_mut().heal();
        assert!(matches!(w.send(NodeId(0), NodeId(1), 7), Sent::Queued(_)));
        assert!(w.step().is_some());
    }

    #[test]
    fn a_timer_survives_a_partition_because_it_is_not_a_message() {
        let mut w: World<u8> = World::new(5);
        w.net_mut()
            .partition(vec![[NodeId(0)].into_iter().collect::<BTreeSet<_>>()]);
        w.timer(NodeId(0), Duration::from_millis(1), 9);
        assert_eq!(w.step().map(|e| e.payload), Some(9));
    }

    #[test]
    fn the_budget_stops_a_livelock_rather_than_hanging() {
        let mut w: World<u32> = World::new(6);
        w.timer(NodeId(0), Duration::from_millis(1), 0);
        let outcome = w.run_until(Instant::START + Duration::from_secs(60), 500, |w, e| {
            // Each event schedules another, forever.
            w.timer(e.to, Duration::from_millis(1), e.payload + 1);
        });
        assert_eq!(outcome, RunOutcome::BudgetExhausted);
    }

    #[test]
    fn a_deadline_leaves_later_events_queued() {
        let mut w: World<u32> = World::new(7);
        w.timer(NodeId(0), Duration::from_secs(1), 1);
        w.timer(NodeId(0), Duration::from_secs(100), 2);
        let mut seen = Vec::new();
        let outcome = w.run_until(Instant::START + Duration::from_secs(10), 1000, |_, e| {
            seen.push(e.payload);
        });
        assert_eq!(outcome, RunOutcome::DeadlineReached);
        assert_eq!(seen, vec![1]);
        assert_eq!(w.now().since(Instant::START), Duration::from_secs(10));
    }

    #[test]
    fn the_same_seed_produces_the_same_trace() {
        fn trace(seed: u64) -> Vec<(u64, u16, u32)> {
            let mut w: World<u32> = World::new(seed);
            w.net_mut().set_link(LinkModel::wide_area());
            for i in 0..200u32 {
                let from = NodeId((i % 5) as u16);
                let to = NodeId(((i + 1) % 5) as u16);
                w.send(from, to, i);
            }
            let mut out = Vec::new();
            w.run_until(Instant::START + Duration::from_secs(10), 10_000, |_, e| {
                out.push((e.at.as_nanos(), e.to.0, e.payload));
            });
            out
        }
        assert_eq!(trace(0xDEAD_BEEF), trace(0xDEAD_BEEF));
        assert_ne!(trace(0xDEAD_BEEF), trace(0xDEAD_BEEE));
    }

    #[test]
    fn the_disk_stream_is_independent_of_the_network_stream() {
        // Sending more messages must not change what the disk decides.
        let writes = |sends: u32| {
            let mut w: World<u32> = World::new(11);
            w.net_mut().set_link(LinkModel::wide_area());
            w.disk_mut().set_model(crate::disk::DiskModel::failing());
            for i in 0..sends {
                w.send(NodeId(0), NodeId(1), i);
            }
            (0..50).map(|_| w.write(4096)).collect::<Vec<_>>()
        };
        assert_eq!(writes(0), writes(500));
    }
}
