//! Mining rewards on a live chain (ADR-044).
//!
//! A valid share is credited, and its epoch's miner budget is split pro rata
//! by work and paid from the reserve, never minted, when the miner claims it
//! or mines again (MINE-01); a replayed share, a share for the wrong epoch, a
//! share over the block cap and work that misses the target are all refused;
//! an epoch boundary pays the committee's part, sets the miners' pot aside,
//! retargets, and anchors the next epoch to the boundary block's parent
//! (#642). Many shares ride one signature (MINE-03). Value is conserved
//! throughout.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::U256;
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::Block;
use custom_l1_node::core::mining_payload::{MiningShare, WorkKind};
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::transaction::{Transaction, TxInput};
use custom_l1_node::crypto::dag::Params;
use custom_l1_node::crypto::dag::hashimoto::hashimoto_light;
use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::meets_target;
use custom_l1_node::crypto::validator_keys as keys;
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
        tail_epoch_reward: primitive_types::U256::zero(),
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
        suite_deprecations: Vec::new(),
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
                initial_base_fee: primitive_types::U256::one(),
                min_base_fee: primitive_types::U256::one(),
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
    find_shares_for(c, &miner().address(), epoch, from, count)
}

/// [`find_shares`] for another payee.
fn find_shares_for(c: &Chain, payee: &[u8; 32], epoch: u64, from: u64, count: usize) -> Vec<u64> {
    let record = c.state().committed_mining().unwrap().unwrap();
    let header = share_header(&common::test_chain().0, epoch, &record.anchor, payee);
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
    batch_by(&miner(), epoch, vec![nonce], tx_nonce)
}

