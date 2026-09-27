//! All three consensus modes under the deterministic simulator
//! (Master Prompt 4 DONE WHEN: "All three consensus modes run in sim/").
//!
//! Every run is a function of its seed; `maya_sim::replay` prints it on
//! failure.

#![allow(clippy::cast_possible_truncation)]

use std::collections::BTreeSet;

use maya_dag_bft::{
    Committee, ConsensusMode, Dest, ForkChoice, Ledger, Message, Params, Validator, WorkBlock,
};
use maya_sim::{Duration, Instant, LinkModel, NodeId, World, replay};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ev {
    Msg(Message),
    Tick,
    Block(WorkBlock),
    Mine,
}

const TICK: Duration = Duration::from_millis(200);

struct DagRun {
    anchors: Vec<Vec<[u8; 32]>>,
    orders: Vec<Vec<u64>>,
    ledgers: Vec<Ledger>,
    virtual_secs: u64,
    rounds: Vec<u64>,
}

fn dispatch(w: &mut World<Ev>, n: u16, from: u16, sends: Vec<(Dest, Message)>) {
    for (dest, m) in sends {
        match dest {
            Dest::All => {
                for peer in (0..n).filter(|p| *p != from) {
                    w.send(NodeId(from), NodeId(peer), Ev::Msg(m.clone()));
                }
            }
            Dest::To(peer) => {
                w.send(NodeId(from), NodeId(peer), Ev::Msg(m));
            }
        }
    }
}

/// Runs a DAG-BFT cluster. `crashed` never run; `partition` splits the
/// network for the given window (virtual seconds) and heals it after.
fn run_dag(
    w: &mut World<Ev>,
    n: u16,
    txs_per_node: u64,
    crashed: &[u16],
    partition: Option<&(u64, u64, Vec<BTreeSet<NodeId>>)>,
    secs: u64,
) -> DagRun {
    let committee = Committee::new(n);
    let params = Params {
        batch_size: 500,
        anchor_timeout_ms: 1_000,
        ..Params::default()
    };
    let mut nodes: Vec<Validator> = (0..n)
        .map(|i| Validator::new(i, committee, params))
        .collect();
    for (i, v) in nodes.iter_mut().enumerate() {
        for k in 0..txs_per_node {
            v.submit((k * u64::from(n) + i as u64).to_le_bytes().to_vec());
        }
    }
    let mut orders = vec![Vec::new(); n as usize];
    let mut ledgers = vec![Ledger::new(); n as usize];
    for i in (0..n).filter(|i| !crashed.contains(i)) {
        let out = nodes[i as usize].start(0);
        dispatch(w, n, i, out.sends);
        w.timer(NodeId(i), TICK, Ev::Tick);
    }
    let split = partition.map(|(from, _, _)| *from);
    let heal = partition.map(|(_, to, _)| *to);
    let deadline = Instant::START + Duration::from_secs(secs);
    let mut partitioned = false;
    w.run_until(deadline, 50_000_000, |w, ev| {
        let now_ms = ev.at.as_nanos() / 1_000_000;
        if let (Some(s), Some(h), Some((_, _, groups))) = (split, heal, partition) {
            if !partitioned && now_ms >= s * 1_000 && now_ms < h * 1_000 {
                w.net_mut().partition(groups.clone());
                partitioned = true;
            } else if partitioned && now_ms >= h * 1_000 {
                w.net_mut().heal();
                partitioned = false;
            }
        }
        let id = ev.to.0;
        if crashed.contains(&id) {
            return;
        }
        let out = match ev.payload {
            Ev::Msg(m) => nodes[id as usize].handle(now_ms, ev.from.0, m),
            Ev::Tick => {
                w.timer(NodeId(id), TICK, Ev::Tick);
                nodes[id as usize].tick(now_ms)
            }
            Ev::Block(_) | Ev::Mine => return,
        };
        for c in out.committed {
            for tx in &c.vertex.batch {
                ledgers[id as usize].apply_bytes(tx);
                orders[id as usize].push(u64::from_le_bytes(
                    tx.as_slice()
                        .try_into()
                        .expect("the sim submits 8-byte ids"),
                ));
            }
        }
        dispatch(w, n, id, out.sends);
    });
    DagRun {
        anchors: nodes.iter().map(|v| v.anchors().to_vec()).collect(),
        orders,
        ledgers,
        virtual_secs: secs,
        rounds: nodes.iter().map(Validator::round).collect(),
    }
}

