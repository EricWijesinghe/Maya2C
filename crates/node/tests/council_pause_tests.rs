//! Emergency-pause rehearsal (Master Prompts 9 and 16): the security council
//! pauses a module, the pause bites, transfers keep working, the pause
//! expires on its own or is lifted early, and nothing short of a quorum works.
//!
//! Timed in blocks: how long from the council's decision to the module being
//! closed, and how long the chain stays usable meanwhile.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::council_payload::{CouncilAction, CouncilKind, Rotation};
use custom_l1_node::core::dex_payload::AssetRegistration;
use custom_l1_node::core::governance_payload::StakeLock;
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys::{self, SigningKey};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::genesis::{Allocation, CouncilGenesis, GenesisConfig};
use custom_l1_node::state::BlockContext;
use custom_l1_node::state::db::StateDB;
use custom_l1_node::state::invariant_guard::Module;
use maya_governance::limits::{
    COUNCIL_RENEWAL_COOLDOWN_BLOCKS, COUNCIL_ROTATION_LEAD_BLOCKS, MAX_SCHEDULE_HORIZON_BLOCKS,
};
use tempfile::TempDir;

mod common;

const MAX_PAUSE: u64 = 20;

fn user() -> HybridSigningKey {
    signing_key_from_seed(&[0x61; 32]).unwrap()
}

fn member(i: u8) -> SigningKey {
    keys::signing_key_from_seed(&[0x70 + i; 32]).unwrap()
}

fn config() -> GenesisConfig {
    GenesisConfig {
        chain_id: "council-rehearsal".into(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: hex::encode(user().address()),
            balance: primitive_types::U256::from(10_000_000),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        suite_deprecations: Vec::new(),
        security_council: Some(CouncilGenesis {
            members: (0..3)
                .map(|i| hex::encode(member(i).verifying_key().to_bytes()))
                .collect(),
            threshold: 2,
            max_pause_blocks: MAX_PAUSE,
        }),
        bft: None,
        shielded_activation_height: None,
    }
}

fn chain(dir: &TempDir) -> Chain {
    let config = config();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    config.seed_state(&state).unwrap();
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

fn signed(kind: TxKind, outputs: Vec<TxOutput>, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.kind = kind;
    tx.sign(&user(), &common::test_chain()).unwrap();
    tx
}

fn council(kind: CouncilKind, nonce: u64, signers: &[u8], sender_nonce: u64) -> Transaction {
    let pairs: Vec<(u8, u8)> = signers.iter().map(|i| (*i, *i)).collect();
    council_with(kind, nonce, &pairs, sender_nonce)
}

/// A council action approved by `(member index, key seed)` pairs: after a
/// rotation, index 0 is a different key from `member(0)`.
fn council_with(
    kind: CouncilKind,
    nonce: u64,
    approvals: &[(u8, u8)],
    sender_nonce: u64,
) -> Transaction {
    let mut action = CouncilAction {
        kind,
        nonce,
        approvals: vec![],
    };
    let message = action.signing_message(&common::test_chain());
    action.approvals = approvals
        .iter()
        .map(|(index, seed)| (*index, Box::new(member(*seed).sign(&message).unwrap())))
        .collect();
    signed(TxKind::Council(Box::new(action)), vec![], sender_nonce)
}

fn lock(nonce: u64) -> Transaction {
    signed(
        TxKind::LockStake(StakeLock {
            amount: primitive_types::U256::from(10),
            unlock_height: 1_000,
        }),
        vec![],
        nonce,
    )
}

/// A DEX transaction: what a pause of [`Module::Dex`] must refuse.
fn register(nonce: u64) -> Transaction {
    let mut symbol = [0u8; 8];
    symbol[..4].copy_from_slice(b"TEST");
    signed(
        TxKind::RegisterAsset(AssetRegistration {
            symbol,
            total_supply: primitive_types::U256::from(1_000),
        }),
        vec![],
        nonce,
    )
}

fn transfer(nonce: u64) -> Transaction {
    signed(
        TxKind::Transfer,
        vec![TxOutput {
            amount: primitive_types::U256::from(1),
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
    mine(&mut c, vec![register(nonce)]).unwrap();
    nonce += 1;

    // One member alone is not a quorum.
    let lone = council(
        CouncilKind::Pause {
            module: Module::Dex.tag(),
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
                module: Module::Dex.tag(),
                blocks: 10,
            },
            0,
            &[0, 2],
            nonce,
        )],
    )
    .unwrap();
    nonce += 1;
    let record = c.state().stored_breaker(Module::Dex).unwrap().unwrap();
    assert_eq!((record.tripped_at, record.until), (decided, decided + 10));

    // The module is closed; transfers are not.
    let err = mine(&mut c, vec![register(nonce)]).unwrap_err();
    assert!(err.contains("halted") || err.contains("Halted"), "{err}");
    mine(&mut c, vec![transfer(nonce)]).unwrap();
    nonce += 1;

    // A replayed approval (old nonce) does nothing.
    let replay = council(
        CouncilKind::Resume {
            module: Module::Dex.tag(),
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
            module: Module::Vm.tag(),
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
                module: Module::Dex.tag(),
            },
            1,
            &[1, 2],
            nonce,
        )],
    )
    .unwrap();
    nonce += 1;
    mine(&mut c, vec![register(nonce)]).unwrap();
    println!(
        "pause rehearsal: quorum decided at height {decided} and the module closed in that block (0 blocks' \
         latency); transfers landed throughout; resumed at {resumed_at} after {} of the 10 paused blocks; \
         a lone member, a replayed approval and an over-long pause were each refused",
        resumed_at - decided
    );
}

#[test]
fn a_pause_expires_without_anyone_acting() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);
    mine(
        &mut c,
        vec![council(
            CouncilKind::Pause {
                module: Module::Dex.tag(),
                blocks: 3,
            },
            0,
            &[0, 1],
            0,
        )],
    )
    .unwrap();
    assert!(mine(&mut c, vec![register(1)]).is_err());
    mine(&mut c, vec![]).unwrap();
    mine(&mut c, vec![]).unwrap();
    mine(&mut c, vec![register(1)]).expect("the pause expired after its three blocks");
}

