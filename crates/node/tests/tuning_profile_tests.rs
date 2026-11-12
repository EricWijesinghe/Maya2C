//! Hardware self-tuning (HW-2, ADR-137): speed only, never a result.
//!
//! What these pin, by the ADR's replay-test names:
//!
//! - the block builder selects the same transactions under the smallest,
//!   default, largest and absurd profiles, with a bad signature at the first,
//!   middle and last position and at each prewarm chunk boundary (R-8, N-2);
//! - the same blocks applied under each profile give the same decisions and
//!   state roots (invariant 24);
//! - only the tuning module and the node binary name the profile type (N-1),
//!   and it has exactly the ADR's knobs (N-7);
//! - hostile probe values are unknown or clamped, a cgroup limit beats host
//!   memory, and unknown inputs keep today's defaults (N-3, N-4, S-18);
//! - `off` is today's storage options exactly (O-4);
//! - `get_node_tuning` is absent unless enabled, refuses forwarded requests,
//!   and the public status carries nothing hardware-derived (N-5, S-9).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use custom_l1_node::config::StorageConfig;
use custom_l1_node::consensus::chain::ChainConfig;
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::rpc::{RpcContext, RpcServer, serve};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use custom_l1_node::tuning::detect::{cap_memory, parse_cgroup_limit, parse_meminfo};
use custom_l1_node::tuning::{
    HardwareProbe, MAX_BACKGROUND_JOBS, MAX_BLOCK_CACHE_MIB, MAX_VERIFY_BATCH, MAX_VERIFY_THREADS,
    MAX_WRITE_BUFFER_MIB, MIN_OPEN_FILES, StorageKind, TuningMode, TuningProfile, TuningReport,
};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HeaderMap, HeaderValue, HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

const BOB: Address = [0xB0; 32];
const FUNDS: u64 = 1_000_000;
const PAYMENT: u64 = 10;
const SENDERS: usize = 6;
const NONCES: u64 = 8;

// ---------------------------------------------------------------------------
// profiles and a ledger under each
// ---------------------------------------------------------------------------

fn profiles() -> Vec<(&'static str, TuningProfile)> {
    let unknown = HardwareProbe::default();
    let base = TuningProfile::off();
    vec![
        (
            "smallest",
            TuningProfile {
                verify_threads: 1,
                verify_batch: 1,
                block_cache_mib: 1,
                write_buffer_mib: 1,
                max_open_files: MIN_OPEN_FILES,
                background_jobs: 1,
            },
        ),
        ("default", base),
        // Chunks of 12 and 10 over 48 transactions: boundaries at 9..12, 23/24, 35/36.
        (
            "four threads",
            TuningProfile {
                verify_threads: 4,
                verify_batch: 1,
                ..base
            },
        ),
        (
            "five threads",
            TuningProfile {
                verify_threads: 5,
                verify_batch: 1,
                ..base
            },
        ),
        (
            "largest",
            TuningProfile {
                verify_threads: MAX_VERIFY_THREADS,
                verify_batch: 1,
                block_cache_mib: MAX_BLOCK_CACHE_MIB,
                write_buffer_mib: MAX_WRITE_BUFFER_MIB,
                max_open_files: -1,
                background_jobs: MAX_BACKGROUND_JOBS,
            },
        ),
        (
            "absurd",
            TuningProfile {
                verify_threads: usize::MAX,
                verify_batch: usize::MAX,
                block_cache_mib: usize::MAX,
                write_buffer_mib: 0,
                max_open_files: i32::MAX,
                background_jobs: -7,
            }
            .clamped(&unknown),
        ),
    ]
}

struct Ledger {
    db: StateDB,
    _dir: TempDir,
}

fn ledger(profile: &TuningProfile, senders: &[Address]) -> Ledger {
    let dir = TempDir::new().unwrap();
    let storage = profile.storage(&StorageConfig::default());
    let db = StateDB::open_tuned(dir.path(), &storage).unwrap();
    common::bind(&db);
    for a in senders {
        let funded = Account {
            balance: U256::from(FUNDS),
            nonce: 0,
        };
        db.put_account(a, &funded).unwrap();
    }
    Ledger { db, _dir: dir }
}

fn header(height: u64) -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_790_000_000 + height,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    }
}

