//! Fifty concurrent workers, an hour's session, and the accounting that has to
//! survive it.
//!
//! ## What "an hour" means here, precisely
//!
//! The CI tier runs **an hour's worth of work**, not an hour of wall time:
//! fifty workers times sixty minutes at the pool's one-share-per-minute target
//! is [`SESSION_SHARES`] submissions, and that is exactly what is submitted, as
//! fast as fifty tasks can produce them. The claim being tested is that the
//! accounting is exact over a session of that size — no share is lost, no
//! credit is invented, and every unit that leaves the treasury was owed.
//!
//! Wall-clock behaviour is deliberately *not* asserted here. Vardiff
//! convergence, the idle-easing timer, and the retention window are functions
//! of elapsed time, and the pool reads the system clock rather than a clock a
//! test can move; those properties are proved against an injected clock in the
//! unit tests (`src/vardiff.rs`, `src/telemetry.rs`), which is the honest place
//! for them. The soak tier at the bottom of this file runs the real hour with
//! real pacing and is `#[ignore]`d for that reason.
//!
//! ## Where the simulated worker stops being simulated
//!
//! Each worker builds real [`Message`]s, encodes them through the real codec,
//! and drives a real [`Session`] against a real [`Validator`] over the real
//! share ledger and the real PPLNS split. What is *not* exercised is the socket
//! and the ML-KEM transport underneath it: fifty encrypted TCP connections
//! would test Tokio and `src/network/pq`, both of which have their own tests,
//! and would put a listener in a unit test's path.
//!
//! ## CUDA
//!
//! The workers search on the CPU. `cuda-miner`'s `cuda` feature is off by
//! default precisely so a GPU-less CI runner compiles a green workspace, and a
//! test that needed a GPU would be a test that never runs. The search loop here
//! is the same `hashimoto_light` the GPU kernel is checked against by
//! `cuda-miner/tests/dag_parity.rs`, so what a real CUDA worker would submit is
//! what these submit.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use custom_l1_node::core::BlockHeader;
use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use custom_l1_node::crypto::pow::{meets_target, target_from_leading_zero_bits};
use custom_l1_node::rpc::{HeaderInfo, MiningCandidate};
use custom_l1_node::state::Address;

use maya_pool_service::config::PoolConfig;
use maya_pool_service::ledger::memory::MemoryLedger;
use maya_pool_service::ledger::rocks::RocksLedger;
use maya_pool_service::ledger::{BlockState, ShareLedger};
use maya_pool_service::metrics::PoolMetrics;
use maya_pool_service::model::{PayoutState, ShareOutcome};
use maya_pool_service::payout::{ChainView, PayoutEngine};
use maya_pool_service::session::{Session, SessionAction};
use maya_pool_service::state::PoolState;
use maya_pool_service::treasury::Treasury;
use maya_pool_service::validator::{FoundBlockShare, Validator};
use maya_stratum_v2::messages::{OpenStandardMiningChannel, SetupConnection, SubmitSharesStandard};
use maya_stratum_v2::{Message, Protocol};

use tokio::sync::{Mutex, mpsc};

mod fake_chain;
use fake_chain::FakeChain;

/// Workers in the session.
const WORKERS: usize = 50;

/// Minutes the session represents.
const SESSION_MINUTES: usize = 60;

/// Shares one worker submits: one a minute, which is the interval the vardiff
/// controller steers toward and the figure `docs/stratum-v2.md` §4 sizes the
/// pool's CPU budget against.
const SHARES_PER_WORKER: usize = SESSION_MINUTES;

/// Submissions in the whole session.
const SESSION_SHARES: usize = WORKERS * SHARES_PER_WORKER;

