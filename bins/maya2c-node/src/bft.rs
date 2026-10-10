//! The DAG-BFT loop: gossip frames and a clock in, blocks out (ADR-027).
//!
//! Everything consensus-shaped lives in `custom_l1_node::consensus::bft`; this
//! file is the async shell around it — which frames to read, when to tick,
//! which pooled transactions to hand the engine, and what to publish.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use custom_l1_node::consensus::Chain;
use custom_l1_node::consensus::bft::attest::CheckpointBook;
use custom_l1_node::consensus::bft::catchup::{self, Position};
use custom_l1_node::consensus::bft::remote::{DEFAULT_DEADLINE, RemoteSigner, ValidatorKey};
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::crypto::keys::{self, SigningKey, VerifyingKey};
use custom_l1_node::genesis::BftGenesis;
use custom_l1_node::metrics::Metrics;
use custom_l1_node::network::{Mempool, NodeEvent, NodeHandle};
use custom_l1_node::rpc::bootstrap::RpcBootstrapSource;
use maya_dag_bft::Params;
use tokio::sync::broadcast::error::RecvError;
use zeroize::Zeroizing;

use super::lock_chain;

/// Engine tick: re-broadcasts and anchor timeouts. Well under the anchor
/// timeout, so a timeout fires within a tenth of its length of being due.
const TICK: Duration = Duration::from_millis(100);

/// How long a pooled transaction waits for the validator whose share it is
/// before any validator proposes it. See `feed_mempool`.
const SHARE_GRACE: Duration = Duration::from_secs(3);

/// Where the loop reports what it did: metrics, and the slots the RPC's
/// `get_checkpoint` and `get_bft_status` read.
pub(super) struct Reporting {
    pub(super) metrics: Arc<Metrics>,
    pub(super) checkpoint: Arc<Mutex<CheckpointBook>>,
    pub(super) status: Arc<Mutex<custom_l1_node::rpc::types::BftStatus>>,
}

/// Puts the driver's checkpoints where `get_checkpoint` reads them: this
/// epoch's newest, and the just-ended epoch's final one, which late
/// attestations can still complete after the switch.
fn publish_checkpoint(driver: &BftDriver, slot: &Mutex<CheckpointBook>) {
    let mut book = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for checkpoint in [driver.closing_checkpoint(), driver.checkpoint()]
        .into_iter()
        .flatten()
    {
        book.offer(checkpoint);
    }
}

/// How often an attested follower looks for newer checkpoints (ADR-038).
const FOLLOW_INTERVAL: Duration = Duration::from_secs(1);
/// A builder's follow loop looks once per this many intervals: 30 s.
const BUILDER_FOLLOW_EVERY: u32 = 30;
/// Least time between two reports of frames lost to a full event channel.
const DROP_REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// Where the per-epoch safety logs live.
const BFT_DIR: &str = "bft";

