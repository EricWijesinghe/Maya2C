//! 2D extension cost, and erasure-coded broadcast versus plain gossip for a
//! leader's upload (Master Prompt 13 §5-6).
//!
//! The broadcast comparison is a **model** (stated in the output): leader
//! upload bytes and the time to push them over a stated uplink, for 100 and
//! 300 validators in 5 regions. Plain gossip: the leader sends the whole batch
//! to `FANOUT` peers. Erasure-coded dispersal: the batch is cut into `n`
//! chunks of which any `n/3` rebuild it, one chunk to each validator, who
//! forward their chunk to everyone. `cargo bench -p maya-da --bench da_bench`.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::time::Instant;

use maya_da::Extended;

const FANOUT: u64 = 8;
const UPLINK_BPS: f64 = 1e9; // 1 Gbit/s leader uplink
const REGION_RTT_MS: [f64; 5] = [0.0, 70.0, 140.0, 180.0, 250.0];

fn main() {
    println!("== 2D Reed-Solomon extension (BLAKE3 Merkle commitments) ==");
    for (k, chunk) in [(16usize, 512usize), (32, 1_024), (64, 2_048)] {
        let data: Vec<u8> = (0..k * k * chunk).map(|i| (i % 253) as u8).collect();
        let t = Instant::now();
        let ext = Extended::encode(&data, k, chunk);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let w = ext.width();
        let (_, proof) = ext.cell(0, 0);
        println!(
            "k={k:>3} block {:>6} KiB -> extended {:>6} KiB, encode {ms:>8.1} ms, header {} B, cell proof {} B",
            data.len() / 1024,
            w * w * chunk / 1024,
            2 * w * 32,
            proof.path.len() * 32
        );
    }
    println!(
        "\n== leader upload: plain gossip vs erasure-coded dispersal (MODEL, {UPLINK_BPS:.0e} bit/s uplink) =="
    );
    for n in [100u64, 300] {
        for batch_mib in [1u64, 8] {
            let b = batch_mib * 1_048_576;
            let gossip = FANOUT * b;
            let chunk = b.div_ceil(n / 3); // any n/3 rebuild
            let rs = n * chunk;
            let t = |bytes: u64| bytes as f64 * 8.0 / UPLINK_BPS * 1e3 + REGION_RTT_MS[4] / 2.0;
            println!(
                "n={n:>3} batch {batch_mib} MiB: gossip upload {:>6.1} MiB ({:>6.0} ms)   RS upload {:>6.1} MiB ({:>6.0} ms)   {:.1}x less",
                gossip as f64 / 1_048_576.0,
                t(gossip),
                rs as f64 / 1_048_576.0,
                t(rs),
                gossip as f64 / rs as f64
            );
        }
    }
}
