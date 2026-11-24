//! Staking driving DAG-BFT (ADR-028), end to end through real chains.
//!
//! Five nodes; four are genesis validators bonded in genesis, the fifth holds
//! a validator key but no seat. It registers with a larger bond over a
//! staking transaction, and at the epoch boundary every node switches to the
//! committee the state chose — the newcomer first, by stake. Then evidence of
//! a genesis validator's double proposal tombstones it, and the committee after
//! the next boundary no longer contains it. No node is told any of this except
//! through blocks.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

use custom_l1_node::core::ChainTag;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use custom_l1_node::consensus::bft::attest::Attestation;
use custom_l1_node::consensus::bft::auth::MlDsaAuthenticator;
use custom_l1_node::consensus::bft::{BROADCAST, BftDriver, BftSetup, Envelope, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::staking_payload::StakingAction;
use custom_l1_node::core::transaction::Transaction;
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::validator_keys::{self as keys, SigningKey, VerifyingKey};
use custom_l1_node::genesis::{
    Allocation, BftGenesis, GenesisBond, GenesisConfig, MIN_VALUE_BEARING_VALIDATORS,
    StakingGenesis, StakingRules,
};
use custom_l1_node::state::db::StateDB;
use custom_l1_node::state::staking::validator_id;
use maya_dag_bft::{Authenticator, Message, Params, Vertex};
use tempfile::TempDir;

mod common;

const GENESIS_VALIDATORS: usize = 4;
const NODES: usize = 5;
const EPOCH_BLOCKS: u64 = 6;

fn operator() -> HybridSigningKey {
    signing_key_from_seed(&[0x42; 32]).unwrap()
}

fn validator_key(i: usize) -> Arc<SigningKey> {
    Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
}

fn genesis() -> GenesisConfig {
    genesis_with(false)
}

fn genesis_with(stake_weighted: bool) -> GenesisConfig {
    let validators = (0..GENESIS_VALIDATORS)
        .map(|i| hex::encode(validator_key(i).verifying_key().to_bytes()))
        .collect();
    let op = hex::encode(operator().address());
    GenesisConfig {
        chain_id: "bft-staking-test".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: op.clone(),
            balance: primitive_types::U256::from(1_000_000),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators,
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 0,
            fees: None,
            mining: None,
            staking: Some(StakingGenesis {
                epoch_blocks: EPOCH_BLOCKS,
                bonds: (0..GENESIS_VALIDATORS)
                    .map(|_| GenesisBond {
                        operator: op.clone(),
                        bond: primitive_types::U256::from(10_000),
                        commission_bps: 0,
                    })
                    .collect(),
                min_self_bond: primitive_types::U256::from(1_000),
                max_validators: 100,
                min_delegation: primitive_types::U256::from(10),
                stake_weighted,
                rules: None,
            }),
        }),
    }
}

struct Member {
    driver: BftDriver,
    chain: Chain,
}

struct Mesh {
    root: TempDir,
    members: Vec<Member>,
    queue: VecDeque<(usize, Vec<u8>)>,
    now: u64,
    equivocations: Vec<maya_dag_bft::Equivocation>,
}

impl Mesh {
    fn new() -> Self {
        Self::with(false)
    }

    fn with(stake_weighted: bool) -> Self {
        let root = TempDir::new().unwrap();
        let config = genesis_with(stake_weighted);
        let committee: Arc<[VerifyingKey]> = config
            .bft
            .as_ref()
            .unwrap()
            .verifying_keys()
            .unwrap()
            .into();
        let mut members = Vec::new();
        let mut opening = Vec::new();
        for i in 0..NODES {
            let state_dir: PathBuf = root.path().join(format!("state-{i}"));
            let state = Arc::new(StateDB::open(&state_dir).unwrap());
            config.seed_state(&state).unwrap();
            let mut chain = common::open_chain(
                state,
                config.genesis_block().unwrap(),
                ChainConfig::without_pow_verification(),
            )
            .unwrap();
            let setup = BftSetup {
                epoch: 0,
                committee: Arc::clone(&committee),
                signer: Some(validator_key(i).into()),
                // Paced, so one pump is about one round and an epoch boundary
                // cannot be overshot by eight epochs between two assertions.
                params: Params {
                    batch_size: 100,
                    min_round_interval_ms: 200,
                    ..Params::default()
                },
            };
            let (driver, step) =
                BftDriver::open(&setup, &root.path().join(format!("bft-{i}")), &mut chain, 0)
                    .unwrap();
            opening.push((i, step));
            members.push(Member { driver, chain });
        }
        let mut mesh = Self {
            root,
            members,
            queue: VecDeque::new(),
            now: 0,
            equivocations: Vec::new(),
        };
        for (i, step) in opening {
            mesh.absorb(i, step);
        }
        mesh
    }