/// Writes a fresh ML-DSA-65 validator key to `path` (hex, refusing to
/// overwrite) and prints the public key an operator puts in genesis.
pub(super) fn generate_validator_key(path: &Path) -> Result<(), Box<dyn Error>> {
    let key = keys::generate_signing_key()?;
    let secret = Zeroizing::new(hex::encode(key.to_bytes().as_slice()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("{}: {e} (refusing to overwrite a key)", path.display()))?;
    std::io::Write::write_all(&mut file, secret.as_bytes())?;
    file.sync_all()?;
    println!("{}", hex::encode(key.verifying_key().to_bytes()));
    Ok(())
}

/// Writes this node's channel identity for the remote signer (a 32-byte
/// seed, hex, 0600, never overwritten) and prints the public key the signer
/// must pin with `--allow-node`. This is a transport key, not the validator
/// key: the validator key stays in the signer (ADR-033).
pub(super) fn generate_signer_identity(path: &Path) -> Result<(), Box<dyn Error>> {
    let seed = maya_crypto_pq::suite::MasterSeed::generate()?;
    let identity = maya_signer::channel::Identity::from_seed(&seed);
    let secret = Zeroizing::new(hex::encode(seed.expose()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| {
        format!(
            "{}: {e} (refusing to overwrite an identity)",
            path.display()
        )
    })?;
    std::io::Write::write_all(&mut file, secret.as_bytes())?;
    file.sync_all()?;
    println!("{}", hex::encode(identity.public_key()));
    Ok(())
}

/// Connects to the remote signer: the node's identity from `identity_path`,
/// the signer pinned by `pin_hex`, holding the validator key `pubkey_hex`.
pub(super) fn remote_signer(
    addr: std::net::SocketAddr,
    identity_path: &Path,
    pin_hex: &str,
    pubkey_hex: &str,
) -> Result<ValidatorKey, Box<dyn Error>> {
    let text = Zeroizing::new(
        std::fs::read_to_string(identity_path)
            .map_err(|e| format!("{}: {e}", identity_path.display()))?,
    );
    let bytes = Zeroizing::new(hex::decode(text.trim())?);
    let seed: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("{}: not a 32-byte identity seed", identity_path.display()))?;
    let identity = maya_signer::channel::Identity::from_seed(
        &maya_crypto_pq::suite::MasterSeed::from_bytes(seed),
    );
    let public: [u8; keys::PUBLIC_KEY_LEN] = hex::decode(pubkey_hex)?
        .try_into()
        .map_err(|_| "--validator-pubkey is not an ML-DSA-65 public key")?;
    let remote = RemoteSigner::connect(
        addr,
        identity,
        hex::decode(pin_hex)?,
        VerifyingKey::from_bytes(&public)?,
        DEFAULT_DEADLINE,
    )?;
    Ok(ValidatorKey::Remote(Arc::new(remote)))
}

/// Reads a key written by [`generate_validator_key`].
pub(super) fn load_validator_key(path: &Path) -> Result<Arc<SigningKey>, Box<dyn Error>> {
    let text = Zeroizing::new(
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
    );
    let bytes = Zeroizing::new(hex::decode(text.trim())?);
    let array: Zeroizing<[u8; keys::SECRET_KEY_LEN]> = Zeroizing::new(
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| format!("{}: not an ML-DSA-65 secret key", path.display()))?,
    );
    Ok(Arc::new(SigningKey::from_bytes(&array)?))
}

/// Builds the engine setup from genesis and an optional validator key.
pub(super) fn setup(
    genesis: &BftGenesis,
    signer: Option<ValidatorKey>,
) -> Result<BftSetup, Box<dyn Error>> {
    Ok(BftSetup {
        epoch: 0,
        committee: genesis.verifying_keys()?.into(),
        signer,
        params: Params {
            batch_size: genesis.batch_size,
            anchor_timeout_ms: genesis.anchor_timeout_ms,
            min_round_interval_ms: genesis.round_interval_ms,
            // Local, not consensus: how often a lost proposal or certificate
            // is re-sent. Half the anchor timeout recovers a loss before the
            // round would time out anyway.
            resend_interval_ms: genesis.anchor_timeout_ms / 2,
            ..Params::default()
        },
    })
}

/// Opens the driver on `data_dir/bft`, replaying its safety log.
pub(super) fn open(
    setup: &BftSetup,
    data_dir: &Path,
    chain: &Mutex<Chain>,
) -> Result<(BftDriver, Step), Box<dyn Error>> {
    let mut chain = lock_chain(chain);
    Ok(BftDriver::open(
        setup,
        &data_dir.join(BFT_DIR),
        &mut chain,
        now_ms(),
    )?)
}

