//! Emergency-pause rehearsal (Master Prompts 9 and 16): the security council
//! pauses a module, the pause bites, transfers keep working, the pause
//! expires on its own or is lifted early, and nothing short of a quorum works.
//!
//! Timed in blocks: how long from the council's decision to the module being
//! closed, and how long the chain stays usable meanwhile.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::council_payload::{CouncilAction, CouncilKind};
use custom_l1_node::core::governance_payload::StakeLock;
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys::{self, SigningKey};
use custom_l1_node::genesis::{Allocation, CouncilGenesis, GenesisConfig};
use custom_l1_node::state::db::StateDB;
use custom_l1_node::state::invariant_guard::Module;
use tempfile::TempDir;

const MAX_PAUSE: u64 = 20;

fn user() -> HybridSigningKey {
    signing_key_from_seed(&[0x61; 32]).unwrap()
}

fn member(i: u8) -> SigningKey {
    keys::signing_key_from_seed(&[0x70 + i; 32]).unwrap()
}

fn chain(dir: &TempDir) -> Chain {
    let config = GenesisConfig {
        chain_id: "council-rehearsal".into(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: hex::encode(user().address()),
            balance: 10_000_000,
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        security_council: Some(CouncilGenesis {
            members: (0..3)
                .map(|i| hex::encode(member(i).verifying_key().to_bytes()))
                .collect(),
            threshold: 2,
            max_pause_blocks: MAX_PAUSE,
        }),
        bft: None,
    };
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    config.seed_state(&state).unwrap();
    Chain::open(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

fn signed(kind: TxKind, outputs: Vec<TxOutput>, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.kind = kind;
    tx.sign(&user(, &common::test_chain())).unwrap();
    tx
}

fn council(kind: CouncilKind, nonce: u64, signers: &[u8], sender_nonce: u64) -> Transaction {
    let mut action = CouncilAction {
        kind,
        nonce,
        approvals: vec![],
    };
    let message = action.signing_message();
    action.approvals = signers
        .iter()
        .map(|i| (*i, Box::new(member(*i).sign(&message).unwrap())))
        .collect();
    signed(TxKind::Council(Box::new(action)), vec![], sender_nonce)
}

fn lock(nonce: u64) -> Transaction {
    signed(
        TxKind::LockStake(StakeLock {
            amount: 10,
            unlock_height: 1_000,
        }),
        vec![],
        nonce,
    )
}

fn transfer(nonce: u64) -> Transaction {
    signed(
        TxKind::Transfer,
        vec![TxOutput {
            amount: 1,
            recipient: [9; 32],
        }],
        nonce,
    )
}

fn mine(c: &mut Chain, txs: Vec<Transaction>) -> Result<(), String> {
    let ts = 1_000_000 + c.height() + 1;
    let block = c.candidate_block(ts, txs).map_err(|e| e.to_string())?;
    c.insert_block(block).map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn a_council_quorum_pauses_one_module_transfers_continue_and_the_pause_ends() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);
    let mut nonce = 0;
    mine(&mut c, vec![lock(nonce)]).unwrap();
    nonce += 1;

    // One member alone is not a quorum.
    let lone = council(
        CouncilKind::Pause {
            module: Module::Governance.tag(),
            blocks: 10,
        },
        0,
        &[1],
        nonce,
    );
    assert!(
        mine(&mut c, vec![lone]).is_err(),
        "one approval paused a module"
    );

    // Two of three: the pause lands in the block that carries it.
    let decided = c.height() + 1;
    mine(
        &mut c,
        vec![council(
            CouncilKind::Pause {
                module: Module::Governance.tag(),
                blocks: 10,
            },
            0,
            &[0, 2],
            nonce,
        )],
    )
    .unwrap();
    nonce += 1;
    let record = c
        .state()
        .stored_breaker(Module::Governance)
        .unwrap()
        .unwrap();
    assert_eq!((record.tripped_at, record.until), (decided, decided + 10));

    // The module is closed; transfers are not.
    let err = mine(&mut c, vec![lock(nonce)]).unwrap_err();
    assert!(err.contains("halted") || err.contains("Halted"), "{err}");
    mine(&mut c, vec![transfer(nonce)]).unwrap();
    nonce += 1;

    // A replayed approval (old nonce) does nothing.
    let replay = council(
        CouncilKind::Resume {
            module: Module::Governance.tag(),
        },
        0,
        &[0, 2],
        nonce,
    );
    assert!(
        mine(&mut c, vec![replay]).is_err(),
        "a replayed approval was accepted"
    );

    // Longer than the council may impose: refused.
    let too_long = council(
        CouncilKind::Pause {
            module: Module::Dex.tag(),
            blocks: MAX_PAUSE + 1,
        },
        1,
        &[0, 1],
        nonce,
    );
    assert!(mine(&mut c, vec![too_long]).is_err());

    // Resume early with a fresh quorum; the module opens in the next block.
    let resumed_at = c.height() + 1;
    mine(
        &mut c,
        vec![council(
            CouncilKind::Resume {
                module: Module::Governance.tag(),
            },
            1,
            &[1, 2],
            nonce,
        )],
    )
    .unwrap();
    nonce += 1;
    mine(&mut c, vec![lock(nonce)]).unwrap();
    println!(
        "pause rehearsal: quorum decided at height {decided} and the module closed in that block (0 blocks' \
         latency); transfers landed throughout; resumed at {resumed_at} after {} of the 10 paused blocks; \
         a lone member, a replayed approval and an over-long pause were each refused",
        resumed_at - decided
    );
}

#[test]
mod common;

fn a_pause_expires_without_anyone_acting() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);
    mine(
        &mut c,
        vec![council(
            CouncilKind::Pause {
                module: Module::Governance.tag(),
                blocks: 3,
            },
            0,
            &[0, 1],
            0,
        )],
    )
    .unwrap();
    assert!(mine(&mut c, vec![lock(1)]).is_err());
    mine(&mut c, vec![]).unwrap();
    mine(&mut c, vec![]).unwrap();
    mine(&mut c, vec![lock(1)]).expect("the pause expired after its three blocks");
}
