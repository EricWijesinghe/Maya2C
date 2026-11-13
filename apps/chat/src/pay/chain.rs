//! Finding a payment on the chain, final (ADR-088, PR #441 review M2).
//!
//! Final means at or below the newest quorum-attested checkpoint the node
//! serves from `get_checkpoint` (ADR-038), the same signal the explorer's
//! indexer reads. Blocks come from `get_block_by_height`, which the public
//! API gateway allows. The node must answer `get_chain_info` with the genesis
//! id the request was signed for, so a node on another chain cannot settle
//! anything.
//!
//! The node is trusted for what it reports: the checkpoint's validator
//! signatures are not checked here. Point the RPC at a node you run, or one
//! you trust.

use std::future::Future;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{ChainTag, ChainTx, FinalTx, MAX_SCAN_BLOCKS};
use crate::{Address, ChatError};

/// One JSON-RPC call to a node.
pub trait ChainRpc {
    /// Calls `method` with `params` and returns the `result` member.
    ///
    /// # Errors
    ///
    /// [`ChatError::Network`] for a transport failure or an RPC error.
    fn call(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, ChatError>> + Send;
}

/// [`ChainRpc`] over HTTP(S) JSON-RPC.
#[derive(Clone, Debug)]
pub struct HttpRpc {
    client: reqwest::Client,
    url: String,
}

impl HttpRpc {
    /// A client for the node at `url`.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.into(),
        }
    }
}

fn net(e: impl std::fmt::Display) -> ChatError {
    ChatError::Network(e.to_string())
}

impl ChainRpc for HttpRpc {
    async fn call(&self, method: &str, params: Value) -> Result<Value, ChatError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(net)?;
        let text = response.text().await.map_err(net)?;
        let mut answer: Value = serde_json::from_str(&text).map_err(net)?;
        if let Some(error) = answer.get("error") {
            return Err(net(error));
        }
        Ok(answer
            .get_mut("result")
            .map(Value::take)
            .unwrap_or(Value::Null))
    }
}

/// Where a search for a transaction ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scan {
    /// Found in a finalized block.
    Found(FinalTx),
    /// Not in any finalized block up to `next - 1`. Search again from `next`
    /// later: the chain may not have finalized the payment yet.
    NotYet {
        /// The first height not yet searched.
        next: u64,
    },
}

#[derive(Deserialize)]
struct ChainInfo {
    genesis: String,
}

#[derive(Deserialize)]
struct Checkpoint {
    height: u64,
}

#[derive(Deserialize)]
struct Block {
    height: u64,
    transactions: Vec<TxInfo>,
}

#[derive(Deserialize)]
struct TxInfo {
    txid: String,
    sender: String,
    outputs: Vec<OutputInfo>,
}

#[derive(Deserialize)]
struct OutputInfo {
    recipient: String,
    amount: String,
}

fn hex32(text: &str) -> Result<[u8; 32], ChatError> {
    let bytes = hex::decode(text).map_err(|_| net("node sent a malformed id"))?;
    <[u8; 32]>::try_from(bytes).map_err(|_| net("node sent an id that is not 32 bytes"))
}

fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ChatError> {
    serde_json::from_value(value).map_err(net)
}

/// Searches finalized blocks from height `from` for transaction `tx_id`, on
/// `chain`, reading at most [`MAX_SCAN_BLOCKS`] blocks. A payee starts at one
/// above the height recorded when it issued the request
/// ([`crate::client::Client::issued_height`]).
///
/// # Errors
///
/// [`ChatError::Refused`] if the node is on another chain;
/// [`ChatError::Network`] for an RPC failure or a malformed answer.
pub async fn find_final<R: ChainRpc>(
    rpc: &R,
    chain: &ChainTag,
    tx_id: &[u8; 32],
    from: u64,
) -> Result<Scan, ChatError> {
    let info: ChainInfo = decode(rpc.call("get_chain_info", json!([])).await?)?;
    if hex32(&info.genesis)? != *chain {
        return Err(ChatError::Refused("node is on another chain"));
    }
    let checkpoint = rpc.call("get_checkpoint", json!([])).await?;
    if checkpoint.is_null() {
        // No checkpoint yet: nothing is final.
        return Ok(Scan::NotYet { next: from });
    }
    let finalized = decode::<Checkpoint>(checkpoint)?.height;
    let last = finalized.min(from.saturating_add(MAX_SCAN_BLOCKS - 1));
    for height in from..=last {
        let block: Block = decode(rpc.call("get_block_by_height", json!([height])).await?)?;
        if block.height != height {
            return Err(net("node answered for another height"));
        }
        if let Some(tx) = find_in(&block, tx_id)? {
            return Ok(Scan::Found(FinalTx::new(height, finalized, tx)));
        }
    }
    Ok(Scan::NotYet {
        next: last.saturating_add(1).max(from),
    })
}