    fn absorb(&mut self, from: usize, step: Step) {
        self.equivocations.extend(step.equivocations);
        self.queue
            .extend(step.frames.into_iter().map(|f| (from, f)));
    }

    fn pump(&mut self, budget: usize) {
        for _ in 0..budget {
            let Some((from, frame)) = self.queue.pop_front() else {
                return;
            };
            for i in 0..self.members.len() {
                if i == from {
                    continue;
                }
                let m = &mut self.members[i];
                let step = m.driver.on_frame(&mut m.chain, self.now, &frame).unwrap();
                self.absorb(i, step);
            }
        }
    }

    fn tick(&mut self) {
        for i in 0..self.members.len() {
            let m = &mut self.members[i];
            let step = m.driver.on_tick(&mut m.chain, self.now).unwrap();
            self.absorb(i, step);
        }
    }

    fn run_until(&mut self, done: impl Fn(&Self) -> bool) {
        for _ in 0..600 {
            if done(self) {
                return;
            }
            self.now += 250;
            self.tick();
            self.pump(3_000);
        }
    }

    fn submit(&mut self, tx: &Transaction) {
        for m in &mut self.members {
            m.driver.submit(tx);
        }
    }

    fn all_in_epoch(&self, epoch: u64) -> bool {
        self.members.iter().all(|m| m.driver.epoch() >= epoch)
    }

    fn agree(&self) {
        let common = self.members.iter().map(|m| m.chain.height()).min().unwrap();
        let ids: Vec<_> = self
            .members
            .iter()
            .map(|m| m.chain.active_chain().unwrap()[usize::try_from(common).unwrap()])
            .collect();
        assert!(
            ids.iter().all(|id| *id == ids[0]),
            "chains diverge at {common}"
        );
    }

    fn committee(&self, i: usize) -> Vec<[u8; 32]> {
        self.members[i]
            .chain
            .state()
            .committed_staking()
            .unwrap()
            .unwrap()
            .staking
            .active
    }
}

fn staking_tx(action: StakingAction, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(vec![], vec![], nonce);
    tx.kind = TxKind::Staking(Box::new(action));
    tx.sign(&operator(), &common::test_chain()).unwrap();
    tx
}

fn register(i: usize, bond: u64, nonce: u64) -> Transaction {
    let key = validator_key(i);
    let possession = key
        .sign(&StakingAction::possession_message(&operator().address()))
        .unwrap();
    staking_tx(
        StakingAction::Register {
            key: Box::new(key.verifying_key().to_bytes()),
            bond: primitive_types::U256::from(bond),
            commission_bps: 0,
            possession: Box::new(possession),
        },
        nonce,
    )
}

fn double_proposal(i: usize, committee: &Arc<[VerifyingKey]>, tag: ChainTag) -> StakingAction {
    let auth = MlDsaAuthenticator::validator(validator_key(i), Arc::clone(committee), tag);
    let frame = |stamp: u64| {
        let vertex = Vertex {
            epoch: 0,
            round: 7,
            author: u16::try_from(i).unwrap(),
            timestamp_ms: stamp,
            parents: vec![],
            batch: vec![],
        };
        let signature = auth.sign(
            maya_dag_bft::SignContext {
                kind: maya_dag_bft::SignKind::Proposal,
                round: vertex.round,
                author: vertex.author,
            },
            &vertex.digest(),
        );
        Envelope {
            epoch: 0,
            from: u16::try_from(i).unwrap(),
            to: BROADCAST,
            message: Message::Propose { vertex, signature },
        }
        .encode()
    };
    StakingAction::ReportEquivocation {
        first: frame(1),
        second: frame(2),
    }
}

