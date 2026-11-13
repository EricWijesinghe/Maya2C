//! The read-only debugging tools (v3 P6): replay, trace, compare.
//!
//! Each one is a thin adapter over code a person already runs: `replay_block`
//! is `maya2c replay` (`maya2c_cli::fork::replay_from`, every fetch through
//! the ADR-084 allowlist and rate limiter in [`Node`]), `trace_transaction` is
//! `maya2c debug` (`maya2c_cli::record_spec`), and `compare_account` is two
//! `get_balance_at_height` reads. None calls a state-changing RPC method,
//! none takes a key, and none reads a file an assistant names: the genesis a
//! replay needs is fixed by whoever started the server (`--genesis`).

use std::sync::Mutex;

use custom_l1_node::core::payload::{MAX_CONTRACT_CODE, TxKind};
use custom_l1_node::core::{Block, BlockHeader, Transaction};
use custom_l1_node::error::{NodeError, Result as NodeResult};
use custom_l1_node::state_pruner::snapshot::{BootstrapSource, SnapshotManifest};
use maya2c_cli::fork::{Source, replay_from};
use maya2c_cli::{CallSpec, parse_caller, parse_hex, record_spec};
use primitive_types::U256;
use serde_json::{Value, json};

use crate::node::Node;

/// The four debugging tools' MCP descriptions, appended to `tools::list`.
pub fn list() -> Value {
    json!([
        {"name": "replay_block", "description": "Read-only. Re-execute the block at a height on a fresh local copy of the configured node's chain (maya2c replay); returns its transactions and every balance it changed. Optional txid checks the transaction is in that block.",
         "inputSchema": {"type": "object", "properties": {"height": {"type": "integer"}, "txid": {"type": "string"}}, "required": ["height"]}},
        {"name": "trace_transaction", "description": "Read-only. Run a contract call once under the time-travel debugger (maya2c debug) and return every host-call step, storage after, events and gas. Give the contract wasm (hex) and either a CallContract transaction (hex) or input/caller/gas.",
         "inputSchema": {"type": "object", "properties": {"wasm": {"type": "string"}, "tx": {"type": "string"}, "input": {"type": "string"}, "caller": {"type": "string"}, "height": {"type": "integer"}, "gas": {"type": "integer"}}, "required": ["wasm"]}},
        {"name": "explain_refusal", "description": "Explain why the node refused something: give the refusal message (or a NodeError kind); returns the variant, its fields and the node's own documentation of it, read from crates/node/src/error.rs.",
         "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}, "kind": {"type": "string"}}}},
        {"name": "compare_account", "description": "Read-only. An account's balance at two heights on the configured node, and the change between them.",
         "inputSchema": {"type": "object", "properties": {"address": {"type": "string"}, "from": {"type": "integer"}, "to": {"type": "integer"}}, "required": ["address", "from", "to"]}}
    ])
}

/// Where the server was pointed when it started.
#[derive(Clone, Copy, Debug, Default)]
pub struct Config<'a> {
    /// `--rpc HOST:PORT`.
    pub rpc: Option<&'a str>,
    /// `--genesis FILE`: the network's genesis, for replays.
    pub genesis: Option<&'a str>,
}

/// The `maya2c debug` default.
const DEFAULT_GAS: u64 = 10_000_000;
/// Ten times the default: a trace runs in this process, so an assistant's
/// `u64::MAX` must not become an unbounded run.
const MAX_TRACE_GAS: u64 = 100_000_000;
/// Steps returned; a longer recording is cut and says so.
const MAX_STEPS: usize = 10_000;
/// Highest block a replay may target. A replay fetches every block below it
/// (full sync), paced at the limiter's 50 per second: 10,000 blocks is about
/// 200 s of node reads, the most one assistant request may cost.
// shortcut: full sync only; a snapshot-based replay needs get_headers and the
// snapshot methods allowlisted too — add them when a chain outgrows the cap.
pub const MAX_REPLAY_HEIGHT: u64 = 10_000;