/// Share difficulty the simulated rigs are held at, in leading zero bits.
///
/// **Below the floor `PoolConfig::validate` enforces**, and deliberately so.
/// That floor exists to stop a fast rig saturating the validator pool in
/// production; it is checked by the binary at startup, not by the library, so a
/// test may go under it. At the production floor of eight bits a rig grinds 256
/// hashes per share, and an hour's worth of shares for fifty workers would be
/// three quarters of a million DAG hashes in an unoptimised test build — a test
/// that reads as a hang rather than one that runs.
///
/// Nothing under test depends on the value. A share is credited its own weight,
/// the window is measured in weight, and the split divides by weight, so halving
/// the difficulty halves both sides of every ratio.
const SHARE_BITS: u32 = 1;

/// Ceiling the vardiff controller may raise a test rig to.
///
/// One bit of headroom, so the controller can still move: the simulated rigs
/// submit far faster than one share a minute, and a controller pinned at its
/// floor would not be the controller production runs.
const MAX_SHARE_BITS: u32 = 2;

/// Network difficulty, in leading zero bits.
///
/// Nine bits above the share ceiling, so roughly one share in five hundred also
/// solves the chain: a handful of blocks across the session, which is enough to
/// exercise crediting, maturation, and payout without the test spending its
/// time on block submission.
const NETWORK_BITS: u32 = MAX_SHARE_BITS + 9;

/// Height every job in the session sits at.
///
/// Above `DagConfig::TESTING`'s activation height, so verification runs the DAG
/// rule against a 512 KiB cache rather than 32 MiB of Argon2id. That is the
/// difference between a test that finishes in seconds and one that takes twenty
/// minutes; the accounting under test is identical either way.
const JOB_HEIGHT: u64 = 4;

/// Confirmations before credits are payable, kept short for the test.
const CONFIRMATIONS: u64 = 3;

/// Value each found block distributes, before the operator's fee.
const REWARD: u64 = 1_000_000;

/// Builds the address of worker `index`.
fn miner_address(index: usize) -> Address {
    let mut address = [0u8; 32];
    address[0..8].copy_from_slice(&(index as u64).to_be_bytes());
    address
}

/// Test configuration.
fn config() -> PoolConfig {
    PoolConfig {
        reward_per_block: REWARD,
        // No fee: the split is being checked to the unit, and a fee would only
        // add a rounding step between the reward and the assertion.
        fee_rate: 0.0,
        confirmations: CONFIRMATIONS,
        min_payout: 1,
        pplns_factor: 2.0,
        // See SHARE_BITS: below the production floor on purpose, and this
        // config is never passed through `validate`.
        min_target_bits: SHARE_BITS,
        max_target_bits: MAX_SHARE_BITS,
        ..PoolConfig::default()
    }
}

/// Everything one simulated session needs.
struct Harness {
    /// Shared pool state.
    state: Arc<PoolState>,
    /// The validation pipeline.
    validator: Validator,
    /// The payout engine.
    engine: Arc<PayoutEngine>,
    /// The chain the engine talks to.
    chain: Arc<FakeChain>,
    /// Solved shares handed over by the validator.
    found: Arc<Mutex<mpsc::Receiver<FoundBlockShare>>>,
}

impl Harness {
    /// Assembles a pool around `ledger`.
    fn new(ledger: Arc<dyn ShareLedger>) -> Self {
        let config = config();
        let state = Arc::new(PoolState::new(
            config.clone(),
            Arc::clone(&ledger),
            Arc::new(PoolMetrics::new()),
            Arc::new(CacheRegistry::new(DagConfig::TESTING)),
        ));

        push_job(&state, 1);

        let chain = Arc::new(FakeChain::with(JOB_HEIGHT, u64::MAX / 2));
        let treasury = Treasury::from_key(
            custom_l1_node::crypto::hybrid::generate_signing_key().expect("key generation"),
        );
        let engine = Arc::new(PayoutEngine::new(
            ledger,
            Arc::clone(&chain) as Arc<dyn ChainView>,
            treasury,
            config,
        ));

        let (blocks, found) = mpsc::channel(256);
        let validator = Validator::spawn(Arc::clone(&state), 4, blocks);

        Self {
            state,
            validator,
            engine,
            chain,
            found: Arc::new(Mutex::new(found)),
        }
    }
}

