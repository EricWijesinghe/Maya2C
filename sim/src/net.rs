//! What the network does to a message between `send` and `deliver`.
//!
//! Four things, because these are the four that break consensus code in ways
//! unit tests do not: it takes a variable amount of time, it sometimes loses
//! the message, it sometimes hands two messages over in the wrong order, and
//! sometimes a set of nodes cannot reach another set at all.
//!
//! Every decision is drawn from the simulation's seed, so the same seed
//! produces the same latencies, the same losses and the same partitions.

use crate::clock::Duration;
use crate::rng::{PPM, SimRng};
use std::collections::BTreeSet;

/// A node in the simulated network.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct NodeId(pub u16);

/// How a link behaves.
///
/// The defaults are a *perfect* network — no latency, no loss, no reordering.
/// A test that wants chaos asks for it, so a test that does not ask cannot
/// fail for a reason it never opted into.
#[derive(Clone, Debug)]
pub struct LinkModel {
    /// Lower bound on one-way delay.
    pub min_latency: Duration,
    /// Upper bound on one-way delay. Must be at least `min_latency`.
    pub max_latency: Duration,
    /// Probability, in parts per million, that a message is dropped.
    pub loss_ppm: u32,
    /// Probability, in parts per million, that a message is held back and
    /// delivered after the one behind it.
    pub reorder_ppm: u32,
    /// Extra delay applied to a reordered message.
    pub reorder_delay: Duration,
}

impl Default for LinkModel {
    fn default() -> Self {
        Self {
            min_latency: Duration::ZERO,
            max_latency: Duration::ZERO,
            loss_ppm: 0,
            reorder_ppm: 0,
            reorder_delay: Duration::ZERO,
        }
    }
}

impl LinkModel {
    /// A plausible wide-area link: 20-80 ms, 0.1% loss, 0.5% reordering.
    ///
    /// The numbers are a starting point for a test to override, not a claim
    /// about any real network.
    #[must_use]
    pub fn wide_area() -> Self {
        Self {
            min_latency: Duration::from_millis(20),
            max_latency: Duration::from_millis(80),
            loss_ppm: 1_000,
            reorder_ppm: 5_000,
            reorder_delay: Duration::from_millis(40),
        }
    }

    /// A link that drops everything, for modelling a dead peer rather than a
    /// partition (a partition is symmetric; a dead peer is not).
    #[must_use]
    pub fn black_hole() -> Self {
        Self {
            loss_ppm: PPM,
            ..Self::default()
        }
    }
}

/// What the network decided to do with one message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Deliver after this delay.
    Deliver(Duration),
    /// Dropped by the loss model.
    Lost,
    /// Dropped because the sender and receiver are in different partitions.
    Partitioned,
}

/// The network model: one link model for everyone, plus partitions.
#[derive(Clone, Debug, Default)]
pub struct Network {
    link: LinkModel,
    /// Each entry is a set of nodes that can reach each other. Nodes in
    /// different sets cannot. An empty list means one connected network.
    partitions: Vec<BTreeSet<NodeId>>,
}

impl Network {
    /// A perfect, fully connected network.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A network whose every link behaves like `link`.
    #[must_use]
    pub fn with_link(link: LinkModel) -> Self {
        Self {
            link,
            partitions: Vec::new(),
        }
    }

    /// Replace the link model.
    pub fn set_link(&mut self, link: LinkModel) {
        self.link = link;
    }

    /// The link model in force.
    #[must_use]
    pub fn link(&self) -> &LinkModel {
        &self.link
    }

    /// Split the network into groups that cannot reach each other.
    ///
    /// A node named in no group is isolated from every node that *is* named,
    /// which is the usual way a test expresses "one node falls off the
    /// network" without listing everybody else.
    pub fn partition(&mut self, groups: Vec<BTreeSet<NodeId>>) {
        self.partitions = groups;
    }

    /// Remove every partition.
    pub fn heal(&mut self) {
        self.partitions.clear();
    }

    /// Whether `a` and `b` can currently exchange messages.
    #[must_use]
    pub fn reachable(&self, a: NodeId, b: NodeId) -> bool {
        if self.partitions.is_empty() {
            return true;
        }
        if a == b {
            return true;
        }
        self.partitions
            .iter()
            .any(|g| g.contains(&a) && g.contains(&b))
    }