fn transfer(key: &HybridSigningKey, nonce: u64) -> Transaction {
    let output = TxOutput {
        amount: U256::from(PAYMENT),
        recipient: BOB,
    };
    let mut tx = Transaction::new(vec![], vec![output], nonce);
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

/// The same transaction with its signature no longer matching its body.
fn corrupted(tx: &Transaction) -> Transaction {
    let mut bad = tx.clone();
    bad.outputs[0].amount = U256::from(PAYMENT + 1);
    bad
}

/// `SENDERS` keys, and their transfers interleaved by nonce.
fn candidates() -> (Vec<Address>, Vec<Transaction>) {
    let keys: Vec<HybridSigningKey> = (0..SENDERS)
        .map(|_| generate_signing_key().unwrap())
        .collect();
    let txs = (0..NONCES)
        .flat_map(|nonce| keys.iter().map(move |k| (k, nonce)))
        .map(|(k, nonce)| transfer(k, nonce))
        .collect();
    (keys.iter().map(HybridSigningKey::address).collect(), txs)
}

fn ids(txs: &[Transaction]) -> Vec<[u8; 32]> {
    txs.iter().map(Transaction::txid).collect()
}

// ---------------------------------------------------------------------------
// neutrality: builder and apply, separately (ADR-137 S-19)
// ---------------------------------------------------------------------------

#[test]
fn prewarm_threads_do_not_change_what_the_builder_selects() {
    let (senders, txs) = candidates();
    let last = txs.len() - 1;
    let positions = [0, 9, 10, 11, 12, 23, 24, 35, 36, last];
    for position in positions {
        let mut offered = txs.clone();
        offered[position] = corrupted(&txs[position]);
        let bad = offered[position].txid();
        let mut selections = Vec::new();
        for (name, profile) in profiles() {
            let ledger = ledger(&profile, &senders);
            let kept = ledger
                .db
                .select_applicable(
                    offered.clone(),
                    BlockContext::at_height(1),
                    target_from_leading_zero_bits(0),
                )
                .unwrap();
            assert!(
                !ids(&kept).contains(&bad),
                "{name}: a bad signature at {position} was selected"
            );
            selections.push((name, ids(&kept)));
        }
        let (first_name, first) = &selections[0];
        for (name, selected) in &selections[1..] {
            assert_eq!(
                selected, first,
                "bad at {position}: {name} selected differently from {first_name}"
            );
        }
    }
}

#[test]
fn every_tuning_profile_gives_the_same_decisions_and_state_root() {
    let (senders, txs) = candidates();
    let per_block = SENDERS * 2;
    let valid = txs[..per_block].to_vec();
    let mut with_bad = txs[per_block..2 * per_block].to_vec();
    with_bad[per_block / 2] = corrupted(&with_bad[per_block / 2]);
    let blocks = [valid, with_bad, txs[per_block..2 * per_block].to_vec()];

    let mut outcomes = Vec::new();
    for (name, profile) in profiles() {
        let ledger = ledger(&profile, &senders);
        let decisions: Vec<Result<[u8; 32], String>> = blocks
            .iter()
            .zip(1_u64..)
            .map(|(txs, height)| {
                let block = Block::new(header(height), txs.clone());
                ledger
                    .db
                    .apply_block(&block, BlockContext::at_height(height))
                    .map_err(|e| e.to_string())
            })
            .collect();
        outcomes.push((name, decisions));
    }
    let (first_name, first) = &outcomes[0];
    assert!(
        first[0].is_ok(),
        "the all-valid block must apply: {:?}",
        first[0]
    );
    for (name, decisions) in &outcomes[1..] {
        assert_eq!(
            decisions, first,
            "{name} decided differently from {first_name}"
        );
    }
}

// ---------------------------------------------------------------------------
// the profile's reach and shape
// ---------------------------------------------------------------------------

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn tuning_profile_is_named_only_by_the_allowed_modules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let allowed = |rel: &str| {
        rel.starts_with("crates/node/src/tuning/") || rel == "bins/maya2c-node/src/main.rs"
    };
    let mut files = Vec::new();
    rust_files(&root.join("crates/node/src"), &mut files);
    rust_files(&root.join("bins/maya2c-node/src"), &mut files);
    let mut offenders = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if std::fs::read_to_string(&file)
            .unwrap()
            .contains("TuningProfile")
            && !allowed(&rel)
        {
            offenders.push(rel);
        }
    }
    assert!(
        offenders.is_empty(),
        "the tuning profile reached: {offenders:?}"
    );
}

#[test]
fn the_profile_has_exactly_the_listed_fields() {
    let value = serde_json::to_value(TuningProfile::off()).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "background_jobs",
            "block_cache_mib",
            "max_open_files",
            "verify_batch",
            "verify_threads",
            "write_buffer_mib"
        ]
    );
}

// ---------------------------------------------------------------------------
// probes and derivation
// ---------------------------------------------------------------------------

fn host(cores: usize, available_mib: u64, storage: StorageKind) -> HardwareProbe {
    HardwareProbe {
        logical_cores: Some(cores),
        total_memory_mib: Some(available_mib.saturating_mul(2)),
        available_memory_mib: Some(available_mib),
        storage,
        ..HardwareProbe::default()
    }
}