/// Fetches from `url` up to its newest checkpoint, without holding the chain
/// lock while the peer answers, then imports under the lock (ADR-038).
async fn catch_up_once(
    url: &str,
    chain: &Arc<Mutex<Chain>>,
    committee: &Arc<[VerifyingKey]>,
    weights: Option<&Arc<[u64]>>,
) -> Result<u64, Box<dyn Error>> {
    let from = Position::of(&lock_chain(chain));
    let source = RpcBootstrapSource::new(url, tokio::runtime::Handle::current())?;
    let committee = Arc::clone(committee);
    let weights = weights.cloned();
    let blocks = tokio::task::spawn_blocking(move || {
        catchup::fetch(from, &source, &committee, weights.as_deref())
    })
    .await??;
    if blocks.is_empty() {
        return Ok(0);
    }
    Ok(catchup::import(&mut lock_chain(chain), blocks)?)
}

/// Whether this node's own engine can derive every block between its tip
/// and the peer's, as on any ordinary restart: the same epoch, the peer's
/// tip anchor within half the engine's window of ours (so every certificate
/// in between can still be fetched), and our tip an ancestor of theirs.
///
/// Then the node resumes building. Otherwise it follows attested blocks,
/// which needs checkpoints that only builders can sign. Following when one
/// block behind is what stranded restarted validators: with more than f of
/// them following, the builders alone never reached a checkpoint quorum
/// (attacknet soak, 2026-10-05). Any failure answers "no", the safe
/// direction.
async fn within_engine_window(url: &str, height: u64, tip: [u8; 32], nonce: u64) -> bool {
    use custom_l1_node::consensus::bft::builder::unseal;
    use custom_l1_node::state_pruner::snapshot::BootstrapSource as _;
    let Ok(source) = RpcBootstrapSource::new(url, tokio::runtime::Handle::current()) else {
        return false;
    };
    tokio::task::spawn_blocking(move || {
        let Ok(theirs) = source.tip_height() else {
            return false;
        };
        if height == 0 || theirs < height {
            return height == 0 && theirs == 0;
        }
        let ancestor = source.block(height).is_ok_and(|b| b.header.id() == tip);
        let Ok(peer_tip) = source.block(theirs) else {
            return false;
        };
        let (our_epoch, our_round) = unseal(nonce);
        let (their_epoch, their_round) = unseal(peer_tip.header.nonce);
        ancestor
            && their_epoch == our_epoch
            && their_round.saturating_sub(our_round) < maya_dag_bft::GC_DEPTH / 2
    })
    .await
    .unwrap_or(false)
}

/// How long one source may take to answer the tip probe. A refused
/// connection answers at once; a blackholed one would otherwise hold the
/// probe for the HTTP client's whole request timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The `--catch-up-from` sources that answered, highest reported tip first
/// (ADR-043). A single fixed source failed whenever it was the node that was
/// down. The tip is the peer's own claim and only orders the attempts: what
/// is imported is still checked against an attested checkpoint, and a source
/// that lies high and then fails is followed by the next one. Probed
/// concurrently; ties keep the order given.
pub(super) async fn rank_catch_up_sources(urls: &[String]) -> Vec<String> {
    let mut probes = tokio::task::JoinSet::new();
    for (order, url) in urls.iter().enumerate() {
        let Ok(source) = RpcBootstrapSource::new(url, tokio::runtime::Handle::current()) else {
            continue;
        };
        probes.spawn(async move {
            let probe = tokio::task::spawn_blocking(move || source.probe_tip_height().ok());
            let tip = tokio::time::timeout(PROBE_TIMEOUT, probe)
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
            tip.map(|tip| (tip, order))
        });
    }
    let mut answered: Vec<(u64, usize)> = probes.join_all().await.into_iter().flatten().collect();
    answered.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    answered
        .into_iter()
        .map(|(_, order)| urls[order].clone())
        .collect()
}

/// Rounds of probing before a node with no reachable source starts as an
/// ordinary restart, and the pause between them: a peer restarting at the
/// same moment gets a chance to answer.
const PROBE_ROUNDS: u32 = 3;
const PROBE_PAUSE: Duration = Duration::from_secs(5);

