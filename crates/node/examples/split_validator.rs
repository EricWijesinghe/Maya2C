//! A validator split across processes (Master Prompt 26 §1), measured.
//!
//! The DAG-BFT builder's heaviest stage is verifying each transaction's
//! signature (`reports/04-consensus.md` §6). This prototype moves that stage
//! out of the validator into separate worker processes connected by pipes —
//! the same boundary a second machine would sit behind, with every byte
//! serialized across it — and measures verified transactions per second as
//! workers are added. The coordinator keeps what must stay in one place: the
//! order, and the sequential execution that decides the block.
//!
//! Workers are this binary re-executed with `--worker`: each reads
//! length-prefixed transactions on stdin and answers one byte per
//! transaction (1 verified, 0 refused) on stdout.
//!
//! ```text
//! cargo run --profile perf -p custom-l1-node --example split_validator -- 4000 1 2 4 8
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::io::{BufReader, BufWriter, Read, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

use custom_l1_node::core::transaction::{Transaction, TxOutput};
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};

/// The chain test signatures commit to (ADR-036).
const CHAIN: custom_l1_node::core::ChainTag =
    custom_l1_node::core::ChainTag::from_genesis([42; 32]);

fn worker() {
    let policy = custom_l1_node::crypto::suites::verification_policy();
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = BufWriter::new(std::io::stdout().lock());
    let mut len = [0u8; 4];
    while input.read_exact(&mut len).is_ok() {
        let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
        input.read_exact(&mut buf).unwrap();
        let ok =
            Transaction::from_bytes(&buf).is_ok_and(|tx| tx.verify_at(1, &policy, &CHAIN).is_ok());
        output.write_all(&[u8::from(ok)]).unwrap();
    }
    output.flush().unwrap();
}

fn transactions(n: usize) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| {
            let mut seed = [0u8; 32];
            seed[..8].copy_from_slice(&(i as u64 % 64).to_le_bytes());
            let key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(seed));
            let mut tx = Transaction::new(
                vec![],
                vec![TxOutput {
                    amount: 1,
                    recipient: [7; 32],
                }],
                i as u64 / 64,
            );
            tx.sign_with_suite::<MlDsa65>(&key, &CHAIN).unwrap();
            tx.to_bytes()
        })
        .collect()
}

/// Verifies `txs` across `workers` processes; returns (verified, seconds).
fn run(txs: &[Vec<u8>], workers: usize) -> (usize, f64) {
    let exe = std::env::current_exe().unwrap();
    let mut children: Vec<_> = (0..workers)
        .map(|_| {
            Command::new(&exe)
                .arg("--worker")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let started = Instant::now();
    let chunk = txs.len().div_ceil(workers);
    let feeders: Vec<_> = children
        .iter_mut()
        .zip(txs.chunks(chunk))
        .map(|(child, part)| {
            let mut stdin = child.stdin.take().unwrap();
            let part: Vec<Vec<u8>> = part.to_vec();
            std::thread::spawn(move || {
                let mut w = BufWriter::new(&mut stdin);
                for tx in part {
                    w.write_all(&(tx.len() as u32).to_le_bytes()).unwrap();
                    w.write_all(&tx).unwrap();
                }
            })
        })
        .collect();
    let mut verified = 0;
    for child in &mut children {
        let mut answers = Vec::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_end(&mut answers)
            .unwrap();
        verified += answers.iter().filter(|b| **b == 1).count();
    }
    for f in feeders {
        f.join().unwrap();
    }
    let secs = started.elapsed().as_secs_f64();
    for mut c in children {
        c.wait().unwrap();
    }
    (verified, secs)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--worker") {
        return worker();
    }
    let n: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(4_000);
    let counts: Vec<usize> = args.iter().skip(2).filter_map(|a| a.parse().ok()).collect();
    let counts = if counts.is_empty() {
        vec![1, 2, 4, 8]
    } else {
        counts
    };
    let txs = transactions(n);
    let bytes: usize = txs.iter().map(Vec::len).sum();
    println!(
        "{n} ML-DSA-65 transfers, {:.1} MB across the process boundary",
        bytes as f64 / 1e6
    );
    let mut base = None;
    for w in counts {
        let (ok, secs) = run(&txs, w);
        assert_eq!(ok, n, "a worker refused a valid transaction");
        let rate = n as f64 / secs;
        let speedup = rate / *base.get_or_insert(rate);
        println!("workers {w:>2}: {rate:>8.0} verified tx/s  (x{speedup:.2})");
    }
}