/// Two attestations by committee member `i` of epoch 0 at one height, to the
/// blocks `[first; 32]` and `[second; 32]`, signed with `key`.
fn double_attestation(
    key: &SigningKey,
    i: usize,
    tag: ChainTag,
    first: u8,
    second: u8,
) -> StakingAction {
    let sign = |block: u8| {
        Attestation::sign(key, u16::try_from(i).unwrap(), &tag, 0, 5, [block; 32])
            .unwrap()
            .encode()
    };
    StakingAction::ReportEquivocation {
        first: sign(first),
        second: sign(second),
    }
}

/// CON-05 (Eric 2026-11-24): an attestation double-sign, two signatures by
/// one member on two blocks at one height, is slashed and tombstoned at the
/// proposal double-sign's rate. A pair that agrees on the block, or one
/// signed by another key, slashes nobody.
#[test]
fn an_attestation_double_sign_is_slashed_at_the_proposal_rate() {
    let mut mesh = Mesh::new();
    // A fifth seat, so the tombstone leaves the set at the next boundary
    // rather than being held by the committee floor (CON-07).
    mesh.submit(&register(4, 20_000, 0));
    mesh.run_until(|m| m.all_in_epoch(1));
    assert!(mesh.all_in_epoch(1), "no epoch boundary reached");
    let tag = ChainTag::from_genesis(mesh.members[0].chain.genesis());
    let offender = validator_id(&validator_key(3).verifying_key().to_bytes());
    let retired = |m: &Mesh| {
        m.members.iter().all(|x| {
            x.chain
                .state()
                .committed_staking()
                .unwrap()
                .is_some_and(|r| r.staking.retired.contains(&offender))
        })
    };

    // Not evidence: one block twice, and a pair whose signer is not member 3.
    mesh.submit(&staking_tx(
        double_attestation(&validator_key(3), 3, tag, 1, 1),
        1,
    ));
    mesh.submit(&staking_tx(
        double_attestation(&validator_key(2), 3, tag, 1, 2),
        1,
    ));
    let h = mesh.members[0].chain.height();
    mesh.run_until(|m| m.members.iter().all(|x| x.chain.height() >= h + 4));
    assert!(
        !retired(&mesh),
        "a pair that is not a double-sign slashed someone"
    );

    mesh.submit(&staking_tx(
        double_attestation(&validator_key(3), 3, tag, 1, 2),
        1,
    ));
    mesh.run_until(retired);
    assert!(retired(&mesh), "attestation evidence never landed");
    mesh.agree();
    let record = mesh.members[0]
        .chain
        .state()
        .committed_staking()
        .unwrap()
        .unwrap();
    assert_eq!(
        record.staking.validators[&offender].self_bond,
        maya_ledger_math::Amount::from_u64(5_000),
        "the proposal rate: half the bond burned"
    );
    mesh.run_until(|m| m.all_in_epoch(2));
    assert!(!mesh.committee(0).contains(&offender), "still seated");
}

#[test]
fn a_registration_joins_the_committee_and_evidence_removes_an_equivocator() {
    let mut mesh = Mesh::new();
    let genesis_committee = mesh.committee(0);
    assert_eq!(genesis_committee.len(), GENESIS_VALIDATORS);

    // A newcomer registers with twice the genesis bond.
    mesh.submit(&register(4, 20_000, 0));
    mesh.run_until(|m| m.all_in_epoch(1));
    assert!(mesh.all_in_epoch(1), "no epoch boundary reached");
    mesh.agree();
    let committee = mesh.committee(0);
    let newcomer = validator_id(&validator_key(4).verifying_key().to_bytes());
    assert_eq!(committee.len(), NODES);
    assert_eq!(
        committee[0], newcomer,
        "the largest stake leads the committee"
    );
    for i in 0..NODES {
        assert_eq!(
            mesh.committee(i),
            committee,
            "node {i} chose another committee"
        );
    }

    // Epoch 1 runs with the new committee: blocks keep coming.
    let h = mesh.members[0].chain.height();
    mesh.run_until(|m| m.members.iter().all(|x| x.chain.height() >= h + 3));
    mesh.agree();

    // Evidence that genesis validator 3 signed two epoch-0 vertices for one slot.
    let keys0: Arc<[VerifyingKey]> = genesis().bft.unwrap().verifying_keys().unwrap().into();
    let tag = ChainTag::from_genesis(mesh.members[0].chain.genesis());
    mesh.submit(&staking_tx(double_proposal(3, &keys0, tag), 1));
    mesh.run_until(|m| m.all_in_epoch(2));
    assert!(mesh.all_in_epoch(2));
    mesh.agree();
    // Entering epoch 2 removed epoch 0's logs and kept epoch 1's: on disk, a
    // validator holds this epoch and the previous one, never the whole past.
    for i in 0..NODES {
        let logs = mesh.root.path().join(format!("bft-{i}"));
        assert!(!logs.join("epoch-0").exists(), "node {i} kept epoch 0");
        assert!(logs.join("epoch-1").exists() && logs.join("epoch-2").exists());
    }
    let offender = validator_id(&validator_key(3).verifying_key().to_bytes());
    let record = mesh.members[0]
        .chain
        .state()
        .committed_staking()
        .unwrap()
        .unwrap();
    assert!(
        !record.staking.active.contains(&offender),
        "tombstoned validator still seated"
    );
    assert!(record.staking.retired.contains(&offender));
    assert_eq!(
        record.staking.validators[&offender].self_bond,
        maya_ledger_math::Amount::from_u64(5_000),
        "half the bond burned"
    );
    let h = mesh.members[0].chain.height();
    mesh.run_until(|m| m.members.iter().all(|x| x.chain.height() >= h + 3));
    mesh.agree();
}