#[test]
fn a_hostile_probe_value_is_unknown_or_clamped() {
    assert_eq!(
        parse_meminfo("MemTotal: 18446744073709551615 kB\nMemAvailable: 0 kB\n"),
        (None, None)
    );
    assert_eq!(parse_meminfo("MemTotal: -5 kB\n"), (None, None));
    let wild = HardwareProbe {
        logical_cores: Some(usize::MAX),
        ..host(1, u64::MAX, StorageKind::Ssd)
    };
    let t = TuningProfile::auto(&wild);
    assert_eq!(t.verify_threads, MAX_VERIFY_THREADS);
    assert_eq!(t.background_jobs, MAX_BACKGROUND_JOBS);
    assert!(t.block_cache_mib <= MAX_BLOCK_CACHE_MIB && t.write_buffer_mib <= MAX_WRITE_BUFFER_MIB);
    let absurd = TuningProfile {
        verify_threads: usize::MAX,
        verify_batch: 0,
        block_cache_mib: usize::MAX,
        write_buffer_mib: usize::MAX,
        max_open_files: -40,
        background_jobs: i32::MAX,
    }
    .clamped(&HardwareProbe::default());
    assert_eq!(absurd.verify_threads, MAX_VERIFY_THREADS);
    assert_eq!(absurd.verify_batch, 1);
    assert!(absurd.verify_batch <= MAX_VERIFY_BATCH);
    assert_eq!(absurd.block_cache_mib, MAX_BLOCK_CACHE_MIB);
    assert_eq!(absurd.write_buffer_mib, MAX_WRITE_BUFFER_MIB);
    assert_eq!(absurd.max_open_files, -1);
    assert_eq!(absurd.background_jobs, MAX_BACKGROUND_JOBS);
}

#[test]
fn the_cgroup_limit_wins_over_host_memory() {
    assert_eq!(parse_cgroup_limit("2147483648\n"), Some(2_048));
    assert_eq!(
        cap_memory((Some(262_144), Some(200_000)), Some(2_048)),
        (Some(2_048), Some(2_048))
    );
    assert_eq!(
        cap_memory((Some(4_096), Some(1_000)), Some(2_048)),
        (Some(2_048), Some(1_000))
    );
    assert_eq!(
        cap_memory((Some(4_096), Some(1_000)), None),
        (Some(4_096), Some(1_000))
    );
    let boxed = TuningProfile::auto(&host(8, 2_048, StorageKind::Ssd));
    assert!(boxed.block_cache_mib <= 2_048 / 4, "{boxed:?}");
}

#[test]
fn a_cgroup_unlimited_sentinel_is_unknown_not_a_limit() {
    assert_eq!(parse_cgroup_limit("max\n"), None, "cgroup v2");
    assert_eq!(
        parse_cgroup_limit("9223372036854771712\n"),
        None,
        "cgroup v1"
    );
    assert_eq!(parse_cgroup_limit("-1"), None);
    assert_eq!(parse_cgroup_limit("0"), None);
    assert_eq!(parse_cgroup_limit("lots"), None);
}

#[test]
fn unknown_memory_or_storage_keeps_todays_defaults() {
    assert_eq!(
        TuningProfile::auto(&HardwareProbe::default()),
        TuningProfile::off()
    );
    let off = TuningProfile::off();
    for storage in [
        StorageKind::Unknown,
        StorageKind::Rotational,
        StorageKind::Ssd,
    ] {
        let cores_only = HardwareProbe {
            logical_cores: Some(16),
            storage,
            ..HardwareProbe::default()
        };
        let t = TuningProfile::auto(&cores_only);
        assert_eq!(
            (t.block_cache_mib, t.write_buffer_mib),
            (off.block_cache_mib, off.write_buffer_mib)
        );
        assert_eq!(
            (t.max_open_files, t.verify_batch),
            (off.max_open_files, off.verify_batch)
        );
    }
}

#[test]
fn an_explicit_storage_value_beats_auto() {
    let tuned = TuningProfile::auto(&host(16, 32_768, StorageKind::Ssd));
    assert_ne!(tuned.block_cache_mib, 100);
    let configured = StorageConfig {
        block_cache_mib: 100,
        verify_batch: 7,
        ..StorageConfig::default()
    };
    let out = tuned.storage(&configured);
    assert_eq!((out.block_cache_mib, out.verify_batch), (100, 7));
    assert_eq!(out.write_buffer_mib, tuned.write_buffer_mib);
    assert_eq!(out.verify_threads, tuned.verify_threads);
}