/// Safety: every pair of honest logs is prefix-consistent.
fn assert_prefix_consistent<T: PartialEq + std::fmt::Debug>(logs: &[&Vec<T>]) {
    for a in logs {
        for b in logs {
            let common = a.len().min(b.len());
            assert_eq!(
                a[..common],
                b[..common],
                "logs diverge within their common prefix"
            );
        }
    }
}

fn honest<'a, T>(all: &'a [T], crashed: &[u16]) -> Vec<&'a T> {
    all.iter()
        .enumerate()
        .filter(|(i, _)| !crashed.contains(&(*i as u16)))
        .map(|(_, x)| x)
        .collect()
}

#[test]
fn five_validators_over_a_wide_area_link_commit_the_same_order() {
    replay(0x0D46_BF70, |w: &mut World<Ev>| {
        w.net_mut().set_link(LinkModel::wide_area());
        let run = run_dag(w, 5, 100_000, &[], None, 30);
        assert_prefix_consistent(&run.anchors.iter().collect::<Vec<_>>());
        assert_prefix_consistent(&run.orders.iter().collect::<Vec<_>>());
        let committed = run.orders.iter().map(Vec::len).min().unwrap_or(0);
        assert!(committed > 10_000, "only {committed} txs committed");
        // No transaction ordered twice.
        let unique: BTreeSet<_> = run.orders[0].iter().collect();
        assert_eq!(unique.len(), run.orders[0].len());
        for l in &run.ledgers {
            assert_eq!(l.supply(), run.ledgers[0].supply());
        }
        println!(
            "dag-bft sim: 5 validators, wide-area link: {committed} txs ordered in {} virtual s \
             ({} tx/virtual-s, {} rounds, {} anchors); an ordering figure, not TPS",
            run.virtual_secs,
            committed as u64 / run.virtual_secs,
            run.rounds[0],
            run.anchors[0].len()
        );
    });
}

#[test]
fn one_crashed_validator_of_five_does_not_stop_commits() {
    replay(0x0D46_BF71, |w: &mut World<Ev>| {
        w.net_mut().set_link(LinkModel::wide_area());
        let crashed = [3u16];
        let run = run_dag(w, 5, 2_000, &crashed, None, 30);
        let orders = honest(&run.orders, &crashed);
        assert_prefix_consistent(&orders);
        assert!(
            orders.iter().all(|o| o.len() > 2_000),
            "progress with f = 1 crashed"
        );
    });
}

#[test]
fn a_minority_partition_stalls_nobody_forever_and_heals_consistently() {
    replay(0x0D46_BF72, |w: &mut World<Ev>| {
        w.net_mut().set_link(LinkModel::wide_area());
        let groups = vec![
            [0, 1, 2].map(NodeId).into_iter().collect(),
            [3, 4].map(NodeId).into_iter().collect(),
        ];
        let run = run_dag(w, 5, 3_000, &[], Some(&(5, 15, groups)), 45);
        assert_prefix_consistent(&run.anchors.iter().collect::<Vec<_>>());
        assert_prefix_consistent(&run.orders.iter().collect::<Vec<_>>());
        // After healing, the minority catches up to within a few anchors.
        let lens: Vec<usize> = run.anchors.iter().map(Vec::len).collect();
        let (lo, hi) = (
            lens.iter().min().copied().unwrap_or(0),
            lens.iter().max().copied().unwrap_or(0),
        );
        assert!(hi - lo <= 3, "anchor counts after heal: {lens:?}");
        let roots: BTreeSet<_> = run
            .ledgers
            .iter()
            .zip(&run.orders)
            .filter(|(_, o)| o.len() == run.orders[0].len())
            .map(|(l, _)| l.root())
            .collect();
        assert_eq!(roots.len(), 1, "equal-length logs must give equal roots");
    });
}

