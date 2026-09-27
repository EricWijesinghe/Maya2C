//! Measured cost of one DAG-BFT round per validator, by committee size
//! (Master Prompt 14 §5: "the validator-count limit is measured").
//!
//! n real engines with ML-DSA-65 authenticators, every frame encoded with the
//! node's wire codec and delivered to every other validator (gossip floods; a
//! vote addressed to one author still reaches all, who decode and drop it).
//! Empty vertices, so this is the consensus overhead alone — the floor under
//! any transaction load.
//!
//! One process does every validator's work in turn, so wall time divided by
//! n is one validator's CPU per round on one core; bytes received are counted
//! per validator.
//!
//! ```text
//! cargo run --profile perf -p custom-l1-node --example committee_scale -- 4 16 32 64 100
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use custom_l1_node::consensus::bft::auth::MlDsaAuthenticator;
use custom_l1_node::consensus::bft::{BROADCAST, Envelope};
use custom_l1_node::crypto::keys::{self, VerifyingKey};
use maya_dag_bft::{Committee, Dest, Output, Params, Validator};

const ROUNDS: u64 = 8;

fn main() {
    let sizes: Vec<u16> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        vec![4, 16, 32, 64, 100]
    } else {
        sizes
    };
    println!(" n   cpu ms/round/validator   MB/round/validator   => at 1 s rounds: cores   Mbit/s");
    for n in sizes {
        let (cpu_ms, bytes) = measure(n);
        println!(
            "{n:>3}   {cpu_ms:>22.1}   {:>18.2}   {:>21.2}   {:>6.0}",
            bytes / 1e6,
            cpu_ms / 1_000.0,
            bytes * 8.0 / 1e6
        );
    }
}

fn measure(n: u16) -> (f64, f64) {
    let signers: Vec<_> = (0..n)
        .map(|i| {
            let mut seed = [0u8; 32];
            seed[..2].copy_from_slice(&i.to_le_bytes());
            Arc::new(keys::signing_key_from_seed(&seed).unwrap())
        })
        .collect();
    let committee: Arc<[VerifyingKey]> = signers.iter().map(|k| k.verifying_key()).collect();
    let params = Params {
        anchor_timeout_ms: 10_000,
        ..Params::default()
    };
    let mut nodes: Vec<Validator<MlDsaAuthenticator>> = (0..n)
        .map(|i| {
            Validator::with_auth(
                i,
                Committee::new(n),
                params,
                MlDsaAuthenticator::validator(
                    Arc::clone(&signers[usize::from(i)]),
                    Arc::clone(&committee),
                ),
            )
        })
        .collect();
    let mut queue: VecDeque<(u16, Vec<u8>)> = VecDeque::new();
    let mut received = vec![0usize; usize::from(n)];
    let push = |q: &mut VecDeque<(u16, Vec<u8>)>, from: u16, out: Output| {
        for (dest, message) in out.sends {
            let to = match dest {
                Dest::All => BROADCAST,
                Dest::To(v) => v,
            };
            q.push_back((
                from,
                Envelope {
                    epoch: 0,
                    from,
                    to,
                    message,
                }
                .encode(),
            ));
        }
    };
    let started = Instant::now();
    for i in 0..n {
        let out = nodes[usize::from(i)].start(0);
        push(&mut queue, i, out);
    }
    while nodes.iter().map(Validator::round).min().unwrap_or(0) < ROUNDS {
        let Some((from, frame)) = queue.pop_front() else {
            break;
        };
        for i in (0..n).filter(|i| *i != from) {
            received[usize::from(i)] += frame.len();
            let env = Envelope::decode(&frame).unwrap();
            if env.to != BROADCAST && env.to != i {
                continue; // decoded, then dropped, as a gossip peer does
            }
            let out = nodes[usize::from(i)].handle(0, env.from, env.message);
            push(&mut queue, i, out);
        }
    }
    let rounds = nodes.iter().map(Validator::round).min().unwrap_or(1).max(1) as f64;
    let wall_ms = started.elapsed().as_secs_f64() * 1e3;
    let per_validator_ms = wall_ms / f64::from(n) / rounds;
    let per_validator_bytes = received.iter().sum::<usize>() as f64 / f64::from(n) / rounds;
    (per_validator_ms, per_validator_bytes)
}
