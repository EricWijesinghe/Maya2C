//! The read-only debugging tools (v3 P6): replay, trace, compare.
//!
//! Each one is a thin adapter over code a person already runs: `replay_block`
//! is `maya2c replay` (`maya2c_cli::fork::replay`), `trace_transaction` is
//! `maya2c debug` (`maya2c_cli::record_spec`), and `compare_account` is two
//! `get_balance_at_height` reads. None calls a state-changing RPC method,
//! none takes a key, and none reads a file an assistant names: the genesis a
//! replay needs is fixed by whoever started the server (`--genesis`).

use custom_l1_node::core::Transaction;
use custom_l1_node::core::payload::{MAX_CONTRACT_CODE, TxKind};
use maya2c_cli::fork::{Source, replay};
use maya2c_cli::{CallSpec, parse_caller, parse_hex, record_spec};
use primitive_types::U256;
use serde_json::{Value, json};

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

/// `replay_block {height, txid?}`: a fresh local copy up to `height - 1`, then
/// that block executed here. Success *is* the proof the state root reproduced.
///
/// # Errors
///
/// No `--rpc` or `--genesis`, a bootstrap failure, or a block whose root local
/// execution does not reproduce.
pub fn replay_block(config: Config<'_>, args: &Value) -> Result<Value, String> {
    let rpc = config
        .rpc
        .ok_or("no node configured: start with --rpc HOST:PORT")?;
    let genesis = config
        .genesis
        .ok_or("replay needs the network's genesis: start with --genesis FILE")?;
    let height = args["height"].as_u64().ok_or("height is required")?;
    // A fresh, unpredictable directory: `fork::copy` wipes whatever is at
    // `dir`, so it is never a path an assistant chose. Removed on drop, panic
    // included.
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    let source = Source {
        url: format!("http://{rpc}"),
        genesis: genesis.into(),
        dir: scratch.path().join("copy"),
        snapshot_depth: None,
    };
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let result = replay(&source, height, runtime.handle().clone());
    // Dropped before the copy is removed: RocksDB holds its files open.
    drop(runtime);
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
pub fn compare_account(config: Config<'_>, args: &Value) -> Result<Value, String> {
    let address = args["address"].as_str().ok_or("address is required")?;
    let at = |key: &str| -> Result<(U256, Value), String> {
        let height = args[key]
            .as_u64()
            .ok_or(format!("{key} (a height) is required"))?;
        let reply = crate::rpc(
            config.rpc,
            &json!({"method": "get_balance_at_height", "params": [address, height]}),
        )?;
        if let Some(error) = reply.get("error") {
            return Err(format!(
                "height {height}: {}",
                error["message"].as_str().unwrap_or("refused")
            ));
        }
        let result = reply["result"].clone();
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