fn find_in(block: &Block, tx_id: &[u8; 32]) -> Result<Option<ChainTx>, ChatError> {
    let want = hex::encode(tx_id);
    let Some(info) = block.transactions.iter().find(|t| t.txid == want) else {
        return Ok(None);
    };
    let outputs = info
        .outputs
        .iter()
        .map(|o| Ok::<(Address, String), ChatError>((hex32(&o.recipient)?, o.amount.clone())))
        .collect::<Result<_, _>>()?;
    Ok(Some(ChainTx {
        id: *tx_id,
        sender: hex32(&info.sender)?,
        outputs,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// A node in memory: a genesis id, a finalized height, and blocks.
    pub(crate) struct FakeNode {
        pub genesis: ChainTag,
        pub finalized: Option<u64>,
        pub blocks: BTreeMap<u64, Value>,
    }

    impl ChainRpc for FakeNode {
        async fn call(&self, method: &str, params: Value) -> Result<Value, ChatError> {
            match method {
                "get_chain_info" => Ok(json!({"genesis": hex::encode(self.genesis), "height": 0})),
                "get_checkpoint" => Ok(self.finalized.map_or(
                    Value::Null,
                    |h| json!({"epoch": 1, "height": h, "block": "", "signatures": []}),
                )),
                "get_block_by_height" => {
                    let height = params[0].as_u64().unwrap();
                    self.blocks
                        .get(&height)
                        .cloned()
                        .ok_or_else(|| net("no block at that height"))
                }
                _ => Err(net("unknown method")),
            }
        }
    }

    const CHAIN: ChainTag = [0xc1; 32];
    const TX: [u8; 32] = [0x42; 32];

    fn block(height: u64, txs: &[Value]) -> Value {
        json!({"height": height, "header": {}, "transactions": txs, "raw": ""})
    }

    fn payment(sender: u8, to: u8, amount: &str) -> Value {
        json!({
            "txid": hex::encode(TX),
            "sender": hex::encode([sender; 32]),
            "outputs": [{"recipient": hex::encode([to; 32]), "amount": amount}],
        })
    }

    fn node(finalized: Option<u64>) -> FakeNode {
        let mut blocks = BTreeMap::new();
        for h in 1..=20 {
            let txs = if h == 12 {
                vec![payment(2, 1, "1000")]
            } else {
                vec![]
            };
            blocks.insert(h, block(h, &txs));
        }
        FakeNode {
            genesis: CHAIN,
            finalized,
            blocks,
        }
    }

    fn run<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn a_finalized_payment_is_found_with_its_sender_and_outputs() {
        let Scan::Found(found) = run(find_final(&node(Some(15)), &CHAIN, &TX, 10)).unwrap() else {
            panic!("expected the payment");
        };
        assert_eq!((found.height(), found.finalized()), (12, 15));
        assert_eq!(found.tx().sender, [2; 32]);
        assert_eq!(found.tx().outputs, vec![([1; 32], "1000".to_owned())]);
    }

    #[test]
    fn a_payment_above_the_checkpoint_is_not_final_yet() {
        assert_eq!(
            run(find_final(&node(Some(11)), &CHAIN, &TX, 10)).unwrap(),
            Scan::NotYet { next: 12 }
        );
        assert_eq!(
            run(find_final(&node(None), &CHAIN, &TX, 10)).unwrap(),
            Scan::NotYet { next: 10 },
            "no checkpoint: nothing is final"
        );
    }

    #[test]
    fn a_payment_below_the_search_start_is_not_found() {
        assert_eq!(
            run(find_final(&node(Some(20)), &CHAIN, &TX, 13)).unwrap(),
            Scan::NotYet { next: 21 }
        );
    }

    #[test]
    fn a_node_on_another_chain_is_refused() {
        let mut other = node(Some(20));
        other.genesis = [0xc2; 32];
        assert_eq!(
            run(find_final(&other, &CHAIN, &TX, 1)),
            Err(ChatError::Refused("node is on another chain"))
        );
    }
}