/// ADR-050 review finding (2026-10-19): a quorum's approvals for another
/// chain, same council keys, same nonce, are refused here; the same action
/// signed for this chain lands.
#[test]
fn approvals_signed_for_another_chain_are_refused() {
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);
    let kind = CouncilKind::Pause {
        module: Module::Dex.tag(),
        blocks: 3,
    };
    let mut action = CouncilAction {
        kind: kind.clone(),
        nonce: 0,
        approvals: vec![],
    };
    let elsewhere = custom_l1_node::core::ChainTag::from_genesis([0xEE; 32]);
    let message = action.signing_message(&elsewhere);
    action.approvals = [0u8, 1]
        .iter()
        .map(|i| (*i, Box::new(member(*i).sign(&message).unwrap())))
        .collect();
    let replayed = signed(TxKind::Council(Box::new(action)), vec![], 0);
    assert!(mine(&mut c, vec![replayed]).is_err());
    mine(&mut c, vec![council(kind, 0, &[0, 1], 0)]).unwrap();
}

#[test]
fn a_council_sunset_is_scheduled_only_at_the_council_lead_and_never_as_a_version() {
    use custom_l1_node::state::upgrades::COUNCIL_SUNSET_LEAD_BLOCKS;
    use custom_l1_node::state::upgrades::SunsetSpec;
    use maya_crypto_pq::agility::MIN_MIGRATION_WINDOW;
    use maya_crypto_pq::suite::SuiteId;

    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);
    let next = c.height() + 1;
    let sunset = |announced_at: u64| CouncilKind::SunsetSuites {
        announced_at,
        sunsets: vec![SunsetSpec {
            suite: SuiteId::MlDsa65.to_byte(),
            sunset_height: announced_at + MIN_MIGRATION_WINDOW,
            emergency: false,
        }],
    };

    // One block under the council's compiled lead: refused, and nothing changes.
    let short = council(sunset(next + COUNCIL_SUNSET_LEAD_BLOCKS - 1), 0, &[0, 1], 0);
    assert!(mine(&mut c, vec![short]).is_err());
    assert!(
        c.state()
            .effective_schedule()
            .unwrap()
            .sunsets
            .entries()
            .next()
            .is_none()
    );

    // At the lead: accepted, and it is a sunset and nothing else.
    mine(
        &mut c,
        vec![council(
            sunset(next + COUNCIL_SUNSET_LEAD_BLOCKS),
            0,
            &[0, 1],
            0,
        )],
    )
    .unwrap();
    let schedule = c.state().effective_schedule().unwrap();
    assert!(
        schedule.upgrades.entries().is_empty(),
        "no version is scheduled"
    );
    assert!(
        schedule
            .sunsets
            .entries()
            .any(|s| s.suite == SuiteId::MlDsa65)
    );
}

