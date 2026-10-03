//! Shared test utilities.
//!
//! Each test binary uses only part of this module.
#![allow(dead_code)]

use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::{Block, ChainTag};
use custom_l1_node::error::NodeError;
use custom_l1_node::state::StateDB;

/// The tag a test signs with before it opens any chain.
const UNOPENED: ChainTag = ChainTag::from_genesis([42; 32]);

thread_local! {
    /// The genesis of the chain this test thread opened (ADR-036). Each test
    /// runs on its own thread, so one test never sees another's.
    static CURRENT: Cell<ChainTag> = const { Cell::new(UNOPENED) };
    /// Whether [`open_chain`] has run on this thread.
    static OPENED: Cell<bool> = const { Cell::new(false) };
}

/// The chain tag a test's transactions sign for: the genesis id of the chain
/// it opened through [`open_chain`], or a fixed tag for a test that drives a
/// bare [`StateDB`] bound with [`bind`].
#[must_use]
pub fn test_chain() -> ChainTag {
    CURRENT.with(Cell::get)
}

/// `Chain::open`, remembering the genesis so [`test_chain`] signs for it.
pub fn open_chain(
    state: Arc<StateDB>,
    genesis: Block,
    config: ChainConfig,
) -> Result<Chain, NodeError> {
    let tag = ChainTag::from_genesis(genesis.header.id());
    // One remembered tag per thread is only sound while a test opens one
    // genesis: a second one would make `test_chain` sign for whichever came
    // last, and a negative test could then pass for the wrong reason.
    if OPENED.with(Cell::get) {
        assert_eq!(
            CURRENT.with(Cell::get),
            tag,
            "a test opened two different geneses; sign with `tag_of(&chain)` instead"
        );
    }
    OPENED.with(|opened| opened.set(true));
    CURRENT.with(|current| current.set(tag));
    Chain::open(state, genesis, config)
}

/// Binds a bare state database to [`test_chain`], as `Chain::open` would.
pub fn bind(state: &StateDB) {
    state.bind_chain(test_chain()).expect("bind the test chain");
}

/// Opens a state database bound to [`test_chain`].
#[must_use]
pub fn bound_state(path: &Path) -> StateDB {
    let state = StateDB::open(path).expect("open state");
    bind(&state);
    state
}

/// Extracts the chain tag from a Chain instance (derived from its genesis block id).
/// Use this when you have a real Chain and need to sign or verify transactions for it.
#[must_use]
pub fn tag_of(chain: &Chain) -> ChainTag {
    ChainTag::from_genesis(chain.genesis())
}

/// Extracts the chain tag from a genesis Block (derived from its id).
/// Use this when you build a Block before opening a Chain.
#[must_use]
pub fn tag_from_block(block: &Block) -> ChainTag {
    ChainTag::from_genesis(block.header.id())
}
