//! The property the whole harness exists for: a seed reproduces a run.
//!
//! These drive a small multi-node gossip protocol through latency, loss,
//! reordering, a partition that heals, and a failing disk — the combination a
//! chaos test would use — and check that two runs of the same seed are
//! byte-identical while two runs of different seeds are not.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use maya_sim::disk::DiskModel;
use maya_sim::{
    Duration, Event, Instant, LinkModel, NodeId, RunOutcome, Sent, SimRng, World, WriteOutcome,
};

const NODES: u16 = 7;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Msg {
    /// A block at this height, from this author.
    Block { height: u64, author: u16 },
    /// A node's own timer: produce a block.
    Tick,
}

/// What a run produced, in an order that does not depend on a hash map.
#[derive(PartialEq, Eq, Debug, Default)]
struct Trace {
    /// (virtual nanos, receiver, message) for every delivery.
    deliveries: Vec<(u64, u16, Msg)>,
    /// Per node, the highest block height it saw.
    tips: BTreeMap<u16, u64>,
    /// Every write outcome, in order.
    writes: Vec<WriteOutcome>,
    /// Delivered and dropped counts.
    counts: (u64, u64),
    /// Why the run stopped.
    outcome: RunOutcome,
}

fn group(ids: &[u16]) -> BTreeSet<NodeId> {
    ids.iter().copied().map(NodeId).collect()
}

/// A gossip run: every node ticks, produces a block, floods it, and records
/// the highest height it has seen. Halfway through, the network splits.
fn run(seed: u64, partition_at: Option<Duration>) -> Trace {
    let mut world: World<Msg> = World::new(seed);
    world.net_mut().set_link(LinkModel {
        min_latency: Duration::from_millis(5),
        max_latency: Duration::from_millis(60),
        loss_ppm: 20_000,    // 2%
        reorder_ppm: 30_000, // 3%
        reorder_delay: Duration::from_millis(25),
    });
    world.disk_mut().set_model(DiskModel::failing());

    for n in 0..NODES {
        world.timer(
            NodeId(n),
            Duration::from_millis(u64::from(n) * 7),
            Msg::Tick,
        );
    }

    let mut trace = Trace::default();
    let mut heights: BTreeMap<u16, u64> = BTreeMap::new();
    let mut split_done = false;

    let outcome = world.run_until(
        Instant::START + Duration::from_secs(3),
        20_000,
        |w, event: Event<Msg>| {
            trace
                .deliveries
                .push((event.at.as_nanos(), event.to.0, event.payload));

            if let Some(at) = partition_at
                && !split_done
                && event.at.since(Instant::START) >= at
            {
                w.net_mut()
                    .partition(vec![group(&[0, 1, 2]), group(&[3, 4, 5, 6])]);
                split_done = true;
            }

            match event.payload {
                Msg::Tick => {
                    let h = heights.entry(event.to.0).or_default();
                    *h += 1;
                    let height = *h;
                    // Writing the block is allowed to fail; the point is that
                    // the *decision* is recorded and reproducible.
                    trace.writes.push(w.write(4096));
                    for peer in 0..NODES {
                        if peer != event.to.0 {
                            let sent = w.send(
                                event.to,
                                NodeId(peer),
                                Msg::Block {
                                    height,
                                    author: event.to.0,
                                },
                            );
                            // A lost send is ordinary, not an error.
                            let _ = matches!(sent, Sent::Lost | Sent::Partitioned);
                        }
                    }
                    if height < 12 {
                        w.timer(event.to, Duration::from_millis(100), Msg::Tick);
                    }
                }
                Msg::Block { height, .. } => {
                    let tip = heights.entry(event.to.0).or_default();
                    if height > *tip {
                        *tip = height;
                    }
                }
            }
        },
    );

    trace.outcome = outcome;
    trace.tips = heights;
    trace.counts = world.counts();
    trace
}

