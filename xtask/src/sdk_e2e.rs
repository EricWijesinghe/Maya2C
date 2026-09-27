//! `cargo xtask sdk-e2e` — the TypeScript SDK against a live node (Master
//! Prompt 9).
//!
//! Starts a one-validator DAG-BFT devnet and `maya2c-gateway`, signs a
//! transfer in Rust (`l1-wallet send --no-broadcast`), and runs
//! `sdks/sdk-js/test/live-node.test.ts` against the gateway. The driver is
//! Rust; only the SDK under test is TypeScript.

use std::path::PathBuf;
use std::process::Command;

use crate::devnet::{self, Procs};

const GATEWAY: &str = "127.0.0.1:32080";
const NETWORK: &str = "maya-sdk-e2e";
const RECIPIENT: &str = "7777777777777777777777777777777777777777777777777777777777777777";
const AMOUNT: u64 = 12_345;

/// Entry point.
///
/// # Errors
///
/// A setup failure or a failing SDK test.
pub fn run(args: &[String]) -> Result<(), String> {
    let work = match args {
        [flag, dir] if flag == "--workdir" => PathBuf::from(dir),
        [] => PathBuf::from("D:/Temp/maya-sdk-e2e"),
        _ => return Err("usage: cargo xtask sdk-e2e [--workdir DIR]".into()),
    };
    devnet::build(&["maya2c-node", "l1-wallet", "maya-api-gateway"])?;
    let sender = devnet::setup(&work, NETWORK)?;
    let mut procs = Procs::default();
    procs.push(devnet::start_node(&work)?);
    let rpc = format!("http://127.0.0.1:{}", devnet::RPC_PORT);
    procs.push(devnet::start(
        &work,
        "maya2c-gateway",
        &["--node", &rpc, "--listen", GATEWAY],
    )?);
    devnet::wait_for_http(GATEWAY)?;
    // Block 1 first: the fee the wallet computes reads the live base fee.
    devnet::wait_for_first_block()?;

    let signed = devnet::output(devnet::wallet(&work).args([
        "send",
        "--to",
        RECIPIENT,
        "--amount",
        &AMOUNT.to_string(),
        "--no-broadcast",
    ]))?;
    let raw = devnet::field(&signed, "raw").ok_or("wallet printed no raw transaction")?;
    let txid = devnet::field(&signed, "txid").unwrap_or_default();
    println!("signed in Rust: txid {txid}");

    let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let status = Command::new(npx)
        .args(["vitest", "run", "test/live-node.test.ts"])
        .current_dir(devnet::root().join("sdks").join("sdk-js"))
        .env("MAYA_GATEWAY_URL", format!("http://{GATEWAY}"))
        .env("MAYA_RAW_TX", raw)
        .env("MAYA_SENDER", sender)
        .env("MAYA_RECIPIENT", RECIPIENT)
        .env("MAYA_AMOUNT", AMOUNT.to_string())
        .status()
        .map_err(|e| format!("running vitest: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("SDK tests failed: {status}"))
}
