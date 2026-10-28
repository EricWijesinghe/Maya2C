//! Measurement for ADR-052: what a node pays to catch up by certificate replay.
//!
//! Ignored by default. Run with
//! `CERT_N=4 CERT_BLOCKS=3600 cargo test -p custom-l1-node --test cert_replay_measure -- --ignored --nocapture`
//! (add `--release` for the release numbers).
//!
//! It runs an in-process committee to `CERT_BLOCKS`, then a fresh observer
//! catches up from member 0's logs through the same `certsync::sync` a
//! restarted node uses: paged fetch, verification of every page, and replay.
//! It also times a verification-only pass over the same pages, and samples
//! the process working set during the catch-up to bound its memory growth.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use custom_l1_node::consensus::bft::attest::Checkpoint;
use custom_l1_node::consensus::bft::certsync::{self, CertSource, PAGE};
use custom_l1_node::consensus::bft::store::{CertPage, read_certificate_page};
use custom_l1_node::consensus::bft::wire::decode_certificate;
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::validator_keys::{self as keys, SigningKey, VerifyingKey};
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisBond, GenesisConfig, StakingGenesis};
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::{Authenticator, Params};
use tempfile::TempDir;

mod common;

const EPOCH_BLOCKS: u64 = 3600;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn operator() -> HybridSigningKey {
    signing_key_from_seed(&[0x42; 32]).unwrap()
}

fn validator_key(i: usize) -> Arc<SigningKey> {
    Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
}

fn genesis_config(n: usize) -> GenesisConfig {
    let validators = (0..n)
        .map(|i| hex::encode(validator_key(i).verifying_key().to_bytes()))
        .collect();
    let op = hex::encode(operator().address());
    GenesisConfig {
        chain_id: "cert-replay-measure".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: op.clone(),
            balance: primitive_types::U256::from(1_000_000),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators,
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 0,
            fees: None,
            mining: None,
            staking: Some(StakingGenesis {
                epoch_blocks: EPOCH_BLOCKS,
                bonds: (0..n)
                    .map(|_| GenesisBond {
                        operator: op.clone(),
                        bond: primitive_types::U256::from(10_000),
                        commission_bps: 0,
                    })
                    .collect(),
                min_self_bond: primitive_types::U256::from(1_000),
                max_validators: 100,
                stake_weighted: false,
            }),
        }),
    }
}

fn open_chain(config: &GenesisConfig, dir: &std::path::Path) -> Chain {
    let state = Arc::new(StateDB::open(dir).unwrap());
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

/// A peer's certificate log as a catch-up source, in process.
struct PeerLog {
    dir: PathBuf,
    epoch: u64,
    checkpoints: Vec<Checkpoint>,
}

impl CertSource for PeerLog {
    fn current_epoch(&self) -> custom_l1_node::error::Result<u64> {
        Ok(self.epoch)
    }

    fn page(
        &self,
        epoch: u64,
        offset: u64,
        max_records: usize,
    ) -> custom_l1_node::error::Result<CertPage> {
        read_certificate_page(&self.dir, epoch, offset, max_records, PAGE.encoded_bytes)
    }

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        Ok(self
            .checkpoints
            .iter()
            .filter(|c| c.epoch == epoch)
            .max_by_key(|c| c.height)
            .cloned())
    }
}

struct Member {
    driver: BftDriver,
    chain: Chain,
}

/// The committee, run to a height, with every member's logs on disk.
struct Run {
    root: TempDir,
    members: Vec<Member>,
    queue: VecDeque<(usize, Vec<u8>)>,
    now: u64,
}

