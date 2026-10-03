//! JSON-RPC server.
//!
//! Handlers are registered as synchronous methods. Every one of them is a short
//! RocksDB read or an in-memory index lookup — microseconds — so dispatching
//! them to a blocking pool would cost more in scheduling than it saves. The one
//! genuine exception is `submit_block`, which verifies proof of work: a 32 MiB
//! Argon2id pass that would stall the reactor, so it runs on a blocking thread.
//!
//! The chain sits behind a `Mutex` because fork choice mutates it and the reorg
//! path must be serialized — two concurrent reorgs would interleave undo
//! journals and corrupt state.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use jsonrpsee::server::{RpcModule, Server, ServerHandle};
use jsonrpsee::types::ErrorObjectOwned;

use crate::consensus::{Chain, InsertOutcome};
use crate::core::{Block, Transaction};
use crate::network::{Mempool, NodeHandle};
use crate::rpc::bootstrap::SnapshotService;
use crate::rpc::types::{
    AccountInfo, BlockInfo, ChainInfo, HeaderInfo, IotDeviceInfo, MiningCandidate, PeerAddressInfo,
    SubmitBlockResult, SubmitTransactionResult, ThreatIndicatorInfo,
};
use crate::state_pruner::cold::ColdBlocks;

/// JSON-RPC error code for a malformed argument.
const INVALID_PARAMS: i32 = -32_602;

/// JSON-RPC error code for a request the node refused.
const REJECTED: i32 = -32_000;

/// JSON-RPC error code for an item that does not exist.
const NOT_FOUND: i32 = -32_001;

/// How far back `get_balance_at_height` walks: the retention depth. Beyond it
/// the per-block changes are pruned anyway, so a deeper bound would only let
/// one call walk further before failing.
const MAX_BALANCE_REWIND: u64 = crate::state_pruner::PRUNE_DEPTH;

fn invalid_params(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(INVALID_PARAMS, message.into(), None::<()>)
}

fn rejected(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(REJECTED, message.into(), None::<()>)
}

fn not_found(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(NOT_FOUND, message.into(), None::<()>)
}

/// JSON-RPC error code for a fault inside the node.
const INTERNAL: i32 = -32_603;

/// A storage fault: the detail goes to the operator's log, the caller gets a
/// fixed message and a code that is not `REJECTED`. Reusing `REJECTED` told
/// clients (and the gateway, which maps it to a non-retryable 400) that a
/// corrupt or unreadable record was their mistake.
#[allow(clippy::needless_pass_by_value)] // used as `.map_err(internal)`
fn internal(error: crate::error::NodeError) -> ErrorObjectOwned {
    eprintln!("rpc: internal error: {error}");
    ErrorObjectOwned::owned(INTERNAL, "internal node error", None::<()>)
}

/// `address`'s balance after the block at `height`, and that block's id: the
/// current balance with each later block's recorded change undone.
///
/// The walk runs **outside** the chain lock. Block production takes the same
/// mutex, and holding it for up to `MAX_BALANCE_REWIND` reads would stall
/// fork choice for every caller. The tip, its id and the current balance are
/// read under the lock; afterwards the lock is taken again only to confirm
/// that tip is still canonical at its height — any reorg at or below it would
/// have replaced it, and the answer is refused rather than served torn. Not
/// caught: a reorg away and back to the same tip within one walk. A DAG-BFT
/// chain does not reorg committed blocks at all.
fn balance_at_height(
    ctx: &RpcContext,
    address: &[u8; 32],
    height: u64,
) -> Result<(u64, [u8; 32]), ErrorObjectOwned> {
    let (state, tip, tip_id, mut balance) = {
        let chain = ctx.chain();
        let balance = chain
            .state()
            .get_account(address)
            .map_err(internal)?
            .balance;
        (
            Arc::clone(chain.state()),
            chain.height(),
            chain.tip(),
            balance,
        )
    };
    if height > tip {
        return Err(not_found(format!("height {height} is above the tip {tip}")));
    }
    if tip - height > MAX_BALANCE_REWIND {
        return Err(invalid_params(format!(
            "more than {MAX_BALANCE_REWIND} blocks back from the tip"
        )));
    }
    let canonical = |h: u64| {
        state
            .canonical_id(h)
            .map_err(internal)?
            .ok_or_else(|| not_found(format!("no block at height {h}")))
    };
    for h in (height + 1..=tip).rev() {
        let changes = state
            .balance_changes(&canonical(h)?)
            .map_err(internal)?
            .ok_or_else(|| not_found(format!("changes for height {h} are not kept")))?;
        balance = crate::state::balance_changes::before_block(&changes, address, balance);
    }
    let id = canonical(height)?;
    let still = ctx.chain().state().canonical_id(tip).map_err(internal)?;
    if still != Some(tip_id) {
        return Err(rejected("the chain reorganized during the lookup; retry"));
    }
    Ok((balance, id))
}