    /// Decide what happens to one message.
    ///
    /// Draw order is fixed — partition, then loss, then latency, then
    /// reordering — because changing it changes every seed's meaning.
    pub fn deliver(&self, from: NodeId, to: NodeId, rng: &mut SimRng) -> Verdict {
        if !self.reachable(from, to) {
            return Verdict::Partitioned;
        }
        if rng.chance(self.link.loss_ppm) {
            return Verdict::Lost;
        }
        let lo = self.link.min_latency.as_nanos();
        let hi = self.link.max_latency.as_nanos().max(lo);
        let mut delay = Duration::from_nanos(rng.between(lo, hi));
        if rng.chance(self.link.reorder_ppm) {
            delay = delay + self.link.reorder_delay;
        }
        Verdict::Deliver(delay)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn group(ids: &[u16]) -> BTreeSet<NodeId> {
        ids.iter().copied().map(NodeId).collect()
    }

    #[test]
    fn a_default_network_delivers_everything_immediately() {
        let net = Network::new();
        let mut rng = SimRng::new(1);
        for _ in 0..1000 {
            assert_eq!(
                net.deliver(NodeId(0), NodeId(1), &mut rng),
                Verdict::Deliver(Duration::ZERO)
            );
        }
    }

    #[test]
    fn a_partition_blocks_both_directions_and_healing_restores_it() {
        let mut net = Network::new();
        net.partition(vec![group(&[0, 1]), group(&[2, 3])]);
        let mut rng = SimRng::new(2);
        assert_eq!(
            net.deliver(NodeId(0), NodeId(2), &mut rng),
            Verdict::Partitioned
        );
        assert_eq!(
            net.deliver(NodeId(2), NodeId(0), &mut rng),
            Verdict::Partitioned
        );
        assert!(matches!(
            net.deliver(NodeId(0), NodeId(1), &mut rng),
            Verdict::Deliver(_)
        ));
        net.heal();
        assert!(matches!(
            net.deliver(NodeId(0), NodeId(2), &mut rng),
            Verdict::Deliver(_)
        ));
    }

    #[test]
    fn a_node_named_in_no_group_is_isolated() {
        let mut net = Network::new();
        net.partition(vec![group(&[0, 1, 2])]);
        let mut rng = SimRng::new(3);
        assert_eq!(
            net.deliver(NodeId(0), NodeId(9), &mut rng),
            Verdict::Partitioned
        );
    }

    #[test]
    fn a_node_can_always_reach_itself() {
        let mut net = Network::new();
        net.partition(vec![group(&[1]), group(&[2])]);
        assert!(net.reachable(NodeId(1), NodeId(1)));
    }

    #[test]
    fn latency_stays_inside_the_stated_bounds() {
        let net = Network::with_link(LinkModel {
            min_latency: Duration::from_millis(20),
            max_latency: Duration::from_millis(80),
            ..LinkModel::default()
        });
        let mut rng = SimRng::new(4);
        for _ in 0..5_000 {
            match net.deliver(NodeId(0), NodeId(1), &mut rng) {
                Verdict::Deliver(d) => {
                    assert!(
                        (20..=80).contains(&d.as_millis()),
                        "latency {d} outside 20..=80ms"
                    );
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn loss_is_drawn_at_roughly_the_stated_rate() {
        let net = Network::with_link(LinkModel {
            loss_ppm: PPM / 4,
            ..LinkModel::default()
        });
        let mut rng = SimRng::new(5);
        let lost = (0..20_000)
            .filter(|_| net.deliver(NodeId(0), NodeId(1), &mut rng) == Verdict::Lost)
            .count();
        assert!(
            (4_000..6_000).contains(&lost),
            "25% of 20000 landed at {lost}"
        );
    }

    #[test]
    fn a_black_hole_link_delivers_nothing() {
        let net = Network::with_link(LinkModel::black_hole());
        let mut rng = SimRng::new(6);
        for _ in 0..1000 {
            assert_eq!(net.deliver(NodeId(0), NodeId(1), &mut rng), Verdict::Lost);
        }
    }

    #[test]
    fn the_same_seed_replays_the_same_verdicts() {
        let net = Network::with_link(LinkModel::wide_area());
        let run = |seed| {
            let mut rng = SimRng::new(seed);
            (0..500)
                .map(|i| net.deliver(NodeId(0), NodeId(1 + (i % 3)), &mut rng))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(77), run(77));
        assert_ne!(run(77), run(78));
    }
}
