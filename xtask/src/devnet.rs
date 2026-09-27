//! A one-validator DAG-BFT devnet for end-to-end checks: build the binaries,
//! write keys and a genesis, start the node, talk JSON-RPC to it.
//!
//! Std only — a hand-rolled HTTP/1.1 POST over `TcpStream` — so xtask stays
//! free of an HTTP client dependency. Local processes on 127.0.0.1 only.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The node's JSON-RPC port.
pub const RPC_PORT: u16 = 32_000;
/// Its p2p port.
const P2P_PORT: u16 = 31_000;
/// Keystore password. A throwaway devnet key, never a real one.
pub const PASSWORD: &str = "devnet-only-password";
/// Funds in the one genesis allocation.
const GENESIS_BALANCE: u64 = 10_000_000;
/// How long a process gets to come up.
pub const BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// Result alias: every failure here is a message for the operator.
pub type Result<T> = std::result::Result<T, String>;

/// Kills its children on drop, so a failed check leaves nothing running.
#[derive(Default)]
pub struct Procs(Vec<Child>);

impl Procs {
    /// Adds a child.
    pub fn push(&mut self, child: Child) {
        self.0.push(child);
    }

    /// Kills child `index` and waits for it; it stays in place until replaced.
    pub fn kill(&mut self, index: usize) {
        if let Some(child) = self.0.get_mut(index) {
            // Already exited is fine; there is nothing else to do on failure.
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Puts `child` in slot `index`, killing whatever was there.
    pub fn replace(&mut self, index: usize, child: Child) {
        self.kill(index);
        if let Some(slot) = self.0.get_mut(index) {
            *slot = child;
        } else {
            self.0.push(child);
        }
    }
}

impl Drop for Procs {
    fn drop(&mut self) {
        for child in &mut self.0 {
            // Already exited is fine; there is nothing else to do on failure.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The workspace root.
pub fn root() -> PathBuf {
    crate::workspace_root()
}

/// Where prebuilt binaries are taken from instead of `target/debug`, so a
/// rehearsal can run the exact artifacts that ship (`target/dist`).
pub const BIN_DIR_ENV: &str = "MAYA2C_BIN_DIR";

/// A built binary: under `$MAYA2C_BIN_DIR` if set, else `target/debug`.
pub fn bin(name: &str) -> PathBuf {
    let dir = std::env::var_os(BIN_DIR_ENV)
        .map_or_else(|| root().join("target").join("debug"), PathBuf::from);
    dir.join(format!("{name}{EXE}"))
}

/// `cargo build -p` each package; nothing when `$MAYA2C_BIN_DIR` names
/// prebuilt binaries.
pub fn build(packages: &[&str]) -> Result<()> {
    if std::env::var_os(BIN_DIR_ENV).is_some() {
        return Ok(());
    }
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.arg("build").current_dir(root()).stdout(Stdio::null());
    for p in packages {
        cmd.args(["-p", p]);
    }
    let status = cmd.status().map_err(|e| format!("cargo build: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("cargo build failed: {status}"))
}

/// Runs `cmd` and returns its stdout.
pub fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The value of `name:` in `l1-wallet` output.
pub fn field(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|l| {
        l.strip_prefix(name)?
            .strip_prefix(':')
            .map(|v| v.trim().to_string())
    })
}

/// A fresh devnet directory: validator key, funded wallet, genesis. Returns
/// the wallet's address.
pub fn setup(work: &Path, chain_id: &str) -> Result<String> {
    if work.exists() {
        std::fs::remove_dir_all(work).map_err(|e| format!("clearing {}: {e}", work.display()))?;
    }
    let v0 = work.join("v0");
    std::fs::create_dir_all(&v0).map_err(|e| format!("creating {}: {e}", v0.display()))?;
    let pubkey = output(
        Command::new(bin("maya2c-node"))
            .arg("--generate-validator-key")
            .arg(v0.join("validator.key")),
    )?;
    let generated = output(wallet(work).arg("generate"))?;
    let address = field(&generated, "address").ok_or("l1-wallet generate printed no address")?;
    let genesis = json!({
        "chain_id": chain_id,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        "difficulty_bits": 0,
        "pow_limit_bits": 0,
        "allocations": [{ "address": address, "balance": GENESIS_BALANCE }],
        "bft": {
            "validators": [pubkey.trim()], "anchor_timeout_ms": 1000, "batch_size": 500,
            "fees": { "initial_base_fee": 1, "min_base_fee": 1,
                      "target_block_bytes": 2_621_440, "change_denominator": 8 },
            "staking": { "epoch_blocks": 20, "bonds": [{ "operator": address, "bond": 100_000 }] }
        }
    });
    std::fs::write(work.join("genesis.json"), genesis.to_string())
        .map_err(|e| format!("writing genesis: {e}"))?;
    Ok(address)
}

/// `l1-wallet` on this devnet's keystore and node.
pub fn wallet(work: &Path) -> Command {
    let mut cmd = Command::new(bin("l1-wallet"));
    cmd.env("L1_WALLET_PASSWORD", PASSWORD)
        .arg("--keystore")
        .arg(work.join("wallet.key"))
        .args(["--rpc-url", &format!("http://127.0.0.1:{RPC_PORT}")]);
    cmd
}

/// Starts the validator, logging to `v0/node.log`.
pub fn start_node(work: &Path) -> Result<Child> {
    let v0 = work.join("v0");
    let log = std::fs::File::create(v0.join("node.log")).map_err(|e| format!("node log: {e}"))?;
    Command::new(bin("maya2c-node"))
        .arg("--genesis")
        .arg(work.join("genesis.json"))
        .arg("--data-dir")
        .arg(&v0)
        .args([
            "--rpc-addr",
            &format!("127.0.0.1:{RPC_PORT}"),
            "--p2p-port",
            &P2P_PORT.to_string(),
        ])
        .arg("--validator-key")
        .arg(v0.join("validator.key"))
        .stdout(log.try_clone().map_err(|e| format!("node log: {e}"))?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("starting maya2c-node: {e}"))
}

/// Starts `name` with `args`, logging to `work/<name>.log`.
pub fn start(work: &Path, name: &str, args: &[&str]) -> Result<Child> {
    let log = std::fs::File::create(work.join(format!("{name}.log")))
        .map_err(|e| format!("{name} log: {e}"))?;
    Command::new(bin(name))
        .args(args)
        .stdout(log.try_clone().map_err(|e| format!("{name} log: {e}"))?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("starting {name}: {e}"))
}

/// POSTs JSON to `http://<addr><path>` and returns the status and body.
pub fn post(addr: &str, path: &str, body: &Value) -> Result<(u16, Value)> {
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("{addr}: {e}"))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let payload = body.to_string();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    )
    .map_err(|e| format!("{addr}: {e}"))?;
    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|e| format!("{addr}: {e}"))?;
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("no HTTP status")?;
    let value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body).map_err(|e| format!("{addr}{path}: {e}"))?
    };
    Ok((status, value))
}

/// A JSON-RPC call to the node.
pub fn rpc(method: &str, params: &Value) -> Result<Value> {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let (_, reply) = post(&format!("127.0.0.1:{RPC_PORT}"), "/", &request)?;
    match reply.get("error") {
        Some(error) => Err(format!("{method}: {error}")),
        None => Ok(reply.get("result").cloned().unwrap_or(Value::Null)),
    }
}

/// Waits until the node has a block above genesis.
pub fn wait_for_first_block() -> Result<()> {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    while Instant::now() < deadline {
        if rpc("get_block_by_height", &json!([1])).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("no block 1 within {BOOT_TIMEOUT:?}"))
}

/// Waits until something answers HTTP on `addr`.
pub fn wait_for_http(addr: &str) -> Result<()> {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    while Instant::now() < deadline {
        if TcpStream::connect(addr).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!("{addr} did not come up within {BOOT_TIMEOUT:?}"))
}

/// Sends `amount` to `to`; `false` on a refusal (a nonce race, typically).
pub fn send(work: &Path, to: &str, amount: u64) -> bool {
    output(wallet(work).args(["send", "--to", to, "--amount", &amount.to_string()])).is_ok()
}
