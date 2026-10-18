//! Certificate catch-up through the real JSON-RPC stack (ADR-052): a restarted
//! node fetches its epoch's certificates from a peer's `get_certificates`,
//! verifies them, and replays. The page limits are tiny so paging and
//! continuation actually run; the production limits are checked against the
//! response limit as constants.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::Cell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use custom_l1_node::consensus::bft::attest::{Checkpoint, CheckpointBook};
use custom_l1_node::consensus::bft::certsync::{
    self, CertSource, PAGE_ENCODED_BYTES, PageLimits,
};
use custom_l1_node::consensus::bft::store::{CertPage, for_each_certificate};
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys::{self, SigningKey, VerifyingKey};
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisBond, GenesisConfig, StakingGenesis};
use custom_l1_node::network::Mempool;
use custom_l1_node::rpc::bootstrap::RpcBootstrapSource;
use custom_l1_node::rpc::server::{RPC_RESPONSE_LIMIT, RpcContext, serve};
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::Params;
use tempfile::TempDir;

mod common;

const VALIDATORS: usize = 4;
const EPOCH_BLOCKS: u64 = 6;

// jsonrpsee's default response limit is 10 MiB. A full production page must fit
// under it, and the server and client limit must leave room for framing.
const _: () = assert!(PAGE_ENCODED_BYTES < 10 * 1024 * 1024);
const _: () = assert!(RPC_RESPONSE_LIMIT as usize >= 2 * PAGE_ENCODED_BYTES);

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
        chain_id: "cert-rpc-test".to_string(),
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

fn open_chain(config: &GenesisConfig, state: Arc<StateDB>) -> Chain {
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

struct Member {
    driver: Option<BftDriver>,
    chain: Chain,
    state_dir: PathBuf,
    bft_dir: PathBuf,
}

/// Four validators on disk, run in process.
struct Net {
    root: TempDir,
    config: GenesisConfig,
    setups: Vec<BftSetup>,
    members: Vec<Member>,
    queue: VecDeque<(usize, Vec<u8>)>,
    now: u64,
}

impl Net {
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
        let (mut setups, mut members, mut opening) = (Vec::new(), Vec::new(), Vec::new());
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
            let mut chain = open_chain(&config, state);
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
        let mut net = Self {
            root,
            config,
            setups,
            members,
            queue: VecDeque::new(),
            now: 0,
        };
        for (i, step) in opening {
            net.absorb(i, step);
        }
        net
    }

    fn absorb(&mut self, from: usize, step: Step) {
        self.queue
            .extend(step.frames.into_iter().map(|f| (from, f)));
    }

    fn run_until(&mut self, done: impl Fn(&Self) -> bool) {
        for _ in 0..800 {
            if done(self) {
                return;
            }
            self.now += 250;
            for i in 0..self.members.len() {
                let m = &mut self.members[i];
                if let Some(driver) = m.driver.as_mut() {
                    let step = driver.on_tick(&mut m.chain, self.now).unwrap();
                    self.absorb(i, step);
                }
            }
            for _ in 0..3_000 {
                let Some((from, frame)) = self.queue.pop_front() else {
                    break;
                };
                for i in 0..self.members.len() {
                    let m = &mut self.members[i];
                    if i == from {
                        continue;
                    }
                    if let Some(driver) = m.driver.as_mut() {
                        let step = driver.on_frame(&mut m.chain, self.now, &frame).unwrap();
                        self.absorb(i, step);
                    }
                }
            }
        }
    }

    fn height(&self, i: usize) -> u64 {
        self.members[i].chain.height()
    }

    fn hold_out(&mut self, i: usize) {
        self.members[i].driver = None;
    }

    /// Reopens member `i`'s chain from disk, releasing the old handle first.
    fn reopen(&mut self, i: usize) {
        let scratch = self.root.path().join(format!("scratch-{i}"));
        self.members[i].chain = open_chain(&self.config, Arc::new(StateDB::open(&scratch).unwrap()));
        let state_dir = self.members[i].state_dir.clone();
        self.members[i].chain = open_chain(&self.config, Arc::new(StateDB::open(&state_dir).unwrap()));
    }

    fn restart_from(
        &mut self,
        i: usize,
        source: &impl CertSource,
        limits: PageLimits,
    ) -> custom_l1_node::error::Result<u64> {
        let setup = &self.setups[i];
        let m = &mut self.members[i];
        let synced = certsync::sync(&mut m.chain, source, setup, &m.bft_dir, self.now, limits)?;
        let crossed = synced.crossed;
        m.driver = Some(synced.driver);
        self.absorb(i, synced.step);
        Ok(crossed)
    }
}

