//! Two validators restarted into attested-follower mode while the chain keeps
//! advancing, then the chain crosses an epoch boundary (ADR-038, ADR-043).
//!
//! The 2026-10-18 incident on maya-testnet-1: v2 and v3 restarted with
//! `--catch-up-from` at tip 64,321. Both imported nothing and followed. The
//! chain halted at the next epoch boundary, because followers switch epochs
//! only by importing the boundary block, and a checkpoint needs `n - f`
//! attesters that followers are not. This test pins that every member keeps
//! advancing its tip, builds or imports blocks, and agrees across the boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use custom_l1_node::consensus::bft::attest::Checkpoint;
use custom_l1_node::consensus::bft::catchup::{self, CheckpointSource};
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::Block;
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys::{self, SigningKey, VerifyingKey};
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisBond, GenesisConfig, StakingGenesis};
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::Params;
use tempfile::TempDir;

mod common;

const VALIDATORS: usize = 4;
const EPOCH_BLOCKS: u64 = 6;

fn operator() -> HybridSigningKey {
    signing_key_from_seed(&[0x42; 32]).unwrap()
}

fn validator_key(i: usize) -> Arc<SigningKey> {
    Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
}

fn genesis_config() -> GenesisConfig {
    let validators = (0..VALIDATORS)
        .map(|i| hex::encode(validator_key(i).verifying_key().to_bytes()))
        .collect();
    let op = hex::encode(operator().address());
    GenesisConfig {
        chain_id: "bft-follower-test".to_string(),
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
                bonds: (0..VALIDATORS)
                    .map(|_| GenesisBond {
                        operator: op.clone(),
                        bond: primitive_types::U256::from(10_000),
                        commission_bps: 0,
                    })
                    .collect(),
                min_self_bond: primitive_types::U256::from(1_000),
                max_validators: 100,
                stake_weighted: false,
            }),
        }),
    }
}

struct Member {
    driver: Option<BftDriver>,
    chain: Chain,
    state_dir: PathBuf,
    bft_dir: PathBuf,
}

struct Mesh {
    root: TempDir,
    // Kept so the directory outlives every chain in `members`.
    config: GenesisConfig,
    committee: Arc<[VerifyingKey]>,
    setups: Vec<BftSetup>,
    members: Vec<Member>,
    queue: VecDeque<(usize, Vec<u8>)>,
    now: u64,
    equivocations: usize,
}

/// A peer as a catch-up source: what the binary's `CheckpointBook` serves
/// (the newest checkpoint of the current epoch and of the one just ended),
/// and its own blocks.
struct Peer<'a> {
    chain: &'a Chain,
    checkpoints: Vec<Checkpoint>,
}

impl CheckpointSource for Peer<'_> {
    fn checkpoint(&self) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        Ok(self.checkpoints.iter().max_by_key(|c| c.epoch).cloned())
    }

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        Ok(self.checkpoints.iter().find(|c| c.epoch == epoch).cloned())
    }

    fn block(&self, height: u64) -> custom_l1_node::error::Result<Block> {
        let state = self.chain.state();
        let id = state.canonical_id(height)?.expect("peer holds the height");
        Ok(state.load_block(&id)?.expect("peer holds the body"))
    }
}