#[test]
fn a_council_sunset_has_no_emergency_window_so_no_path_exits_a_frozen_vault_early() {
    // H3: the emergency window is a governance-only path. A council sunset with
    // one is refused even at the full lead.
    use custom_l1_node::state::upgrades::SunsetSpec;
    use maya_crypto_pq::agility::MIN_EMERGENCY_WINDOW;
    use maya_crypto_pq::suite::SuiteId;

    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);
    let announced = c.height() + 1 + custom_l1_node::state::upgrades::COUNCIL_SUNSET_LEAD_BLOCKS;
    let emergency = CouncilKind::SunsetSuites {
        announced_at: announced,
        sunsets: vec![SunsetSpec {
            suite: SuiteId::MlDsa65.to_byte(),
            sunset_height: announced + MIN_EMERGENCY_WINDOW,
            emergency: true,
        }],
    };
    assert!(mine(&mut c, vec![council(emergency, 0, &[0, 1], 0)]).is_err());
    assert!(
        c.state()
            .effective_schedule()
            .unwrap()
            .sunsets
            .entries()
            .next()
            .is_none()
    );
}

// --- Council hardening (founder audit Q7 and Q8, Eric 2026-11-14) ---------
//
// The tests below drive a bare state database so a block can sit at any
// height: the cooldown and the rotation lead are a day of blocks, and the
// boundaries are what they pin.

/// A state database seeded with the council genesis and bound to the test tag.
fn bare(dir: &TempDir) -> StateDB {
    let state = common::bound_state(dir.path());
    config().seed_state(&state).unwrap();
    state
}

