//! `cargo xtask eth-headers | eth-beacon | eth-exposure` — the committed Ethereum
//! mainnet fixtures the interop and quantum-harbor tests read (MP25, MP28).
//!
//! Without `--fetch` each command is offline: it parses the committed fixture,
//! writes it back in Python's `json.dumps(indent=1)` form, and fails unless the
//! bytes match. The tests never touch the network, so this round trip is the
//! proof that the file is what the fetch produced. `--fetch` refreshes a fixture
//! from the public endpoints; it stamps a new `fetched_utc`, so review the diff.

use std::fs;
use std::str::FromStr;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use serde_json::{Map, Value, json};

use crate::ordered_json::Json;
use crate::pyfmt;

const HEADERS_OUT: &str = "crates/interop/tests/fixtures/eth_mainnet_headers.json";
const BEACON_OUT: &str = "crates/interop/tests/fixtures/eth_beacon_finality.json";
const EXPOSURE_OUT: &str = "crates/quantum-harbor/tests/fixtures/eth_mainnet_sample.json";
const EXECUTION_RPC: &str = "https://ethereum-rpc.publicnode.com";
const BEACON_API: &str = "https://ethereum-beacon-api.publicnode.com";

/// The header fields the fixture keeps, in the order it keeps them.
const HEADER_FIELDS: &[&str] = &[
    "hash",
    "parentHash",
    "sha3Uncles",
    "miner",
    "stateRoot",
    "transactionsRoot",
    "receiptsRoot",
    "logsBloom",
    "difficulty",
    "number",
    "gasLimit",
    "gasUsed",
    "timestamp",
    "extraData",
    "mixHash",
    "nonce",
    "baseFeePerGas",
    "withdrawalsRoot",
    "blobGasUsed",
    "excessBlobGas",
    "parentBeaconBlockRoot",
    "requestsHash",
];

/// Python wrote the fixtures with `indent=1`.
const INDENT: usize = 1;

pub fn eth_headers(args: &[String]) -> Result<(), String> {
    check_args(args, &["--rpc", "--count"])?;
    if !fetching(args) {
        return verify(HEADERS_OUT);
    }
    let rpc = value(args, "--rpc")?.unwrap_or(EXECUTION_RPC);
    let count: u64 = number(args, "--count", 64)?;
    let http = Http::new("maya2c-headers-fixture/1", 30)?;
    let head = hex_u64(&http.rpc(rpc, "eth_blockNumber", &json!([]))?)?;
    let first = (head + 1).saturating_sub(count);
    let mut headers = Vec::new();
    for n in first..=head {
        let block = http.rpc(
            rpc,
            "eth_getBlockByNumber",
            &json!([format!("0x{n:x}"), false]),
        )?;
        headers.push(Value::Object(pick(&block, HEADER_FIELDS)?));
    }
    let len = headers.len();
    let doc =
        json!({ "source": public_origin(rpc), "fetched_utc": fetched_utc(), "headers": headers });
    write(HEADERS_OUT, &doc)?;
    println!("{len} headers {first}..{head} -> {HEADERS_OUT}");
    Ok(())
}

pub fn eth_beacon(args: &[String]) -> Result<(), String> {
    check_args(args, &[])?;
    if !fetching(args) {
        return verify(BEACON_OUT);
    }
    let http = Http::new("maya2c-beacon-fixture/1", 60)?;
    let update = http.get(&format!(
        "{BEACON_API}/eth/v1/beacon/light_client/finality_update"
    ))?;
    let finalized = at(&update, "/data/finalized_header/beacon")?;
    let root = header_root(&http, finalized)?;
    let bootstrap = http.get(&format!(
        "{BEACON_API}/eth/v1/beacon/light_client/bootstrap/{root}"
    ))?;
    let genesis = http.get(&format!("{BEACON_API}/eth/v1/beacon/genesis"))?;
    let fork = http.get(&format!("{BEACON_API}/eth/v1/beacon/states/head/fork"))?;
    let block_hash = at(&update, "/data/finalized_header/execution/block_hash")?.clone();
    let execution = http.rpc(
        EXECUTION_RPC,
        "eth_getBlockByHash",
        &json!([block_hash, false]),
    )?;
    let slot = plain(at(finalized, "/slot")?);
    let doc = json!({
        "source": { "beacon": BEACON_API, "execution": EXECUTION_RPC },
        "fetched_utc": fetched_utc(),
        "genesis_validators_root": at(&genesis, "/data/genesis_validators_root")?,
        "fork": at(&fork, "/data")?,
        "finalized_root_from_api": root,
        "finality_update": update,
        "bootstrap": bootstrap,
        "execution_header": execution,
    });
    write(BEACON_OUT, &doc)?;
    println!(
        "finalized slot {slot} root {root} exec {} -> {BEACON_OUT}",
        plain(&block_hash)
    );
    Ok(())
}

