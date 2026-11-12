//! ADR-096: the exit window. A passed scheduled upgrade must leave every holder
//! time to exit before it activates, so `MIN_UPGRADE_LEAD_BLOCKS` has to cover
//! the vault delay plus the unbonding window. The governance crate cannot
//! reference the vault or staking (the dependency runs the other way), so the
//! inequality is pinned here, against the real constants.

use custom_l1_node::state::vault::MAX_DELAY_BLOCKS;
use maya_governance::limits::MIN_UPGRADE_LEAD_BLOCKS;
use maya_staking::Params;

/// Staking's unbonding delay in epochs. Genesis cannot override it.
const UNBONDING_EPOCHS: u64 = Params::DEVNET.unbonding_epochs;
/// `epoch_blocks` in `docs/site/public/testnet/genesis.json`.
const TESTNET_EPOCH_BLOCKS: u64 = 3_600;
/// `epoch_blocks` in the devnet genesis scripts.
const DEVNET_EPOCH_BLOCKS: u64 = 20;

/// Blocks a holder may need after an upgrade executes: the longest vault delay
/// plus the unbonding window. The governance unlock adds nothing after
/// execution, because a voter's stake is required only until `executable_from`.
fn exit_blocks(epoch_blocks: u64) -> u64 {
    MAX_DELAY_BLOCKS + UNBONDING_EPOCHS * epoch_blocks
}

#[test]
fn the_lead_covers_the_exit_on_the_testnet_genesis() {
    assert!(
        MIN_UPGRADE_LEAD_BLOCKS >= exit_blocks(TESTNET_EPOCH_BLOCKS),
        "lead {MIN_UPGRADE_LEAD_BLOCKS} < exit {}",
        exit_blocks(TESTNET_EPOCH_BLOCKS)
    );
}

#[test]
fn the_lead_covers_the_exit_on_the_devnet_genesis() {
    assert!(MIN_UPGRADE_LEAD_BLOCKS >= exit_blocks(DEVNET_EPOCH_BLOCKS));
}

#[test]
fn the_longest_epoch_the_lead_admits_is_227_428_blocks() {
    // A genesis epoch longer than this makes the exit window shorter than the
    // vault delay plus unbonding. Genesis does not bound `epoch_blocks` yet
    // (ADR-096, finding); this pins the number a mainnet genesis must respect.
    let longest = (MIN_UPGRADE_LEAD_BLOCKS - MAX_DELAY_BLOCKS) / UNBONDING_EPOCHS;
    assert_eq!(longest, 227_428);
    assert!(exit_blocks(longest) <= MIN_UPGRADE_LEAD_BLOCKS);
    assert!(exit_blocks(longest + 1) > MIN_UPGRADE_LEAD_BLOCKS);
}