/// Incident-response rehearsal (Master Prompt 16 §5), timed in engine time.
///
/// A validator key is stolen and the thief signs a second proposal for a
/// round the real validator already proposed in. The rehearsal follows
/// `docs/runbooks/` incident flow end to end through consensus: honest nodes
/// detect it, anyone turns the detection into evidence, the chain tombstones
/// the key and burns half its bond, and the operator rejoins under a fresh key
/// at the next epoch — without the chain stopping at any step.
#[test]
fn incident_rehearsal_a_stolen_key_is_detected_slashed_and_replaced() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.members.iter().all(|x| x.chain.height() >= 3));
    let keys0: Arc<[VerifyingKey]> = genesis().bft.unwrap().verifying_keys().unwrap().into();
    let t_steal = mesh.now;

    // The thief's conflicting proposal for validator 2's current round.
    let round = mesh.members[2].driver.round().max(1);
    let thief = MlDsaAuthenticator::validator(
        validator_key(2),
        Arc::clone(&keys0),
        ChainTag::from_genesis(mesh.members[0].chain.genesis()),
    );
    let forged = Vertex {
        epoch: 0,
        round,
        author: 2,
        timestamp_ms: 424_242,
        parents: vec![],
        batch: vec![],
    };
    let signature = thief.sign(
        maya_dag_bft::SignContext {
            kind: maya_dag_bft::SignKind::Proposal,
            round: forged.round,
            author: forged.author,
        },
        &forged.digest(),
    );
    let frame = Envelope {
        epoch: 0,
        from: 2,
        to: BROADCAST,
        message: Message::Propose {
            vertex: forged,
            signature,
        },
    }
    .encode();
    mesh.queue.push_back((2, frame));
    mesh.run_until(|m| !m.equivocations.is_empty());
    assert!(
        !mesh.equivocations.is_empty(),
        "no honest node noticed the double proposal"
    );
    let t_detect = mesh.now;

    // Response: whoever saw it files the evidence — here, the operator.
    let evidence = mesh.equivocations[0].clone();
    // Through the file an operator would hand to `l1-wallet report-equivocation`.
    let filed = custom_l1_node::consensus::bft::evidence::from_text(
        &custom_l1_node::consensus::bft::evidence::to_text(&evidence),
    )
    .unwrap();
    mesh.submit(&staking_tx(filed, 0));
    let offender = validator_id(&validator_key(2).verifying_key().to_bytes());
    let retired = |m: &Mesh| {
        m.members.iter().all(|x| {
            x.chain
                .state()
                .committed_staking()
                .unwrap()
                .is_some_and(|r| r.staking.retired.contains(&offender))
        })
    };
    mesh.run_until(retired);
    assert!(retired(&mesh), "evidence never landed");
    let t_slashed = mesh.now;

    // Recovery: a fresh key, registered with a fresh bond, seated next epoch.
    mesh.submit(&register(4, 10_000, 1));
    let fresh = validator_id(&validator_key(4).verifying_key().to_bytes());
    let seated = |m: &Mesh| {
        m.members.iter().all(|x| {
            let r = x.chain.state().committed_staking().unwrap().unwrap();
            r.staking.active.contains(&fresh) && !r.staking.active.contains(&offender)
        })
    };
    mesh.run_until(seated);
    assert!(
        seated(&mesh),
        "the replacement key never joined the committee"
    );
    let t_rejoined = mesh.now;
    let h = mesh.members[0].chain.height();
    mesh.run_until(|m| m.members.iter().all(|x| x.chain.height() >= h + 3));
    mesh.agree();
    println!(
        "incident rehearsal (engine time): detected {} ms after the stolen key signed; tombstoned {} ms after detection; replacement seated {} ms after that; chain kept finalizing throughout (height {} and agreeing)",
        t_detect - t_steal,
        t_slashed - t_detect,
        t_rejoined - t_slashed,
        mesh.members[0].chain.height()
    );
}

