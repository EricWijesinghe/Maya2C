//! Reading the chain. The service talks to a node over its public JSON-RPC and
//! never links the node crate; it reads only what the RPC returns.

use async_trait::async_trait;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use primitive_types::U256;
use serde::Deserialize;

use crate::config::redact_url;
use crate::error::{CheckoutError, Result};

/// The node's cap on `get_blocks_by_height_range` per call.
pub const MAX_RANGE: u64 = 32;

/// The chain as the service sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tip {
    /// Highest block the node has.
    pub height: u64,
    /// Network name, if the node was given one.
    pub chain_id: Option<String>,
}

/// One output of one transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// Recipient address, 32 bytes.
    pub recipient: [u8; 32],
    /// Amount, base units.
    pub amount: U256,
}

/// A transaction and its outputs, in order. Output position is its index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxOutputs {
    /// Hex transaction id.
    pub txid: String,
    /// Outputs in order.
    pub outputs: Vec<Output>,
}

/// A block's transactions, reduced to what matching needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockOutputs {
    /// Block height.
    pub height: u64,
    /// This block's id. Two nodes that agree on the chain agree on this.
    pub id: String,
    /// The parent block's id. Must equal the id stored for `height - 1`, or a
    /// reorg has replaced that block.
    pub prev_id: String,
    /// Transactions in block order.
    pub txs: Vec<TxOutputs>,
}

/// The newest DAG-BFT checkpoint (ADR-038): a quorum-attested block. It and
/// every block below it on its branch are final.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finalized {
    /// Height of the attested block.
    pub height: u64,
    /// Hex id of the attested block, in the same form as [`BlockOutputs::id`].
    pub block_id: String,
}

/// Anything that can report the tip and hand out blocks. The RPC client is one
/// implementation; tests use an in-memory chain.
#[async_trait]
pub trait ChainSource: Send + Sync {
    /// The current tip.
    async fn tip(&self) -> Result<Tip>;
    /// Up to `count` blocks from `from`. A short answer means the node has no
    /// more blocks yet, which is not an error.
    async fn blocks(&self, from: u64, count: u64) -> Result<Vec<BlockOutputs>>;
    /// The newest checkpoint, or `None` when the node has none to serve.
    async fn finalized(&self) -> Result<Option<Finalized>>;
}

#[derive(Deserialize)]
struct RawChainInfo {
    height: u64,
    chain_id: Option<String>,
}

#[derive(Deserialize)]
struct RawBlock {
    height: u64,
    header: RawHeader,
    transactions: Vec<RawTx>,
}

#[derive(Deserialize)]
struct RawHeader {
    id: String,
    prev_hash: String,
}

#[derive(Deserialize)]
struct RawTx {
    txid: String,
    outputs: Vec<RawOutput>,
}

/// What `get_checkpoint` returns. The signatures are not checked here: the
/// service already trusts its node for every block it reads.
#[derive(Deserialize)]
struct RawCheckpoint {
    height: u64,
    block: String,
}

#[derive(Deserialize)]
struct RawOutput {
    recipient: String,
    amount: String,
}

/// JSON-RPC client over HTTP.
pub struct RpcChain {
    client: HttpClient,
    /// The configured URL, which may carry credentials. Kept only to scrub it
    /// out of transport error messages.
    url: String,
}

impl RpcChain {
    /// Builds a client for `url`.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Config`] if the URL cannot be used.
    pub fn new(url: &str) -> Result<Self> {
        let client = HttpClientBuilder::default().build(url).map_err(|e| {
            CheckoutError::Config(format!("rpc url: {}", scrub(&e.to_string(), url)))
        })?;
        Ok(Self {
            client,
            url: url.to_owned(),
        })
    }
}

/// Replaces the configured URL in `text` with its redacted form, so credentials
/// never reach an error message or a log line.
fn scrub(text: &str, url: &str) -> String {
    text.replace(url, &redact_url(url))
}

/// A transport failure. The message is scrubbed, because the HTTP stack can
/// include the full URL in its text.
fn transport_err(e: impl std::fmt::Display, url: &str) -> CheckoutError {
    CheckoutError::Chain(scrub(&e.to_string(), url))
}

fn rpc_err(e: impl std::fmt::Display) -> CheckoutError {
    CheckoutError::Chain(e.to_string())
}

#[async_trait]
impl ChainSource for RpcChain {
    async fn tip(&self) -> Result<Tip> {
        let info: RawChainInfo = self
            .client
            .request("get_chain_info", rpc_params![])
            .await
            .map_err(|e| transport_err(e, &self.url))?;
        Ok(Tip {
            height: info.height,
            chain_id: info.chain_id,
        })
    }

    async fn blocks(&self, from: u64, count: u64) -> Result<Vec<BlockOutputs>> {
        if count == 0 || count > MAX_RANGE {
            return Err(CheckoutError::Internal(format!(
                "range count {count} out of bounds"
            )));
        }
        let raw: Vec<RawBlock> = self
            .client
            .request("get_blocks_by_height_range", rpc_params![from, count])
            .await
            .map_err(|e| transport_err(e, &self.url))?;
        raw.into_iter().map(convert_block).collect()
    }

    async fn finalized(&self) -> Result<Option<Finalized>> {
        let raw: Option<RawCheckpoint> = self
            .client
            .request("get_checkpoint", rpc_params![])
            .await
            .map_err(|e| transport_err(e, &self.url))?;
        Ok(raw.map(|c| Finalized {
            height: c.height,
            block_id: c.block,
        }))
    }
}

/// Turns a node block into matching form. A malformed output is an error for
/// the whole batch: skipping it would be a silent gap in the scan.
fn convert_block(block: RawBlock) -> Result<BlockOutputs> {
    let txs = block
        .transactions
        .into_iter()
        .map(|tx| -> Result<TxOutputs> {
            let outputs = tx
                .outputs
                .into_iter()
                .map(|o| -> Result<Output> {
                    Ok(Output {
                        recipient: decode_recipient(&o.recipient)?,
                        amount: crate::invoice::amount::parse(&o.amount)
                            .map_err(|e| rpc_err(format!("output amount: {e}")))?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(TxOutputs {
                txid: tx.txid,
                outputs,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(BlockOutputs {
        height: block.height,
        id: block.header.id,
        prev_id: block.header.prev_hash,
        txs,
    })
}

fn decode_recipient(text: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(text).map_err(|e| rpc_err(format!("recipient hex: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| rpc_err("recipient is not 32 bytes"))
}