/// One transaction carrying `nonces` for `epoch`, signed by `key`. No
/// nonces: a claim of `epoch`'s credit.
fn batch_by(key: &HybridSigningKey, epoch: u64, nonces: Vec<u64>, tx_nonce: u64) -> Transaction {
    let mut tx = Transaction::new(vec![], vec![], tx_nonce);
    tx.kind = TxKind::MiningShare(MiningShare {
        kind: WorkKind::DagPow,
        epoch,
        nonces,
    });
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

fn claim(epoch: u64, tx_nonce: u64) -> Transaction {
    batch_by(&miner(), epoch, Vec::new(), tx_nonce)
}

fn balance(c: &Chain, who: &[u8; 32]) -> u64 {
    u64::try_from(c.state().get_account(who).unwrap().balance).expect("test balances fit u64")
}

fn mine(c: &mut Chain, height: u64, txs: Vec<Transaction>) -> Result<(), NodeError> {
    let block = c.candidate_block(1_000_000 + height, txs)?;
    c.insert_block(block).map(|_| ())
}

/// The miners' part of an epoch's reward, split among them by work.
fn budget() -> u64 {
    EPOCH_REWARD * 7 / 10
}

/// Mines empty blocks from `from` through the boundary that closes `epoch`.
fn close_epoch(c: &mut Chain, from: u64, epoch: u64) {
    for h in from..=(epoch + 1) * EPOCH_BLOCKS {
        mine(c, h, Vec::new()).unwrap();
    }
    assert_eq!(
        c.state().committed_mining().unwrap().unwrap().epoch,
        epoch + 1
    );
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
fn a_share_is_credited_then_its_claim_is_paid_from_the_reserve_with_no_fee() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let nonce = find_shares(&c, 1, 0, 1)[0];

    mine(&mut c, H1, vec![share(1, nonce, 0)]).unwrap();

    // Credited, not paid: the epoch is still open.
    assert_eq!(balance(&c, &miner().address()), 0);
    let credit = c.state().committed_credit(&miner().address()).unwrap();
    assert_eq!(credit.map(|k| (k.epoch, k.shares)), Some((1, 1)));
    let record = c.state().committed_mining().unwrap().unwrap();
    assert_eq!((record.shares, record.miners), (1, 1));

    // A claim before the epoch closes has nothing to claim.
    let err = c
        .candidate_block(1_000_000 + H1 + 1, vec![claim(1, 1)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");

    close_epoch(&mut c, H1 + 1, 1);
    let pot = c.state().committed_pot(1).unwrap().unwrap();
    assert_eq!((pot.shares, pot.miners), (1, 1));
    let h = 2 * EPOCH_BLOCKS + 1;
    mine(&mut c, h, vec![claim(1, 1)]).unwrap();

    // The only miner did all the work, so it earns the whole budget.
    assert_eq!(balance(&c, &miner().address()), budget());
    assert_eq!(
        balance(&c, &MINING_RESERVE),
        RESERVE - 2 * validators_part() - budget()
    );
    assert!(
        c.state()
            .committed_credit(&miner().address())
            .unwrap()
            .is_none()
    );
    assert!(
        c.state().committed_pot(1).unwrap().is_none(),
        "the last credit took the pot"
    );

    // Claimed once: a second claim finds nothing.
    let err = c
        .candidate_block(1_000_000 + h + 1, vec![claim(1, 2)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
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

/// MINE-01: no first-come budget. Shares past `target_shares` still land
/// and split the same budget; the overshoot hardens the next target instead.
#[test]
fn shares_past_target_still_land_split_the_budget_and_harden_the_target() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(
        &dir,
        Some(MiningGenesis {
            target_shares: 2,
            ..mining()
        }),
    );
    to_epoch_one(&mut c);
    let found = find_shares(&c, 1, 0, 4);
    mine(
        &mut c,
        H1,
        vec![batch_by(&miner(), 1, found[..2].to_vec(), 0)],
    )
    .unwrap();
    // Past the target: before MINE-01 this share was refused as unpaid.
    let late = share(1, found[2], 1);
    let pool = Mempool::new(Arc::clone(c.state()));
    pool.validate(&late).unwrap();
    mine(&mut c, H1 + 1, vec![late, share(1, found[3], 2)]).unwrap();
    assert_eq!(c.state().committed_mining().unwrap().unwrap().shares, 4);

    close_epoch(&mut c, H1 + 2, 1);
    let next = c.state().committed_mining().unwrap().unwrap();
    // 4 shares against a target of 2: the target halves.
    let half = U256::from_be_bytes(&TARGET).mul_div_u64(1, 2).unwrap();
    assert_eq!(U256::from_be_bytes(&next.target), half);
    mine(&mut c, 2 * EPOCH_BLOCKS + 1, vec![claim(1, 3)]).unwrap();
    assert_eq!(
        balance(&c, &miner().address()),
        budget(),
        "all four shares, all the budget"
    );
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
    assert_eq!((record.shares, record.miners), (0, 0));
    assert_ne!(record.anchor, anchor_one);
    // One share against a target of ten would ease the target, but never past
    // the genesis target.
    assert_eq!(record.target, TARGET);
    assert_eq!(c.state().committed_seen_shares(1).unwrap(), 0, "pruned");
    assert_eq!(
        balance(&c, &MINING_RESERVE),
        RESERVE - 2 * validators_part(),
        "the committee's part left the reserve at both boundaries; the miners' waits in its pot"
    );
    let pot = c.state().committed_pot(1).unwrap().unwrap();
    assert_eq!(pot.budget, maya_ledger_math::Amount::from_u64(budget()));

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
    let credit = c.state().committed_credit(&miner().address()).unwrap();
    assert_eq!(credit.map(|k| k.shares), Some(1));

    // Another payee gets another header: work cannot be lifted.
    let (other, _) = c
        .state()
        .committed_share_work(&record, &[0x77; 32])
        .unwrap();
    assert_ne!(other, header);
}

#[test]
fn a_tail_needs_a_reserve_that_pays_it_until_the_last_height() {
    let tailed = MiningGenesis {
        tail_epoch_reward: primitive_types::U256::from(EPOCH_REWARD / 10),
        ..mining()
    };
    let lifetime = tailed
        .record(EPOCH_BLOCKS)
        .unwrap()
        .lifetime_emission()
        .unwrap();
    let lifetime = primitive_types::U256::from_little_endian(&lifetime.to_le_bytes());

    let short = TempDir::new().unwrap();
    let mut config = config(Some(tailed.clone()));
    config.allocations[0].balance = lifetime - 1;
    let state = Arc::new(StateDB::open(short.path()).unwrap());
    let err = config.seed_state(&state).unwrap_err().to_string();
    assert!(err.contains("lifetime emission"), "{err}");

    let funded = TempDir::new().unwrap();
    config.allocations[0].balance = lifetime;
    let state = Arc::new(StateDB::open(funded.path()).unwrap());
    config.seed_state(&state).unwrap();
}

#[test]
fn the_mempool_charges_a_share_that_names_an_input_the_base_fee() {
    // Valid epoch and work, but an input the share spends: it is not
    // fee-free, so admission refuses it for the fee, as the block would.
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let pool = Mempool::new(Arc::clone(c.state()));
    let good = find_shares(&c, 1, 0, 64);

    let mut tx = Transaction::new(
        vec![TxInput {
            prev_tx: [0; 32],
            index: 0,
        }],
        vec![],
        0,
    );
    tx.kind = TxKind::MiningShare(MiningShare::single(1, good[0]));
    tx.sign(&miner(), &common::test_chain()).unwrap();

    let err = pool.validate(&tx).unwrap_err();
    assert!(matches!(err, NodeError::FeeTooLow { .. }), "{err:?}");
}

#[test]
fn a_block_refuses_a_share_that_names_an_input_without_its_base_fee() {
    // The executor applies the same rule as the pool: only a share with no
    // inputs and no outputs is exempt from the base fee.
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let good = find_shares(&c, 1, 0, 1);

    let mut tx = Transaction::new(
        vec![TxInput {
            prev_tx: [0; 32],
            index: 0,
        }],
        vec![],
        0,
    );
    tx.kind = TxKind::MiningShare(MiningShare::single(1, good[0]));
    tx.sign(&miner(), &common::test_chain()).unwrap();

    let err = c.candidate_block(1_000_000 + H1, vec![tx]).unwrap_err();
    assert!(matches!(err, NodeError::FeeTooLow { .. }), "{err:?}");
}

/// One share's work signed under many transaction nonces is still one share:
/// only the first copy can ever be paid, so only the first may be pooled.
/// Before, every copy was admitted fee-free and outside the per-sender cap.
#[test]
fn one_shares_work_under_many_tx_nonces_is_pooled_once() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let pool = Mempool::new(Arc::clone(c.state()));
    let good = find_shares(&c, 1, 0, 2);

    assert!(pool.insert(share(1, good[0], 0)).unwrap());
    for tx_nonce in 1..4 {
        let err = pool.insert(share(1, good[0], tx_nonce)).unwrap_err();
        assert!(matches!(err, NodeError::MempoolRejected(_)), "{err:?}");
    }
    // Different work under the next nonce is a different share.
    assert!(pool.insert(share(1, good[1], 1)).unwrap());
    assert_eq!(pool.len(), 2);
}

/// A share is executed at its exact nonce like any transaction, so one that
/// leaves a gap can never run and would only be re-proposed fee-free.
#[test]
fn a_share_whose_nonce_leaves_a_gap_is_refused() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let pool = Mempool::new(Arc::clone(c.state()));
    let good = find_shares(&c, 1, 0, 2);

    let err = pool.insert(share(1, good[0], 1)).unwrap_err();
    assert!(matches!(err, NodeError::MempoolRejected(_)), "{err:?}");
    let far = custom_l1_node::network::mempool::MAX_NONCE_AHEAD + 1;
    let err = pool.insert(share(1, good[0], far)).unwrap_err();
    assert!(matches!(err, NodeError::MempoolRejected(_)), "{err:?}");

    assert!(pool.insert(share(1, good[0], 0)).unwrap());
    assert!(
        pool.insert(share(1, good[1], 1)).unwrap(),
        "the gap is filled"
    );
}

/// A pooled share's work was hashed at admission and cannot go bad, so the
/// per-round prune on the BFT consensus task must not hash it again: a pool
/// of shares past the block's share cap would otherwise cost every validator
/// ~2 ms per share every round (review of #653).
#[test]
fn pruning_dropped_shares_does_not_hash_their_work_again() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let pool = Mempool::new(Arc::clone(c.state()));
    let good = find_shares(&c, 1, 0, 3);
    let txs: Vec<Transaction> = (0u64..)
        .zip(&good)
        .map(|(tx_nonce, &work)| share(1, work, tx_nonce))
        .collect();
    for tx in &txs {
        assert!(pool.insert(tx.clone()).unwrap());
    }

    let before = c.state().share_proofs();
    let ids: Vec<_> = txs.iter().map(Transaction::txid).collect();
    assert_eq!(pool.prune_invalid(&ids), 0, "every share can still pay");
    assert_eq!(c.state().share_proofs(), before, "the prune re-hashed work");
    assert_eq!(pool.len(), 3);
}

/// The cheap share rules still prune: once the epoch closes, a pooled share
/// for it can never pay, and it goes without its work being hashed.
#[test]
fn a_dropped_share_from_a_closed_epoch_is_pruned_without_hashing() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let pool = Mempool::new(Arc::clone(c.state()));
    let work = find_shares(&c, 1, 0, 1)[0];
    let tx = share(1, work, 0);
    assert!(pool.insert(tx.clone()).unwrap());
    for h in H1..=2 * EPOCH_BLOCKS {
        mine(&mut c, h, Vec::new()).unwrap();
    }
    assert_eq!(c.state().committed_mining().unwrap().unwrap().epoch, 2);

    let before = c.state().share_proofs();
    assert_eq!(pool.prune_invalid(&[tx.txid()]), 1);
    assert_eq!(c.state().share_proofs(), before);
    assert!(pool.is_empty());
}

/// [`mine`], keeping a copy of the block for a replay.
fn mine_kept(c: &mut Chain, height: u64, txs: Vec<Transaction>, kept: &mut Vec<Block>) {
    let block = c.candidate_block(1_000_000 + height, txs).unwrap();
    kept.push(block.clone());
    c.insert_block(block).unwrap();
}

/// #642: the next epoch's anchor is part of the state the boundary block's
/// root commits to, so it must not be derived from that root. A validator
/// derives a block with its root zeroed and reuses that preview; a syncing
/// node, `maya2c replay` or a missed preview applies the sealed block fresh.
/// Both must reach the same root at every mining boundary. With the anchor
/// set to the boundary header's own id, the fresh replay fails at height 5.
#[test]
fn a_fresh_replay_reproduces_every_root_across_mining_boundaries() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    let mut kept = Vec::new();
    for h in 1..=EPOCH_BLOCKS {
        mine_kept(&mut c, h, Vec::new(), &mut kept);
    }
    let nonce = find_shares(&c, 1, 0, 1)[0];
    mine_kept(&mut c, H1, vec![share(1, nonce, 0)], &mut kept);
    for h in H1 + 1..=2 * EPOCH_BLOCKS {
        mine_kept(&mut c, h, Vec::new(), &mut kept);
    }
    mine_kept(&mut c, 2 * EPOCH_BLOCKS + 1, vec![claim(1, 1)], &mut kept);
    for h in 2 * EPOCH_BLOCKS + 2..=3 * EPOCH_BLOCKS {
        mine_kept(&mut c, h, Vec::new(), &mut kept);
    }

    let fresh_dir = TempDir::new().unwrap();
    let mut fresh = chain(&fresh_dir, Some(mining()));
    for block in kept {
        let height = fresh.height() + 1;
        fresh
            .insert_block(block)
            .unwrap_or_else(|e| panic!("fresh replay diverged at height {height}: {e}"));
    }
    assert_eq!(fresh.tip(), c.tip());
    assert_eq!(
        fresh.state().committed_mining().unwrap(),
        c.state().committed_mining().unwrap()
    );
}

