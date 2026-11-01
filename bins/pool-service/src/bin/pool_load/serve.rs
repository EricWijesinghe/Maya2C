//! The pool under test: the real daemon, listening on an ephemeral localhost port.
//!
//! Prints `LISTEN <addr>` once ready, `RSS <bytes>` every 200 ms, and
//! `JOBSENT <id>` after each `JOB` line on stdin. Nothing else on stdout.

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use custom_l1_node::core::BlockHeader;
use custom_l1_node::crypto::dag::DAG_ACTIVATION_HEIGHT;
use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::rpc::{HeaderInfo, MiningCandidate};
use maya_pool_service::config::PoolConfig;
use maya_pool_service::daemon;
use maya_pool_service::job::Job;
use maya_pool_service::ledger::ShareLedger;
use maya_pool_service::ledger::rocks::RocksLedger;
use maya_pool_service::metrics::PoolMetrics;
use maya_pool_service::state::PoolState;
use maya_pool_service::validator::{QUEUE_CAPACITY, Validator};
use primitive_types::U256;
use tokio::sync::{broadcast, mpsc};

use crate::{NETWORK_BITS, SHARE_BITS, working_set};

/// A template at `salt`. Different salts give different parent hashes, so each
/// one is new work to the registry rather than a timestamp refresh.
pub fn candidate(salt: u8) -> MiningCandidate {
    let header = BlockHeader {
        version: 0,
        prev_hash: [salt; 32],
        state_root: [salt.wrapping_add(1); 32],
        timestamp: 1_700_000_000,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(NETWORK_BITS),
        tx_root: [0; 32],
    };
    MiningCandidate {
        height: DAG_ACTIVATION_HEIGHT,
        difficulty_target: hex::encode(header.difficulty_target),
        header_bytes: hex::encode(header.serialize()),
        header: HeaderInfo::from(&header),
    }
}

/// The pool's configuration for the load run. Pinned difficulty, see `main.rs`.
fn pool_config() -> PoolConfig {
    PoolConfig {
        reward_per_block: U256::from(1_000_000u64),
        max_batch_value: U256::MAX,
        fee_rate: 0.0,
        confirmations: 3,
        min_payout: U256::from(1u64),
        pplns_factor: 2.0,
        min_target_bits: SHARE_BITS,
        max_target_bits: SHARE_BITS,
        ..PoolConfig::default()
    }
}

/// Pushes a template into the registry, hands it to every channel, and
/// broadcasts it, as `daemon::job_loop` does for a polled template.
pub fn push_and_broadcast(
    state: &PoolState,
    jobs: &broadcast::Sender<Arc<Job>>,
    salt: u8,
) -> Result<u32, Box<dyn Error>> {
    let id = {
        let mut registry = state.jobs_mut()?;
        registry
            .push(&candidate(salt))?
            .ok_or("the candidate was not new work")?
    };
    let job = state
        .jobs()?
        .get(id)
        .cloned()
        .ok_or("the job was retired immediately")?;
    {
        let mut channels = state.channels()?;
        for channel in channels.iter_mut() {
            channel.push_job(id);
        }
    }
    // A send with no subscribers is not an error: no miner is connected yet.
    let _ = jobs.send(Arc::new(job));
    Ok(id)
}

/// Starts the pool and serves until killed.
pub async fn run(validators: usize, dir: &Path) -> Result<(), Box<dyn Error>> {
    let ledger: Arc<dyn ShareLedger> = Arc::new(RocksLedger::open(dir)?);
    let dag = Arc::new(CacheRegistry::new(DagConfig::MAINNET));
    // Built before the first RSS sample, so the 64 MiB epoch cache is part of
    // the baseline rather than a step that lands in the middle of a run.
    dag.cache_for_height(DAG_ACTIVATION_HEIGHT)?;

    let state = Arc::new(PoolState::new(
        pool_config(),
        ledger,
        Arc::new(PoolMetrics::new()),
        dag,
    ));

    // Blocks are never found at network difficulty, so this drains nothing.
    let (blocks, mut found) = mpsc::channel(16);
    tokio::spawn(async move { while found.recv().await.is_some() {} });
    let validator = Validator::spawn(Arc::clone(&state), validators, blocks);
    let (jobs, _) = broadcast::channel(16);

    push_and_broadcast(&state, &jobs, 0)?;
    let server = daemon::serve_stratum(
        "127.0.0.1:0".parse()?,
        Arc::clone(&state),
        validator,
        jobs.clone(),
    )
    .await?;
    println!("LISTEN {}", server.address);
    println!("QUEUE {QUEUE_CAPACITY}");

    std::thread::spawn(|| {
        loop {
            println!("RSS {}", working_set::now());
            std::thread::sleep(Duration::from_millis(200));
        }
    });

    let stdin_state = Arc::clone(&state);
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if line.trim() != "JOB" {
                continue;
            }
            match push_and_broadcast(&stdin_state, &jobs, 1) {
                Ok(id) => println!("JOBSENT {id}"),
                Err(error) => eprintln!("pool-load serve: job push failed: {error}"),
            }
        }
    });

    std::future::pending::<()>().await;
    Ok(())
}