/// Pushes a work template whose parent distinguishes it from the last.
fn push_job(state: &PoolState, salt: u8) -> u32 {
    let header = BlockHeader {
        prev_hash: [salt; 32],
        state_root: [salt.wrapping_add(1); 32],
        timestamp: 1_700_000_000,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(NETWORK_BITS),
        tx_root: [0; 32],
    };

    state
        .jobs_mut()
        .expect("job lock")
        .push(&MiningCandidate {
            height: JOB_HEIGHT,
            difficulty_target: hex::encode(header.difficulty_target),
            header_bytes: hex::encode(header.serialize()),
            header: HeaderInfo::from(&header),
        })
        .expect("the candidate must decode")
        // The salt varies the parent hash, so every call here is genuinely new
        // work. The registry refuses a candidate that only differs by its
        // timestamp, which is what a real node returns on every poll.
        .expect("each salt is new work")
}

/// What one simulated worker did.
#[derive(Debug, Default)]
struct WorkerReport {
    /// Shares the pool credited.
    accepted: u64,
    /// Shares the pool refused.
    rejected: u64,
    /// Weight the pool credited.
    weight: u64,
}

/// Runs one worker for `shares` submissions.
///
/// Every message goes through the real codec on the way in, so the simulation
/// exercises encoding and decoding rather than only the state machine behind
/// them.
async fn run_worker(
    index: usize,
    shares: usize,
    state: Arc<PoolState>,
    validator: Validator,
) -> WorkerReport {
    let mut session = Session::new();
    let mut report = WorkerReport::default();

    // Setup, encoded and decoded exactly as it would arrive off a socket.
    let setup = Message::SetupConnection(SetupConnection {
        protocol: Protocol::Mining,
        min_version: 2,
        max_version: 2,
        flags: 0,
        endpoint_host: "pool.test".to_string(),
        endpoint_port: 3333,
        vendor: "sim".to_string(),
        hardware_version: "1".to_string(),
        firmware: "1".to_string(),
        device_id: format!("rig-{index}"),
    });
    session
        .handle(round_trip(&setup), &state)
        .expect("setup must not fail");

    let open = Message::OpenStandardMiningChannel(OpenStandardMiningChannel {
        request_id: index as u32,
        // Two workers share miner 0's address, so the test also covers a farm
        // split across rigs paying the same as one rig would.
        user_identity: format!(
            "{}.rig-{index}",
            hex::encode(miner_address(if index == WORKERS - 1 { 0 } else { index }))
        ),
        nominal_hash_rate: 1_000.0,
        max_target: [0xFF; 32],
    });

    let actions = session
        .handle(round_trip(&open), &state)
        .expect("the channel must open");
    let channel_id = match &actions[0] {
        SessionAction::Reply(Message::OpenStandardMiningChannelSuccess(success)) => {
            success.channel_id
        }
        other => panic!("worker {index} could not open a channel: {other:?}"),
    };

    let mut nonce = {
        let channels = state.channels().expect("channel lock");
        channels
            .get(channel_id)
            .expect("the channel just opened")
            .range
            .min
    };

    for sequence in 0..shares {
        let (job_id, ntime, height) = {
            let jobs = state.jobs().expect("job lock");
            let job = jobs.current().expect("work must exist");
            (job.id, job.header.timestamp, job.height)
        };

        // Re-read every time. A real miner applies `SetTarget` when the pool
        // sends one, and a simulated one that kept searching against the target
        // it opened with would submit work the pool had since stopped accepting
        // — which would look like the pool losing shares.
        let share_target = {
            let channels = state.channels().expect("channel lock");
            match channels.get(channel_id) {
                Some(channel) => channel.target(),
                None => break,
            }
        };

        nonce = search(&state, job_id, nonce, height, &share_target);

        let submit = Message::SubmitSharesStandard(SubmitSharesStandard {
            channel_id,
            sequence_number: sequence as u32,
            job_id,
            nonce,
            ntime,
        });
        nonce += 1;

        for action in session
            .handle(round_trip(&submit), &state)
            .expect("submission must not fail")
        {
            match action {
                SessionAction::Validate(request) => {
                    let verdict = validator.submit(request).await.expect("a verdict");
                    match verdict.outcome {
                        ShareOutcome::Accepted { weight, .. } => {
                            report.accepted += 1;
                            report.weight += weight;
                        }
                        ShareOutcome::Rejected(_) => report.rejected += 1,
                    }
                }
                SessionAction::Reply(Message::SubmitSharesError(_)) => report.rejected += 1,
                SessionAction::Reply(_) => {}
                SessionAction::Close(reason) => panic!("worker {index} was closed: {reason}"),
            }
        }
    }

    session.disconnect(&state).expect("clean disconnect");
    report
}

