//! `pool-load hash-cost`: single-thread cost of verifying one share.
//!
//! Verification is `BlockHeader::pow_hash_at` at a height past the DAG
//! activation, which is what the pool runs for every share. This times that call
//! on one thread, with the epoch cache already built, so the number is the
//! per-share CPU cost the validator workers pay.

use std::error::Error;
use std::time::Instant;

use custom_l1_node::crypto::dag::DAG_ACTIVATION_HEIGHT;
use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use maya_pool_service::job::Job;

use crate::serve;

/// Hashes timed after a warm-up.
const SAMPLES: u64 = 2_000;

/// Times `SAMPLES` share verifications on the calling thread.
pub fn run() -> Result<(), Box<dyn Error>> {
    let dag = CacheRegistry::new(DagConfig::MAINNET);
    dag.cache_for_height(DAG_ACTIVATION_HEIGHT)?;
    let job = Job::from_candidate(0, &serve::candidate(0))?;

    let time = |count: u64| -> Result<f64, Box<dyn Error>> {
        let started = Instant::now();
        for nonce in 0..count {
            job.header_with(nonce, job.header.timestamp)
                .pow_hash_at(job.height, &dag)
                .map_err(|e| format!("hash: {e}"))?;
        }
        Ok(started.elapsed().as_secs_f64() * 1e6 / count as f64)
    };

    time(SAMPLES / 10)?;
    let micros = time(SAMPLES)?;
    println!("== pool-load hash-cost ==");
    println!(
        "share verification (pow_hash_at, DagConfig::MAINNET, height {}): {micros:.1} us per hash, single thread, {SAMPLES} samples after warm-up",
        job.height
    );
    println!("single-thread shares/s: {:.0}", 1e6 / micros);
    Ok(())
}