#[test]
fn the_same_seed_reproduces_a_whole_run() {
    for seed in [1u64, 0xDEAD_BEEF, u64::MAX / 3] {
        let a = run(seed, None);
        let b = run(seed, None);
        assert_eq!(a, b, "seed {seed:#x} did not reproduce");
        assert!(!a.deliveries.is_empty(), "seed {seed:#x} did nothing");
    }
}

#[test]
fn different_seeds_produce_different_runs() {
    let a = run(1, None);
    let b = run(2, None);
    assert_ne!(a.deliveries, b.deliveries);
}

#[test]
fn a_partition_is_reproducible_and_changes_the_outcome() {
    let split = Duration::from_millis(400);
    let with_split_a = run(99, Some(split));
    let with_split_b = run(99, Some(split));
    assert_eq!(
        with_split_a, with_split_b,
        "a partitioned run did not replay"
    );

    let without = run(99, None);
    assert_ne!(
        with_split_a.deliveries, without.deliveries,
        "the partition changed nothing, so it was not applied"
    );
    assert!(
        with_split_a.counts.1 > without.counts.1,
        "a partition should drop more than no partition: {:?} vs {:?}",
        with_split_a.counts,
        without.counts
    );
}

#[test]
fn every_node_makes_progress_when_the_network_is_whole() {
    let trace = run(7, None);
    for n in 0..NODES {
        let tip = trace.tips.get(&n).copied().unwrap_or_default();
        assert!(tip > 0, "node {n} never reached a block: {:?}", trace.tips);
    }
}

#[test]
fn the_run_finishes_inside_its_budget() {
    let trace = run(13, None);
    assert_ne!(
        trace.outcome,
        RunOutcome::BudgetExhausted,
        "the scenario livelocked"
    );
}

#[test]
fn the_disk_is_consulted_during_a_run_and_replays_with_it() {
    let a = run(0xF00D, None);
    let b = run(0xF00D, None);
    assert!(!a.writes.is_empty(), "no write was recorded");
    assert_eq!(a.writes, b.writes, "the write sequence did not replay");
}

#[test]
fn a_faulty_disk_produces_faults_and_they_replay() {
    // Deliberately not asserted against `run()`: `DiskModel::failing` is a
    // *rate*, and a run of eighty-odd writes can legitimately see none. A
    // test that depends on that is flaky by construction. This asks a model
    // that always fails for the property instead.
    let sample = |seed: u64| {
        let mut w: World<Msg> = World::new(seed);
        w.disk_mut().set_model(DiskModel {
            corrupt_ppm: 250_000,
            torn_ppm: 250_000,
            lost_on_crash_ppm: 250_000,
            ..DiskModel::default()
        });
        (0..500)
            .map(|_| w.write(4096))
            .collect::<Vec<WriteOutcome>>()
    };
    let first = sample(0x0BAD_D15C);
    assert_eq!(first, sample(0x0BAD_D15C), "the disk did not replay");
    let clean = first
        .iter()
        .filter(|w| matches!(w, WriteOutcome::Durable(_)))
        .count();
    assert!(
        clean < first.len(),
        "a disk set to fail three times in four produced no faults in {} writes",
        first.len()
    );
    assert!(
        clean > 0,
        "every write failed, which is not what was configured"
    );
}

#[test]
fn a_forked_stream_does_not_disturb_the_models() {
    // A test drawing its own randomness must not change what the network and
    // the disk decide, or every seed changes meaning when a test adds a draw.
    let sample = |draws: usize| {
        let mut w: World<Msg> = World::new(555);
        w.net_mut().set_link(LinkModel::wide_area());
        w.disk_mut().set_model(DiskModel::failing());
        let mut side: SimRng = w.fork_rng();
        for _ in 0..draws {
            side.next_u64();
        }
        let sends: Vec<Sent> = (0..64)
            .map(|i| {
                w.send(
                    NodeId(0),
                    NodeId(1),
                    Msg::Block {
                        height: i,
                        author: 0,
                    },
                )
            })
            .collect();
        let writes: Vec<WriteOutcome> = (0..64).map(|_| w.write(4096)).collect();
        (sends, writes)
    };
    assert_eq!(sample(0), sample(10_000));
}