/// Registers seats that never come online: the committee-capture attack of
/// ADR-039. Keys 5, 6 and 7 belong to no running node.
fn register_absent_seats(mesh: &mut Mesh) {
    for (nonce, i) in (5..8).enumerate() {
        mesh.submit(&register(i, 1_000, u64::try_from(nonce).unwrap()));
    }
}

/// The capture attack against the dual quorum (CON-20), and STK-23's answer.
/// Without the entry limit all three absent seats join at the first
/// boundary: four answering seats of seven are short of the five a quorum
/// needs, so the chain halts before the boundary that would jail them, on a
/// stake-weighted chain as on an equal-weight one. With it one joins per
/// epoch, is jailed at the next boundary, and the chain goes on throughout.
fn absent_seats_join_one_per_epoch_and_the_chain_goes_on(stake_weighted: bool) {
    let mut mesh = Mesh::with(stake_weighted);
    register_absent_seats(&mut mesh);
    let keys: Vec<[u8; 32]> = (5..8)
        .map(|i| validator_id(&validator_key(i).verifying_key().to_bytes()))
        .collect();
    let absent = |m: &Mesh| m.committee(0).iter().filter(|v| keys.contains(v)).count();
    for epoch in 1..=2 {
        mesh.run_until(|m| m.all_in_epoch(epoch));
        assert!(mesh.all_in_epoch(epoch), "no boundary into epoch {epoch}");
        assert_eq!(mesh.committee(0).len(), 5, "epoch {epoch}");
        assert_eq!(absent(&mesh), 1, "epoch {epoch}: one absent seat joined");
        let h = mesh.members[0].chain.height();
        mesh.run_until(|m| m.members.iter().all(|x| x.chain.height() >= h + 3));
        assert!(
            mesh.members.iter().all(|x| x.chain.height() >= h + 3),
            "epoch {epoch}: the chain stopped with one absent seat of five"
        );
    }
    mesh.agree();
}

#[test]
fn absent_cheap_seats_join_one_per_epoch_so_an_equal_weight_chain_goes_on() {
    // Was `absent_cheap_seats_halt_a_one_seat_one_vote_chain`: the halt
    // ADR-039 described, closed for both weightings by STK-23.
    absent_seats_join_one_per_epoch_and_the_chain_goes_on(false);
}

#[test]
fn absent_cheap_seats_join_one_per_epoch_so_a_weighted_chain_goes_on() {
    // Was `absent_cheap_seats_past_a_third_of_the_seats_halt_a_weighted_chain_too`,
    // which asserted the halt (dual-quorum review 2026-11-25, issue 1).
    absent_seats_join_one_per_epoch_and_the_chain_goes_on(true);
}

#[test]
fn an_absent_cheap_seat_within_a_third_of_the_seats_cannot_halt_a_weighted_chain() {
    // Normal finality under the dual quorum: one absent seat of five is
    // within f = 1, so the four online validators carry both thresholds
    // and the chain goes on.
    let mut mesh = Mesh::with(true);
    mesh.submit(&register(5, 1_000, 0));
    mesh.run_until(|m| m.all_in_epoch(1));
    assert!(mesh.all_in_epoch(1), "no epoch boundary reached");
    assert_eq!(mesh.committee(0).len(), 5);
    let h = mesh.members[0].chain.height();
    mesh.run_until(|m| m.members.iter().all(|x| x.chain.height() >= h + 3));
    assert!(
        mesh.members.iter().all(|x| x.chain.height() >= h + 3),
        "the weighted chain stopped"
    );
    mesh.agree();
}

#[test]
fn stake_weighting_is_part_of_the_genesis_id() {
    // Two operators who disagree about weighting disagree about block zero.
    assert_ne!(
        genesis_with(false).chain_id_commitment(),
        genesis_with(true).chain_id_commitment()
    );
}