/// Encodes a message and decodes it back, as the wire would.
fn round_trip(message: &Message) -> Message {
    let bytes = message.to_frame().expect("encodable").encode();
    let frame = maya_stratum_v2::Frame::decode(&bytes).expect("decodable");
    Message::from_frame(&frame).expect("the pool must accept its own encoding")
}

/// Finds a nonce at or above `from` whose digest meets `target`.
fn search(state: &PoolState, job_id: u32, from: u64, height: u64, target: &[u8; 32]) -> u64 {
    let jobs = state.jobs().expect("job lock");
    let job = jobs.get(job_id).expect("the job is live");

    for offset in 0..1_000_000u64 {
        let nonce = from + offset;
        let header = job.header_with(nonce, job.header.timestamp);
        let digest = header.pow_hash_at(height, &state.dag).expect("hashing");
        if meets_target(&digest, target) {
            return nonce;
        }
    }
    panic!("no solution within the attempt budget");
}

/// Waits until nothing but the caller holds a reference to the ledger.
///
/// The validator's dispatcher task and its blocking workers each hold an
/// `Arc<PoolState>`, and they release it only after the queue closes and the
/// last verification finishes. Dropping the harness starts that; this waits for
/// it to finish, which is what the operating system does for a real daemon
/// before its replacement starts.
async fn shutdown(ledger: &Arc<RocksLedger>) {
    for _ in 0..2_000 {
        if Arc::strong_count(ledger) == 1 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!("the pool did not release the ledger within two seconds");
}

/// Drains solved shares, records them on the chain, and credits their windows.
///
/// This is `daemon::block_loop` with the RPC replaced by the fake chain: the
/// same order of operations, so a block is on the chain before its credits are
/// written, and a credit failure is loud.
async fn credit_blocks(harness: &Harness) -> usize {
    let mut credited = 0;
    let mut found = harness.found.lock().await;

    while let Ok(share) = found.try_recv() {
        let job = {
            let jobs = harness.state.jobs().expect("job lock");
            jobs.get(share.job_id).cloned()
        };
        let Some(job) = job else { continue };

        let block = maya_pool_service::share::block_for(&job, share.nonce, share.ntime);
        let id = hex::encode(block.header.id());

        // Each found block is recorded at its own height. Every job in the
        // simulation sits at one height so the DAG cache is built once, but a
        // chain cannot hold two blocks at one height — and maturity is decided
        // by comparing the id the chain holds *at a height* against the id the
        // pool recorded. Nine blocks filed at one height would be eight blocks
        // the pool correctly orphans, which is a test of the wrong thing.
        let height = job.height + credited as u64;
        harness.chain.put_block(height, &id);

        harness
            .engine
            .credit_block(
                &id,
                height,
                share.sequence,
                share.key.miner,
                &job.network_target,
            )
            .expect("a found block must credit its window");
        credited += 1;
    }

    credited
}

// ---------------------------------------------------------------------------
// the session
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fifty_workers_over_an_hours_session_lose_no_credit() {
    let ledger = Arc::new(MemoryLedger::new());
    let harness = Harness::new(Arc::clone(&ledger) as Arc<dyn ShareLedger>);

    let mut tasks = Vec::with_capacity(WORKERS);
    for index in 0..WORKERS {
        let state = Arc::clone(&harness.state);
        let validator = harness.validator.clone();
        tasks.push(tokio::spawn(async move {
            run_worker(index, SHARES_PER_WORKER, state, validator).await
        }));
    }

    let mut reports = Vec::with_capacity(WORKERS);
    for task in tasks {
        reports.push(task.await.expect("a worker panicked"));
    }

    // The oracle: what the workers themselves believe they were credited,
    // summed independently of anything the pool recorded.
    let expected_shares: u64 = reports.iter().map(|report| report.accepted).sum();
    let expected_weight: u64 = reports.iter().map(|report| report.weight).sum();
    let rejected: u64 = reports.iter().map(|report| report.rejected).sum();

    assert_eq!(
        expected_shares + rejected,
        SESSION_SHARES as u64,
        "every submission must have had a verdict"
    );
    assert_eq!(rejected, 0, "no honest share should have been refused");

    // And what the ledger holds.
    let tip = ledger
        .tip_sequence()
        .unwrap()
        .expect("shares were credited");
    assert_eq!(
        tip + 1,
        expected_shares,
        "the ledger lost or gained a share"
    );

    let window = ledger
        .window(tip, custom_l1_node::consensus::U256::MAX)
        .unwrap();
    let ledger_weight: u64 = window.shares.iter().map(|share| share.weight).sum();
    assert_eq!(
        ledger_weight, expected_weight,
        "credited weight does not match what the workers were told"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sessions_blocks_pay_out_to_the_unit() {
    let ledger = Arc::new(MemoryLedger::new());
    let harness = Harness::new(Arc::clone(&ledger) as Arc<dyn ShareLedger>);

    let mut tasks = Vec::with_capacity(WORKERS);
    for index in 0..WORKERS {
        let state = Arc::clone(&harness.state);
        let validator = harness.validator.clone();
        tasks.push(tokio::spawn(async move {
            run_worker(index, SHARES_PER_WORKER, state, validator).await
        }));
    }
    for task in tasks {
        task.await.expect("a worker panicked");
    }

    let blocks = credit_blocks(&harness).await;
    assert!(
        blocks > 0,
        "the session found no blocks; NETWORK_BITS is set too high for {SESSION_SHARES} shares"
    );

    // Every credited block distributes exactly the configured reward.
    let immature: u64 = ledger
        .balances()
        .unwrap()
        .iter()
        .map(|(_, balance)| balance.immature)
        .sum();
    assert_eq!(
        immature,
        REWARD * blocks as u64,
        "the split did not conserve the reward across {blocks} blocks"
    );

    // Bury them and settle.
    harness
        .chain
        .set_height(JOB_HEIGHT + blocks as u64 + CONFIRMATIONS + 1);
    let report = harness.engine.settle().await.expect("settlement");
    assert_eq!(report.matured, blocks);

    let paid: u64 = ledger
        .balances()
        .unwrap()
        .iter()
        .map(|(_, balance)| balance.paid)
        .sum();
    let unpaid: u64 = ledger
        .balances()
        .unwrap()
        .iter()
        .map(|(_, balance)| balance.unpaid)
        .sum();

    assert_eq!(
        paid + unpaid,
        REWARD * blocks as u64,
        "value was created or destroyed between crediting and paying"
    );
    assert_eq!(
        paid,
        harness.chain.broadcast_value(),
        "the treasury moved a different amount than the ledger recorded"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_mid_session_does_not_pay_anyone_twice() {
    // The expensive failure. A daemon that crashes between signing a batch and
    // broadcasting it must rebroadcast those bytes, not sign new ones against
    // the same nonce — and must never select the same credits again.
    let dir = tempfile::TempDir::new().expect("temp dir");

    let (expected_shares, blocks) = {
        let ledger = Arc::new(RocksLedger::open(dir.path()).unwrap());
        let harness = Harness::new(Arc::clone(&ledger) as Arc<dyn ShareLedger>);

        let mut tasks = Vec::with_capacity(WORKERS);
        for index in 0..WORKERS {
            let state = Arc::clone(&harness.state);
            let validator = harness.validator.clone();
            tasks.push(tokio::spawn(async move {
                run_worker(index, SHARES_PER_WORKER / 2, state, validator).await
            }));
        }

        let mut accepted = 0u64;
        for task in tasks {
            accepted += task.await.expect("a worker panicked").accepted;
        }

        let blocks = credit_blocks(&harness).await;
        harness
            .chain
            .set_height(JOB_HEIGHT + blocks as u64 + CONFIRMATIONS + 1);
        harness.engine.settle().await.expect("settlement");

        // Tear the daemon down and wait for it to let go, which is what a
        // restart actually involves. RocksDB holds an exclusive lock on its
        // directory, so reopening while the old handle is still alive fails —
        // the same protection that stops two pool daemons sharing one ledger
        // and, with it, one treasury nonce sequence.
        drop(harness);
        shutdown(&ledger).await;
        drop(ledger);

        (accepted, blocks)
    };

    // Only the RocksDB directory survives.
    let ledger = Arc::new(RocksLedger::open(dir.path()).unwrap());
    assert_eq!(
        ledger.tip_sequence().unwrap().unwrap() + 1,
        expected_shares,
        "the reopened ledger lost shares"
    );

    let paid_before: u64 = ledger
        .balances()
        .unwrap()
        .iter()
        .map(|(_, balance)| balance.paid)
        .sum();

    // A fresh daemon over the same ledger, with a chain that has moved on.
    let harness = Harness::new(Arc::clone(&ledger) as Arc<dyn ShareLedger>);
    harness.chain.set_height(JOB_HEIGHT + CONFIRMATIONS * 10);
    for _ in 0..3 {
        // The resumed batch was signed by the previous run's treasury key, so
        // the chain refuses nothing here — what is under test is whether the
        // pool tries to pay the same credits a second time.
        let _ = harness.engine.settle().await;
    }

    let paid_after: u64 = ledger
        .balances()
        .unwrap()
        .iter()
        .map(|(_, balance)| balance.paid)
        .sum();
    let unpaid_after: u64 = ledger
        .balances()
        .unwrap()
        .iter()
        .map(|(_, balance)| balance.unpaid)
        .sum();

    assert_eq!(
        paid_after + unpaid_after,
        REWARD * blocks as u64,
        "the restart created or destroyed value"
    );
    assert!(
        paid_after >= paid_before,
        "the restart un-paid credits that had already been sent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn splitting_a_farm_across_two_rigs_pays_the_same_as_one() {
    // Worker `WORKERS - 1` mines under miner 0's address, so miner 0 owns two
    // rigs. Their credit must be the sum of both rigs' weight and nothing else:
    // the pool pays addresses, and rig names are an operational detail.
    let ledger = Arc::new(MemoryLedger::new());
    let harness = Harness::new(Arc::clone(&ledger) as Arc<dyn ShareLedger>);

    let mut tasks = Vec::with_capacity(WORKERS);
    for index in 0..WORKERS {
        let state = Arc::clone(&harness.state);
        let validator = harness.validator.clone();
        tasks.push(tokio::spawn(async move {
            run_worker(index, SHARES_PER_WORKER, state, validator).await
        }));
    }

    let mut reports = Vec::with_capacity(WORKERS);
    for task in tasks {
        reports.push(task.await.expect("a worker panicked"));
    }

    let blocks = credit_blocks(&harness).await;
    assert!(blocks > 0, "the session found no blocks");

    // Fifty rigs, forty-nine addresses. The credited set must be addresses:
    // if the pool paid rigs, miner 0 would appear twice and every other miner's
    // share of the reward would be diluted by an operational detail.
    let credited = ledger.balances().unwrap();
    assert!(
        credited.len() < WORKERS,
        "{} credited accounts against {} distinct addresses — rigs were not \
         aggregated to their miner",
        credited.len(),
        WORKERS - 1
    );

    // And miner 0 is credited for both rigs, so their balance is not simply the
    // one rig the pool happened to see last.
    let balance = ledger.balance(&miner_address(0)).unwrap();
    assert!(
        balance.immature > 0,
        "the miner running two rigs was credited nothing"
    );
    assert_eq!(
        credited
            .iter()
            .filter(|(address, _)| *address == miner_address(0))
            .count(),
        1,
        "one address, one account"
    );

    // And the two rigs are still visible separately, because an operator needs
    // to know which of their machines stopped.
    let workers = harness
        .state
        .telemetry()
        .unwrap()
        .workers_of(&miner_address(0), maya_pool_service::model::now_millis());
    assert_eq!(workers.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_share_in_flight_when_work_changes_is_still_credited() {
    // At a fifteen-second block target, work changes under a rig constantly. A
    // share that was already in flight when the job changed is the pool's push
    // latency showing up as the miner's rejection rate, and refusing it would
    // bill the miner for it.
    //
    // Driven by hand rather than by timing: a race that reproduces most of the
    // time is a test that fails some of the time.
    let ledger = Arc::new(MemoryLedger::new());
    let harness = Harness::new(Arc::clone(&ledger) as Arc<dyn ShareLedger>);
    let state = Arc::clone(&harness.state);

    let mut session = Session::new();
    session
        .handle(
            Message::SetupConnection(SetupConnection {
                protocol: Protocol::Mining,
                min_version: 2,
                max_version: 2,
                flags: 0,
                endpoint_host: "pool.test".to_string(),
                endpoint_port: 3333,
                vendor: "sim".to_string(),
                hardware_version: "1".to_string(),
                firmware: "1".to_string(),
                device_id: "rig".to_string(),
            }),
            &state,
        )
        .unwrap();

    let actions = session
        .handle(
            Message::OpenStandardMiningChannel(OpenStandardMiningChannel {
                request_id: 1,
                user_identity: hex::encode(miner_address(0)),
                nominal_hash_rate: 1.0,
                max_target: [0xFF; 32],
            }),
            &state,
        )
        .unwrap();
    let channel_id = match &actions[0] {
        SessionAction::Reply(Message::OpenStandardMiningChannelSuccess(success)) => {
            success.channel_id
        }
        other => panic!("the channel did not open: {other:?}"),
    };

    // Solve against the job the rig currently holds.
    let (old_job, ntime, range_min, target) = {
        let jobs = state.jobs().unwrap();
        let job = jobs.current().unwrap();
        let channels = state.channels().unwrap();
        let channel = channels.get(channel_id).unwrap();
        (
            job.id,
            job.header.timestamp,
            channel.range.min,
            channel.target(),
        )
    };
    let nonce = search(&state, old_job, range_min, JOB_HEIGHT, &target);

    // Now work changes underneath it, once.
    let job_id = push_job(&state, 2);
    state
        .channels()
        .unwrap()
        .get_mut(channel_id)
        .unwrap()
        .push_job(job_id);

    let actions = session
        .handle(
            Message::SubmitSharesStandard(SubmitSharesStandard {
                channel_id,
                sequence_number: 0,
                job_id: old_job,
                nonce,
                ntime,
            }),
            &state,
        )
        .unwrap();

    // One job back is still credited...
    match &actions[0] {
        SessionAction::Validate(request) => {
            let verdict = harness.validator.submit(request.clone()).await.unwrap();
            assert!(
                matches!(verdict.outcome, ShareOutcome::Accepted { .. }),
                "a share one job behind was refused: {:?}",
                verdict.outcome
            );
        }
        other => panic!("a share one job behind never reached the validator: {other:?}"),
    }

    // ...and one more job change makes it genuinely stale.
    let job_id = push_job(&state, 3);
    state
        .channels()
        .unwrap()
        .get_mut(channel_id)
        .unwrap()
        .push_job(job_id);

    let actions = session
        .handle(
            Message::SubmitSharesStandard(SubmitSharesStandard {
                channel_id,
                sequence_number: 1,
                job_id: old_job,
                nonce: nonce + 1,
                ntime,
            }),
            &state,
        )
        .unwrap();

    match &actions[0] {
        SessionAction::Reply(Message::SubmitSharesError(error)) => {
            assert_eq!(
                error.error_code,
                maya_stratum_v2::messages::mining::error_codes::STALE_SHARE
            );
        }
        other => panic!("work three jobs back was not refused: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// soak tier
// ---------------------------------------------------------------------------

/// The real hour, at the real pace.
///
/// `#[ignore]`d because it takes an hour by construction. Run it before a
/// release, or against a testnet node, with:
///
/// ```text
/// cargo test -p maya-pool-service --test worker_simulation -- --ignored --nocapture
/// ```
///
/// This is where wall-clock behaviour is actually observed: vardiff moving
/// under a sustained rate, idle channels being eased, and the telemetry
/// retention window doing its work. The CI tier above cannot see any of that
/// and does not pretend to.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "runs for a full hour by design; the CI tier covers the accounting"]
async fn a_real_hour_with_fifty_workers() {
    let ledger = Arc::new(MemoryLedger::new());
    let harness = Harness::new(Arc::clone(&ledger) as Arc<dyn ShareLedger>);
    let submitted = Arc::new(AtomicU64::new(0));

    let mut tasks = Vec::with_capacity(WORKERS);
    for index in 0..WORKERS {
        let state = Arc::clone(&harness.state);
        let validator = harness.validator.clone();
        let submitted = Arc::clone(&submitted);

        tasks.push(tokio::spawn(async move {
            for minute in 0..SESSION_MINUTES {
                // Staggered, so fifty rigs do not submit in lockstep — which
                // would be a load pattern no real farm produces and would hide
                // exactly the queueing behaviour this tier exists to observe.
                let offset = (index * 60 / WORKERS) as u64;
                tokio::time::sleep(std::time::Duration::from_secs(60 * minute as u64 + offset))
                    .await;
                run_worker(index, 1, Arc::clone(&state), validator.clone()).await;
                submitted.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for task in tasks {
        task.await.expect("a worker panicked");
    }

    println!(
        "soak: {} submissions, {} shares credited, {} channels left open",
        submitted.load(Ordering::Relaxed),
        ledger.tip_sequence().unwrap().map_or(0, |tip| tip + 1),
        harness.state.channels().unwrap().len()
    );

    let blocks = credit_blocks(&harness).await;
    harness
        .chain
        .set_height(JOB_HEIGHT + blocks as u64 + CONFIRMATIONS + 1);
    harness.engine.settle().await.expect("settlement");

    for block in ledger.recent_blocks(usize::MAX).unwrap() {
        assert_ne!(
            block.state,
            BlockState::Immature,
            "a block never matured across the whole session"
        );
    }
    println!("soak: {blocks} blocks credited and settled");

    assert!(
        harness
            .engine
            .settle()
            .await
            .is_ok_and(|report| report.broadcast == 0),
        "a second settlement pass tried to pay the same credits again"
    );
    assert!(matches!(
        ledger
            .recent_batches(1)
            .unwrap()
            .first()
            .map(|batch| batch.state),
        Some(PayoutState::Submitted | PayoutState::Confirmed)
    ));
}