#[test]
fn mode_off_builds_the_same_storage_options_as_before() {
    // Literals, not the constants: changing a default must fail here.
    let off = TuningProfile::off();
    assert_eq!(
        (
            off.verify_threads,
            off.verify_batch,
            off.block_cache_mib,
            off.write_buffer_mib
        ),
        (0, 32, 512, 64)
    );
    assert_eq!((off.max_open_files, off.background_jobs), (-1, 0));
    let big = host(128, 1 << 20, StorageKind::Rotational);
    for mode in [TuningMode::Off, TuningMode::Manual] {
        assert_eq!(TuningProfile::resolve(&mode, &big).unwrap(), off);
    }
    let custom = StorageConfig {
        block_cache_mib: 2_000,
        max_open_files: 512,
        ..StorageConfig::default()
    };
    for configured in [StorageConfig::default(), custom] {
        assert_eq!(off.storage(&configured), configured);
    }
}

#[test]
fn verification_threads_stay_below_the_core_count_above_two_cores() {
    for cores in 1..=256 {
        let t = TuningProfile::auto(&host(cores, 8_192, StorageKind::Ssd));
        assert!(t.verify_threads >= 1, "{cores}: {t:?}");
        if cores > 2 {
            assert!(t.verify_threads < cores, "{cores}: {t:?}");
        }
    }
}

#[test]
fn no_profile_exceeds_the_memory_ceiling() {
    for available in [1_u64, 15, 64, 300, 1_000, 4_096, 65_536, 1 << 22] {
        for storage in [
            StorageKind::Ssd,
            StorageKind::Rotational,
            StorageKind::Unknown,
        ] {
            let hw = host(8, available, storage);
            let greedy = TuningProfile {
                block_cache_mib: usize::MAX,
                write_buffer_mib: usize::MAX,
                ..TuningProfile::off()
            };
            for t in [TuningProfile::auto(&hw), greedy.clamped(&hw)] {
                let cache_cap = usize::try_from(available / 4).unwrap().max(1);
                let buffer_cap = usize::try_from(available / 16).unwrap().max(1);
                assert!(
                    (1..=cache_cap).contains(&t.block_cache_mib),
                    "{available}: {t:?}"
                );
                assert!(
                    (1..=buffer_cap).contains(&t.write_buffer_mib),
                    "{available}: {t:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// exposure (ADR-137 N-5, S-9)
// ---------------------------------------------------------------------------

struct Served {
    server: RpcServer,
    _dir: TempDir,
}

async fn serve_node(tuning: bool) -> Served {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    let genesis = Block::new(header(0), Vec::new());
    let chain = common::open_chain(
        Arc::clone(&state),
        genesis,
        ChainConfig::without_pow_verification(),
    )
    .unwrap();
    let mut context = RpcContext::new(Arc::new(Mutex::new(chain)), Mempool::new(state));
    if tuning {
        context = context.with_tuning(Arc::new(TuningReport {
            mode: "off".to_string(),
            hardware: HardwareProbe::default(),
            profile: TuningProfile::off(),
        }));
    }
    let server = serve("127.0.0.1:0".parse().unwrap(), context)
        .await
        .unwrap();
    Served { server, _dir: dir }
}

fn client(served: &Served, header: Option<(&'static str, &'static str)>) -> HttpClient {
    let mut headers = HeaderMap::new();
    if let Some((name, value)) = header {
        headers.insert(name, HeaderValue::from_static(value));
    }
    HttpClientBuilder::default()
        .set_headers(headers)
        .build(format!("http://{}", served.server.address))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tuning_method_is_absent_unless_enabled() {
    let off = serve_node(false).await;
    let answer: Result<serde_json::Value, _> = client(&off, None)
        .request("get_node_tuning", rpc_params![])
        .await;
    let error = answer
        .expect_err("an unenabled node must not serve the method")
        .to_string();
    assert!(!error.contains("loopback"), "absent, not refused: {error}");

    let on = serve_node(true).await;
    let report: serde_json::Value = client(&on, None)
        .request("get_node_tuning", rpc_params![])
        .await
        .unwrap();
    assert_eq!(report["mode"], "off");
    assert_eq!(report["profile"]["verify_batch"], 32);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tuning_status_is_refused_when_a_forwarded_header_arrives() {
    let on = serve_node(true).await;
    for header in [
        ("x-forwarded-for", "203.0.113.7"),
        ("cf-connecting-ip", "203.0.113.7"),
        ("forwarded", "for=203.0.113.7"),
        ("x-real-ip", "203.0.113.7"),
    ] {
        let answer: Result<serde_json::Value, _> = client(&on, Some(header))
            .request("get_node_tuning", rpc_params![])
            .await;
        let error = answer
            .expect_err("a forwarded call must be refused")
            .to_string();
        assert!(error.contains("loopback"), "{header:?}: {error}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_public_status_carries_no_hardware_derived_field() {
    let on = serve_node(true).await;
    let status: serde_json::Value = client(&on, None)
        .request("get_node_version", rpc_params![])
        .await
        .unwrap();
    let mut keys: Vec<&str> = status
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["header_version", "protocol", "version"]);
}