/// Eric 2026-11-24: slashing, jail, unbonding, downtime and commission are
/// written in the genesis and hashed into its id, so two operators who
/// disagree about a penalty disagree about block zero. Writing them out at
/// the devnet values still commits to them.
#[test]
fn the_staking_rules_are_part_of_the_genesis_id_and_are_the_rules_run() {
    fn rules(config: &mut GenesisConfig) -> &mut Option<StakingRules> {
        &mut config.bft.as_mut().unwrap().staking.as_mut().unwrap().rules
    }
    let plain = genesis_with(true);
    let mut written = genesis_with(true);
    *rules(&mut written) = Some(StakingRules::DEVNET);
    assert_ne!(plain.chain_id_commitment(), written.chain_id_commitment());
    let mut harsher = written.clone();
    *rules(&mut harsher) = Some(StakingRules {
        downtime_slash_bps: 200,
        ..StakingRules::DEVNET
    });
    assert_ne!(written.chain_id_commitment(), harsher.chain_id_commitment());
    let params = harsher.bft.unwrap().staking.unwrap().params();
    assert_eq!(params.downtime_slash_bps, 200);
    assert_eq!(
        params.min_self_bond,
        maya_ledger_math::Amount::from_u64(1_000)
    );
}

/// C4 and C5 (Eric 2026-11-24): the node refuses a genesis with more
/// validators than `max_validators`, and a value-bearing one with fewer than
/// seven or without written staking rules, whatever tool wrote the file.
#[test]
fn the_loader_refuses_a_committee_outside_its_bounds() {
    let mut over = genesis();
    over.bft
        .as_mut()
        .unwrap()
        .staking
        .as_mut()
        .unwrap()
        .max_validators = 3;
    let err = over.validate().unwrap_err().to_string();
    assert!(err.contains("max_validators"), "{err}");

    let mut mainnet = genesis();
    mainnet.chain_id = "maya-mainnet".to_string();
    let err = mainnet.validate().unwrap_err().to_string();
    assert!(err.contains("at least 7"), "{err}");

    let op = hex::encode(operator().address());
    let bft = mainnet.bft.as_mut().unwrap();
    for i in GENESIS_VALIDATORS..MIN_VALUE_BEARING_VALIDATORS {
        bft.validators
            .push(hex::encode(validator_key(i).verifying_key().to_bytes()));
        bft.staking.as_mut().unwrap().bonds.push(GenesisBond {
            operator: op.clone(),
            bond: primitive_types::U256::from(10_000),
            commission_bps: 0,
        });
    }
    let err = mainnet.validate().unwrap_err().to_string();
    assert!(err.contains("bft.staking.rules"), "{err}");
    mainnet
        .bft
        .as_mut()
        .unwrap()
        .staking
        .as_mut()
        .unwrap()
        .rules = Some(StakingRules::DEVNET);
    mainnet.validate().expect("seven seats and written rules");
    // A testnet keeps its four seats and its defaults.
    genesis().validate().expect("a testnet genesis");
}