pub fn eth_exposure(args: &[String]) -> Result<(), String> {
    check_args(args, &["--rpc", "--blocks", "--max-accounts"])?;
    if !fetching(args) {
        return verify(EXPOSURE_OUT);
    }
    let rpc = value(args, "--rpc")?.unwrap_or(EXECUTION_RPC);
    let blocks: u64 = number(args, "--blocks", 3)?;
    let max_accounts: usize = number(args, "--max-accounts", 300)?;
    let http = Http::new("maya2c-exposure-fixture/1", 30)?;

    let head = hex_u64(&http.rpc(rpc, "eth_blockNumber", &json!([]))?)?;
    let pinned = format!("0x{head:x}");
    let mut addresses: Vec<String> = Vec::new();
    let mut raw: Option<Value> = None;
    for n in (head + 1).saturating_sub(blocks)..=head {
        let block = http.rpc(
            rpc,
            "eth_getBlockByNumber",
            &json!([format!("0x{n:x}"), true]),
        )?;
        for tx in at(&block, "/transactions")?
            .as_array()
            .into_iter()
            .flatten()
        {
            for field in ["from", "to"] {
                let Some(a) = tx.get(field).and_then(Value::as_str) else {
                    continue;
                };
                if !addresses.iter().any(|known| known == a) {
                    addresses.push(a.to_string());
                }
            }
            if raw.is_none() && tx.get("type").and_then(Value::as_str) == Some("0x2") {
                let hash = at(tx, "/hash")?.clone();
                let signed = http.rpc(rpc, "eth_getRawTransactionByHash", &json!([hash]))?;
                raw = Some(json!({ "hash": hash, "from": at(tx, "/from")?, "raw": signed }));
            }
        }
    }
    addresses.truncate(max_accounts);

    let mut accounts = Vec::new();
    for a in &addresses {
        let nonce = hex_u64(&http.rpc(rpc, "eth_getTransactionCount", &json!([a, pinned]))?)?;
        let code = plain(&http.rpc(rpc, "eth_getCode", &json!([a, pinned]))?);
        // ETH supply is about 1.2e26 wei, far under u128, so no balance overflows.
        let balance = hex_u128(&http.rpc(rpc, "eth_getBalance", &json!([a, pinned]))?)?;
        accounts.push(json!({
            "address": a,
            "nonce": nonce,
            // The first three bytes are enough to tell a contract from an
            // EIP-7702-delegated EOA (0xef0100...), which still has a key.
            "code_prefix": code.chars().take(8).collect::<String>(),
            "balance_wei": balance.to_string(),
        }));
    }
    let count = accounts.len();
    let doc = json!({
        "source": public_origin(rpc),
        "fetched_utc": fetched_utc(),
        "pinned_block": head,
        "blocks_sampled": blocks,
        "signed_transaction": raw.unwrap_or(Value::Null),
        "accounts": accounts,
    });
    write(EXPOSURE_OUT, &doc)?;
    println!("{count} accounts at block {head} -> {EXPOSURE_OUT}");
    Ok(())
}

/// The scheme and host of an RPC URL, for the fixture's `source` field.
///
/// A `--rpc` URL from a provider often carries an API key in the userinfo,
/// path or query; the fixture is committed to a public repository, so only
/// the origin is recorded.
fn public_origin(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if scheme.is_empty() {
        host.to_owned()
    } else {
        format!("{scheme}://{host}")
    }
}

/// The finalized header's block root, as the beacon API reports it. The Python
/// fetched it the same way and the test recomputes it; the two must agree.
fn header_root(http: &Http, finalized: &Value) -> Result<String, String> {
    let slot = plain(at(finalized, "/slot")?);
    let header = http.get(&format!("{BEACON_API}/eth/v1/beacon/headers/{slot}"))?;
    Ok(plain(at(&header, "/data/root")?))
}

/// Keeps `fields` from a block object, in the order given (missing ones omitted).
fn pick(block: &Value, fields: &[&str]) -> Result<Map<String, Value>, String> {
    let object = block
        .as_object()
        .ok_or("eth_getBlockByNumber returned no block")?;
    Ok(fields
        .iter()
        .filter_map(|k| object.get(*k).map(|v| ((*k).to_string(), v.clone())))
        .collect())
}