/// Decodes a hex string into a fixed-size array.
fn decode_array<const N: usize>(value: &str, what: &str) -> Result<[u8; N], ErrorObjectOwned> {
    let bytes = hex::decode(value.trim_start_matches("0x"))
        .map_err(|e| invalid_params(format!("{what} is not valid hex: {e}")))?;
    <[u8; N]>::try_from(bytes.as_slice())
        .map_err(|_| invalid_params(format!("{what} must be {N} bytes, got {}", bytes.len())))
}

/// Shared state the RPC handlers read and mutate.
#[derive(Clone)]
pub struct RpcContext {
    /// Block index and fork choice.
    pub chain: Arc<Mutex<Chain>>,
    /// Pending transaction pool.
    pub mempool: Mempool,
    /// Snapshots this node serves to pruned nodes bootstrapping from it.
    pub snapshots: Option<Arc<SnapshotService>>,
    /// Where pruned bodies are fetched back from for `get_block_by_height`.
    pub cold: Option<Arc<ColdBlocks>>,
    /// The running node, for `threat_peer_addresses`. Absent unless the
    /// operator wires it: peer addresses are this node's private knowledge,
    /// so bind an RPC that carries them to a local interface.
    pub peers: Option<NodeHandle>,
    /// Whether `submit_block` accepts blocks. False on a DAG-BFT network,
    /// where a block is derived from certificates and a submitted one is only
    /// somebody's claim (`consensus::bft`); the chain there verifies no work,
    /// so accepting one would let anyone write the tip.
    pub accepts_blocks: bool,
    /// The network's name (`maya-testnet-1`), for `get_chain_info`. A label
    /// for people: what a signature binds to is the genesis id (ADR-036).
    pub network: Option<String>,
    /// The newest attested checkpoint, kept current by the DAG-BFT loop, for
    /// `get_checkpoint` (ADR-038). Absent on a node that runs no DAG-BFT.
    pub checkpoint: Option<Arc<Mutex<Option<crate::consensus::bft::attest::Checkpoint>>>>,
}

impl RpcContext {
    /// Builds a context over a chain and mempool, serving no snapshots and
    /// no pruned blocks.
    #[must_use]
    pub fn new(chain: Arc<Mutex<Chain>>, mempool: Mempool) -> Self {
        Self {
            chain,
            mempool,
            snapshots: None,
            cold: None,
            peers: None,
            accepts_blocks: true,
            network: None,
            checkpoint: None,
        }
    }

    /// The same context, serving `slot` from `get_checkpoint`.
    #[must_use]
    pub fn with_checkpoints(
        self,
        slot: Arc<Mutex<Option<crate::consensus::bft::attest::Checkpoint>>>,
    ) -> Self {
        Self {
            checkpoint: Some(slot),
            ..self
        }
    }

    /// The same context, naming its network in `get_chain_info`.
    #[must_use]
    pub fn with_network(self, name: impl Into<String>) -> Self {
        Self {
            network: Some(name.into()),
            ..self
        }
    }

    /// The same context, serving snapshots from `service`.
    #[must_use]
    pub fn with_snapshots(self, service: Arc<SnapshotService>) -> Self {
        Self {
            snapshots: Some(service),
            ..self
        }
    }

    /// The same context, fetching pruned blocks back through `cold`.
    #[must_use]
    pub fn with_cold_blocks(self, cold: Arc<ColdBlocks>) -> Self {
        Self {
            cold: Some(cold),
            ..self
        }
    }

    /// The same context, refusing `submit_block` (a DAG-BFT node).
    #[must_use]
    pub fn refusing_blocks(self) -> Self {
        Self {
            accepts_blocks: false,
            ..self
        }
    }