#[test]
fn a_weighted_node_restarts_after_a_mid_epoch_tombstone_and_keeps_weights() {
    // Review of #77: a tombstone shrinks `active` mid-epoch while the epoch's
    // weights stay frozen at its boundary. A node rebuilding its committee
    // then must take keys and weights from the same snapshot, or it refuses
    // to start and its checkpoints stop.
    let mut mesh = Mesh::with(true);
    // A fifth seat: at four the committee floor holds a tombstone's seat
    // (CON-07), and this test is about the one that leaves mid-epoch.
    mesh.submit(&register(4, 10_000, 0));
    mesh.run_until(|m| m.all_in_epoch(1));
    assert!(mesh.all_in_epoch(1), "no epoch boundary reached");
    let keys0: Arc<[VerifyingKey]> = genesis().bft.unwrap().verifying_keys().unwrap().into();
    let tag = ChainTag::from_genesis(mesh.members[0].chain.genesis());
    mesh.submit(&staking_tx(double_proposal(3, &keys0, tag), 1));
    let offender = validator_id(&validator_key(3).verifying_key().to_bytes());
    mesh.run_until(|m| !m.committee(0).contains(&offender));
    assert!(!mesh.committee(0).contains(&offender), "no tombstone");
    let epoch = mesh.members[0].driver.epoch();
    let weights = mesh.members[0]
        .chain
        .state()
        .committee_weights(epoch)
        .unwrap()
        .expect("weights for the running epoch");
    assert_eq!(weights.len(), NODES, "weights frozen at the boundary");

    // A restart in the middle of that epoch.
    let setup = BftSetup {
        epoch: 0,
        committee: Arc::clone(&keys0),
        signer: None,
        params: Params::default(),
    };
    let restart = mesh.root.path().join("bft-restart");
    let member = &mut mesh.members[0];
    let (driver, _) = BftDriver::open(&setup, &restart, &mut member.chain, mesh.now)
        .expect("a mid-epoch restart after a tombstone");
    assert_eq!(driver.epoch(), epoch);
    assert_eq!(driver.committee().len(), weights.len());
    assert_eq!(driver.weights().as_deref(), Some(weights.as_slice()));

    // The weighting survives several boundaries, the tombstone's included.
    mesh.run_until(|m| m.all_in_epoch(epoch + 3));
    assert!(mesh.all_in_epoch(epoch + 3), "the weighted chain stopped");
    mesh.agree();
    let state = mesh.members[0].chain.state();
    let now = mesh.members[0].driver.epoch();
    let ids = state.committee_ids(now).unwrap().unwrap();
    let weights = state.committee_weights(now).unwrap().unwrap();
    assert_eq!(ids.len(), weights.len());
    assert!(!ids.contains(&offender));
}

/// A peer's checkpoints and blocks, as a catching-up node sees them: the
/// book a running node keeps (closing and live checkpoints, by epoch).
struct PeerBook<'a> {
    book: custom_l1_node::consensus::bft::attest::CheckpointBook,
    chain: &'a Chain,
}

impl custom_l1_node::consensus::bft::catchup::CheckpointSource for PeerBook<'_> {
    fn checkpoint(
        &self,
    ) -> custom_l1_node::error::Result<Option<custom_l1_node::consensus::bft::attest::Checkpoint>>
    {
        Ok(self.book.newest().cloned())
    }

    fn checkpoint_of(
        &self,
        epoch: u64,
    ) -> custom_l1_node::error::Result<Option<custom_l1_node::consensus::bft::attest::Checkpoint>>
    {
        Ok(self.book.of_epoch(epoch).cloned())
    }

    fn block(
        &self,
        height: u64,
    ) -> custom_l1_node::error::Result<custom_l1_node::core::block::Block> {
        let state = self.chain.state();
        let id = state.canonical_id(height)?.expect("peer holds the height");
        Ok(state.load_block(&id)?.expect("peer holds the body"))
    }

    fn tip_height(&self) -> custom_l1_node::error::Result<u64> {
        Ok(self.chain.height())
    }
}

#[test]
fn a_node_catches_up_by_checkpoints_across_a_boundary_that_shrinks_the_committee() {
    // Pre-audit S-011 (attacknet round 8): the boundary into epoch 2 jails the
    // absent seat, so five seats become four. A node that slept through it
    // must cross by checkpoints alone: epoch 1's final checkpoint against
    // epoch 1's committee, then epoch 2's against the four. One absent seat,
    // not the three of round 8: under the dual quorum (CON-20) three absent
    // seats of seven halt the chain before it reaches the boundary.
    let mut mesh = Mesh::with(true);
    mesh.submit(&register(5, 1_000, 0));
    let mut book = custom_l1_node::consensus::bft::attest::CheckpointBook::default();
    for _ in 0..600 {
        if mesh.all_in_epoch(2) && book.newest().is_some_and(|c| c.epoch >= 2) {
            break;
        }
        mesh.now += 250;
        mesh.tick();
        mesh.pump(3_000);
        let d = &mesh.members[0].driver;
        for c in [d.closing_checkpoint(), d.checkpoint()]
            .into_iter()
            .flatten()
        {
            book.offer(c);
        }
    }
    assert!(mesh.all_in_epoch(2), "no second boundary");
    assert_eq!(
        mesh.committee(0).len(),
        4,
        "the absent seats were not jailed"
    );
    let target = book.newest().expect("an epoch-2 checkpoint").height;

    let config = genesis_with(true);
    let state = Arc::new(StateDB::open(mesh.root.path().join("state-sleeper")).unwrap());
    config.seed_state(&state).unwrap();
    let mut sleeper = common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap();
    let source = PeerBook {
        book,
        chain: &mesh.members[0].chain,
    };
    for _ in 0..8 {
        let (committee, weights) = BftDriver::staked_committee(&sleeper).unwrap().unwrap();
        let n = custom_l1_node::consensus::bft::catchup::catch_up(
            &mut sleeper,
            &source,
            &committee,
            weights.as_deref(),
        )
        .unwrap();
        if n == 0 {
            break;
        }
    }
    assert!(
        sleeper.height() >= target,
        "stuck at {} below the attested {target}",
        sleeper.height()
    );
}