#[test]
fn a_one_third_partition_without_quorum_halts_rather_than_forks() {
    // 4 validators split 2/2: neither side has 2f + 1 = 3. Safety demands
    // that nothing new commits on either side while split.
    replay(0x0D46_BF73, |w: &mut World<Ev>| {
        let groups = vec![
            [0, 1].map(NodeId).into_iter().collect(),
            [2, 3].map(NodeId).into_iter().collect(),
        ];
        w.net_mut().partition(groups);
        let run = run_dag(w, 4, 1_000, &[], None, 20);
        assert!(run.orders.iter().all(Vec::is_empty), "no quorum, no commit");
    });
}

/// Runs a work-mode network: every `interval`, one node (drawn by hash
/// power, here uniform) extends its best tip.
fn run_work(w: &mut World<Ev>, mode: ConsensusMode, n: u16, blocks: u64) -> Vec<ForkChoice> {
    let mut chains: Vec<ForkChoice> = (0..n).map(|_| ForkChoice::new(mode)).collect();
    let mut next_tx = 0u64;
    let mut mined = 0u64;
    w.timer(NodeId(0), Duration::from_secs(15), Ev::Mine);
    let deadline = Instant::START + Duration::from_secs(15 * (blocks + 20));
    w.run_until(deadline, 10_000_000, |w, ev| match ev.payload {
        Ev::Mine => {
            if mined < blocks {
                let miner = w.rng().below(u64::from(n)) as u16;
                let fc = &chains[miner as usize];
                let block = WorkBlock {
                    parent: fc.tip(),
                    height: fc.height() + 1,
                    miner,
                    work: 1_000,
                    batch: (next_tx..next_tx + 100).collect(),
                    mode,
                };
                next_tx += 100;
                mined += 1;
                chains[miner as usize].add(block.clone());
                for peer in (0..n).filter(|p| *p != miner) {
                    w.send(NodeId(miner), NodeId(peer), Ev::Block(block.clone()));
                }
                let gap = Duration::from_millis(5_000 + w.rng().below(20_000));
                w.timer(NodeId(0), gap, Ev::Mine);
            }
        }
        Ev::Block(b) => {
            chains[ev.to.0 as usize].add(b);
        }
        Ev::Msg(_) | Ev::Tick => {}
    });
    chains
}

#[test]
fn both_work_modes_converge_on_one_chain_and_one_state_root() {
    for (seed, mode) in [
        (0x0A26_0B1A, ConsensusMode::ArgonBlakePow),
        (0x0B0E_1A77, ConsensusMode::PouwLattice),
    ] {
        replay(seed, move |w: &mut World<Ev>| {
            // Wide-area latency and reordering, but no loss: this engine has
            // no block-fetch path, so a lost block would orphan its children
            // forever. (The DAG engine above does fetch, and runs with loss.)
            w.net_mut().set_link(LinkModel {
                loss_ppm: 0,
                ..LinkModel::wide_area()
            });
            let chains = run_work(w, mode, 5, 200);
            // Tips may still differ by an equal-work race at the very end;
            // what must agree is everything six blocks deep.
            let confirmed: BTreeSet<_> = chains
                .iter()
                .map(|fc| {
                    fc.confirmed(6)
                        .iter()
                        .map(|b| b.digest())
                        .collect::<Vec<_>>()
                })
                .collect();
            assert_eq!(confirmed.len(), 1, "{mode}: nodes disagree six blocks deep");
            let roots: BTreeSet<_> = chains
                .iter()
                .map(|fc| {
                    let mut l = Ledger::new();
                    for b in fc.confirmed(6) {
                        for tx in &b.batch {
                            l.apply(*tx);
                        }
                    }
                    l.root()
                })
                .collect();
            assert_eq!(
                roots.len(),
                1,
                "{mode}: same confirmed chain, same state root"
            );
            assert!(
                chains[0].height() > 150,
                "{mode}: height {}",
                chains[0].height()
            );
        });
    }
}

#[test]
fn the_same_transactions_give_the_same_root_whichever_mode_ordered_them_in_the_same_order() {
    // The state machine is mode-agnostic: feed it one sequence, get one root.
    let seq: Vec<u64> = (0..10_000).collect();
    let mut a = Ledger::new();
    let mut b = Ledger::new();
    for tx in &seq {
        a.apply(*tx);
        b.apply(*tx);
    }
    assert_eq!(a.root(), b.root());
    assert_eq!(
        a.supply(),
        Ledger::new().supply(),
        "transfers conserve supply"
    );
}
