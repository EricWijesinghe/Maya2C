//! Folding time and memory for 1,000,000 blocks (Master Prompt 3 §6).
//!
//! A plain harness rather than criterion: the quantity is one long run's
//! wall time and its resident set, not a per-iteration estimate. Run with
//! `cargo bench -p maya-history-compactor --bench compactor_bench`.
//! Memory figures are what the structures hold (counted), not RSS, so they
//! do not depend on the allocator.

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::time::Instant;

use maya_history_compactor::{ArchiveMmr, BlockLeaf, Compactor};

fn leaf(h: u64) -> BlockLeaf {
    BlockLeaf {
        height: h,
        block_id: *blake3::hash(&h.to_le_bytes()).as_bytes(),
        tx_root: *blake3::hash(&h.to_be_bytes()).as_bytes(),
    }
}

fn main() {
    let blocks: u64 = std::env::var("COMPACTOR_BLOCKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    let leaves: Vec<BlockLeaf> = (0..blocks).map(leaf).collect();

    let t = Instant::now();
    let mut pruned = Compactor::new();
    for l in &leaves {
        pruned.append(l);
    }
    let fold = t.elapsed();

    let t = Instant::now();
    let mut archive = ArchiveMmr::new();
    for l in &leaves {
        archive.append(l);
    }
    let archive_time = t.elapsed();
    assert_eq!(pruned.commitment(), archive.commitment());

    let t = Instant::now();
    let mut proof_bytes = 0usize;
    let samples = 10_000u64;
    let commitment = pruned.commitment();
    for i in 0..samples {
        let index = (i * 7919) % blocks;
        let p = archive.prove(index).expect("held");
        proof_bytes = proof_bytes.max(p.encoded_len());
        assert!(p.verify(&commitment, &leaves[index as usize]));
    }
    let prove_verify = t.elapsed();

    // Nodes held by the archive: 2n - popcount(n).
    let archive_nodes = 2 * blocks - u64::from(blocks.count_ones());
    println!("history-compactor bench (MMR accumulator; NOT a validity proof)");
    println!("blocks                         {blocks}");
    println!("pruned fold time               {:.3} s ({:.0} ns/block)", fold.as_secs_f64(), fold.as_nanos() as f64 / blocks as f64);
    println!("pruned node memory             {} peaks x 32 B = {} B", pruned.peak_count(), pruned.peak_count() * 32);
    println!("archive build time             {:.3} s", archive_time.as_secs_f64());
    println!("archive memory                 {archive_nodes} nodes x 32 B = {:.1} MiB", (archive_nodes * 32) as f64 / 1_048_576.0);
    println!("max inclusion proof            {proof_bytes} B");
    println!("prove+verify ({samples} samples)   {:.1} us each", prove_verify.as_micros() as f64 / samples as f64);
}