/// `get_supply` once summed account balances only, so it reported a staked
/// genesis 4 x 10,000 short of the total supply genesis declares (found by
/// `cargo xtask ceremony-rehearsal`, 2026-11-08).
#[test]
fn get_supply_counts_what_staking_holds() {
    let root = TempDir::new().unwrap();
    let config = genesis();
    let state = Arc::new(StateDB::open(root.path()).unwrap());
    config.seed_state(&state).unwrap();
    let chain = common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap();
    let chain = Arc::new(std::sync::Mutex::new(chain));

    let report = custom_l1_node::rpc::market::supply(&chain).unwrap();

    let expected = config.total_supply().unwrap();
    assert_eq!(
        expected,
        primitive_types::U256::from(1_000_000 + 4 * 10_000)
    );
    assert_eq!(report.total, expected);
    assert_eq!(report.circulating, expected);
}

/// Dual-quorum review 2026-11-25, issue 2: stake above the 20% operator cap
/// carries no weight, so it costs its owner nothing to spend it on extra
/// seats. The PR's own example (80% of the capped weight on 4 of 10 seats)
/// plus eleven minimum-bond seats on eleven fresh payout addresses was an
/// attacker-only quorum after one boundary: 15 of 21 seats, 811 of 675
/// needed weight. Grouping seats by operator would not see it (every address
/// is new); STK-23 lets the seats in at `entry_limit` per epoch, each one
/// online through a boundary, and this measures how many epochs that takes.
#[test]
fn cheap_seats_added_to_capped_stake_reach_no_quorum_in_one_epoch() {
    use maya_staking::{Amount, Participation, Staking};
    // DEVNET: min_self_bond 1,000, max_validators 100 (free seats).
    let mut s = Staking::new(maya_staking::Params::DEVNET).unwrap();
    let key = |n: u8| [n; 32];
    let payout = |n: u8| [0xC0 ^ n; 32];
    // Honest: six seats at 33,334 (200,004 in all). Attacker: four seats at
    // 10^7 each, capped to 20% apiece, so 80% of the weight.
    for n in 1..=6 {
        s.register(payout(n), key(n), Amount::from_u64(33_334), 0)
            .unwrap();
    }
    for n in 7..=10 {
        s.register(payout(n), key(n), Amount::from_u64(10_000_000), 0)
            .unwrap();
    }
    s.active = (1..=10).map(key).collect();
    for n in 20..31 {
        s.register(payout(n), key(n), Amount::from_u64(1_000), 0)
            .unwrap();
    }
    let attacker = |id: &[u8; 32]| id[0] >= 7;
    let mut epochs = 0;
    let quorum_after = loop {
        // Everyone online: the attacker's seats behave until they have enough.
        let authored = s.active.iter().map(|v| (*v, 10)).collect();
        s.end_epoch(
            &Participation {
                rounds: 10,
                expected: s.active.iter().map(|v| (*v, 10)).collect(),
                authored,
            },
            Amount::ZERO,
        )
        .unwrap();
        epochs += 1;
        let weights = custom_l1_node::state::staking::stake_weights(&s, &s.active);
        let committee = maya_dag_bft::Committee::weighted(weights).unwrap();
        let theirs = (0..committee.size()).filter(|i| attacker(&s.active[usize::from(*i)]));
        if committee.is_quorum(theirs) {
            break epochs;
        }
        assert!(epochs < 50, "the attacker never reached a quorum");
    };
    println!(
        "dual quorum + STK-23 measured: capped stake plus minimum-bond seats reached an attacker-only quorum after {quorum_after} epochs ({} seats)",
        s.active.len()
    );
    assert!(
        quorum_after > 1,
        "one boundary of cheap seats gave an attacker-only quorum"
    );
}