/// Offline check: the committed file must re-serialise to exactly its own bytes.
fn verify(rel: &str) -> Result<(), String> {
    let path = crate::workspace_root().join(rel);
    let bytes = fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
    let value: Json = serde_json::from_slice(&bytes).map_err(|e| format!("{rel}: {e}"))?;
    let rebuilt = pyfmt::json(&value, INDENT)?;
    if rebuilt.as_bytes() != bytes.as_slice() {
        return Err(format!(
            "{rel}: re-serialised bytes differ from the committed file; \
             re-run with --fetch only if the endpoint data changed"
        ));
    }
    println!("ok  {rel} reproduces byte-identically ({} B)", bytes.len());
    Ok(())
}

fn write(rel: &str, doc: &Value) -> Result<(), String> {
    let path = crate::workspace_root().join(rel);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = pyfmt::json(doc, INDENT)?;
    fs::write(&path, text).map_err(|e| format!("{rel}: {e}"))
}

fn fetched_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

fn fetching(args: &[String]) -> bool {
    args.iter().any(|a| a == "--fetch")
}

/// Rejects any argument that is neither `--fetch` nor one of `value_flags`
/// (each followed by its value).
fn check_args(args: &[String], value_flags: &[&str]) -> Result<(), String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--fetch" {
            continue;
        }
        if !value_flags.contains(&arg.as_str()) {
            return Err(format!("unknown argument `{arg}`"));
        }
        if iter.next().is_none() {
            return Err(format!("{arg} needs a value"));
        }
    }
    Ok(())
}

fn value<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>, String> {
    match args.iter().position(|a| a == name) {
        None => Ok(None),
        Some(i) => args
            .get(i + 1)
            .map(|v| Some(v.as_str()))
            .ok_or_else(|| format!("{name} needs a value")),
    }
}

fn number<T: FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
    match value(args, name)? {
        None => Ok(default),
        Some(v) => v
            .parse()
            .map_err(|_| format!("{name}: `{v}` is not a number")),
    }
}

/// Walks a JSON pointer, naming the path in the error so a changed endpoint
/// shape says where it changed.
fn at<'a>(value: &'a Value, pointer: &str) -> Result<&'a Value, String> {
    value
        .pointer(pointer)
        .ok_or_else(|| format!("reply has no `{pointer}`"))
}

/// A JSON scalar as the Python `f"{x}"` would print it.
fn plain(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn hex_u64(value: &Value) -> Result<u64, String> {
    u64::from_str_radix(hex_digits(value)?, 16).map_err(|e| format!("{value}: {e}"))
}

fn hex_u128(value: &Value) -> Result<u128, String> {
    u128::from_str_radix(hex_digits(value)?, 16).map_err(|e| format!("{value}: {e}"))
}

fn hex_digits(value: &Value) -> Result<&str, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{value} is not a hex string"))?;
    Ok(text.strip_prefix("0x").unwrap_or(text))
}

/// One JSON-RPC and HTTP client, with the user agent each fixture sent.
struct Http {
    client: Client,
    user_agent: &'static str,
}

impl Http {
    fn new(user_agent: &'static str, timeout_secs: u64) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, user_agent })
    }

    /// A JSON-RPC call. An `error` member fails it, as the Python did.
    fn rpc(&self, url: &str, method: &str, params: &Value) -> Result<Value, String> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let reply = self.post(url, body.to_string())?;
        if let Some(error) = reply.get("error") {
            return Err(format!("{method}: {error}"));
        }
        at(&reply, "/result")
            .cloned()
            .map_err(|e| format!("{method}: {e}"))
    }

    fn get(&self, url: &str) -> Result<Value, String> {
        let bytes = self
            .client
            .get(url)
            .header(USER_AGENT, self.user_agent)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::bytes)
            .map_err(|e| format!("{url}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{url}: {e}"))
    }

    fn post(&self, url: &str, body: String) -> Result<Value, String> {
        let bytes = self
            .client
            .post(url)
            .header(USER_AGENT, self.user_agent)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::bytes)
            .map_err(|e| format!("{url}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{url}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_source_field_drops_keys_in_userinfo_path_and_query() {
        assert_eq!(
            super::public_origin("https://u:k@rpc.example/v3/KEY?apikey=x"),
            "https://rpc.example"
        );
        assert_eq!(
            super::public_origin("http://127.0.0.1:8545"),
            "http://127.0.0.1:8545"
        );
    }
}
