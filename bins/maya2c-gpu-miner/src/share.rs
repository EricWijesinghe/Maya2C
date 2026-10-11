//! Share mining against a node's `get_mining_work` (ADR-044).
//!
//! Hashes in light mode on CPU threads: each digest rebuilds its dataset items
//! from the 64 MiB cache, which is what a validator does to verify a share.
//! It is slow by design and it is real; the GPU full-dataset path replaces the
//! hashing loop, not this protocol.
//!
//! With `--json` every line on stdout is one event, for the Hub GUI:
//! `{"event":"work"|"hashrate"|"share"|"idle"|"error", ...}`.

use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use custom_l1_node::core::mining_payload::MiningShare;
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::transaction::{ChainTag, Transaction};
use custom_l1_node::crypto::dag::cache::Cache;
use custom_l1_node::crypto::dag::hashimoto::hashimoto_light;
use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::meets_target;
use custom_l1_node::state::mining::DagProfile;
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

/// How often the hash rate is reported.
const REPORT_EVERY: Duration = Duration::from_secs(2);
/// How often work is re-fetched; a new epoch replaces the header. Shares
/// mined after their epoch closed are refused, so this stays short.
const REFRESH_EVERY: Duration = Duration::from_secs(3);
/// How long to wait while mining has not opened.
const IDLE_WAIT: Duration = Duration::from_secs(15);

/// What the miner was asked to do.
pub struct ShareArgs<'a> {
    pub rpc: &'a str,
    pub key_file: &'a Path,
    pub threads: usize,
    pub json: bool,
}

#[derive(Deserialize)]
struct Work {
    active: bool,
    epoch: u64,
    header: String,
    target: String,
    /// Empty while mining has not opened, so parsed only for active work.
    dag: String,
    dag_epoch: u64,
    share_reward: u64,
}

#[derive(Deserialize)]
struct ChainInfo {
    genesis: String,
}

#[derive(Deserialize)]
struct Account {
    balance: u64,
    nonce: u64,
}

#[derive(Deserialize)]
struct Submitted {
    txid: String,
}

struct Rpc {
    url: String,
    http: reqwest::blocking::Client,
}

/// One miner's connection: who it pays, which chain it signs for, and the
/// next transaction nonce, kept locally so two shares found in one block
/// do not both claim the account's committed nonce.
struct Session {
    rpc: Rpc,
    key: HybridSigningKey,
    chain: ChainTag,
    address: String,
    json: bool,
    next_nonce: Cell<u64>,
}

impl Rpc {
    fn call<T: for<'de> Deserialize<'de>>(&self, method: &str, params: Value) -> Result<T, String> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let reply: Value = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .and_then(reqwest::blocking::Response::json)
            .map_err(|e| format!("{method}: {e}"))?;
        if let Some(error) = reply.get("error") {
            return Err(format!("{method}: {}", error["message"]));
        }
        serde_json::from_value(reply["result"].clone()).map_err(|e| format!("{method}: {e}"))
    }
}

/// Emits one event: a JSON line, or a readable line for a person.
fn emit(json_mode: bool, event: &Value) {
    if json_mode {
        println!("{event}");
    } else {
        println!("{}", event.to_string().replace(['{', '}', '"'], ""));
    }
}

/// Loads the payout key's seed, or creates one. The address it derives is
/// where shares pay; the file is the wallet, so it is never printed.
fn load_or_create_seed(path: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut seed = Zeroizing::new([0u8; 32]);
    if path.exists() {
        let text = Zeroizing::new(std::fs::read_to_string(path).map_err(|e| e.to_string())?);
        hex::decode_to_slice(text.trim(), seed.as_mut())
            .map_err(|_| "key file is not 64 hex characters".to_string())?;
        return Ok(seed);
    }
    getrandom::fill(seed.as_mut()).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, Zeroizing::new(hex::encode(seed.as_ref()))).map_err(|e| e.to_string())?;
    Ok(seed)
}

fn decode32(value: &str, what: &str) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    hex::decode_to_slice(value, &mut out).map_err(|_| format!("{what} is not 32 hex bytes"))?;
    Ok(out)
}

