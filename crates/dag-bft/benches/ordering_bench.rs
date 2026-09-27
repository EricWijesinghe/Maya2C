//! Real CPU cost of DAG-BFT ordering: how many transaction references per
//! second one core can certify-and-order when message delivery is free.
//!
//! This isolates the consensus engine from the network and from execution.
//! It is an **ordering** figure — no signature is verified, no transaction is
//! executed, nothing is persisted — and must never be quoted as TPS (the
//! Production Standing Orders' definition). It answers one question: is the
//! commit rule itself a bottleneck? Run with
//! `cargo bench -p maya-dag-bft --bench ordering_bench`.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::collections::VecDeque;
use std::time::Instant;

use maya_dag_bft::{Committee, Dest, Params, Validator};

fn run(n: u16, batch: usize, txs_per_node: u64) -> (u64, f64, u64) {
    let committee = Committee::new(n);
    let params = Params {
        batch_size: batch,
        anchor_timeout_ms: 0,
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
    let mut queue = VecDeque::new();
    for i in 0..n {
        for (d, m) in nodes[i as usize].start(0).sends {
            queue.push_back((i, d, m));
        }
    }
    let mut committed_at_0 = 0u64;
    let mut messages = 0u64;
    let t = Instant::now();
    while let Some((from, dest, m)) = queue.pop_front() {
        let targets: Vec<u16> = match dest {
            Dest::All => (0..n).filter(|p| *p != from).collect(),
            Dest::To(p) => vec![p],
        };
        for to in targets {
            messages += 1;
            let out = nodes[to as usize].handle(0, from, m.clone());
            if to == 0 {
                committed_at_0 += out
                    .committed
                    .iter()
                    .map(|c| c.vertex.batch.len() as u64)
                    .sum::<u64>();
            }
            for (d, m2) in out.sends {
                queue.push_back((to, d, m2));
            }
        }
        // Done once node 0 has ordered every transaction; the cap only
        // stops a regression from spinning forever.
        if committed_at_0 >= u64::from(n) * txs_per_node || messages > 200_000_000 {
            break;
        }
    }
    (committed_at_0, t.elapsed().as_secs_f64(), messages)
}

fn main() {
    println!("dag-bft ordering bench: all validators in one thread, free delivery");
    println!("(ORDERING throughput only: no signatures, no execution, no disk. Not TPS.)");
    println!(
        "{:>4} {:>6} {:>12} {:>9} {:>14} {:>12}",
        "n", "batch", "tx ordered", "wall s", "tx-refs/s", "msgs"
    );
    for (n, batch, per) in [
        (4u16, 500usize, 20_000u64),
        (10, 500, 10_000),
        (20, 500, 5_000),
        (50, 500, 2_000),
    ] {
        let (tx, secs, msgs) = run(n, batch, per);
        println!(
            "{n:>4} {batch:>6} {tx:>12} {secs:>9.3} {:>14.0} {msgs:>12}",
            tx as f64 / secs
        );
    }
}