/// The replay's view of the node: [`BootstrapSource`] over [`Node::call_paced`],
/// so a replay sends nothing the allowlist refuses and never outruns the
/// limiter. Only the full-sync methods are served.
struct Allowlisted<'a>(Mutex<&'a mut Node>);

impl Allowlisted<'_> {
    fn call(&self, method: &str, params: &Value) -> NodeResult<Value> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .call_paced(method, params)
            .map_err(NodeError::Network)
    }
}

impl BootstrapSource for Allowlisted<'_> {
    fn tip_height(&self) -> NodeResult<u64> {
        self.call("get_tip_height", &json!([]))?
            .as_u64()
            .ok_or_else(|| NodeError::Decode("get_tip_height: not a height".into()))
    }
    fn headers(&self, _: u64, _: u64) -> NodeResult<Vec<BlockHeader>> {
        Err(NodeError::Network(
            "replay_block does a full sync; no headers".into(),
        ))
    }
    fn snapshot_manifest(&self) -> NodeResult<SnapshotManifest> {
        Err(NodeError::Network(
            "replay_block does a full sync; no snapshot".into(),
        ))
    }
    fn snapshot_chunk(&self, _: usize) -> NodeResult<Vec<u8>> {
        Err(NodeError::Network(
            "replay_block does a full sync; no snapshot".into(),
        ))
    }
    fn block(&self, height: u64) -> NodeResult<Block> {
        let info = self.call("get_block_by_height", &json!([height]))?;
        let raw = info["raw"]
            .as_str()
            .ok_or_else(|| NodeError::Decode(format!("block {height}: no raw bytes")))?;
        Block::from_bytes(
            &hex::decode(raw).map_err(|e| NodeError::Decode(format!("block {height}: {e}")))?,
        )
    }
}

/// `replay_block {height, txid?}`: a fresh local copy up to `height - 1`, then
/// that block executed here. Success *is* the proof the state root reproduced.
///
/// # Errors
///
/// No `--rpc` or `--genesis`, a bootstrap failure, or a block whose root local
/// execution does not reproduce.
pub fn replay_block(node: &mut Node, config: Config<'_>, args: &Value) -> Result<Value, String> {
    let genesis = config
        .genesis
        .ok_or("replay needs the network's genesis: start with --genesis FILE")?;
    let height = args["height"].as_u64().ok_or("height is required")?;
    if height > MAX_REPLAY_HEIGHT {
        return Err(format!(
            "replay re-executes every block from genesis, so it is capped at height {MAX_REPLAY_HEIGHT}; use `maya2c replay --snapshot-depth N` for deeper blocks"
        ));
    }
    // A fresh, unpredictable directory: `fork::copy_from` wipes whatever is
    // at `dir`, so it is never a path an assistant chose. Removed on drop,
    // panic included.
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    let source = Source {
        url: String::new(),
        genesis: genesis.into(),
        dir: scratch.path().join("copy"),
        snapshot_depth: None,
    };
    // Every fetch goes through `Node`: the ADR-084 allowlist and the limiter.
    let result = replay_from(&source, &Allowlisted(Mutex::new(node)), height);
    drop(scratch);
    let r = result.map_err(|e| format!("{e:#}"))?;
    let txid = args["txid"].as_str();
    if let Some(txid) = txid
        && !r.txids.iter().any(|t| t == txid)
    {
        return Err(format!("transaction {txid} is not in block {height}"));
    }
    let changes: Vec<Value> = r
        .changes
        .iter()
        .map(|c| json!({"address": hex::encode(c.address), "before": c.before.to_string(), "after": c.after.to_string()}))
        .collect();
    let mut out = json!({"height": r.height, "block_id": r.block_id, "state_root": r.state_root,
        "reproduced": true, "transactions": r.txids, "balance_changes": changes});
    if txid.is_some() {
        // The node records balance changes per block, not per transaction.
        out["note"] = json!("balance_changes cover the whole block that carries this transaction");
    }
    Ok(out)
}