/// Counts the pages a catching-up node asks for, to show paging ran.
struct Counting<'a> {
    inner: &'a RpcBootstrapSource,
    pages: Cell<u32>,
}

impl CertSource for Counting<'_> {
    fn current_epoch(&self) -> custom_l1_node::error::Result<u64> {
        self.inner.current_epoch()
    }

    fn page(
        &self,
        epoch: u64,
        offset: u64,
        max_records: usize,
    ) -> custom_l1_node::error::Result<CertPage> {
        self.pages.set(self.pages.get() + 1);
        self.inner.page(epoch, offset, max_records)
    }

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        self.inner.checkpoint_of(epoch)
    }
}

fn logged(dir: &std::path::Path, epoch: u64) -> usize {
    let mut count = 0;
    for_each_certificate(dir, epoch, |_| {
        count += 1;
        Ok(())
    })
    .unwrap();
    count
}

#[test]
fn a_restarted_node_catches_up_through_paged_rpc() {
    let mut net = Net::new();
    net.run_until(|n| n.height(0) >= 3);
    net.hold_out(2);
    net.run_until(|n| n.height(0) >= 8);

    // The peer: its logs, its checkpoints and its status, behind the real RPC.
    let peer = net.members[0].driver.as_ref().unwrap();
    let mut book = CheckpointBook::default();
    for checkpoint in [peer.closing_checkpoint(), peer.checkpoint()]
        .into_iter()
        .flatten()
    {
        book.offer(checkpoint);
    }
    let status = peer.status();
    let state = Arc::new(StateDB::open(&net.root.path().join("context-state")).unwrap());
    let context = RpcContext::new(
        Arc::new(Mutex::new(open_chain(&net.config, Arc::clone(&state)))),
        Mempool::new(state),
    )
    .with_bft_dir(net.members[0].bft_dir.clone())
    .with_checkpoints(Arc::new(Mutex::new(book)))
    .with_bft_status(Arc::new(Mutex::new(status)))
    // Tiny pages: three records, and a budget of one record's worth of JSON.
    .with_page_limits(PageLimits {
        records: 3,
        encoded_bytes: 2_000,
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime
        .block_on(serve("127.0.0.1:0".parse().unwrap(), context))
        .unwrap();
    let url = format!("http://{}", server.address);

    // The restarted node: catches up through the RPC, not in process.
    net.reopen(2);
    let source = RpcBootstrapSource::new(&url, runtime.handle().clone()).unwrap();
    let counting = Counting {
        inner: &source,
        pages: Cell::new(0),
    };
    let tiny = PageLimits {
        records: 3,
        encoded_bytes: 2_000,
    };
    net.restart_from(2, &counting, tiny).unwrap();
    assert!(
        counting.pages.get() > 4,
        "paging must continue past the first page: {} requests",
        counting.pages.get()
    );
    for epoch in 0..=1 {
        let (mine, theirs) = (
            logged(&net.members[2].bft_dir, epoch),
            logged(&net.members[0].bft_dir, epoch),
        );
        assert_eq!(mine, theirs, "epoch {epoch}: every page arrived");
    }
    assert!(net.height(2) >= 6, "the replay reached the first attested end: {}", net.height(2));
    drop(server);
}
