//! `cargo xtask sdk-e2e [--lang ts|python|go|all]` — the SDKs against a live
//! node (Master Prompt 9).
//!
//! Starts a one-validator DAG-BFT devnet (and `maya2c-gateway` for the
//! TypeScript SDK) and runs each SDK's live-node test against it. Every SDK
//! signs through the Rust wallet (`l1-wallet`), so none of them contains a
//! second implementation of the hybrid signature or the wire format. The
//! driver is Rust; only the SDK under test is in its own language.

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
    let (mut work, mut langs) = (std::env::temp_dir().join("maya-sdk-e2e"), vec!["ts"]);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--workdir" => work = PathBuf::from(value),
            "--lang" if value == "all" => langs = vec!["ts", "python", "go"],
            "--lang" => langs = vec![value.as_str()],
            _ => {
                return Err(
                    "usage: cargo xtask sdk-e2e [--lang ts|python|go|all] [--workdir DIR]".into(),
                );
            }
        }
    }
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
    for lang in langs {
        match lang {
            "ts" => typescript(&work, &sender)?,
            "python" => python(&work, &rpc)?,
            "go" => go(&work, &rpc)?,
            other => return Err(format!("unknown SDK language `{other}`")),
        }
    }
    Ok(())
}

/// What the Python and Go live-node tests need: the node, the Rust wallet,
/// its keystore and password, and a recipient.
fn sdk_env(cmd: &mut Command, work: &std::path::Path, rpc: &str) {
    cmd.env("MAYA_RPC_URL", rpc)
        .env("L1_WALLET", devnet::bin("l1-wallet"))
        .env("MAYA_KEYSTORE", work.join("wallet.key"))
        .env("L1_WALLET_PASSWORD", devnet::PASSWORD)
        .env("MAYA_RECIPIENT", RECIPIENT);
}

fn check(status: std::io::Result<std::process::ExitStatus>, what: &str) -> Result<(), String> {
    let status = status.map_err(|e| format!("running {what}: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{what} failed: {status}"))
}

fn python(work: &std::path::Path, rpc: &str) -> Result<(), String> {
    let mut cmd = Command::new("python");
    cmd.args(["-m", "unittest", "discover", "-s", "tests", "-v"])
        .current_dir(devnet::root().join("sdks").join("python"));
    sdk_env(&mut cmd, work, rpc);
    check(cmd.status(), "the Python SDK tests")
}

fn go(work: &std::path::Path, rpc: &str) -> Result<(), String> {
    let mut cmd = Command::new(std::env::var("GO").unwrap_or_else(|_| "go".into()));
    cmd.args(["test", "-v", "-count=1", "./..."])
        .current_dir(devnet::root().join("sdks").join("go"));
    sdk_env(&mut cmd, work, rpc);
    check(cmd.status(), "the Go SDK tests")
}

fn typescript(work: &std::path::Path, sender: &str) -> Result<(), String> {
    let signed = devnet::output(devnet::wallet(work).args([
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
        .status();
    check(status, "the TypeScript SDK tests")
}