impl Run {
    fn new(n: usize, config: &GenesisConfig) -> Self {
        let root = TempDir::new().unwrap();
        let committee: Arc<[VerifyingKey]> = config
            .bft
            .as_ref()
            .unwrap()
            .verifying_keys()
            .unwrap()
            .into();
        let params = Params {
            batch_size: 100,
            min_round_interval_ms: 200,
            ..Params::default()
        };
        let mut members = Vec::new();
        let mut opening = Vec::new();
        for i in 0..n {
            let setup = BftSetup {
                epoch: 0,
                committee: Arc::clone(&committee),
                signer: Some(validator_key(i).into()),
                params,
            };
            let dir = root.path().join(format!("state-{i}"));
            let mut chain = open_chain(config, &dir);
            let (driver, step) =
                BftDriver::open(&setup, &root.path().join(format!("bft-{i}")), &mut chain, 0)
                    .unwrap();
            opening.push((i, step));
            members.push(Member { driver, chain });
        }
        let mut run = Self {
            root,
            members,
            queue: VecDeque::new(),
            now: 0,
        };
        for (i, step) in opening {
            run.absorb(i, step);
        }
        run
    }

    fn absorb(&mut self, from: usize, step: Step) {
        self.queue
            .extend(step.frames.into_iter().map(|f| (from, f)));
    }

    fn pump(&mut self, budget: usize) {
        for _ in 0..budget {
            let Some((from, frame)) = self.queue.pop_front() else {
                return;
            };
            for i in 0..self.members.len() {
                if i == from {
                    continue;
                }
                let m = &mut self.members[i];
                let step = m.driver.on_frame(&mut m.chain, self.now, &frame).unwrap();
                self.absorb(i, step);
            }
        }
    }

    fn tick(&mut self) {
        for i in 0..self.members.len() {
            let m = &mut self.members[i];
            let step = m.driver.on_tick(&mut m.chain, self.now).unwrap();
            self.absorb(i, step);
        }
    }

    fn run_to(&mut self, height: u64) {
        let mut rounds = 0_u64;
        while self.members[0].chain.height() < height {
            self.now += 250;
            self.tick();
            self.pump(3_000);
            rounds += 1;
            assert!(rounds < 2_000_000, "the committee stalled");
        }
    }
}

/// Process working-set size in bytes, from the OS (Windows `psapi`).
#[cfg(windows)]
mod working_set {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }

    /// The current process's working set, or 0 if the OS refuses.
    pub fn now() -> usize {
        let mut counters = Counters {
            cb: u32::try_from(std::mem::size_of::<Counters>()).unwrap(),
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        // SAFETY: `counters` is a live, correctly sized PROCESS_MEMORY_COUNTERS
        // for this call; -1 is the documented pseudo-handle of the current
        // process, which needs no CloseHandle.
        let ok = unsafe { GetProcessMemoryInfo(-1, &raw mut counters, counters.cb) };
        if ok == 0 {
            return 0;
        }
        counters.working_set_size
    }
}

#[cfg(not(windows))]
mod working_set {
    pub fn now() -> usize {
        0
    }
}

/// Samples the working set on a thread until stopped, keeping the peak.
struct Sampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicUsize>,
    handle: std::thread::JoinHandle<()>,
}

impl Sampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicUsize::new(working_set::now()));
        let (s, p) = (Arc::clone(&stop), Arc::clone(&peak));
        let handle = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(working_set::now(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Self { stop, peak, handle }
    }

    /// The highest working set seen, in bytes.
    fn finish(self) -> usize {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().unwrap();
        self.peak.load(Ordering::Relaxed)
    }
}

/// Every certificate of every epoch `0..=head` in `dir`, verified against
/// `keys`, with nothing else done: the verification cost on its own.
fn verify_only(dir: &std::path::Path, head: u64, keys: &Arc<[VerifyingKey]>) -> (usize, usize) {
    let auth = custom_l1_node::consensus::bft::auth::MlDsaAuthenticator::observer(Arc::clone(keys));
    let (mut certs, mut sigs) = (0_usize, 0_usize);
    for epoch in 0..=head {
        let mut offset = 0_u64;
        loop {
            let page = read_certificate_page(dir, epoch, offset, PAGE.records, PAGE.encoded_bytes)
                .unwrap();
            for record in &page.records {
                let c = decode_certificate(record).unwrap();
                let digest = c.digest();
                for (voter, sig) in c.votes.iter().zip(&c.signatures) {
                    assert!(auth.verify(*voter, &digest, sig));
                    sigs += 1;
                }
                certs += 1;
            }
            offset = page.next;
            if page.done {
                break;
            }
        }
    }
    (certs, sigs)
}