/// Catches up from the best source that works (ADR-043): sources are tried
/// in [`rank_catch_up_sources`] order, so one that lies about its tip and
/// then fails costs one attempt. `Ok(None)` when no source answered at all,
/// the cold-boot case; an error only when every answering source failed.
pub(super) async fn catch_up_from_any(
    urls: &[String],
    chain: &Arc<Mutex<Chain>>,
    driver: &mut BftDriver,
) -> Result<Option<String>, Box<dyn Error>> {
    let mut ranked = Vec::new();
    for round in 1..=PROBE_ROUNDS {
        ranked = rank_catch_up_sources(urls).await;
        if !ranked.is_empty() || round == PROBE_ROUNDS {
            break;
        }
        tokio::time::sleep(PROBE_PAUSE).await;
    }
    let mut last_error = None;
    for url in ranked {
        match catch_up(&url, chain, driver).await {
            Ok(_) => return Ok(Some(url)),
            Err(error) => {
                eprintln!("catch-up:    {url} failed: {error}; trying the next source");
                last_error = Some(error);
            }
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

/// Startup catch-up for a node that was down longer than the engine's
/// window (`--catch-up-from`): imports to the peer's checkpoint, then turns
/// the driver into an attested follower so it votes and proposes again.
pub(super) async fn catch_up(
    url: &str,
    chain: &Arc<Mutex<Chain>>,
    driver: &mut BftDriver,
) -> Result<u64, Box<dyn Error>> {
    // Where this node's own engine and DAG log stand, before any import:
    // what `within_engine_window` must measure the network against.
    let (height, tip, nonce) = {
        let c = lock_chain(chain);
        let nonce = c.get(&c.tip()).map_or(0, |b| b.header.nonce);
        (c.height(), c.tip(), nonce)
    };
    // One epoch per round: each import can carry the chain over a boundary,
    // after which the next committee is the one in state.
    let mut imported = 0;
    loop {
        let trusted = BftDriver::staked_committee(&lock_chain(chain))?;
        let (committee, weights) =
            trusted.unwrap_or_else(|| (driver.committee(), driver.weights()));
        let n = catch_up_once(url, chain, &committee, weights.as_ref()).await?;
        imported += n;
        if n == 0 {
            break;
        }
    }
    // Follow attested blocks only when this node's engine cannot derive the
    // gap itself; see `within_engine_window`. Blocks just imported change
    // nothing here: the engine would have derived the same ones, and skips
    // anchors already built.
    if within_engine_window(url, height, tip, nonce).await {
        println!(
            "catch-up:    imported {imported}; within the engine's window of {url}; resuming as a full validator"
        );
        return Ok(imported);
    }
    driver.follow_attested(&lock_chain(chain));
    println!(
        "catch-up:    imported {imported} blocks from {url}; following attested blocks (ADR-038)"
    );
    Ok(imported)
}

/// Keeps an attested follower at the network's tip: every second, imports
/// the blocks up to the newest checkpoint `url` serves. A failed round is
/// reported and retried; the chain is never left half-extended, because
/// each block is inserted whole or not at all.
pub(super) async fn follow_loop(
    url: String,
    chain: Arc<Mutex<Chain>>,
    genesis_committee: Arc<[VerifyingKey]>,
    status: Arc<Mutex<custom_l1_node::rpc::types::BftStatus>>,
) {
    let mut ticker = tokio::time::interval(FOLLOW_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_error: Option<String> = None;
    let mut idle: u32 = 0;
    loop {
        ticker.tick().await;
        // Every second only while following. A builder derives its own
        // blocks, and polling a peer's RPC every second regardless made each
        // node a permanent load on its source (KNOWN_ISSUES 20); it still
        // looks now and then, so a builder that fell behind is overtaken and
        // rescued.
        let following = status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .follower;
        if !following {
            idle += 1;
            if idle < BUILDER_FOLLOW_EVERY {
                continue;
            }
        }
        idle = 0;
        // Re-read each round: the loop outlives the epoch it started in, and a
        // checkpoint is only valid against its own epoch's committee.
        let current = BftDriver::staked_committee(&lock_chain(&chain));
        let (committee, weights) = match current {
            Ok(Some(now)) => now,
            Ok(None) => (Arc::clone(&genesis_committee), None),
            Err(e) => {
                eprintln!("catch-up: reading the committee: {e}");
                continue;
            }
        };
        match catch_up_once(&url, &chain, &committee, weights.as_ref()).await {
            Ok(_) => last_error = None,
            Err(e) => {
                let message = e.to_string();
                // Once per distinct failure, not once a second.
                if last_error.as_deref() != Some(message.as_str()) {
                    eprintln!("catch-up: {message}; retrying");
                    last_error = Some(message);
                }
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Transactions not yet handed to the engine, with when this node first saw
/// each, so a share nobody proposed is picked up after [`SHARE_GRACE`].
#[derive(Default)]
pub(super) struct Feed {
    /// Committee size, which decides each validator's share.
    committee: usize,
    /// Validator registrations bonding less than this are never proposed
    /// (`--min-register-bond`). Node policy, not consensus: ADR-039 option 3,
    /// a stopgap against cheap committee capture while one operator
    /// proposes every block. Zero proposes everything.
    min_register_bond: u64,
    first_seen: BTreeMap<[u8; 32], Instant>,
    /// RPC-submitted transactions already gossiped, so the validator whose
    /// share one is hears of it without waiting out the grace period.
    gossiped: std::collections::BTreeSet<[u8; 32]>,
}

impl Feed {
    pub(super) fn new(committee: usize, min_register_bond: u64) -> Self {
        Self {
            committee,
            min_register_bond,
            ..Self::default()
        }
    }

    /// A validator registration bonding less than the floor.
    fn below_floor(&self, tx: &custom_l1_node::core::Transaction) -> bool {
        use custom_l1_node::core::TxKind;
        use custom_l1_node::core::staking_payload::StakingAction;
        matches!(
            &tx.kind,
            TxKind::Staking(action)
                if matches!(**action, StakingAction::Register { bond, .. } if bond < self.min_register_bond)
        )
    }

    /// Hands pooled transactions to the engine.
    ///
    /// Every validator receives every gossiped transaction, and if every one
    /// proposed all of them each would be ordered `n` times and deduplicated
    /// at build. So each validator proposes its *share* — the transactions
    /// whose id's first byte is its index modulo the committee size —
    /// straight away, and anything else only once it has waited
    /// [`SHARE_GRACE`] without appearing in a block: the owner may be down.
    /// Local policy, not consensus; any split gives the same blocks.
    fn feed(&mut self, driver: &mut BftDriver, pools: &[&Mempool]) {
        let Some(me) = driver.validator_id() else {
            return;
        };
        let now = Instant::now();
        for pool in pools {
            let mut refused = Vec::new();
            for tx in pool.snapshot() {
                let id = tx.txid();
                if self.below_floor(&tx) {
                    refused.push(id);
                    continue;
                }
                if driver.is_queued(&id) {
                    continue;
                }
                let seen = *self.first_seen.entry(id).or_insert(now);
                let mine = usize::from(id[0]) % self.committee.max(1) == usize::from(me);
                if mine || now.duration_since(seen) >= SHARE_GRACE {
                    driver.submit(&tx);
                }
            }
            for id in &refused {
                eprintln!(
                    "bft: not proposing registration {}: bond below --min-register-bond {}",
                    hex::encode(&id[..8]),
                    self.min_register_bond
                );
            }
            pool.remove_all(&refused);
        }
    }

    fn forget(&mut self, included: &[[u8; 32]]) {
        for id in included {
            self.first_seen.remove(id);
            self.gossiped.remove(id);
        }
    }

    /// Transactions that reached this node over RPC and have not been
    /// gossiped yet. The RPC pool is local; without this only this validator
    /// would ever learn of them, and a transaction outside its share would
    /// wait out [`SHARE_GRACE`] (measured: 3.7 s against 0.75 s in-share).
    fn unannounced(&mut self, rpc_pool: &Mempool) -> Vec<custom_l1_node::core::Transaction> {
        rpc_pool
            .snapshot()
            .into_iter()
            .filter(|tx| self.gossiped.insert(tx.txid()))
            .collect()
    }
}

/// Publishes, prunes the pools, and reports what `step` did.
async fn act(step: Step, network: &NodeHandle, pools: &[&Mempool], feed: &mut Feed) {
    for frame in step.frames {
        if let Err(error) = network.publish_bft(frame).await {
            // With no peer in the mesh yet every publish fails; a validator
            // alone re-broadcasts on its tick once peers arrive.
            let message = error.to_string();
            if !message.contains("NoPeersSubscribedToTopic")
                && !message.contains("InsufficientPeers")
            {
                eprintln!("bft: publish failed: {error}");
            }
        }
    }
    // Ordered but not executed: no longer queued, still pooled, so the next
    // feed proposes them again. Forgetting when they were first seen restarts
    // their share grace, which is what spreads a nonce chain across rounds.
    feed.forget(
        &step
            .dropped
            .iter()
            .map(custom_l1_node::Transaction::txid)
            .collect::<Vec<_>>(),
    );
    if !step.included.is_empty() {
        for pool in pools {
            pool.remove_all(&step.included);
        }
        feed.forget(&step.included);
    }
    for id in &step.blocks {
        println!("bft: built block {}", hex::encode(&id[..8]));
    }
    for notice in &step.notices {
        eprintln!("bft: {notice}");
    }
    for evidence in &step.equivocations {
        eprintln!(
            "bft: validator {} equivocated in round {} — evidence held for slashing",
            evidence.first.author, evidence.first.round
        );
    }
}

/// Runs DAG-BFT until the process stops.
/// Consensus frames lost because this loop fell behind the network's event
/// channel. They used to vanish without a word; a validator losing votes is
/// one an operator needs to see, even though peers re-send what matters.
#[derive(Default)]
struct Dropped {
    since_report: u64,
    last_report: Option<std::time::Instant>,
}

impl Dropped {
    fn add(&mut self, lost: u64) {
        self.since_report = self.since_report.saturating_add(lost);
        let now = std::time::Instant::now();
        if self
            .last_report
            .is_none_or(|t| now.duration_since(t) >= DROP_REPORT_INTERVAL)
        {
            eprintln!(
                "bft: fell behind and lost {} consensus frame(s); peers re-send what is still needed",
                self.since_report
            );
            self.since_report = 0;
            self.last_report = Some(now);
        }
    }
}

pub(super) async fn bft_loop(
    chain: Arc<Mutex<Chain>>,
    network: NodeHandle,
    rpc_pool: Mempool,
    mut driver: BftDriver,
    opening: Step,
    mut feed: Feed,
    reporting: Reporting,
) {
    let Reporting {
        metrics,
        checkpoint,
        status,
    } = reporting;
    let mut events = network.subscribe();
    let mut ticker = tokio::time::interval(TICK);
    let gossip_pool = network.mempool().clone();
    let pools = [&gossip_pool, &rpc_pool];
    let mut dropped = Dropped::default();
    act(opening, &network, &pools, &mut feed).await;
    loop {
        let step = tokio::select! {
            event = events.recv() => match event {
                Ok(NodeEvent::BftFrame(frame)) => {
                    let mut guard = lock_chain(&chain);
                    driver.on_frame(&mut guard, now_ms(), &frame)
                }
                Err(RecvError::Lagged(lost)) => {
                    dropped.add(lost);
                    continue;
                }
                Ok(_) => continue,
                Err(RecvError::Closed) => return,
            },
            _ = ticker.tick() => {
                for tx in feed.unannounced(&rpc_pool) {
                    // Best effort: a lone node has nobody to tell.
                    let _ = network.publish_transaction(&tx).await;
                }
                feed.feed(&mut driver, &pools);
                let mut guard = lock_chain(&chain);
                driver.on_tick(&mut guard, now_ms())
            }
        };
        publish_checkpoint(&driver, &checkpoint);
        *status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = driver.status();
        match step {
            Ok(step) => {
                if !step.blocks.is_empty() {
                    metrics.set_height(lock_chain(&chain).height());
                }
                let now = now_ms();
                for anchor in &step.anchor_times_ms {
                    metrics.record_import();
                    // u64 ms to f64 s: exact below 2^53 ms, i.e. ~285,000 years.
                    #[allow(clippy::cast_precision_loss)]
                    metrics.observe_finality(now.saturating_sub(*anchor) as f64 / 1_000.0);
                }
                act(step, &network, &pools, &mut feed).await;
            }
            // A storage fault here means a vote could not be made durable, so
            // it was not sent. Carry on: the next tick retries, and a disk
            // that stays broken is the operator's page, not a reason to sign
            // without a record.
            Err(error) => eprintln!("bft: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Feed, PROBE_TIMEOUT, rank_catch_up_sources};
    use custom_l1_node::core::staking_payload::StakingAction;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A JSON-RPC peer that answers every request with `tip` as its height.
    async fn peer(tip: u64) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = format!(r#"{{"jsonrpc":"2.0","id":0,"result":{tip}}}"#);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        url
    }

    /// A URL nothing listens on.
    async fn dead() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    /// A peer that accepts the connection and never answers (blackholed).
    async fn silent() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        url
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn catch_up_sources_are_empty_when_no_peer_answers() {
        let urls = vec![dead().await, dead().await];
        assert!(rank_catch_up_sources(&urls).await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn catch_up_sources_rank_reachable_peers_by_tip() {
        let (low, high) = (peer(5).await, peer(9).await);
        let urls = vec![dead().await, low.clone(), high.clone()];
        assert_eq!(rank_catch_up_sources(&urls).await, vec![high, low]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_silent_peer_costs_one_probe_timeout_not_the_client_timeout() {
        let live = peer(7).await;
        let urls = vec![silent().await, silent().await, live.clone()];
        let started = std::time::Instant::now();
        assert_eq!(rank_catch_up_sources(&urls).await, vec![live]);
        // Concurrent probes: two silent peers cost one timeout, not two.
        assert!(
            started.elapsed() < PROBE_TIMEOUT * 2,
            "took {:?}",
            started.elapsed()
        );
    }
    use custom_l1_node::core::{Transaction, TxKind};
    use custom_l1_node::crypto::{PUBLIC_KEY_LEN, SIGNATURE_LENGTH};

    fn tx(kind: TxKind) -> Transaction {
        let mut tx = Transaction::new(vec![], vec![], 0);
        tx.kind = kind;
        tx
    }

    fn register(bond: u64) -> Transaction {
        tx(TxKind::Staking(Box::new(StakingAction::Register {
            key: Box::new([0; PUBLIC_KEY_LEN]),
            bond,
            commission_bps: 0,
            possession: Box::new([0; SIGNATURE_LENGTH]),
        })))
    }

    #[test]
    fn only_a_registration_below_the_floor_is_held_back() {
        let feed = Feed::new(1, 1_000_000);
        assert!(feed.below_floor(&register(999_999)));
        assert!(!feed.below_floor(&register(1_000_000)));
        // Other staking actions and transfers pass: the floor is about seats.
        let delegate = tx(TxKind::Staking(Box::new(StakingAction::Delegate {
            validator: [1; 32],
            amount: 1,
        })));
        assert!(!feed.below_floor(&delegate));
        assert!(!feed.below_floor(&tx(TxKind::Transfer)));
    }

    #[test]
    fn a_zero_floor_holds_nothing_back() {
        assert!(!Feed::new(4, 0).below_floor(&register(0)));
    }
}
