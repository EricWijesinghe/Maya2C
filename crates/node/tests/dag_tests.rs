//! DAG proof-of-work: frozen vectors, and the consensus rules built on them.
//!
//! Two kinds of test live here.
//!
//! **Vectors.** `tests/fixtures/dag_vectors.json` pins the bytes of the epoch
//! seeds, the cache, the dataset and hashimoto itself, at test sizes and at the
//! sizes consensus actually uses. Every other property in this repository is
//! internal consistency — this is the one that says *which* function the chain
//! computes. `hal/cuda-miner/tests/dag_parity.rs` checks the GPU miner against the
//! same numbers, so node and kernel are pinned to one definition rather than to
//! each other.
//!
//! **Rules.** That the height a block lands at selects its proof-of-work rule,
//! that the epoch boundary rotates the dataset, and that a block mined under
//! the wrong rule or the wrong epoch is rejected.
//!
//! The mainnet vectors cost one 64 MiB cache and no dataset at all: a dataset
//! item is computable from the cache alone, which is the asymmetry the whole
//! design rests on. A test suite that could not check mainnet without 4 GiB
//! would be evidence that asymmetry did not exist.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use custom_l1_node::consensus::{Chain, ChainConfig, InsertOutcome, PowMode, mine_header_with};
use custom_l1_node::core::{Block, BlockHeader};
use custom_l1_node::crypto::dag::cache::Cache;
use custom_l1_node::crypto::dag::dataset::{Dataset, dataset_item};
use custom_l1_node::crypto::dag::hashimoto::{hashimoto_full, hashimoto_light};
use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use custom_l1_node::crypto::dag::{Params, epoch_seed, item_to_le_bytes};
use custom_l1_node::crypto::pow::{meets_target, target_from_leading_zero_bits};
use custom_l1_node::state::StateDB;
use serde_json::Value;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Leading zero bits every mined block in this file is ground to.
///
/// Four bits is one solution in sixteen: a couple of hundred milliseconds of
/// light hashing, and enough that "the chain accepted it" means the digest was
/// actually checked rather than trivially satisfied by an all-ones target.
const MINING_BITS: u32 = 4;

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/dag_vectors.json"
    );
    let text = std::fs::read_to_string(path).expect("the frozen vectors must be readable");
    serde_json::from_str(&text).expect("the frozen vectors must be valid JSON")
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("the vectors are missing `{key}`"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    field(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is not a string"))
}

fn open_state() -> (Arc<StateDB>, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open state");
    (Arc::new(db), dir)
}

/// Genesis at the difficulty every block here is mined to.
fn genesis() -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(MINING_BITS),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

/// A chain that verifies proof of work, with the DAG active from `activation`.
fn chain_with_activation(state: Arc<StateDB>, activation: u64) -> Chain {
    let genesis = genesis();
    let config =
        ChainConfig::with_pow_limit(genesis.header.difficulty_target).with_dag(DagConfig {
            params: Params::TESTING,
            activation_height: activation,
            ..DagConfig::TESTING
        });
    Chain::open(state, genesis, config).expect("open chain")
}

/// An unsolved child of the chain's tip.
fn candidate(chain: &Chain, timestamp: u64) -> BlockHeader {
    let tip = chain.tip();
    BlockHeader {
        prev_hash: tip,
        state_root: [0u8; 32],
        timestamp,
        nonce: 0,
        difficulty_target: chain.next_target(&tip).expect("next target"),
        tx_root: [0; 32],
    }
}

/// Grinds `header` under `mode` until it satisfies its own target.
fn solve(header: &BlockHeader, mode: &PowMode<'_>) -> BlockHeader {
    let cancel = AtomicBool::new(false);
    mine_header_with(header, 2, &cancel, Some(200_000), mode)
        .expect("hashing must succeed")
        .expect("a solution must exist at this difficulty")
        .header
}

/// The first nonce at or after `from` whose DAG digest *fails* `header`'s
/// target.
///
/// Used instead of "tamper with the winning nonce and hope": at one solution in
/// sixteen, a tampered nonce solves the block by accident often enough to make
/// a test flaky, and a flaky consensus test is worse than none.
fn first_failing_nonce(header: &BlockHeader, cache: &Cache, from: u64) -> u64 {
    let seed = header.pow_seed();
    (from..from + 1024)
        .find(|nonce| {
            !meets_target(
                &hashimoto_light(cache, &seed, *nonce).result,
                &header.difficulty_target,
            )
        })
        .expect("a failing nonce must exist within a thousand tries")
}

// ---------------------------------------------------------------------------
// frozen vectors
// ---------------------------------------------------------------------------

#[test]
fn the_epoch_seed_chain_matches_the_frozen_vectors() {
    let vectors = vectors();
    let seeds = field(&vectors, "epoch_seeds")
        .as_array()
        .expect("epoch_seeds must be an array");

    for (epoch, expected) in seeds.iter().enumerate() {
        let expected = expected.as_str().expect("a seed must be a hex string");
        assert_eq!(
            hex::encode(epoch_seed(epoch as u64)),
            expected,
            "epoch {epoch} seed changed"
        );
    }
}

#[test]
fn the_parameter_sets_match_the_frozen_vectors() {
    // A vector file taken at different sizes would compare the wrong bytes and
    // pass for the wrong reason, so the sizes are pinned too.
    let vectors = vectors();
    for (key, params) in [
        ("testing_params", Params::TESTING),
        ("mainnet_params", Params::MAINNET),
    ] {
        let recorded = field(&vectors, key);
        assert_eq!(
            field(recorded, "epoch_length").as_u64(),
            Some(params.epoch_length)
        );
        assert_eq!(
            field(recorded, "cache_bytes").as_u64(),
            Some(params.cache_bytes as u64)
        );
        assert_eq!(
            field(recorded, "dataset_bytes").as_u64(),
            Some(params.dataset_bytes as u64)
        );
        assert_eq!(
            field(recorded, "cache_items").as_u64(),
            Some(u64::from(params.cache_items()))
        );
        assert_eq!(
            field(recorded, "dataset_pages").as_u64(),
            Some(u64::from(params.dataset_pages()))
        );
    }
}

#[test]
fn the_cache_dataset_and_hashimoto_match_the_frozen_vectors() {
    let vectors = vectors();
    let cases = field(&vectors, "testing_cases")
        .as_array()
        .expect("testing_cases must be an array");

    for case in cases {
        let epoch = field(case, "epoch")
            .as_u64()
            .expect("epoch must be a number");
        let cache = Cache::generate(epoch, Params::TESTING).expect("cache generation");
        let dataset = Dataset::generate(&cache).expect("dataset generation");

        assert_eq!(
            hex::encode(item_to_le_bytes(cache.item(0))),
            text(case, "cache_item_first"),
            "epoch {epoch}: first cache item changed"
        );
        assert_eq!(
            hex::encode(item_to_le_bytes(cache.item(cache.len() - 1))),
            text(case, "cache_item_last"),
            "epoch {epoch}: last cache item changed"
        );

        check_dataset_items(case, &cache, epoch);
        check_hashimoto(case, &cache, Some(&dataset), epoch);

        // The whole dataset, not five spot checks: a generator that got one
        // region wrong would otherwise slip through.
        let mut hasher = blake3::Hasher::new();
        for item in dataset.items() {
            hasher.update(&item_to_le_bytes(item));
        }
        assert_eq!(
            hex::encode(hasher.finalize().as_bytes()),
            text(case, "dataset_digest"),
            "epoch {epoch}: the dataset as a whole changed"
        );
    }
}

#[test]
fn the_mainnet_dag_matches_the_frozen_vectors() {
    // The vectors that actually govern the chain. Costs one 64 MiB cache and no
    // dataset — see the module docs.
    let vectors = vectors();
    let case = field(&vectors, "mainnet_case");
    let cache = Cache::generate(0, Params::MAINNET).expect("cache generation");

    assert_eq!(
        hex::encode(item_to_le_bytes(cache.item(0))),
        text(case, "cache_item_first"),
        "the mainnet cache changed"
    );
    assert_eq!(
        hex::encode(item_to_le_bytes(cache.item(cache.len() - 1))),
        text(case, "cache_item_last"),
        "the mainnet cache changed"
    );

    check_dataset_items(case, &cache, 0);
    check_hashimoto(case, &cache, None, 0);
}

fn check_dataset_items(case: &Value, cache: &Cache, epoch: u64) {
    let items = field(case, "dataset_items")
        .as_array()
        .expect("dataset_items must be an array");

    for entry in items {
        let index = field(entry, "index")
            .as_u64()
            .expect("index must be a number");
        let index = u32::try_from(index).expect("index must fit a u32");
        assert_eq!(
            hex::encode(item_to_le_bytes(&dataset_item(cache, index))),
            text(entry, "item"),
            "epoch {epoch}: dataset item {index} changed"
        );
    }
}

fn check_hashimoto(case: &Value, cache: &Cache, dataset: Option<&Dataset>, epoch: u64) {
    let proofs = field(case, "hashimoto")
        .as_array()
        .expect("hashimoto must be an array");

    for entry in proofs {
        let header: [u8; 32] = hex::decode(text(entry, "header"))
            .expect("header must be hex")
            .try_into()
            .expect("header must be 32 bytes");
        let nonce = field(entry, "nonce")
            .as_u64()
            .expect("nonce must be a number");

        let light = hashimoto_light(cache, &header, nonce);
        assert_eq!(
            hex::encode(light.mix),
            text(entry, "mix"),
            "epoch {epoch}: mix changed at nonce {nonce}"
        );
        assert_eq!(
            hex::encode(light.result),
            text(entry, "result"),
            "epoch {epoch}: digest changed at nonce {nonce}"
        );

        if let Some(dataset) = dataset {
            assert_eq!(
                hashimoto_full(dataset, &header, nonce),
                light,
                "epoch {epoch}: the miner's path disagrees with the validator's"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// epoch selection
// ---------------------------------------------------------------------------

#[test]
fn the_epoch_boundary_changes_which_cache_a_height_validates_against() {
    let registry = CacheRegistry::new(DagConfig::TESTING);
    let length = Params::TESTING.epoch_length;

    let before = registry.cache_for_height(length - 1).expect("cache");
    let after = registry.cache_for_height(length).expect("cache");

    assert_eq!(before.epoch(), 0);
    assert_eq!(after.epoch(), 1);
    assert_ne!(before.item(0), after.item(0));

    // Both stay resident, so a reorg back across the boundary does not pay for
    // a regeneration inside fork choice.
    assert_eq!(registry.held_epochs(), vec![1, 0]);
}

#[test]
fn the_same_header_hashes_differently_either_side_of_a_boundary() {
    // What rotating the dataset has to mean at the level a miner experiences
    // it: work done for one epoch is worth nothing in the next.
    let registry = CacheRegistry::new(DagConfig::TESTING);
    let length = Params::TESTING.epoch_length;

    let header = BlockHeader {
        prev_hash: [7u8; 32],
        state_root: [9u8; 32],
        timestamp: 1_700_000_000,
        nonce: 42,
        difficulty_target: [0xFF; 32],
        tx_root: [0; 32],
    };

    let before = header.pow_hash_at(length - 1, &registry).expect("hash");
    let after = header.pow_hash_at(length, &registry).expect("hash");
    assert_ne!(before, after);
}

#[test]
fn the_rule_below_the_activation_height_is_argonblake() {
    let registry = CacheRegistry::new(DagConfig {
        activation_height: 4,
        ..DagConfig::TESTING
    });

    let header = BlockHeader {
        prev_hash: [1u8; 32],
        state_root: [2u8; 32],
        timestamp: 1_700_000_000,
        nonce: 3,
        difficulty_target: [0xFF; 32],
        tx_root: [0; 32],
    };

    // Below: the pre-fork digest, byte for byte.
    assert_eq!(
        header.pow_hash_at(3, &registry).expect("hash"),
        header.pow_hash().expect("hash")
    );
    // At and above: not the pre-fork digest.
    assert_ne!(
        header.pow_hash_at(4, &registry).expect("hash"),
        header.pow_hash().expect("hash")
    );
    // And nothing was generated for the heights that did not need it.
    assert_eq!(registry.held_epochs(), vec![0]);
}

// ---------------------------------------------------------------------------
// chain validation
// ---------------------------------------------------------------------------

#[test]
fn a_block_mined_against_the_epoch_cache_is_accepted() {
    let (state, _dir) = open_state();
    let mut chain = chain_with_activation(state, 1);

    let cache = chain.dag().cache_for_height(1).expect("cache");
    let header = candidate(&chain, 1_000_015);
    let solved = solve(&header, &PowMode::DagLight(&cache));

    let outcome = chain
        .insert_block(Block::new(solved, Vec::new()))
        .expect("a correctly mined block must be accepted");
    assert!(matches!(outcome, InsertOutcome::Extended { .. }));
}

#[test]
fn a_block_whose_dag_digest_misses_the_target_is_rejected() {
    let (state, _dir) = open_state();
    let mut chain = chain_with_activation(state, 1);

    let cache = chain.dag().cache_for_height(1).expect("cache");
    let mut header = candidate(&chain, 1_000_015);
    header.nonce = first_failing_nonce(&header, &cache, 0);

    let error = chain
        .insert_block(Block::new(header, Vec::new()))
        .expect_err("a block that misses its target must be rejected");
    assert!(
        error.to_string().contains("insufficient proof of work"),
        "unexpected rejection: {error}"
    );
}

#[test]
fn work_done_under_the_pre_fork_rule_does_not_satisfy_the_dag() {
    // The fork has to actually be a fork. A miner that kept running ArgonBlake
    // past the activation height must find its blocks rejected, or the memory
    // requirement would be optional.
    let (state, _dir) = open_state();
    let mut chain = chain_with_activation(state, 1);

    let header = candidate(&chain, 1_000_015);
    let argon_solved = solve(&header, &PowMode::Argon);
    assert!(
        argon_solved.meets_difficulty().expect("hash"),
        "the fixture must genuinely satisfy the pre-fork rule"
    );

    let cache = chain.dag().cache_for_height(1).expect("cache");
    // Advance to a nonce that solves ArgonBlake but not the DAG. Searching for
    // it rather than asserting the first one fails keeps the test deterministic.
    let mut candidate_header = argon_solved.clone();
    candidate_header.nonce = first_failing_nonce(&candidate_header, &cache, argon_solved.nonce);

    let error = chain
        .insert_block(Block::new(candidate_header, Vec::new()))
        .expect_err("pre-fork work must not satisfy the post-fork rule");
    assert!(
        error.to_string().contains("insufficient proof of work"),
        "unexpected rejection: {error}"
    );
}

#[test]
fn the_fork_block_takes_the_pinned_target_rather_than_inheriting() {
    // A target calibrated for a 25 ms hash would take many retarget windows to
    // unwind once a hash costs a thousandth of that. The fork block is pinned
    // instead; everything after it retargets normally from the pinned value.
    let (state, _dir) = open_state();
    let pinned = target_from_leading_zero_bits(9);
    let chain = {
        let genesis = genesis();
        let config =
            ChainConfig::with_pow_limit(genesis.header.difficulty_target).with_dag(DagConfig {
                activation_height: 1,
                activation_target: pinned,
                ..DagConfig::TESTING
            });
        Chain::open(state, genesis, config).expect("open chain")
    };

    assert_ne!(
        pinned,
        genesis().header.difficulty_target,
        "the fixture must actually differ from what would be inherited"
    );
    assert_eq!(chain.next_target(&chain.tip()).expect("target"), pinned);
}

#[test]
fn the_pinned_fork_target_cannot_undercut_the_network_floor() {
    // The pin replaces the inherited target; it does not get to be easier than
    // the floor, any more than a retarget does.
    let (state, _dir) = open_state();
    let floor = target_from_leading_zero_bits(6);
    let chain = {
        let genesis = genesis();
        let config = ChainConfig::with_pow_limit(floor).with_dag(DagConfig {
            activation_height: 1,
            activation_target: [0xFF; 32],
            ..DagConfig::TESTING
        });
        Chain::open(state, genesis, config).expect("open chain")
    };

    assert_eq!(chain.next_target(&chain.tip()).expect("target"), floor);
}

#[test]
fn a_pre_fork_block_is_still_validated_with_argonblake() {
    // Syncing from genesis means checking the chain as it was. If the DAG rule
    // were applied retroactively, no node could ever validate history.
    let (state, _dir) = open_state();
    let mut chain = chain_with_activation(state, 100);

    let header = candidate(&chain, 1_000_015);
    let solved = solve(&header, &PowMode::Argon);

    let outcome = chain
        .insert_block(Block::new(solved, Vec::new()))
        .expect("a pre-fork block must be accepted under the pre-fork rule");
    assert!(matches!(outcome, InsertOutcome::Extended { .. }));

    // And no cache was generated for a chain that never reached the fork.
    assert!(chain.dag().held_epochs().is_empty());
}