fn open_chain(config: &GenesisConfig, state_dir: &PathBuf) -> Chain {
    let state = Arc::new(StateDB::open(state_dir).unwrap());
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

impl Mesh {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let config = genesis_config();
        let committee: Arc<[VerifyingKey]> = config
            .bft
            .as_ref()
            .unwrap()
            .verifying_keys()
            .unwrap()
            .into();
        let params = Params {
            batch_size: 100,
            min_round_interval_ms: 200,
            ..Params::default()
        };
        let mut setups = Vec::new();
        let mut members = Vec::new();
        let mut opening = Vec::new();
        for i in 0..VALIDATORS {
            let state_dir = root.path().join(format!("state-{i}"));
            let bft_dir = root.path().join(format!("bft-{i}"));
            let setup = BftSetup {
                epoch: 0,
                committee: Arc::clone(&committee),
                signer: Some(validator_key(i).into()),
                params,
            };
            let state = Arc::new(StateDB::open(&state_dir).unwrap());
            config.seed_state(&state).unwrap();
            let mut chain = common::open_chain(
                state,
                config.genesis_block().unwrap(),
                ChainConfig::without_pow_verification(),
            )
            .unwrap();
            let (driver, step) = BftDriver::open(&setup, &bft_dir, &mut chain, 0).unwrap();
            opening.push((i, step));
            members.push(Member {
                driver: Some(driver),
                chain,
                state_dir,
                bft_dir,
            });
            setups.push(setup);
        }
        let mut mesh = Self {
            root,
            config,
            committee,
            setups,
            members,
            queue: VecDeque::new(),
            now: 0,
            equivocations: 0,
        };
        for (i, step) in opening {
            mesh.absorb(i, step);
        }
        mesh
    }

    fn absorb(&mut self, from: usize, step: Step) {
        self.equivocations += step.equivocations.len();
        self.queue
            .extend(step.frames.into_iter().map(|f| (from, f)));
    }

    fn tick(&mut self) {
        for i in 0..self.members.len() {
            let m = &mut self.members[i];
            let Some(driver) = m.driver.as_mut() else {
                continue;
            };
            let step = driver.on_tick(&mut m.chain, self.now).unwrap();
            self.absorb(i, step);
        }
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
                let Some(driver) = m.driver.as_mut() else {
                    continue;
                };
                let step = driver.on_frame(&mut m.chain, self.now, &frame).unwrap();
                self.absorb(i, step);
            }
        }
    }

    /// What the binary's follow loop does once a second: each follower imports
    /// up to the newest checkpoint member `from` serves.
    fn follow(&mut self, from: usize) {
        for i in 0..self.members.len() {
            if i == from
                || !self.members[i]
                    .driver
                    .as_ref()
                    .is_some_and(BftDriver::is_follower)
            {
                continue;
            }
            self.catch_up(i, from);
        }
    }

    /// As the binary's catch-up: one epoch per round, each checked against the
    /// committee the node's own state names, until nothing more is imported.
    fn catch_up(&mut self, lagging: usize, from: usize) -> u64 {
        let mut imported = 0;
        loop {
            let (committee, weights) = BftDriver::staked_committee(&self.members[lagging].chain)
                .expect("committee in state")
                .unwrap_or_else(|| (Arc::clone(&self.committee), None));
            let n = self.catch_up_once(lagging, from, &committee, weights.as_deref());
            imported += n;
            if n == 0 {
                return imported;
            }
        }
    }

    fn catch_up_once(
        &mut self,
        lagging: usize,
        from: usize,
        committee: &[VerifyingKey],
        weights: Option<&[u64]>,
    ) -> u64 {
        let (a, b) = self.members.split_at_mut(from.max(lagging));
        let (peer, lag) = if from < lagging {
            (&a[from], &mut b[0])
        } else {
            (&b[0], &mut a[lagging])
        };
        let checkpoints = peer.driver.as_ref().map_or_else(Vec::new, |d| {
            [d.closing_checkpoint(), d.checkpoint()]
                .into_iter()
                .flatten()
                .cloned()
                .collect()
        });
        let peer = Peer {
            chain: &peer.chain,
            checkpoints,
        };
        catchup::catch_up(&mut lag.chain, &peer, committee, weights)
            .expect("catch-up from a peer's checkpoint")
    }

    /// Runs until `done`, letting followers catch up from member 0 each step.
    fn run_until(&mut self, done: impl Fn(&Self) -> bool) {
        for _ in 0..800 {
            if done(self) {
                return;
            }
            self.now += 250;
            self.tick();
            self.pump(3_000);
            self.follow(0);
        }
    }

    fn hold_out(&mut self, i: usize) {
        self.members[i].driver = None;
    }

    /// Reopens member `i` from disk and restarts it the way the incident did:
    /// catch up from `from`, then follow attested blocks.
    fn restart_following(&mut self, i: usize, from: usize) {
        // Release the old chain's database lock first, as a restart would.
        let scratch = self.root.path().join(format!("scratch-{i}"));
        self.members[i].chain = open_chain(&self.config, &scratch);
        let state_dir = self.members[i].state_dir.clone();
        self.members[i].chain = open_chain(&self.config, &state_dir);
        self.catch_up(i, from);
        let m = &mut self.members[i];
        let (mut driver, step) =
            BftDriver::open(&self.setups[i], &m.bft_dir, &mut m.chain, self.now).unwrap();
        driver.follow_attested(&m.chain);
        m.driver = Some(driver);
        self.absorb(i, step);
    }

    fn height(&self, i: usize) -> u64 {
        self.members[i].chain.height()
    }

    fn epoch(&self, i: usize) -> Option<u64> {
        self.members[i].driver.as_ref().map(BftDriver::epoch)
    }

    /// Every live member's block at the lowest common height has one id.
    fn agree(&self) {
        let common = self.members.iter().map(|m| m.chain.height()).min().unwrap();
        let at = |m: &Member| m.chain.active_chain().unwrap()[usize::try_from(common).unwrap()];
        let first = at(&self.members[0]);
        for (i, m) in self.members.iter().enumerate() {
            assert_eq!(
                at(m),
                first,
                "member {i} built a different block at {common}"
            );
        }
    }
}

#[test]
fn two_validators_restarted_into_follower_mode_keep_pace_across_an_epoch_boundary() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 3);

    // Member 2 is down while the chain advances, then restarts as a follower.
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 9);
    mesh.restart_following(2, 0);

    // Member 3 does the same while 2 follows: two followers, two builders.
    mesh.hold_out(3);
    mesh.run_until(|m| m.height(0) >= 15);
    mesh.restart_following(3, 0);

    let before: Vec<u64> = (0..VALIDATORS).map(|i| mesh.height(i)).collect();
    let target = mesh.height(0) + 12;
    mesh.run_until(|m| m.height(0) >= target);

    let after: Vec<u64> = (0..VALIDATORS).map(|i| mesh.height(i)).collect();
    for i in 0..VALIDATORS {
        assert!(
            after[i] > before[i],
            "member {i} stopped at {} (was {}); network at {}",
            after[i],
            before[i],
            mesh.height(0)
        );
    }
    // Twelve blocks is two epochs: the chain crossed at least one boundary.
    for i in 0..VALIDATORS {
        assert!(
            mesh.epoch(i).is_some_and(|e| e >= 2),
            "member {i} is not in epoch 2: {:?}",
            mesh.epoch(i)
        );
    }
    mesh.agree();
    assert_eq!(mesh.equivocations, 0);
}