    /// The same context, answering `threat_peer_addresses` from `node`.
    #[must_use]
    pub fn with_peers(self, node: NodeHandle) -> Self {
        Self {
            peers: Some(node),
            ..self
        }
    }

    /// Recovers the chain guard if a previous holder panicked.
    ///
    /// A poisoned chain lock is not automatically safe to ignore, but the
    /// alternative — refusing every subsequent request — takes the node down
    /// permanently. State itself is protected by RocksDB's atomic batches, so
    /// the worst case is an index that lags committed state.
    pub(crate) fn chain(&self) -> std::sync::MutexGuard<'_, Chain> {
        self.chain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Builds the RPC module with every method registered.
///
/// # Errors
///
/// Returns an error if two methods are registered under the same name.
pub fn build_module(context: RpcContext) -> Result<RpcModule<RpcContext>, ErrorObjectOwned> {
    let mut module = RpcModule::new(context);

    module
        .register_method("get_balance", |params, ctx, _| {
            let address_hex: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let address = decode_array::<32>(&address_hex, "address")?;

            let account = ctx
                .chain()
                .state()
                .get_account(&address)
                .map_err(|e| rejected(e.to_string()))?;

            Ok::<_, ErrorObjectOwned>(AccountInfo::new(&address, &account))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_checkpoint", |_params, ctx, _| {
            let held = ctx.checkpoint.as_ref().and_then(|slot| {
                slot.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .map(crate::rpc::types::CheckpointInfo::from)
            });
            Ok::<_, ErrorObjectOwned>(held)
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_chain_info", |_params, ctx, _| {
            let chain = ctx.chain();
            let genesis = chain.genesis();
            Ok::<_, ErrorObjectOwned>(ChainInfo {
                genesis: hex::encode(genesis),
                chain_id: ctx.network.clone(),
            })
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_balance_at_height", |params, ctx, _| {
            let (address_hex, height): (String, u64) =
                params.parse().map_err(|e| invalid_params(e.to_string()))?;
            let address = decode_array::<32>(&address_hex, "address")?;
            let (balance, id) = balance_at_height(ctx, &address, height)?;
            Ok::<_, ErrorObjectOwned>(serde_json::json!({
                "address": address_hex,
                "balance": balance,
                "height": height,
                "block_id": hex::encode(id),
            }))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("vault_get", |params, ctx, _| {
            let address_hex: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let address = decode_array::<32>(&address_hex, "address")?;
            let chain = ctx.chain();
            let state = chain.state();
            let Some(vault) = state.committed_vault(&address).map_err(internal)? else {
                return Ok::<_, ErrorObjectOwned>(serde_json::Value::Null);
            };
            let height = chain.height();
            let config = vault.effective(height);
            let requests: Vec<serde_json::Value> = state
                .committed_withdrawals(&address)
                .map_err(internal)?
                .iter()
                .map(|(id, w)| {
                    serde_json::json!({
                        "id": id, "to": hex::encode(w.to), "amount": w.amount,
                        "ready_height": w.ready_height,
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "delay_blocks": config.delay_blocks,
                "limit": config.limit,
                "guardians": config.guardians.iter().map(hex::encode).collect::<Vec<_>>(),
                "pending_reconfiguration_height": vault.pending.as_ref().map(|(_, ready)| *ready),
                "open_requests": vault.open,
                "requests": requests,
            }))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_account_at_tip", |params, ctx, _| {
            let address_hex: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let address = decode_array::<32>(&address_hex, "address")?;
            // One guard for the tip and the read: blocks commit under it.
            let chain = ctx.chain();
            let account = chain.state().get_account(&address).map_err(internal)?;
            Ok::<_, ErrorObjectOwned>(crate::rpc::types::AccountAtTip {
                account: AccountInfo::new(&address, &account),
                height: chain.height(),
                block_id: hex::encode(chain.tip()),
            })
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_balance_changes", |params, ctx, _| {
            let height: u64 = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let chain = ctx.chain();
            let state = chain.state();
            let id = state
                .canonical_id(height)
                .map_err(internal)?
                .ok_or_else(|| not_found(format!("no block at height {height}")))?;
            let changes = state
                .balance_changes(&id)
                .map_err(internal)?
                .ok_or_else(|| {
                    not_found(format!(
                        "no balance changes kept for height {height} (genesis or pruned)"
                    ))
                })?;
            Ok::<_, ErrorObjectOwned>(crate::rpc::types::BalanceChangesInfo {
                height,
                block_id: hex::encode(id),
                changes: changes
                    .iter()
                    .map(|c| crate::rpc::types::BalanceChangeInfo {
                        address: hex::encode(c.address),
                        before: c.before,
                        after: c.after,
                    })
                    .collect(),
            })
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_fee_info", |_params, ctx, _| {
            let fees = ctx
                .chain()
                .state()
                .committed_fees()
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(crate::rpc::types::FeeInfo {
                active: fees.is_some(),
                base_fee: fees.map_or(0, |f| f.base_fee),
                collector: hex::encode(crate::state::fees::FEE_COLLECTOR),
            })
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("send_raw_transaction", |params, ctx, _| {
            let raw: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let bytes = hex::decode(raw.trim_start_matches("0x"))
                .map_err(|e| invalid_params(format!("transaction is not valid hex: {e}")))?;

            let tx = Transaction::from_bytes(&bytes)
                .map_err(|e| invalid_params(format!("malformed transaction: {e}")))?;
            let txid = hex::encode(tx.txid());

            // The mempool re-validates against committed state: signature,
            // nonce, and balance. A transaction that fails here never enters
            // the pool and is never relayed.
            let accepted = ctx
                .mempool
                .insert(tx)
                .map_err(|e| rejected(e.to_string()))?;

            Ok::<_, ErrorObjectOwned>(SubmitTransactionResult { txid, accepted })
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_block_by_height", |params, ctx, _| {
            let height: u64 = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let chain = ctx.chain();
            let state = chain.state();

            // The canonical index, not a walk from the tip: one read instead of
            // one per block of history.
            let id = state
                .canonical_id(height)
                .map_err(|e| rejected(e.to_string()))?
                .ok_or_else(|| not_found(format!("no block at height {height}")))?;
            let block = match state.load_block(&id).map_err(|e| rejected(e.to_string()))? {
                Some(block) => block,
                // Pruned here: fetched back from the archive and verified
                // against the header this node kept, or refused.
                None => ctx
                    .cold
                    .as_ref()
                    .ok_or_else(|| not_found(format!("height {height} is pruned on this node")))?
                    .fetch(state, height)
                    .map_err(|e| {
                        // The detail names stores and CIDs: the operator's log,
                        // not a public caller's business.
                        eprintln!("rpc: cold fetch of height {height} failed: {e}");
                        not_found(format!(
                            "height {height} is pruned and no archived copy verified"
                        ))
                    })?,
            };

            Ok::<_, ErrorObjectOwned>(BlockInfo::new(height, &block))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_mining_candidate", |_params, ctx, _| {
            let chain = ctx.chain();

            // Timestamps come from the node clock, which is what a miner would
            // otherwise have to guess and get wrong.
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let header = chain
                .candidate_header(timestamp)
                .map_err(|e| rejected(e.to_string()))?;

            Ok::<_, ErrorObjectOwned>(MiningCandidate {
                height: chain.height() + 1,
                difficulty_target: hex::encode(header.difficulty_target),
                header_bytes: hex::encode(header.serialize()),
                header: HeaderInfo::from(&header),
            })
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_supply", |_params, ctx, _| {
            crate::rpc::market::supply(&ctx.chain).map_err(|e| rejected(e.to_string()))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_async_method("submit_block", |params, ctx, _| async move {
            if !ctx.accepts_blocks {
                return Err(rejected(
                    "this network is ordered by DAG-BFT; blocks are derived from                      certificates, not submitted"
                        .to_string(),
                ));
            }
            let raw: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let bytes = hex::decode(raw.trim_start_matches("0x"))
                .map_err(|e| invalid_params(format!("block is not valid hex: {e}")))?;
            let block = Block::from_bytes(&bytes)
                .map_err(|e| invalid_params(format!("malformed block: {e}")))?;

            // Proof-of-work verification is a 32 MiB Argon2id pass. Running it
            // inline would block the reactor for tens of milliseconds and stall
            // every other connection.
            let context = ctx.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut chain = context.chain();
                let outcome = chain.insert_block(block)?;
                Ok::<_, crate::error::NodeError>((outcome, chain.tip(), chain.height()))
            })
            .await
            .map_err(|e| rejected(format!("submission task failed: {e}")))?;

            let (outcome, tip, height) = result.map_err(|e| rejected(e.to_string()))?;

            let label = match outcome {
                InsertOutcome::Extended { .. } => "extended",
                InsertOutcome::Reorganized { .. } => "reorganized",
                InsertOutcome::SideBranch { .. } => "side_branch",
                InsertOutcome::Duplicate { .. } => "duplicate",
            };

            Ok::<_, ErrorObjectOwned>(SubmitBlockResult {
                outcome: label.to_string(),
                tip: hex::encode(tip),
                height,
            })
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("htlc_get_lock", |params, ctx, _| {
            let id_hex: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let lock_id = decode_array::<32>(&id_hex, "lock_id")?;
            let record = ctx
                .chain()
                .state()
                .stored_htlc_lock(&lock_id)
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(
                record.map(|record| crate::rpc::types::HtlcLockInfo::new(&lock_id, &record)),
            )
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("stateless_transaction_witness", |params, ctx, _| {
            // Research branch: refused until stateless accounts are active,
            // which is never on any network today. The witness is against the
            // current tip and goes stale with the next block.
            let raw: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let bytes = hex::decode(raw.trim_start_matches("0x"))
                .map_err(|e| invalid_params(format!("transaction is not valid hex: {e}")))?;
            let tx = Transaction::from_bytes(&bytes)
                .map_err(|e| invalid_params(format!("malformed transaction: {e}")))?;
            let witness = ctx
                .chain()
                .state()
                .transaction_witness(&tx)
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(hex::encode(witness.encode()))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("threat_indicators", |_params, ctx, _| {
            // Every indicator, lifted ones included, judged at the tip.
            let chain = ctx.chain();
            let height = chain.height();
            let indicators = chain
                .state()
                .stored_threat_indicators()
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(
                indicators
                    .iter()
                    .map(|(author, indicator)| ThreatIndicatorInfo::new(author, indicator, height))
                    .collect::<Vec<_>>(),
            )
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_async_method("threat_peer_addresses", |_params, ctx, _| async move {
            let Some(node) = ctx.peers.clone() else {
                return Err(rejected("this node does not serve peer addresses"));
            };
            let addresses = node
                .peer_addresses()
                .await
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(
                addresses
                    .into_iter()
                    .map(|(peer, ip)| PeerAddressInfo {
                        peer_id: peer.to_string(),
                        author: maya_threat_intel::author_of_peer_id(&peer.to_bytes())
                            .map(hex::encode),
                        ip: ip.to_string(),
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("iot_device", |params, ctx, _| {
            // Status and the latest batch, judged at the tip. Never readings:
            // those stay off chain.
            let id_hex: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let device = decode_array::<32>(&id_hex, "device")?;
            let chain = ctx.chain();
            let height = chain.height();
            let record = chain
                .state()
                .stored_iot_device(&device)
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(
                record.map(|record| IotDeviceInfo::new(&device, &record, height)),
            )
        })
        .map_err(|e| rejected(e.to_string()))?;

    crate::rpc::bootstrap::register(&mut module)?;

    Ok(module)
}

/// A running RPC server.
pub struct RpcServer {
    /// Address the server is bound to. Resolved, so a `:0` request reports the
    /// port actually assigned.
    pub address: SocketAddr,
    /// Handle keeping the server alive; dropping it stops the server.
    pub handle: ServerHandle,
}

/// Starts the JSON-RPC server on `address`.
///
/// Pass port `0` to let the OS assign one and read it back from
/// [`RpcServer::address`].
///
/// # Errors
///
/// Returns [`crate::error::NodeError::Network`] if the address cannot be bound,
/// the module cannot be built, or a context wired with
/// [`RpcContext::with_peers`] is asked to listen beyond loopback.
pub async fn serve(address: SocketAddr, context: RpcContext) -> crate::error::Result<RpcServer> {
    check_peer_exposure(address, context.peers.is_some())?;
    let module =
        build_module(context).map_err(|e| crate::error::NodeError::Network(e.to_string()))?;

    let server = Server::builder()
        .build(address)
        .await
        .map_err(|e| crate::error::NodeError::Network(format!("bind {address}: {e}")))?;
    let local_address = server
        .local_addr()
        .map_err(|e| crate::error::NodeError::Network(format!("local address: {e}")))?;
    Ok(RpcServer {
        address: local_address,
        handle: server.start(module),
    })
}

/// [`serve`], recording every call in `metrics` (SLO rpc-availability and
/// rpc-latency) and refusing callers over `limiter`'s per-address rate with
/// HTTP 429. Batched calls are passed through unmetered, and a batch costs
/// one token.
///
/// The limiter needs each caller's address, which jsonrpsee's own server
/// keeps to itself, so this runs the accept loop and hands every connection
/// to jsonrpsee's service. Until it did, `rpc.rate_limit_per_second` was read
/// from the config, printed at startup, and enforced nowhere.
///
/// # Errors
///
/// As [`serve`].
pub async fn serve_metered(
    address: SocketAddr,
    context: RpcContext,
    metrics: std::sync::Arc<crate::metrics::Metrics>,
    limiter: Arc<crate::rpc::limit::RateLimiter>,
) -> crate::error::Result<RpcServer> {
    check_peer_exposure(address, context.peers.is_some())?;
    let module =
        build_module(context).map_err(|e| crate::error::NodeError::Network(e.to_string()))?;
    let known: std::sync::Arc<std::collections::BTreeSet<String>> =
        std::sync::Arc::new(module.method_names().map(str::to_string).collect());
    let middleware =
        jsonrpsee::server::middleware::rpc::RpcServiceBuilder::new().layer_fn(move |service| {
            crate::rpc::metered::Metered::new(
                service,
                std::sync::Arc::clone(&metrics),
                std::sync::Arc::clone(&known),
            )
        });
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|e| crate::error::NodeError::Network(format!("bind {address}: {e}")))?;
    let local_address = listener
        .local_addr()
        .map_err(|e| crate::error::NodeError::Network(format!("local address: {e}")))?;
    let builder = Server::builder()
        .set_rpc_middleware(middleware)
        .to_service_builder();
    let methods: jsonrpsee::server::Methods = module.into();
    let (stop, handle) = jsonrpsee::server::stop_channel();

    tokio::spawn(async move {
        loop {
            let (socket, remote) = tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok(pair) => pair,
                    Err(error) => {
                        eprintln!("rpc: accept failed: {error}");
                        continue;
                    }
                },
                () = stop.clone().shutdown() => break,
            };
            let (builder, methods, stop2, limiter) = (
                builder.clone(),
                methods.clone(),
                stop.clone(),
                Arc::clone(&limiter),
            );
            let service = tower::service_fn(move |request| {
                let allowed = limiter.check(remote.ip());
                let mut inner = builder.clone().build(methods.clone(), stop2.clone());
                async move {
                    if !allowed {
                        return Ok(jsonrpsee::server::http::response::too_many_requests());
                    }
                    tower::Service::call(&mut inner, request).await
                }
            });
            tokio::spawn(jsonrpsee::server::serve_with_graceful_shutdown(
                socket,
                service,
                stop.clone().shutdown(),
            ));
        }
    });

    Ok(RpcServer {
        address: local_address,
        handle,
    })
}

/// Refuses to serve `threat_peer_addresses` anywhere but this host.
///
/// The method lists where peers connect from, and it shares a module with the
/// public methods. A comment saying "bind it locally" is a promise; this is the
/// check. `build_module` stays usable for in-process tests, which bind nothing.
fn check_peer_exposure(address: SocketAddr, serves_peers: bool) -> crate::error::Result<()> {
    if serves_peers && !address.ip().is_loopback() {
        return Err(crate::error::NodeError::Network(format!(
            "refusing to serve threat_peer_addresses on {address}: an RPC wired with peer \
             addresses must bind a loopback address"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn peer_addresses_are_served_on_loopback_only() {
        let public: SocketAddr = "0.0.0.0:8545".parse().expect("address");
        let local: SocketAddr = "127.0.0.1:8545".parse().expect("address");
        let local_v6: SocketAddr = "[::1]:8545".parse().expect("address");
        assert!(check_peer_exposure(public, true).is_err());
        assert!(check_peer_exposure(local, true).is_ok());
        assert!(check_peer_exposure(local_v6, true).is_ok());
        assert!(check_peer_exposure(public, false).is_ok());
    }
}