/// `trace_transaction {wasm, tx? | input?, caller?, height?, gas?}`: the call
/// run once under the time-travel debugger's recorder, every step returned.
///
/// With `tx` (a hex `CallContract` transaction) the input, gas limit and
/// caller come from the transaction itself.
///
/// # Errors
///
/// Bad hex, a `tx` that is not a contract call, or a VM that cannot start.
pub fn trace_transaction(args: &Value) -> Result<Value, String> {
    let wasm = parse_hex(
        "wasm",
        args["wasm"].as_str().ok_or("wasm (hex) is required")?,
    )
    .map_err(|e| e.to_string())?;
    if wasm.len() > MAX_CONTRACT_CODE {
        return Err(format!(
            "wasm is {} bytes; the chain stores at most {MAX_CONTRACT_CODE}",
            wasm.len()
        ));
    }
    let height = args["height"].as_u64().unwrap_or(0);
    let spec = match args["tx"].as_str() {
        Some(tx_hex) => {
            let bytes = parse_hex("tx", tx_hex).map_err(|e| e.to_string())?;
            let tx = Transaction::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let TxKind::CallContract(call) = &tx.kind else {
                return Err("tx is not a contract call; only calls have execution steps".into());
            };
            CallSpec {
                wasm,
                input: call.input.clone(),
                caller: Some(tx.sender()),
                height,
                gas: call.gas_limit,
            }
        }
        None => CallSpec {
            wasm,
            input: parse_hex("input", args["input"].as_str().unwrap_or(""))
                .map_err(|e| e.to_string())?,
            caller: args["caller"]
                .as_str()
                .map(parse_caller)
                .transpose()
                .map_err(|e| e.to_string())?,
            height,
            gas: args["gas"].as_u64().unwrap_or(DEFAULT_GAS),
        },
    };
    // shortcut: storage starts empty, not as the chain held it at `height`; the
    // RPC serves no contract storage to seed it from.
    let spec = CallSpec {
        gas: spec.gas.min(MAX_TRACE_GAS),
        ..spec
    };
    let mut session = record_spec(&spec).map_err(|e| format!("{e:#}"))?;
    let listing = session.listing();
    let steps: Vec<&str> = listing.lines().skip(1).take(MAX_STEPS).collect();
    let truncated = session.total() > MAX_STEPS;
    session.goto(session.total());
    let (gas_used, outcome) = session.summary();
    Ok(json!({
        "outcome": outcome,
        "gas_used": gas_used,
        "gas_limit": spec.gas,
        "steps": steps,
        "steps_truncated": truncated,
        "storage_after": session.storage_pairs(),
        "events": session.event_pairs(),
        "initial_storage": "empty",
    }))
}

/// `compare_account {address, from, to}`: one account's balance at two heights.
///
/// # Errors
///
/// No `--rpc`, missing arguments, or the node's refusal (e.g. a pruned height).
pub fn compare_account(node: &mut Node, args: &Value) -> Result<Value, String> {
    let address = args["address"].as_str().ok_or("address is required")?;
    let mut at = |key: &str| -> Result<(U256, Value), String> {
        let height = args[key]
            .as_u64()
            .ok_or(format!("{key} (a height) is required"))?;
        // Through the ADR-084 allowlist and rate limiter, like every other read.
        let result = node
            .call("get_balance_at_height", &json!([address, height]))
            .map_err(|e| format!("height {height}: {e}"))?;
        let balance = U256::from_dec_str(result["balance"].as_str().unwrap_or(""))
            .map_err(|e| format!("height {height}: node returned no balance: {e:?}"))?;
        Ok((balance, result))
    };
    let (before, from) = at("from")?;
    let (after, to) = at("to")?;
    let change = if after >= before {
        format!("+{}", after - before)
    } else {
        format!("-{}", before - after)
    };
    Ok(json!({"address": address, "from": from, "to": to, "change": change}))
}
