//! Mining rewards on a live chain (ADR-044).
//!
//! A valid share is paid from the reserve, never minted; a replayed share, a
//! share for the wrong epoch, a share over the block cap and work that misses
//! the target are all refused; an epoch boundary pays the committee's part,
//! retargets, and anchors the next epoch to the boundary block. Value is
//! conserved throughout.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::U256;
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::mining_payload::MiningShare;
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::transaction::Transaction;
use custom_l1_node::crypto::dag::Params;
use custom_l1_node::crypto::dag::hashimoto::hashimoto_light;
use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys;
use custom_l1_node::crypto::pow::meets_target;
use custom_l1_node::error::NodeError;
use custom_l1_node::genesis::{
    Allocation, BftGenesis, FeesGenesis, GenesisBond, GenesisConfig, StakingGenesis,
};
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::db::StateDB;
use custom_l1_node::state::mining::{DagProfile, MINING_RESERVE, MiningGenesis, share_header};
use tempfile::TempDir;

mod common;

const EPOCH_BLOCKS: u64 = 5;
const RESERVE: u64 = 1_000_000_000;
const EPOCH_REWARD: u64 = 1_000_000;
const TARGET_SHARES: u64 = 10;
/// Easy enough that a test finds a share in a few dozen tries.
const TARGET: [u8; 32] = [0x0f; 32];

fn miner() -> HybridSigningKey {
    signing_key_from_seed(&[0x51; 32]).unwrap()
}

fn mining() -> MiningGenesis {
    MiningGenesis {
        initial_epoch_reward: primitive_types::U256::from(EPOCH_REWARD),
        halving_epochs: 100,
        miner_share_bps: 7_000,
        target_shares: TARGET_SHARES,
        max_shares_per_block: 2,
        initial_target: hex::encode(TARGET),
        dag: DagProfile::Testing,
    }
}

fn config(mining: Option<MiningGenesis>) -> GenesisConfig {
    let validator = keys::signing_key_from_seed(&[1; 32]).unwrap();
    GenesisConfig {
        chain_id: "mining-live".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: hex::encode(MINING_RESERVE),
            balance: primitive_types::U256::from(RESERVE),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators: vec![hex::encode(validator.verifying_key().to_bytes())],
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 500,
            staking: Some(StakingGenesis::with_bonds(
                EPOCH_BLOCKS,
                vec![GenesisBond {
                    operator: hex::encode([0x0a; 32]),
                    bond: primitive_types::U256::from(10_000),
                    commission_bps: 0,
                }],
            )),
            fees: Some(FeesGenesis {
                initial_base_fee: 1,
                min_base_fee: 1,
                target_block_bytes: 100_000,
                change_denominator: 8,
            }),
            mining,
        }),
    }
}

