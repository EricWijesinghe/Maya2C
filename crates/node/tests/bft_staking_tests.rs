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

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use custom_l1_node::consensus::bft::auth::MlDsaAuthenticator;
use custom_l1_node::consensus::bft::{BROADCAST, BftDriver, BftSetup, Envelope, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::staking_payload::StakingAction;
use custom_l1_node::core::transaction::Transaction;
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys::{self, SigningKey, VerifyingKey};
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisBond, GenesisConfig, StakingGenesis};
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
            balance: 1_000_000,
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators,
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 0,
            fees: None,
            staking: Some(StakingGenesis {
                epoch_blocks: EPOCH_BLOCKS,
                bonds: (0..GENESIS_VALIDATORS)
                    .map(|_| GenesisBond {
                        operator: op.clone(),
                        bond: 10_000,
                        commission_bps: 0,
                    })
                    .collect(),
                min_self_bond: 1_000,
                max_validators: 100,
                stake_weighted,
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
            bond,
            commission_bps: 0,
            possession: Box::new(possession),
        },
        nonce,
    )
}

fn double_proposal(i: usize, committee: &Arc<[VerifyingKey]>) -> StakingAction {
    let auth = MlDsaAuthenticator::validator(validator_key(i), Arc::clone(committee));
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
    mesh.submit(&staking_tx(double_proposal(3, &keys0), 1));
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
        record.staking.validators[&offender].self_bond, 5_000,
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
    let thief = MlDsaAuthenticator::validator(validator_key(2), Arc::clone(&keys0));
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
    let frame_of = |v: &Vertex, sig: &[u8]| {
        Envelope {
            epoch: v.epoch,
            from: v.author,
            to: BROADCAST,
            message: Message::Propose {
                vertex: v.clone(),
                signature: sig.to_vec(),
            },
        }
        .encode()
    };
    mesh.submit(&staking_tx(
        StakingAction::ReportEquivocation {
            first: frame_of(&evidence.first, &evidence.first_signature),
            second: frame_of(&evidence.second, &evidence.second_signature),
        },
        0,
    ));
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

#[test]
fn absent_cheap_seats_halt_a_one_seat_one_vote_chain() {
    // The control for the test below. Four validators at 10,000 each; three
    // seats bought at the 1,000 minimum never answer. Seven seats need five
    // votes, four answer, and nothing more is ever committed.
    let mut mesh = Mesh::with(false);
    register_absent_seats(&mut mesh);
    mesh.run_until(|m| m.all_in_epoch(1));
    assert!(mesh.all_in_epoch(1), "no epoch boundary reached");
    assert_eq!(mesh.committee(0).len(), 7);
    let h = mesh.members[0].chain.height();
    mesh.run_until(|m| m.members[0].chain.height() >= h + 3);
    assert!(
        mesh.members[0].chain.height() < h + 3,
        "an equal-weight chain kept going without a quorum"
    );
}

#[test]
fn absent_cheap_seats_cannot_halt_a_stake_weighted_chain() {
    // ADR-040 part 2: the same attack against a genesis with stake_weighted.
    // The four online validators hold 40,000 of 43,000 stake, a quorum by
    // weight, so the chain goes on; the cheap seats only cost anchor
    // timeouts until the epoch boundary jails them.
    let mut mesh = Mesh::with(true);
    register_absent_seats(&mut mesh);
    mesh.run_until(|m| m.all_in_epoch(1));
    assert!(mesh.all_in_epoch(1), "no epoch boundary reached");
    assert_eq!(mesh.committee(0).len(), 7);
    let weights = mesh.members[0]
        .chain
        .state()
        .committee_weights(1)
        .unwrap()
        .expect("a stake-weighted chain records each epoch's weights");
    let mut sorted = weights.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [1_000, 1_000, 1_000, 10_000, 10_000, 10_000, 10_000]
    );
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

#[test]
fn a_weighted_node_restarts_after_a_mid_epoch_tombstone_and_keeps_weights() {
    // Review of #77: a tombstone shrinks `active` mid-epoch while the epoch's
    // weights stay frozen at its boundary. A node rebuilding its committee
    // then must take keys and weights from the same snapshot, or it refuses
    // to start and its checkpoints stop.
    let mut mesh = Mesh::with(true);
    mesh.run_until(|m| m.all_in_epoch(1));
    assert!(mesh.all_in_epoch(1), "no epoch boundary reached");
    let keys0: Arc<[VerifyingKey]> = genesis().bft.unwrap().verifying_keys().unwrap().into();
    mesh.submit(&staking_tx(double_proposal(3, &keys0), 0));
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
    assert_eq!(
        weights.len(),
        GENESIS_VALIDATORS,
        "weights frozen at the boundary"
    );

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
