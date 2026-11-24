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
use std::net::IpAddr;
use std::time::Duration;

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

/// Longest a node gets to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest one call may take, answer included. The node is trusted for what
/// it reports, not for answering at all.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest answer read. A block is at most 5 MiB raw (`MAX_BLOCK_BYTES`)
/// and its hex JSON about twice that.
/// shortcut: fixed bound, raise it if governance lifts the block cap past ~12 MiB.
const MAX_ANSWER_BYTES: usize = 32 * 1024 * 1024;

/// [`ChainRpc`] over HTTP(S) JSON-RPC.
#[derive(Clone, Debug)]
pub struct HttpRpc {
    client: reqwest::Client,
    url: reqwest::Url,
    max_answer: usize,
}

impl HttpRpc {
    /// A client for the node at `url`: `https`, or plain `http` to this
    /// machine only, since anyone on the path could rewrite an answer.
    ///
    /// # Errors
    ///
    /// [`ChatError::Network`] for a malformed URL, plain `http` to another
    /// host, any other scheme, or a client that cannot be built.
    pub fn new(url: &str) -> Result<Self, ChatError> {
        let url = reqwest::Url::parse(url).map_err(net)?;
        match url.scheme() {
            "https" => {}
            "http" if is_loopback(&url) => {}
            _ => return Err(net("the node URL must be https, or http to this machine")),
        }
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            // A redirect could take an https call to plain http.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(net)?;
        Ok(Self {
            client,
            url,
            max_answer: MAX_ANSWER_BYTES,
        })
    }
}

fn is_loopback(url: &reqwest::Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

fn net(e: impl std::fmt::Display) -> ChatError {
    ChatError::Network(e.to_string())
}

/// Reads the body, refusing one longer than `max` bytes before buffering it.
async fn read_capped(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>, ChatError> {
    let too_long = || net(format!("the node's answer is over {max} bytes"));
    if response
        .content_length()
        .is_some_and(|n| usize::try_from(n).map_or(true, |n| n > max))
    {
        return Err(too_long());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(net)? {
        if body.len().saturating_add(chunk.len()) > max {
            return Err(too_long());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

impl ChainRpc for HttpRpc {
    async fn call(&self, method: &str, params: Value) -> Result<Value, ChatError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response = self
            .client
            .post(self.url.clone())
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(net)?;
        if !response.status().is_success() {
            return Err(net(format!("the node answered HTTP {}", response.status())));
        }
        let bytes = read_capped(response, self.max_answer).await?;
        let mut answer: Value = serde_json::from_slice(&bytes).map_err(net)?;
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
    #[serde(default)]
    height: u64,
}

/// The node's tip height on `chain`: what a payee passes to
/// [`crate::client::Client::queue_request`] as the request's issue height.
///
/// # Errors
///
/// [`ChatError::Refused`] if the node is on another chain;
/// [`ChatError::Network`] for an RPC failure or a malformed answer.
pub async fn tip_height<R: ChainRpc>(rpc: &R, chain: &ChainTag) -> Result<u64, ChatError> {
    let info: ChainInfo = decode(rpc.call("get_chain_info", json!([])).await?)?;
    if hex32(&info.genesis)? != *chain {
        return Err(ChatError::Refused("node is on another chain"));
    }
    Ok(info.height)
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
                "get_chain_info" => Ok(json!({"genesis": hex::encode(self.genesis), "height": 42})),
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
        assert_eq!(
            run(tip_height(&other, &CHAIN)),
            Err(ChatError::Refused("node is on another chain"))
        );
        assert_eq!(run(tip_height(&node(Some(20)), &CHAIN)), Ok(42));
    }

    #[test]
    fn plain_http_is_refused_except_to_this_machine() {
        for url in ["http://example.com:8545", "ftp://127.0.0.1/", "not a url"] {
            assert!(HttpRpc::new(url).is_err(), "accepted: {url}");
        }
        for url in [
            "https://rpc.example.com",
            "http://127.0.0.1:8545",
            "http://localhost:8545",
            "http://[::1]:8545",
        ] {
            assert!(HttpRpc::new(url).is_ok(), "refused: {url}");
        }
    }

    /// Answers one HTTP request on a loopback port with `reply`, raw.
    fn one_shot_node(reply: String) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(reply.as_bytes());
        });
        format!("http://{addr}")
    }

    fn call_once(reply: String, max_answer: usize) -> Result<Value, ChatError> {
        let mut rpc = HttpRpc::new(&one_shot_node(reply)).unwrap();
        rpc.max_answer = max_answer;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(rpc.call("get_chain_info", json!([])))
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn a_node_answer_is_read_within_its_bounds() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"height":7}}"#;
        assert_eq!(
            call_once(http("200 OK", body), 1024),
            Ok(json!({"height": 7}))
        );
        let refused = call_once(http("503 Service Unavailable", body), 1024);
        assert!(
            matches!(refused, Err(ChatError::Network(ref e)) if e.contains("503")),
            "{refused:?}"
        );
        let refused = call_once(http("200 OK", body), 16);
        assert!(
            matches!(refused, Err(ChatError::Network(ref e)) if e.contains("over 16")),
            "{refused:?}"
        );
        // No Content-Length: the cap holds while the body streams in.
        let chunked = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}");
        let refused = call_once(chunked, 16);
        assert!(
            matches!(refused, Err(ChatError::Network(ref e)) if e.contains("over 16")),
            "{refused:?}"
        );
    }
}