fn chain(dir: &TempDir, mining: Option<MiningGenesis>) -> Chain {
    let config = config(mining);
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    config.seed_state(&state).unwrap();
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

/// Nonces at or after `from` whose work meets the target, for `epoch`.
fn find_shares(c: &Chain, epoch: u64, from: u64, count: usize) -> Vec<u64> {
    let record = c.state().committed_mining().unwrap().unwrap();
    let header = share_header(
        &common::test_chain().0,
        epoch,
        &record.anchor,
        &miner().address(),
    );
    let registry = CacheRegistry::new(DagConfig {
        params: Params::TESTING,
        activation_height: 0,
        activation_target: [0xff; 32],
    });
    let start = epoch * EPOCH_BLOCKS;
    let cache = registry
        .cache_for_epoch(start / Params::TESTING.epoch_length)
        .unwrap();
    (from..)
        .filter(|&n| meets_target(&hashimoto_light(&cache, &header, n).result, &record.target))
        .take(count)
        .collect()
}

fn share(epoch: u64, nonce: u64, tx_nonce: u64) -> Transaction {
    let mut tx = Transaction::new(vec![], vec![], tx_nonce);
    tx.kind = TxKind::MiningShare(MiningShare { epoch, nonce });
    tx.sign(&miner(), &common::test_chain()).unwrap();
    tx
}

fn balance(c: &Chain, who: &[u8; 32]) -> u64 {
    u64::try_from(c.state().get_account(who).unwrap().balance).expect("test balances fit u64")
}

fn mine(c: &mut Chain, height: u64, txs: Vec<Transaction>) -> Result<(), NodeError> {
    let block = c.candidate_block(1_000_000 + height, txs)?;
    c.insert_block(block).map(|_| ())
}

fn per_share() -> u64 {
    EPOCH_REWARD * 7 / 10 / TARGET_SHARES
}

fn validators_part() -> u64 {
    EPOCH_REWARD - EPOCH_REWARD * 7 / 10
}

/// Mines epoch 0 empty. Mining opens at its boundary, anchored to a block.
fn to_epoch_one(c: &mut Chain) {
    for h in 1..=EPOCH_BLOCKS {
        mine(c, h, Vec::new()).unwrap();
    }
    assert_eq!(c.state().committed_mining().unwrap().unwrap().epoch, 1);
}

const H1: u64 = EPOCH_BLOCKS + 1;

#[test]
fn epoch_zero_work_is_refused_because_its_anchor_was_known_before_launch() {
    let dir = TempDir::new().unwrap();
    let c = chain(&dir, Some(mining()));
    let nonce = find_shares(&c, 0, 0, 1)[0];
    let err = c
        .candidate_block(1_000_001, vec![share(0, nonce, 0)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}

#[test]
fn a_share_is_paid_from_the_reserve_and_a_new_miner_pays_no_fee() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let nonce = find_shares(&c, 1, 0, 1)[0];

    mine(&mut c, H1, vec![share(1, nonce, 0)]).unwrap();

    assert_eq!(balance(&c, &miner().address()), per_share());
    assert_eq!(
        balance(&c, &MINING_RESERVE),
        RESERVE - validators_part() - per_share()
    );
    let record = c.state().committed_mining().unwrap().unwrap();
    assert_eq!(
        (record.shares, record.paid),
        (1, maya_ledger_math::Amount::from_u64(per_share()))
    );
}

#[test]
fn the_mempool_admits_a_paying_share_from_an_empty_account_and_refuses_bad_work() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let pool = Mempool::new(Arc::clone(c.state()));
    let good = find_shares(&c, 1, 0, 64);
    let miss = (0..).find(|n| !good.contains(n)).unwrap();

    assert!(pool.validate(&share(1, good[0], 0)).is_ok());
    let err = pool.validate(&share(1, miss, 0)).unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
    let err = pool.validate(&share(2, good[0], 0)).unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}

#[test]
fn a_share_cannot_be_paid_twice() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let nonce = find_shares(&c, 1, 0, 1)[0];
    mine(&mut c, H1, vec![share(1, nonce, 0)]).unwrap();

    // Same work, fresh transaction nonce: still the same share.
    let err = c
        .candidate_block(1_000_000 + H1 + 1, vec![share(1, nonce, 1)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}

/// Found on the local mining devnet: once an epoch's miner budget was paid,
/// further shares still landed paying 0, taking block slots fee-free and
/// spending the miner's nonces.
#[test]
fn a_share_past_the_epoch_budget_is_refused_by_pool_and_block() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(
        &dir,
        Some(MiningGenesis {
            target_shares: 2,
            ..mining()
        }),
    );
    to_epoch_one(&mut c);
    let found = find_shares(&c, 1, 0, 3);
    mine(
        &mut c,
        H1,
        vec![share(1, found[0], 0), share(1, found[1], 1)],
    )
    .unwrap();
    let record = c.state().committed_mining().unwrap().unwrap();
    assert_eq!(
        record.share_reward(),
        maya_ledger_math::Amount::from_u64(0),
        "budget spent"
    );

    let late = share(1, found[2], 2);
    let pool = Mempool::new(Arc::clone(c.state()));
    let err = pool.validate(&late).unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
    let err = c
        .candidate_block(1_000_000 + H1 + 1, vec![late])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}

/// Security review of the budget refusal: with shares past the budget
/// refused, the share count alone could only ease the target. A budget
/// drained in the epoch's first block must harden the next one.
#[test]
fn a_budget_drained_early_hardens_the_next_epochs_target() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(
        &dir,
        Some(MiningGenesis {
            target_shares: 2,
            ..mining()
        }),
    );
    to_epoch_one(&mut c);
    let found = find_shares(&c, 1, 0, 2);
    mine(
        &mut c,
        H1,
        vec![share(1, found[0], 0), share(1, found[1], 1)],
    )
    .unwrap();
    assert_eq!(
        c.state().committed_mining().unwrap().unwrap().spent_after,
        1
    );
    for h in H1 + 1..=2 * EPOCH_BLOCKS {
        mine(&mut c, h, Vec::new()).unwrap();
    }

    let next = c.state().committed_mining().unwrap().unwrap();
    assert_eq!((next.epoch, next.spent_after), (2, 0));
    // 2 shares in 1 of 5 blocks is demand of 10 against 2: clamped at x4.
    let quarter = U256::from_be_bytes(&TARGET).mul_div_u64(1, 4).unwrap();
    assert_eq!(U256::from_be_bytes(&next.target), quarter);
}