/// Applies `txs` as the block at `height`.
fn at(db: &StateDB, height: u64, txs: Vec<Transaction>) -> Result<(), String> {
    let header = BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_000_000 + height,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    db.apply_block(&Block::new(header, txs), BlockContext::at_height(height))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn pause(module: Module, blocks: u64) -> CouncilKind {
    CouncilKind::Pause {
        module: module.tag(),
        blocks,
    }
}

/// A rotation to `members` (key seeds), each of whom signs its acceptance.
fn rotation(activation: u64, threshold: u8, members: &[u8]) -> CouncilKind {
    let rotation = Rotation {
        activation,
        threshold,
        members: members
            .iter()
            .map(|i| member(*i).verifying_key().to_bytes())
            .collect(),
    };
    let accepted = rotation.acceptance_message(&common::test_chain());
    CouncilKind::Rotate {
        acceptances: members
            .iter()
            .map(|i| Box::new(member(*i).sign(&accepted).unwrap()))
            .collect(),
        rotation,
    }
}

#[test]
fn the_council_can_neither_pause_nor_resume_governance() {
    // Q7 A: a council that could pause Governance could freeze the vote that
    // would answer it.
    let dir = TempDir::new().unwrap();
    let mut c = chain(&dir);
    let paused = mine(
        &mut c,
        vec![council(pause(Module::Governance, 10), 0, &[0, 1, 2], 0)],
    )
    .unwrap_err();
    assert!(paused.contains("governance"), "{paused}");
    let resumed = mine(
        &mut c,
        vec![council(
            CouncilKind::Resume {
                module: Module::Governance.tag(),
            },
            0,
            &[0, 1, 2],
            0,
        )],
    )
    .unwrap_err();
    assert!(resumed.contains("governance"), "{resumed}");
    assert_eq!(c.state().stored_breaker(Module::Governance).unwrap(), None);
    // Staking and votes go on.
    mine(&mut c, vec![lock(0)]).unwrap();
}

#[test]
fn a_council_pause_cannot_be_renewed_within_the_cooldown() {
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    at(&db, 1, vec![council(pause(Module::Dex, 3), 0, &[0, 1], 0)]).unwrap();

    // While it holds: no extension.
    let renewed = at(&db, 2, vec![council(pause(Module::Dex, 3), 1, &[0, 1], 1)]).unwrap_err();
    assert!(renewed.contains("cooldown"), "{renewed}");

    // Lifted early: the cooldown runs from the lift.
    at(
        &db,
        2,
        vec![council(
            CouncilKind::Resume {
                module: Module::Dex.tag(),
            },
            1,
            &[0, 1],
            1,
        )],
    )
    .unwrap();
    let open_again = 2 + COUNCIL_RENEWAL_COOLDOWN_BLOCKS;
    let early = at(
        &db,
        open_again - 1,
        vec![council(pause(Module::Dex, 3), 2, &[0, 1], 2)],
    )
    .unwrap_err();
    assert!(early.contains("cooldown"), "{early}");

    // Another module is not cooled down by this one.
    at(
        &db,
        open_again - 1,
        vec![council(pause(Module::Vm, 3), 2, &[0, 1], 2)],
    )
    .unwrap();

    // At the cooldown's end: accepted.
    at(
        &db,
        open_again,
        vec![council(pause(Module::Dex, 3), 3, &[0, 1], 3)],
    )
    .unwrap();
    let record = db.stored_breaker(Module::Dex).unwrap().unwrap();
    assert_eq!(
        (record.tripped_at, record.until),
        (open_again, open_again + 3)
    );
}

#[test]
fn a_council_sunset_cannot_follow_another_within_the_cooldown() {
    use custom_l1_node::state::upgrades::{COUNCIL_SUNSET_LEAD_BLOCKS, SunsetSpec};
    use maya_crypto_pq::agility::MIN_MIGRATION_WINDOW;
    use maya_crypto_pq::suite::SuiteId;

    let sunset = |now: u64, suite: SuiteId| CouncilKind::SunsetSuites {
        announced_at: now + COUNCIL_SUNSET_LEAD_BLOCKS,
        sunsets: vec![SunsetSpec {
            suite: suite.to_byte(),
            sunset_height: now + COUNCIL_SUNSET_LEAD_BLOCKS + MIN_MIGRATION_WINDOW,
            emergency: false,
        }],
    };
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    at(
        &db,
        1,
        vec![council(sunset(1, SuiteId::MlDsa65), 0, &[0, 1], 0)],
    )
    .unwrap();

    let next = 1 + COUNCIL_RENEWAL_COOLDOWN_BLOCKS;
    let early = at(
        &db,
        next - 1,
        vec![council(
            sunset(next - 1, SuiteId::SlhDsaSha2_128s),
            1,
            &[0, 1],
            1,
        )],
    )
    .unwrap_err();
    assert!(early.contains("cooldown"), "{early}");
    at(
        &db,
        next,
        vec![council(
            sunset(next, SuiteId::SlhDsaSha2_128s),
            1,
            &[0, 1],
            1,
        )],
    )
    .unwrap();
}

#[test]
fn a_rotation_with_too_few_approvals_is_refused() {
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    let kind = rotation(1 + COUNCIL_ROTATION_LEAD_BLOCKS, 2, &[3, 4, 5]);
    let lone = at(&db, 1, vec![council(kind.clone(), 0, &[0], 0)]).unwrap_err();
    assert!(lone.contains("approvals"), "{lone}");
    // A member's approval counted twice is still one approval.
    let twice = at(&db, 1, vec![council(kind, 0, &[0, 0], 0)]).unwrap_err();
    assert!(twice.contains("approvals"), "{twice}");
    let record = db.committed_council(1).unwrap().unwrap();
    assert_eq!((record.nonce, record.pending), (0, None));
}

#[test]
fn a_rotation_must_clear_the_lead_and_name_a_sound_set() {
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    let lead = 1 + COUNCIL_ROTATION_LEAD_BLOCKS;
    for (kind, why) in [
        (rotation(lead - 1, 2, &[3, 4, 5]), "lead"),
        // 2 of 4 is not a strict majority: half could act over the other half.
        (rotation(lead, 2, &[3, 4, 5, 6]), "strict majority"),
        (rotation(lead, 2, &[3, 3, 4]), "repeats"),
        (rotation(lead, 0, &[]), "strict majority"),
    ] {
        let err = at(&db, 1, vec![council(kind, 0, &[0, 1], 0)]).unwrap_err();
        assert!(err.contains(why), "wanted {why}: {err}");
    }
    assert_eq!(db.committed_council(1).unwrap().unwrap().pending, None);
}

#[test]
fn a_rotation_takes_effect_at_its_height_and_not_before() {
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    let activation = 1 + COUNCIL_ROTATION_LEAD_BLOCKS;
    at(
        &db,
        1,
        vec![council(rotation(activation, 2, &[3, 4, 5]), 0, &[0, 2], 0)],
    )
    .unwrap();
    // Scheduled, and public in the record from the block that carried it.
    let record = db.committed_council(1).unwrap().unwrap();
    assert_eq!(record.nonce, 1);
    assert_eq!(record.pending.as_ref().unwrap().activation, activation);
    assert_eq!(record.members[0], member(0).verifying_key().to_bytes());

    let resume = || CouncilKind::Resume {
        module: Module::Vm.tag(),
    };
    // The new set signs as indices 0..3 of the rotated council.
    let new_set = [(0u8, 3u8), (1, 4), (2, 5)];
    // One block before: the new members are not yet the council; the old are.
    let early = at(
        &db,
        activation - 1,
        vec![council_with(resume(), 1, &new_set[..2], 1)],
    )
    .unwrap_err();
    assert!(early.contains("approvals"), "{early}");
    at(&db, activation - 1, vec![council(resume(), 1, &[0, 1], 1)]).unwrap();

    // At the height: the old members are no longer the council; the new are.
    let old = at(&db, activation, vec![council(resume(), 2, &[0, 1], 2)]).unwrap_err();
    assert!(old.contains("approvals"), "{old}");
    at(
        &db,
        activation,
        vec![council_with(resume(), 2, &new_set[1..], 2)],
    )
    .unwrap();
    let record = db.committed_council(activation).unwrap().unwrap();
    assert_eq!(record.pending, None);
    assert_eq!(record.nonce, 3);
    assert_eq!(
        record.members,
        [3u8, 4, 5]
            .iter()
            .map(|i| member(*i).verifying_key().to_bytes())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_council_pause_that_expires_unresumed_cools_down_from_its_end() {
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    at(&db, 1, vec![council(pause(Module::Dex, 3), 0, &[0, 1], 0)]).unwrap();
    // Nobody resumes; the pause ends at 4 on its own.
    let open_at = 1 + 3 + COUNCIL_RENEWAL_COOLDOWN_BLOCKS;
    let early = at(
        &db,
        open_at - 1,
        vec![council(pause(Module::Dex, 3), 1, &[0, 1], 1)],
    )
    .unwrap_err();
    assert!(early.contains("cooldown"), "{early}");
    at(
        &db,
        open_at,
        vec![council(pause(Module::Dex, 3), 1, &[0, 1], 1)],
    )
    .unwrap();
}

#[test]
fn a_rotation_past_the_schedule_horizon_is_refused() {
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    let beyond = 1 + MAX_SCHEDULE_HORIZON_BLOCKS + 1;
    let err = at(
        &db,
        1,
        vec![council(rotation(beyond, 2, &[3, 4, 5]), 0, &[0, 1], 0)],
    )
    .unwrap_err();
    assert!(err.contains("horizon"), "{err}");
    // The horizon itself is inside.
    at(
        &db,
        1,
        vec![council(rotation(beyond - 1, 2, &[3, 4, 5]), 0, &[0, 1], 0)],
    )
    .unwrap();
}

#[test]
fn a_rotation_naming_a_key_nobody_signed_for_is_refused() {
    // A typo or a hostile builder must not hand the council to a key with no
    // holder: every new member signs its acceptance.
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    let activation = 1 + COUNCIL_ROTATION_LEAD_BLOCKS;
    let CouncilKind::Rotate {
        rotation: set,
        mut acceptances,
    } = rotation(activation, 2, &[3, 4, 5])
    else {
        unreachable!()
    };
    // Member 5's seat accepted by member 6's key: not member 5.
    let wrong = member(6)
        .sign(&set.acceptance_message(&common::test_chain()))
        .unwrap();
    acceptances[2] = Box::new(wrong);
    let kind = CouncilKind::Rotate {
        rotation: set.clone(),
        acceptances: acceptances.clone(),
    };
    let err = at(&db, 1, vec![council(kind, 0, &[0, 1], 0)]).unwrap_err();
    assert!(err.contains("acceptance"), "{err}");
    // An acceptance for another chain is not one for this chain.
    let elsewhere = custom_l1_node::core::ChainTag::from_genesis([0xEE; 32]);
    acceptances[2] = Box::new(member(5).sign(&set.acceptance_message(&elsewhere)).unwrap());
    let kind = CouncilKind::Rotate {
        rotation: set.clone(),
        acceptances: acceptances.clone(),
    };
    let err = at(&db, 1, vec![council(kind, 0, &[0, 1], 0)]).unwrap_err();
    assert!(err.contains("acceptance"), "{err}");
    // One acceptance short.
    acceptances.pop();
    let kind = CouncilKind::Rotate {
        rotation: set,
        acceptances,
    };
    let err = at(&db, 1, vec![council(kind, 0, &[0, 1], 0)]).unwrap_err();
    assert!(err.contains("acceptance"), "{err}");
    assert_eq!(db.committed_council(1).unwrap().unwrap().pending, None);
}

#[test]
fn a_later_rotation_replaces_a_pending_one_and_clears_the_lead_again() {
    // How a mistaken rotation is withdrawn (ADR-082 amendment).
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    let first = 1 + COUNCIL_ROTATION_LEAD_BLOCKS;
    at(
        &db,
        1,
        vec![council(rotation(first, 2, &[3, 4, 5]), 0, &[0, 1], 0)],
    )
    .unwrap();
    // The replacement must clear the lead from its own block, 10, so the
    // first one's activation height is too early for it.
    let early = at(
        &db,
        10,
        vec![council(rotation(first, 1, &[6]), 1, &[0, 1], 1)],
    )
    .unwrap_err();
    assert!(early.contains("lead"), "{early}");
    let second = 10 + COUNCIL_ROTATION_LEAD_BLOCKS;
    at(
        &db,
        10,
        vec![council(rotation(second, 1, &[6]), 1, &[0, 1], 1)],
    )
    .unwrap();
    let record = db.committed_council(10).unwrap().unwrap();
    assert_eq!(record.nonce, 2);
    let pending = record.pending.unwrap();
    assert_eq!((pending.activation, pending.threshold), (second, 1));
    assert_eq!(pending.members, vec![member(6).verifying_key().to_bytes()]);
    // At the first one's height nothing changes: it was replaced.
    let still_old = db.committed_council(first).unwrap().unwrap();
    assert_eq!(still_old.members[0], member(0).verifying_key().to_bytes());
    assert_eq!(still_old.threshold, 2);
}

#[test]
fn the_committed_council_read_after_activation_is_the_new_one() {
    // No council action between the activation and the read: the stored
    // record still holds the rotation as pending, and the read applies it.
    let dir = TempDir::new().unwrap();
    let db = bare(&dir);
    let activation = 1 + COUNCIL_ROTATION_LEAD_BLOCKS;
    at(
        &db,
        1,
        vec![council(rotation(activation, 2, &[3, 4, 5]), 0, &[0, 1], 0)],
    )
    .unwrap();
    let before = db.committed_council(activation - 1).unwrap().unwrap();
    assert_eq!(before.members[0], member(0).verifying_key().to_bytes());
    assert!(before.pending.is_some());
    let after = db.committed_council(activation).unwrap().unwrap();
    assert_eq!(after.members[0], member(3).verifying_key().to_bytes());
    assert_eq!((after.pending, after.nonce), (None, 1));
}

#[test]
fn a_genesis_council_of_more_than_32_members_is_refused() {
    // The genesis rule a rotation shares: before 2026-11-14 genesis had no
    // upper bound, and a strict majority of 33 could never fit in
    // MAX_APPROVALS (32) approvals.
    let council = |n: u8| CouncilGenesis {
        members: (0..n)
            .map(|i| {
                hex::encode(
                    keys::signing_key_from_seed(&[i; 32])
                        .unwrap()
                        .verifying_key()
                        .to_bytes(),
                )
            })
            .collect(),
        threshold: n / 2 + 1,
        max_pause_blocks: MAX_PAUSE,
    };
    council(32).record().unwrap();
    let err = council(33).record().unwrap_err().to_string();
    assert!(err.contains("32"), "{err}");
}
