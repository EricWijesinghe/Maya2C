//! Reading the chain. The service talks to a node over its public JSON-RPC and
//! never links the node crate; it reads only what the RPC returns.

use async_trait::async_trait;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use primitive_types::U256;
use serde::Deserialize;

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
    /// Transactions in block order.
    pub txs: Vec<TxOutputs>,
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
}

#[derive(Deserialize)]
struct RawChainInfo {
    height: u64,
    chain_id: Option<String>,
}

#[derive(Deserialize)]
struct RawBlock {
    height: u64,
    transactions: Vec<RawTx>,
}

#[derive(Deserialize)]
struct RawTx {
    txid: String,
    outputs: Vec<RawOutput>,
}

#[derive(Deserialize)]
struct RawOutput {
    recipient: String,
    amount: String,
}

/// JSON-RPC client over HTTP.
pub struct RpcChain {
    client: HttpClient,
}

impl RpcChain {
    /// Builds a client for `url`.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Config`] if the URL cannot be used.
    pub fn new(url: &str) -> Result<Self> {
        let client = HttpClientBuilder::default()
            .build(url)
            .map_err(|e| CheckoutError::Config(format!("rpc url: {e}")))?;
        Ok(Self { client })
    }
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
            .map_err(rpc_err)?;
        Ok(Tip { height: info.height, chain_id: info.chain_id })
    }

    async fn blocks(&self, from: u64, count: u64) -> Result<Vec<BlockOutputs>> {
        if count == 0 || count > MAX_RANGE {
            return Err(CheckoutError::Internal(format!("range count {count} out of bounds")));
        }
        let raw: Vec<RawBlock> = self
            .client
            .request("get_blocks_by_height_range", rpc_params![from, count])
            .await
            .map_err(rpc_err)?;
        raw.into_iter().map(convert_block).collect()
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
            Ok(TxOutputs { txid: tx.txid, outputs })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(BlockOutputs { height: block.height, txs })
}

fn decode_recipient(text: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(text).map_err(|e| rpc_err(format!("recipient hex: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| rpc_err("recipient is not 32 bytes"))
}