/// The anchor is the boundary block's parent: unknown until that block is
/// final, so work for the epoch still cannot be done before it.
#[test]
fn an_epoch_is_anchored_to_its_boundary_blocks_parent() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    for h in 1..EPOCH_BLOCKS {
        mine(&mut c, h, Vec::new()).unwrap();
    }
    let parent = c.tip();
    mine(&mut c, EPOCH_BLOCKS, Vec::new()).unwrap();
    let record = c.state().committed_mining().unwrap().unwrap();
    assert_eq!((record.epoch, record.anchor), (1, parent));
}

/// Eric 2026-11-24, "a sole miner, or hundreds": a solo miner and a hundred
/// other miners in one epoch are each paid exactly `⌊budget × work / total⌋`,
/// whatever order their shares landed in, and the boundary that closes the
/// epoch writes one pot and touches none of their records (M10).
#[test]
fn a_solo_miner_and_a_hundred_miners_are_each_paid_in_proportion_to_work() {
    const OTHERS: usize = 100;
    const SOLO_SHARES: u64 = 4;
    const PER_BLOCK: usize = 26;
    let dir = TempDir::new().unwrap();
    let mut c = chain(
        &dir,
        Some(MiningGenesis {
            max_shares_per_block: 128,
            ..mining()
        }),
    );
    to_epoch_one(&mut c);
    let solo = miner();
    let others: Vec<HybridSigningKey> = (0..OTHERS)
        .map(|i| {
            let mut seed = [0x60; 32];
            seed[0] = u8::try_from(i).unwrap();
            signing_key_from_seed(&seed).unwrap()
        })
        .collect();
    let solo_work = find_shares(&c, 1, 0, usize::try_from(SOLO_SHARES).unwrap());
    let mut txs = vec![batch_by(&solo, 1, solo_work, 0)];
    for key in &others {
        let work = find_shares_for(&c, &key.address(), 1, 0, 1);
        txs.push(batch_by(key, 1, work, 0));
    }
    for (h, chunk) in (H1..).zip(txs.chunks(PER_BLOCK)) {
        mine(&mut c, h, chunk.to_vec()).unwrap();
    }
    let open = c.state().committed_mining().unwrap().unwrap();
    let total = SOLO_SHARES + u64::try_from(OTHERS).unwrap();
    assert_eq!((open.shares, open.miners), (total, 101));

    close_epoch(&mut c, H1 + 4, 1);
    // The boundary set one pot aside and left every credit as it was.
    let pot = c.state().committed_pot(1).unwrap().unwrap();
    assert_eq!((pot.shares, pot.miners), (total, 101));
    assert_eq!(
        c.state()
            .committed_credit(&solo.address())
            .unwrap()
            .map(|k| k.shares),
        Some(SOLO_SHARES)
    );
    for key in &others {
        let credit = c.state().committed_credit(&key.address()).unwrap();
        assert_eq!(credit.map(|k| (k.epoch, k.shares)), Some((1, 1)));
        assert_eq!(
            balance(&c, &key.address()),
            0,
            "nothing is pushed at the boundary"
        );
    }

    let mut claims = vec![claim(1, 1)];
    claims.extend(others.iter().map(|key| batch_by(key, 1, Vec::new(), 1)));
    let first_claim = 2 * EPOCH_BLOCKS + 1;
    for (h, chunk) in (first_claim..).zip(claims.chunks(PER_BLOCK)) {
        mine(&mut c, h, chunk.to_vec()).unwrap();
    }

    let solo_paid = balance(&c, &solo.address());
    assert_eq!(solo_paid, budget() * SOLO_SHARES / total);
    let mut paid = solo_paid;
    for key in &others {
        let got = balance(&c, &key.address());
        assert_eq!(got, budget() / total, "one share's part");
        paid += got;
    }
    // Four times the work, four times the pay, to the unit's floor.
    assert!(solo_paid >= 4 * (budget() / total));
    assert!(paid <= budget(), "never more than the budget");
    assert!(
        budget() - paid <= 101,
        "at most a unit of dust per miner stays in the reserve"
    );
    assert_eq!(
        balance(&c, &MINING_RESERVE),
        RESERVE - 2 * validators_part() - paid
    );
    assert!(
        c.state().committed_pot(1).unwrap().is_none(),
        "every credit collected"
    );
}