/// Runs until the process is stopped.
///
/// # Errors
///
/// An unreadable key file or an unreachable node at startup.
pub fn run(args: &ShareArgs<'_>) -> Result<(), String> {
    let rpc = Rpc {
        url: args.rpc.to_string(),
        http: reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?,
    };
    let seed = load_or_create_seed(args.key_file)?;
    let key = signing_key_from_seed(&seed).map_err(|e| e.to_string())?;
    let address = hex::encode(key.address());
    let info: ChainInfo = rpc.call("get_chain_info", json!([]))?;
    let chain = ChainTag::from_genesis(decode32(&info.genesis, "genesis")?);
    let session = Session {
        rpc,
        key,
        chain,
        address,
        json: args.json,
        next_nonce: Cell::new(0),
    };
    session.emit(&json!({"event": "start", "address": session.address,
        "threads": args.threads, "mode": "cpu-light"}));

    let mut caches: Option<(DagProfile, u64, Arc<Cache>)> = None;
    loop {
        let work: Work = match session
            .rpc
            .call("get_mining_work", json!([session.address]))
        {
            Ok(work) => work,
            Err(error) => {
                session.emit(&json!({"event": "error", "message": error}));
                thread::sleep(IDLE_WAIT);
                continue;
            }
        };
        if !work.active {
            session.emit(&json!({"event": "idle",
                "reason": "mining opens at the first epoch boundary"}));
            thread::sleep(IDLE_WAIT);
            continue;
        }
        if work.share_reward == 0 {
            session.emit(&json!({"event": "idle", "epoch": work.epoch,
                "reason": "this epoch's miner budget is spent; waiting for the next epoch"}));
            thread::sleep(REFRESH_EVERY);
            continue;
        }
        let dag: DagProfile = serde_json::from_value(json!(work.dag))
            .map_err(|e| format!("get_mining_work dag {:?}: {e}", work.dag))?;
        let cache = cache_for(&mut caches, dag, work.dag_epoch)?;
        session.emit(
            &json!({"event": "work", "epoch": work.epoch, "target": work.target,
            "share_reward": work.share_reward}),
        );
        session.mine_epoch(&work, &cache, args.threads.max(1))?;
    }
}

fn cache_for(
    slot: &mut Option<(DagProfile, u64, Arc<Cache>)>,
    dag: DagProfile,
    epoch: u64,
) -> Result<Arc<Cache>, String> {
    if let Some((d, e, cache)) = slot
        && *d == dag
        && *e == epoch
    {
        return Ok(Arc::clone(cache));
    }
    let registry = CacheRegistry::new(DagConfig {
        params: dag.params(),
        activation_height: 0,
        activation_target: [0xff; 32],
    });
    let cache = registry.cache_for_epoch(epoch).map_err(|e| e.to_string())?;
    *slot = Some((dag, epoch, Arc::clone(&cache)));
    Ok(cache)
}

impl Session {
    fn emit(&self, event: &Value) {
        emit(self.json, event);
    }

    /// Hashes one epoch's header until the node hands out different work.
    fn mine_epoch(&self, work: &Work, cache: &Arc<Cache>, threads: usize) -> Result<(), String> {
        let header = decode32(&work.header, "header")?;
        let target = decode32(&work.target, "target")?;
        let stop = Arc::new(AtomicBool::new(false));
        let hashes = Arc::new(AtomicU64::new(0));
        let (found_tx, found_rx) = mpsc::channel::<u64>();
        let mut start = [0u8; 8];
        getrandom::fill(&mut start).map_err(|e| e.to_string())?;
        let base = u64::from_le_bytes(start);
        let stride = threads as u64;
        // Shares of the closed epoch were evicted from the pool, so nonces
        // handed to them never land; restart from the chain's count or every
        // later share waits behind the gap.
        self.next_nonce.set(0);

        let workers: Vec<_> = (0..stride)
            .map(|lane| {
                let (stop, hashes, found, cache) = (
                    Arc::clone(&stop),
                    Arc::clone(&hashes),
                    found_tx.clone(),
                    Arc::clone(cache),
                );
                thread::spawn(move || {
                    let mut nonce = base.wrapping_add(lane);
                    while !stop.load(Ordering::Relaxed) {
                        let digest = hashimoto_light(&cache, &header, nonce).result;
                        hashes.fetch_add(1, Ordering::Relaxed);
                        if meets_target(&digest, &target) && found.send(nonce).is_err() {
                            return;
                        }
                        nonce = nonce.wrapping_add(stride);
                    }
                })
            })
            .collect();

        self.watch(work, &hashes, &found_rx);
        stop.store(true, Ordering::Relaxed);
        for worker in workers {
            let _ = worker.join();
        }
        Ok(())
    }