#[test]
fn work_for_another_epoch_or_below_target_is_refused() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let good = find_shares(&c, 1, 0, 64);

    let err = c
        .candidate_block(1_000_000 + H1, vec![share(2, good[0], 0)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");

    let miss = (0..).find(|n| !good.contains(n)).unwrap();
    let err = c
        .candidate_block(1_000_000 + H1, vec![share(1, miss, 0)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}

#[test]
fn a_block_over_the_share_cap_is_refused() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let txs = find_shares(&c, 1, 0, 3)
        .iter()
        .zip(0..)
        .map(|(&n, i)| share(1, n, i))
        .collect();
    let err = c.candidate_block(1_000_000 + H1, txs).unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}

#[test]
fn the_epoch_boundary_pays_the_committee_reanchors_and_prunes_spent_shares() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let anchor_one = c.state().committed_mining().unwrap().unwrap().anchor;
    assert_ne!(anchor_one, [0; 32], "anchored to the boundary block");
    let nonce = find_shares(&c, 1, 0, 1)[0];
    mine(&mut c, H1, vec![share(1, nonce, 0)]).unwrap();
    assert_eq!(c.state().committed_seen_shares(1).unwrap(), 1);
    for h in H1 + 1..=2 * EPOCH_BLOCKS {
        mine(&mut c, h, Vec::new()).unwrap();
    }

    let record = c.state().committed_mining().unwrap().unwrap();
    assert_eq!(record.epoch, 2);
    assert_eq!(
        (record.shares, record.paid),
        (0, maya_ledger_math::Amount::ZERO)
    );
    assert_ne!(record.anchor, anchor_one);
    // One share against a target of ten would ease the target, but never past
    // the genesis target.
    assert_eq!(record.target, TARGET);
    assert_eq!(c.state().committed_seen_shares(1).unwrap(), 0, "pruned");
    assert_eq!(
        balance(&c, &MINING_RESERVE),
        RESERVE - 2 * validators_part() - per_share(),
        "the committee's part left the reserve at both boundaries"
    );

    // Work from epoch 1 is worthless now.
    let err = c
        .candidate_block(1_000_100, vec![share(1, nonce, 1)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}

#[test]
fn a_chain_without_mining_refuses_shares() {
    let dir = TempDir::new().unwrap();
    let c = chain(&dir, None);
    let err = c
        .candidate_block(1_000_001, vec![share(0, 0, 0)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}

#[test]
fn mining_without_fees_is_refused_at_genesis() {
    let mut config = config(Some(mining()));
    if let Some(bft) = config.bft.as_mut() {
        bft.fees = None;
    }
    let dir = TempDir::new().unwrap();
    let state = StateDB::open(dir.path()).unwrap();
    assert!(config.seed_state(&state).is_err());
}

#[test]
fn the_work_handed_to_miners_finds_shares_the_chain_pays() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let record = c.state().committed_mining().unwrap().unwrap();

    let (header, dag_epoch) = c
        .state()
        .committed_share_work(&record, &miner().address())
        .unwrap();
    let registry = CacheRegistry::new(DagConfig {
        params: Params::TESTING,
        activation_height: 0,
        activation_target: [0xff; 32],
    });
    let cache = registry.cache_for_epoch(dag_epoch).unwrap();
    let nonce = (0..)
        .find(|&n| meets_target(&hashimoto_light(&cache, &header, n).result, &record.target))
        .unwrap();

    assert_eq!(
        nonce,
        find_shares(&c, 1, 0, 1)[0],
        "same header as the chain"
    );
    mine(&mut c, H1, vec![share(1, nonce, 0)]).unwrap();
    assert_eq!(balance(&c, &miner().address()), per_share());

    // Another payee gets another header: work cannot be lifted.
    let (other, _) = c
        .state()
        .committed_share_work(&record, &[0x77; 32])
        .unwrap();
    assert_ne!(other, header);
}
