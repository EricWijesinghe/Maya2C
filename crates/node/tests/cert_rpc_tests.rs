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
    self, CertSource, PAGE, PAGE_CERTS, PAGE_ENCODED_BYTES, PageLimits,
};
use custom_l1_node::consensus::bft::store::{CertPage, for_each_certificate};
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::validator_keys::{self as keys, SigningKey, VerifyingKey};
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
                min_delegation: primitive_types::U256::from(10),
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
        self.members[i].chain =
            open_chain(&self.config, Arc::new(StateDB::open(&scratch).unwrap()));
        let state_dir = self.members[i].state_dir.clone();
        self.members[i].chain =
            open_chain(&self.config, Arc::new(StateDB::open(&state_dir).unwrap()));
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
    let state = Arc::new(StateDB::open(net.root.path().join("context-state")).unwrap());
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
    assert!(
        net.height(2) >= 6,
        "the replay reached the first attested end: {}",
        net.height(2)
    );
    drop(server);
}

/// A running committee's peer, served over the real RPC with `budget` as the
/// certificate byte budget. Returns the server (keep it alive) and its URL.
fn serve_peer(
    net: &mut Net,
    runtime: &tokio::runtime::Runtime,
    budget: Arc<custom_l1_node::rpc::limit::RateLimiter>,
) -> (custom_l1_node::rpc::server::RpcServer, String) {
    let peer = net.members[0].driver.as_ref().unwrap();
    let mut book = CheckpointBook::default();
    for checkpoint in [peer.closing_checkpoint(), peer.checkpoint()]
        .into_iter()
        .flatten()
    {
        book.offer(checkpoint);
    }
    let status = peer.status();
    let state = Arc::new(StateDB::open(net.root.path().join("budget-state")).unwrap());
    let context = RpcContext::new(
        Arc::new(Mutex::new(open_chain(&net.config, Arc::clone(&state)))),
        Mempool::new(state),
    )
    .with_bft_dir(net.members[0].bft_dir.clone())
    .with_checkpoints(Arc::new(Mutex::new(book)))
    .with_bft_status(Arc::new(Mutex::new(status)))
    .with_cert_budget(budget);
    let server = runtime
        .block_on(serve("127.0.0.1:0".parse().unwrap(), context))
        .unwrap();
    let url = format!("http://{}", server.address);
    (server, url)
}

#[test]
fn a_client_over_its_certificate_byte_budget_is_refused_by_the_server() {
    let mut net = Net::new();
    net.run_until(|n| n.height(0) >= 3);
    net.hold_out(2);
    net.run_until(|n| n.height(0) >= 8);
    // One byte of budget: no page of real certificates fits, so the first
    // page the client asks for is refused with the budget error.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (server, url) = serve_peer(
        &mut net,
        &runtime,
        Arc::new(custom_l1_node::rpc::limit::RateLimiter::new(1, 1)),
    );
    // A raw call, not the catch-up client: that client backs off and retries a
    // budget refusal, which would take minutes here. The server's answer is what
    // is under test.
    use jsonrpsee::core::client::ClientT;
    let client = jsonrpsee::http_client::HttpClientBuilder::default()
        .build(&url)
        .unwrap();
    let refused = runtime
        .block_on(client.request::<serde_json::Value, _>(
            "get_certificates",
            jsonrpsee::rpc_params![0_u64, 0_u64, 256_usize],
        ))
        .expect_err("a client past its byte budget must be refused");
    assert!(refused.to_string().contains("byte budget"), "{refused}");
    drop(server);
}

#[test]
fn a_genesis_above_the_record_bounds_is_refused() {
    use custom_l1_node::consensus::bft::store::{MAX_BATCH_ITEMS, MAX_COMMITTEE};
    // The record bound is derived from these two limits, so a genesis above them
    // could declare a legal-looking chain whose certificates the log refuses.
    assert!(genesis_config().validate().is_ok());
    let mut too_many_items = genesis_config();
    too_many_items.bft.as_mut().unwrap().batch_size = MAX_BATCH_ITEMS + 1;
    assert!(
        too_many_items.validate().is_err(),
        "batch_size above the bound"
    );
    let mut too_many_validators = genesis_config();
    too_many_validators
        .bft
        .as_mut()
        .unwrap()
        .staking
        .as_mut()
        .unwrap()
        .max_validators = u16::try_from(MAX_COMMITTEE + 1).unwrap();
    assert!(
        too_many_validators.validate().is_err(),
        "max_validators above the bound"
    );
    // Without a staking section the committee is the validator list alone, and
    // that list is what a quorum certificate is sized by.
    let mut big_committee = genesis_config();
    let bft = big_committee.bft.as_mut().unwrap();
    bft.staking = None;
    bft.validators = (0..=MAX_COMMITTEE)
        .map(|i| hex::encode(validator_key(i).verifying_key().to_bytes()))
        .collect();
    assert!(
        big_committee.validate().is_err(),
        "a committee of MAX_COMMITTEE + 1 without staking"
    );
}

