//! A DAG-BFT chain must not stop extending because of proof-of-work arithmetic.
//!
//! maya-testnet-1 halted at height 12,530 on 2026-09-29. DAG-BFT verifies no
//! work, but the difficulty retarget still ran (ADR-027: "the retarget rule at
//! the unlimited floor"). With a block a second against a PoW spacing far
//! longer, every window made the target harder, so each block's work grew
//! about fourfold per window. The saturating `total_work` sum reached
//! 2^256 - 1. From then on a new block's total could not exceed the tip's, and
//! `insert_block` filed every block the validator built as a side branch
//! forever. ADR-035.

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::consensus::{InsertOutcome, U256};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::db::StateDB;
use tempfile::TempDir;

/// One second apart, as DAG-BFT produces them: the spacing that drove the
/// retarget toward ever harder targets.
const SPACING_S: u64 = 1;
const GENESIS_TIME: u64 = 1_790_000_000;

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: GENESIS_TIME,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

fn open(config: ChainConfig) -> (TempDir, Chain) {
    let dir = TempDir::new().expect("tempdir");
    let state = Arc::new(StateDB::open(dir.path()).expect("state"));
    let chain = Chain::open(state, genesis(), config).expect("chain");
    (dir, chain)
}

/// Builds and inserts the next empty block, as the DAG-BFT builder does.
fn extend(chain: &mut Chain) -> InsertOutcome {
    let height = chain.height() + 1;
    let block = chain
        .candidate_block_sealed(GENESIS_TIME + height * SPACING_S, Vec::new(), height)
        .expect("candidate");
    chain.insert_block(block).expect("insert")
}

fn tip_target(chain: &Chain) -> [u8; 32] {
    chain
        .get(&chain.tip())
        .expect("tip")
        .header
        .difficulty_target
}

#[test]
fn the_proof_of_work_retarget_hardens_a_one_second_chain() {
    // The mechanism behind the halt: under the retargeting rule, fast blocks
    // make the target harder every window. Kept as a test so the reason for
    // `dag_bft` stays demonstrated rather than remembered.
    let (_dir, mut chain) = open(ChainConfig::without_pow_verification());
    let genesis_target = tip_target(&chain);
    for _ in 0..300 {
        extend(&mut chain);
    }
    assert!(
        tip_target(&chain) < genesis_target,
        "three windows of 1 s blocks did not harden the target"
    );
}

#[test]
fn a_dag_bft_chain_keeps_its_genesis_target() {
    let (_dir, mut chain) = open(ChainConfig::dag_bft());
    let genesis_target = tip_target(&chain);
    for _ in 0..300 {
        assert!(matches!(extend(&mut chain), InsertOutcome::Extended { .. }));
    }
    assert_eq!(tip_target(&chain), genesis_target);
    // One unit of work per block at the unlimited target, genesis included:
    // total work is the height plus one, linear and nowhere near 2^256.
    assert_eq!(chain.total_work(), U256::from_u64(chain.height() + 1));
}

#[test]
fn a_dag_bft_chain_extends_past_the_height_where_testnet_1_halted() {
    // The halt came at 12,530. 13,000 one-second blocks cover that and a
    // margin; every one must extend the tip.
    const PAST_THE_HALT: u64 = 13_000;
    let (_dir, mut chain) = open(ChainConfig::dag_bft());
    for height in 1..=PAST_THE_HALT {
        let outcome = extend(&mut chain);
        assert!(
            matches!(outcome, InsertOutcome::Extended { .. }),
            "block {height} did not extend the tip: {outcome:?}"
        );
    }
    assert_eq!(chain.height(), PAST_THE_HALT);
}
