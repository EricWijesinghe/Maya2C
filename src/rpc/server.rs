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
use crate::network::Mempool;
use crate::rpc::types::{
    AccountInfo, BlockInfo, HeaderInfo, MiningCandidate, SubmitBlockResult, SubmitTransactionResult,
};

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
}

impl RpcContext {
    /// Builds a context over a chain and mempool.
    #[must_use]
    pub fn new(chain: Arc<Mutex<Chain>>, mempool: Mempool) -> Self {
        Self { chain, mempool }
    }

    /// Recovers the chain guard if a previous holder panicked.
    ///
    /// A poisoned chain lock is not automatically safe to ignore, but the
    /// alternative — refusing every subsequent request — takes the node down
    /// permanently. State itself is protected by RocksDB's atomic batches, so
    /// the worst case is an index that lags committed state.
    fn chain(&self) -> std::sync::MutexGuard<'_, Chain> {
        self.chain
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

            let ids = chain.active_chain().map_err(|e| rejected(e.to_string()))?;
            let id = ids
                .get(height as usize)
                .ok_or_else(|| not_found(format!("no block at height {height}")))?;
            let record = chain.get(id).ok_or_else(|| {
                not_found(format!("block {} missing from index", hex::encode(id)))
            })?;

            Ok::<_, ErrorObjectOwned>(BlockInfo::new(record.height, &record.block))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_mining_candidate", |_params, ctx, _| {
            let chain = ctx.chain();

            let state_root = chain
                .state()
                .state_root()
                .map_err(|e| rejected(e.to_string()))?;

            // Timestamps come from the node clock, which is what a miner would
            // otherwise have to guess and get wrong.
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let header = chain
                .candidate_header(timestamp, state_root)
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
/// Returns [`crate::error::NodeError::Network`] if the address cannot be bound
/// or the module cannot be built.
pub async fn serve(address: SocketAddr, context: RpcContext) -> crate::error::Result<RpcServer> {
    let module =
        build_module(context).map_err(|e| crate::error::NodeError::Network(e.to_string()))?;

    let server = Server::builder()
        .build(address)
        .await
        .map_err(|e| crate::error::NodeError::Network(format!("bind {address}: {e}")))?;

    let local_address = server
        .local_addr()
        .map_err(|e| crate::error::NodeError::Network(format!("local address: {e}")))?;

    let handle = server.start(module);

    Ok(RpcServer {
        address: local_address,
        handle,
    })
}