    /// Reports the rate, submits found shares, and returns when work changes.
    fn watch(&self, work: &Work, hashes: &AtomicU64, found: &mpsc::Receiver<u64>) {
        let (mut last_report, mut last_refresh) = (Instant::now(), Instant::now());
        let mut counted = 0u64;
        loop {
            if let Ok(nonce) = found.recv_timeout(Duration::from_millis(200))
                && !self.submit(work, nonce)
            {
                // A refusal usually means the epoch closed or its budget is
                // spent: look now rather than hashing stale work.
                last_refresh = Instant::now()
                    .checked_sub(REFRESH_EVERY)
                    .unwrap_or(last_refresh);
            }
            if last_report.elapsed() >= REPORT_EVERY {
                let total = hashes.load(Ordering::Relaxed);
                // Display only: a rate for a person, never a consensus value.
                #[allow(clippy::cast_precision_loss)]
                let rate = (total - counted) as f64 / last_report.elapsed().as_secs_f64();
                self.emit(&json!({"event": "hashrate", "hps": rate, "total": total}));
                (counted, last_report) = (total, Instant::now());
            }
            if last_refresh.elapsed() >= REFRESH_EVERY {
                last_refresh = Instant::now();
                match self
                    .rpc
                    .call::<Work>("get_mining_work", json!([self.address]))
                {
                    // Same header but nothing left to pay: stop hashing too.
                    Ok(next)
                        if next.active && next.header == work.header && next.share_reward > 0 => {}
                    Ok(_) => return,
                    Err(error) => self.emit(&json!({"event": "error", "message": error})),
                }
            }
        }
    }

    /// Signs and sends one share. The node checks the work before the
    /// signature, so a refused share costs it 2 ms; the reason is reported,
    /// never retried. Returns whether the node accepted it.
    fn submit(&self, work: &Work, nonce: u64) -> bool {
        let result = self
            .rpc
            .call::<Account>("get_balance", json!([self.address]))
            .and_then(|account| {
                let tx_nonce = account.nonce.max(self.next_nonce.get());
                let mut tx = Transaction::new(vec![], vec![], tx_nonce);
                tx.kind = TxKind::MiningShare(MiningShare {
                    epoch: work.epoch,
                    nonce,
                });
                tx.sign(&self.key, &self.chain).map_err(|e| e.to_string())?;
                let sent: Submitted = self
                    .rpc
                    .call("send_raw_transaction", json!([hex::encode(tx.to_bytes())]))?;
                self.next_nonce.set(tx_nonce + 1);
                Ok((sent.txid, account.balance))
            });
        let accepted = result.is_ok();
        let event = match result {
            Ok((txid, balance)) => json!({"event": "share", "accepted": true, "txid": txid,
                "reward": work.share_reward, "balance": balance, "epoch": work.epoch}),
            Err(reason) => json!({"event": "share", "accepted": false, "reason": reason,
                "epoch": work.epoch}),
        };
        self.emit(&event);
        accepted
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_new_key_file_is_created_once_and_read_back_the_same() {
        let dir = std::env::temp_dir().join(format!("maya-miner-key-{}", std::process::id()));
        let path = dir.join("miner.key");
        let first = load_or_create_seed(&path).unwrap();
        let again = load_or_create_seed(&path).unwrap();
        assert_eq!(*first, *again);
        assert_ne!(*first, [0u8; 32]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_damaged_key_file_is_refused_not_replaced() {
        let dir = std::env::temp_dir().join(format!("maya-miner-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("miner.key");
        std::fs::write(&path, "not hex").unwrap();
        assert!(load_or_create_seed(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not hex");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