/// MINE-03 and M10: one more share in a batch costs its 8-byte nonce, not
/// another post-quantum signature.
#[test]
fn an_extra_share_in_a_batch_costs_eight_bytes() {
    let one = batch_by(&miner(), 1, vec![7], 0).to_bytes().len();
    let two = batch_by(&miner(), 1, vec![7, 8], 0).to_bytes().len();
    let hundred = batch_by(&miner(), 1, (0..100).collect(), 0)
        .to_bytes()
        .len();
    assert_eq!(two - one, 8);
    assert_eq!(hundred - one, 99 * 8);
}

/// A batch takes as many block slots as it carries shares, and one bad
/// nonce fails the whole batch.
#[test]
fn a_batch_counts_every_share_against_the_block_cap_and_fails_whole() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let good = find_shares(&c, 1, 0, 3);

    let err = c
        .candidate_block(1_000_000 + H1, vec![batch_by(&miner(), 1, good.clone(), 0)])
        .unwrap_err();
    assert!(
        matches!(err, NodeError::Mining(_)),
        "three past a cap of two: {err:?}"
    );

    let miss = (good[0] + 1..).find(|n| !good.contains(n)).unwrap();
    let mut mixed = vec![good[0], miss];
    mixed.sort_unstable();
    let err = c
        .candidate_block(1_000_000 + H1, vec![batch_by(&miner(), 1, mixed, 0)])
        .unwrap_err();
    assert!(
        matches!(err, NodeError::Mining(_)),
        "one miss fails the batch: {err:?}"
    );

    mine(
        &mut c,
        H1,
        vec![batch_by(&miner(), 1, good[..2].to_vec(), 0)],
    )
    .unwrap();
    let credit = c.state().committed_credit(&miner().address()).unwrap();
    assert_eq!(credit.map(|k| k.shares), Some(2));
    assert_eq!(c.state().committed_seen_shares(1).unwrap(), 2);
}