#[test]
fn a_certificate_from_a_full_committee_fits_the_record_bound() {
    use custom_l1_node::consensus::bft::store::{
        MAX_BATCH_BYTES, MAX_COMMITTEE, MAX_RECORD_BYTES, SafetyStore,
    };
    use custom_l1_node::consensus::bft::wire::encode_certificate;
    use custom_l1_node::crypto::validator_keys::SIGNATURE_LENGTH;
    use maya_dag_bft::{Certificate, Vertex};

    // Every member votes and is a parent, and the batch is the largest the
    // protocol certifies.
    let members = u16::try_from(MAX_COMMITTEE).unwrap();
    let mut vertex = Vertex::genesis_in(0, members);
    vertex.round = 1;
    vertex.parents = (0..MAX_COMMITTEE)
        .map(|i| [u8::try_from(i).unwrap(); 32])
        .collect();
    vertex.batch = vec![vec![0x5a_u8; MAX_BATCH_BYTES / 4]; 4];
    let cert = Certificate {
        vertex,
        votes: (0..members).collect(),
        signatures: vec![vec![0x33_u8; SIGNATURE_LENGTH]; MAX_COMMITTEE],
    };
    assert!(
        encode_certificate(&cert).len() <= MAX_RECORD_BYTES,
        "a full-committee certificate exceeds the record bound"
    );
    let root = TempDir::new().unwrap();
    let (mut store, _) = SafetyStore::open(root.path(), 0).unwrap();
    store
        .record_certificate(&cert)
        .expect("a full-committee certificate is written");
}

#[test]
fn a_client_holds_one_certificate_page_at_a_time() {
    let gate = custom_l1_node::rpc::server::CertGate::default();
    let a: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let b: std::net::IpAddr = "10.0.0.2".parse().unwrap();
    let first = gate.claim(a).expect("the first page is admitted");
    assert!(
        gate.claim(a).is_none(),
        "a second page from the same client waits"
    );
    assert!(gate.claim(b).is_some(), "another client is not held up");
    drop(first);
    assert!(gate.claim(a).is_some(), "released when the page is done");
}

#[test]
fn a_max_size_batch_certificate_round_trips_through_the_log_and_the_rpc() {
    use custom_l1_node::consensus::bft::store::{MAX_BATCH_BYTES, SafetyStore};
    use custom_l1_node::consensus::bft::wire::encode_certificate;
    use custom_l1_node::crypto::validator_keys::SIGNATURE_LENGTH;
    use maya_dag_bft::{Certificate, Vertex};

    // The largest batch the protocol certifies: four items of a quarter of the
    // payload bound each. Votes from a four-validator committee.
    let root = TempDir::new().unwrap();
    let mut vertex = Vertex::genesis_in(0, 4);
    vertex.round = 1;
    vertex.batch = vec![vec![0x5a_u8; MAX_BATCH_BYTES / 4]; 4];
    let cert = Certificate {
        vertex,
        votes: vec![0, 1, 2],
        signatures: vec![vec![0x33_u8; SIGNATURE_LENGTH]; 3],
    };
    {
        let (mut store, _) = SafetyStore::open(root.path(), 0).unwrap();
        store
            .record_certificate(&cert)
            .expect("a legal certificate is written");
    }
    // Reopen: nothing is cut, and the certificate reads back from the log.
    let (_, recovered) = SafetyStore::open(root.path(), 0).unwrap();
    assert_eq!(recovered.torn_bytes, 0, "a legal record was cut");
    let mut seen = Vec::new();
    for_each_certificate(root.path(), 0, |c| {
        seen.push(c);
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, vec![cert.clone()]);

    // Served by the RPC and fetched through it.
    let state = Arc::new(StateDB::open(root.path().join("state")).unwrap());
    let context = RpcContext::new(
        Arc::new(Mutex::new(open_chain(
            &genesis_config(),
            Arc::clone(&state),
        ))),
        Mempool::new(state),
    )
    .with_bft_dir(root.path().to_path_buf());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime
        .block_on(serve("127.0.0.1:0".parse().unwrap(), context))
        .unwrap();
    let source = RpcBootstrapSource::new(
        &format!("http://{}", server.address),
        runtime.handle().clone(),
    )
    .unwrap();
    let page = source.page(0, 0, PAGE_CERTS).unwrap();
    assert!(page.done, "one record is the whole epoch");
    assert_eq!(page.records, vec![encode_certificate(&cert)]);
    drop(server);
}

#[test]
fn a_client_refused_for_its_byte_budget_backs_off_and_succeeds() {
    use custom_l1_node::rpc::limit::RateLimiter;
    let mut net = Net::new();
    net.run_until(|n| n.height(0) >= 3);
    net.hold_out(2);
    net.run_until(|n| n.height(0) >= 8);
    // The first request reserves a whole page (4 MiB) before its disk read. The
    // bucket is spent just before it, so that request is refused with the budget
    // message, and the client must back off until the bucket holds a page's
    // worth again: about four seconds at 1 MB/s.
    let burst: u32 = 4 * 1024 * 1024;
    let budget = Arc::new(RateLimiter::new(1_000_000, burst));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (server, url) = serve_peer(&mut net, &runtime, Arc::clone(&budget));
    net.reopen(2);
    assert!(budget.charge(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        u64::from(burst)
    ));
    let source = RpcBootstrapSource::new(&url, runtime.handle().clone()).unwrap();
    net.restart_from(2, &source, PAGE)
        .expect("a refused page is retried after backing off, not fatal");
    for epoch in 0..=1 {
        assert_eq!(
            logged(&net.members[2].bft_dir, epoch),
            logged(&net.members[0].bft_dir, epoch),
            "epoch {epoch}: every page arrived"
        );
    }
    drop(server);
}