#[test]
#[ignore = "measurement for ADR-052; run with --ignored"]
fn catching_up_by_replay_costs_this_much() {
    let n = env_usize("CERT_N", 4);
    let blocks = u64::try_from(env_usize("CERT_BLOCKS", 3600)).unwrap();
    let config = genesis_config(n);
    let committee: Arc<[VerifyingKey]> = config
        .bft
        .as_ref()
        .unwrap()
        .verifying_keys()
        .unwrap()
        .into();

    let t = Instant::now();
    let mut run = Run::new(n, &config);
    run.run_to(blocks);
    let head = run.members[0].driver.epoch();
    println!(
        "RUN n={n} blocks={} epoch={head} run_secs={:.1}",
        run.members[0].chain.height(),
        t.elapsed().as_secs_f64()
    );
    // Keep member 0 as the peer and as the reference for agreement; drop the
    // rest so the baseline below is not theirs.
    let peer_dir = run.root.path().join("bft-0");
    let reference = run.members.swap_remove(0);
    run.members.clear();
    let want = reference.chain.height();

    let (certs, sigs) = verify_only(&peer_dir, head, &committee);
    let t = Instant::now();
    let _ = verify_only(&peer_dir, head, &committee);
    println!(
        "VERIFY n={n} certs={certs} sigs={sigs} verify_secs={:.2}",
        t.elapsed().as_secs_f64()
    );

    let fresh = TempDir::new().unwrap();
    let mut chain = open_chain(&config, &fresh.path().join("state"));
    let setup = BftSetup {
        epoch: 0,
        committee: Arc::clone(&committee),
        signer: None,
        params: Params {
            batch_size: 100,
            min_round_interval_ms: 200,
            ..Params::default()
        },
    };
    let checkpoints = [
        reference.driver.closing_checkpoint(),
        reference.driver.checkpoint(),
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect();
    let source = PeerLog {
        checkpoints,
        dir: peer_dir,
        epoch: head,
    };
    let base = working_set::now();
    let sampler = Sampler::start();
    let t = Instant::now();
    let synced = certsync::sync(
        &mut chain,
        &source,
        &setup,
        &fresh.path().join("bft"),
        1_000_000,
        PAGE,
    )
    .unwrap();
    let catch_up_secs = t.elapsed().as_secs_f64();
    let peak = sampler.finish();
    println!(
        "CATCHUP n={n} height={} of {want} epochs_crossed={} catch_up_secs={catch_up_secs:.2}",
        chain.height(),
        synced.crossed
    );
    if base == 0 {
        // Off Windows there is no psapi reading; a zero would look like a result.
        println!("MEMORY n={n} not measured (no working-set probe on this platform)");
    } else {
        println!(
            "MEMORY n={n} working_set_before_mb={:.1} peak_mb={:.1} growth_mb={:.1}",
            mb(base),
            mb(peak),
            mb(peak.saturating_sub(base))
        );
    }
    let shared = usize::try_from(chain.height().min(want)).unwrap();
    let mine = chain.active_chain().unwrap();
    let theirs = reference.chain.active_chain().unwrap();
    assert_eq!(
        mine[..=shared],
        theirs[..=shared],
        "caught-up blocks differ"
    );
    println!("AGREE n={n} shared_blocks={shared}");
}

fn mb(bytes: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let mb = bytes as f64 / (1024.0 * 1024.0);
    mb
}
