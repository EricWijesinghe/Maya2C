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
    AccountInfo, BlockInfo, HeaderInfo, IotDeviceInfo, MiningCandidate, PeerAddressInfo,
    SubmitBlockResult, SubmitTransactionResult, ThreatIndicatorInfo,
};
use crate::state_pruner::cold::ColdBlocks;

/// JSON-RPC error code for a malformed argument.
const INVALID_PARAMS: i32 = -32_602;

/// JSON-RPC error code for a request the node refused.
const REJECTED: i32 = -32_000;

/// JSON-RPC error code for an item that does not exist.
const NOT_FOUND: i32 = -32_001;

fn invalid_params(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(INVALID_PARAMS, message.into(), None::<()>)
}

fn rejected(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(REJECTED, message.into(), None::<()>)
}

fn not_found(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(NOT_FOUND, message.into(), None::<()>)
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
/// rpc-latency). Batched calls are passed through unmetered.
///
/// # Errors
///
/// As [`serve`].
pub async fn serve_metered(
    address: SocketAddr,
    context: RpcContext,
    metrics: std::sync::Arc<crate::metrics::Metrics>,
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
    let server = Server::builder()
        .set_rpc_middleware(middleware)
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
