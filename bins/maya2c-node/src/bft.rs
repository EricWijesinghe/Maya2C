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
use custom_l1_node::consensus::bft::remote::{DEFAULT_DEADLINE, RemoteSigner, ValidatorKey};
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::crypto::keys::{self, SigningKey, VerifyingKey};
use custom_l1_node::genesis::BftGenesis;
use custom_l1_node::metrics::Metrics;
use custom_l1_node::network::{Mempool, NodeEvent, NodeHandle};
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Transactions not yet handed to the engine, with when this node first saw
/// each, so a share nobody proposed is picked up after [`SHARE_GRACE`].
#[derive(Default)]
struct Feed {
    first_seen: BTreeMap<[u8; 32], Instant>,
    /// RPC-submitted transactions already gossiped, so the validator whose
    /// share one is hears of it without waiting out the grace period.
    gossiped: std::collections::BTreeSet<[u8; 32]>,
}

impl Feed {
    /// Hands pooled transactions to the engine.
    ///
    /// Every validator receives every gossiped transaction, and if every one
    /// proposed all of them each would be ordered `n` times and deduplicated
    /// at build. So each validator proposes its *share* — the transactions
    /// whose id's first byte is its index modulo the committee size —
    /// straight away, and anything else only once it has waited
    /// [`SHARE_GRACE`] without appearing in a block: the owner may be down.
    /// Local policy, not consensus; any split gives the same blocks.
    fn feed(&mut self, driver: &mut BftDriver, pools: &[&Mempool], committee: usize) {
        let Some(me) = driver.validator_id() else {
            return;
        };
        let now = Instant::now();
        for pool in pools {
            for tx in pool.snapshot() {
                let id = tx.txid();
                if driver.is_queued(&id) {
                    continue;
                }
                let seen = *self.first_seen.entry(id).or_insert(now);
                let mine = usize::from(id[0]) % committee.max(1) == usize::from(me);
                if mine || now.duration_since(seen) >= SHARE_GRACE {
                    driver.submit(&tx);
                }
            }
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
    for evidence in &step.equivocations {
        eprintln!(
            "bft: validator {} equivocated in round {} — evidence held for slashing",
            evidence.first.author, evidence.first.round
        );
    }
}

/// Runs DAG-BFT until the process stops.
pub(super) async fn bft_loop(
    chain: Arc<Mutex<Chain>>,
    network: NodeHandle,
    rpc_pool: Mempool,
    mut driver: BftDriver,
    opening: Step,
    committee: usize,
    metrics: Arc<Metrics>,
) {
    let mut events = network.subscribe();
    let mut ticker = tokio::time::interval(TICK);
    let gossip_pool = network.mempool().clone();
    let pools = [&gossip_pool, &rpc_pool];
    let mut feed = Feed::default();
    act(opening, &network, &pools, &mut feed).await;
    loop {
        let step = tokio::select! {
            event = events.recv() => match event {
                Ok(NodeEvent::BftFrame(frame)) => {
                    let mut guard = lock_chain(&chain);
                    driver.on_frame(&mut guard, now_ms(), &frame)
                }
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            },
            _ = ticker.tick() => {
                for tx in feed.unannounced(&rpc_pool) {
                    // Best effort: a lone node has nobody to tell.
                    let _ = network.publish_transaction(&tx).await;
                }
                feed.feed(&mut driver, &pools, committee);
                let mut guard = lock_chain(&chain);
                driver.on_tick(&mut guard, now_ms())
            }
        };
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