/// One credit per miner: a miner that keeps mining is paid for the closed
/// epoch by its next epoch's first share, with no claim and no extra record.
#[test]
fn a_miner_that_keeps_mining_is_paid_by_its_next_epochs_first_share() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir, Some(mining()));
    to_epoch_one(&mut c);
    let nonce = find_shares(&c, 1, 0, 1)[0];
    mine(&mut c, H1, vec![share(1, nonce, 0)]).unwrap();
    close_epoch(&mut c, H1 + 1, 1);

    let next = find_shares(&c, 2, 0, 1)[0];
    mine(&mut c, 2 * EPOCH_BLOCKS + 1, vec![share(2, next, 1)]).unwrap();

    assert_eq!(balance(&c, &miner().address()), budget());
    let credit = c.state().committed_credit(&miner().address()).unwrap();
    assert_eq!(credit.map(|k| (k.epoch, k.shares)), Some((2, 1)));
    assert!(c.state().committed_pot(1).unwrap().is_none());
    // Its epoch-1 credit is gone, so a claim for it now finds nothing.
    let err = c
        .candidate_block(1_000_000 + 2 * EPOCH_BLOCKS + 2, vec![claim(1, 2)])
        .unwrap_err();
    assert!(matches!(err, NodeError::Mining(_)), "{err:?}");
}
