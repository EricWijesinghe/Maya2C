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
            }),
        }),
    }
}

struct Member {
    driver: BftDriver,
    chain: Chain,
}

struct Mesh {
    _root: TempDir,
    members: Vec<Member>,
    queue: VecDeque<(usize, Vec<u8>)>,
    now: u64,
}

impl Mesh {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let config = genesis();
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
            let mut chain = Chain::open(
                state,
                config.genesis_block().unwrap(),
                ChainConfig::without_pow_verification(),
            )
            .unwrap();
            let setup = BftSetup {
                epoch: 0,
                committee: Arc::clone(&committee),
                signer: Some(validator_key(i)),
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
            _root: root,
            members,
            queue: VecDeque::new(),
            now: 0,
        };
        for (i, step) in opening {
            mesh.absorb(i, step);
        }
        mesh
    }

    fn absorb(&mut self, from: usize, step: Step) {
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
    tx.sign(&operator()).unwrap();
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
        let signature = auth.sign(&vertex.digest());
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
