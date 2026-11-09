//! `cargo xtask ceremony-rehearsal` — the mainnet genesis ceremony, end to end,
//! with throwaway keys: the steps of `deploy/genesis/mainnet/README.md` and
//! `docs/runbooks/regenesis-checklist.md`, then the chain that genesis makes,
//! run as seven validators on this machine.
//!
//! 1. Seven sealed ML-DSA-87 validator keys (`maya2c-node
//!    --generate-validator-key`, production build), seven operator payout
//!    addresses, three council keys (`genesis-ceremony council key`).
//! 2. `genesis-ceremony` on `maya-mainnet` with `--supply 0`, the mining
//!    reserve priced from `mining.json` by the rule `genesis-check` applies,
//!    and every bond exactly `--min-self-bond`.
//! 3. `cargo xtask genesis-check` on the output.
//! 4. Seven validators from that genesis, and the checks: one chain with
//!    blocks committed, `get_chain_info` names `maya-mainnet`, `get_supply`
//!    equals reserve + 7 x bond, addresses print as `maya1…`, and (unless
//!    `--no-mining`) a mined share is paid from the reserve with nothing
//!    minted. Mining opens at the first epoch boundary, 3,600 blocks in.
//!
//! Every check runs and is printed; the command fails if any did. Nothing here
//! is a mainnet key: the passphrase is a fixed throwaway string and the work
//! directory (default `$TEMP/ceremony-rehearsal`) is wiped on the next run.
//!
//! Options: `--work DIR`, `--no-mining`, `--mining-timeout-mins N` (default
//! 150), `--bond-coins N` (default: the mainnet floor in `genesis_check.rs`).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use maya_amount::Amount;
use serde_json::{Value, json};

use crate::devnet::{self, Procs, Result};
use crate::genesis_check;

const VALIDATORS: u16 = 7;
const COUNCIL: [&str; 3] = ["council-a", "council-b", "council-c"];
const CHAIN_ID: &str = "maya-mainnet";
/// The mainnet staking epoch (`genesis-ceremony`'s default, which the
/// mining schedule's halving and tail were computed for).
const EPOCH_BLOCKS: u64 = 3_600;
/// ADR-050: six hours of blocks at about one block a second.
const MAX_PAUSE_BLOCKS: &str = "21600";
/// Clear of the live testnet ports and Windows' reserved 31249-31448.
const P2P_BASE: u16 = 36_100;
const RPC_BASE: u16 = 36_200;
/// Throwaway: it seals keys that live in a temp directory and nowhere else.
const PASSPHRASE: &str = "ceremony-rehearsal-throwaway-passphrase";
const MINER_NODE: u16 = 1;
const POLL: Duration = Duration::from_secs(2);
/// The reserve address: "maya2c/mining/reserve/v1" zero-padded to 32 bytes.
const RESERVE_ADDRESS: &str = "6d61796132632f6d696e696e672f726573657276652f76310000000000000000";

struct Opts {
    work: PathBuf,
    mining: bool,
    mining_timeout: Duration,
    bond_coins: u128,
}

fn parse(args: &[String]) -> Result<Opts> {
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    let mins: u64 = value("--mining-timeout-mins")
        .map_or(Ok(150), |s| s.parse())
        .map_err(|e| format!("--mining-timeout-mins: {e}"))?;
    Ok(Opts {
        work: value("--work").map_or_else(
            || std::env::temp_dir().join("ceremony-rehearsal"),
            PathBuf::from,
        ),
        mining: !args.iter().any(|a| a == "--no-mining"),
        mining_timeout: Duration::from_secs(mins * 60),
        bond_coins: value("--bond-coins")
            .map_or(
                Ok(genesis_check::MIN_SELF_BOND / maya_amount::ONE_COIN),
                |s| s.parse(),
            )
            .map_err(|e| format!("--bond-coins: {e}"))?,
    })
}

/// The production node: what `cargo xtask release-check` builds (profile `ci`).
fn node_bin() -> PathBuf {
    std::env::var_os(devnet::BIN_DIR_ENV)
        .map_or_else(|| devnet::root().join("target").join("ci"), PathBuf::from)
        .join(format!("maya2c-node{}", std::env::consts::EXE_SUFFIX))
}

fn build() -> Result<()> {
    if std::env::var_os(devnet::BIN_DIR_ENV).is_some() {
        return Ok(());
    }
    devnet::build(&["genesis-ceremony", "l1-wallet"])?;
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "build",
            "--profile",
            "ci",
            "-p",
            "maya2c-node",
            "--no-default-features",
        ])
        .args(["--features", "maya2c-node/production"])
        .current_dir(devnet::root())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("cargo build node: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("production node build failed: {status}"))
}

fn rpc(i: u16, method: &str, params: &Value) -> Result<Value> {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let (_, reply) = devnet::post(&format!("127.0.0.1:{}", RPC_BASE + i), "/", &request)?;
    match reply.get("error") {
        Some(error) => Err(format!("node {i} {method}: {error}")),
        None => Ok(reply.get("result").cloned().unwrap_or(Value::Null)),
    }
}

fn wait(what: &str, limit: Duration, mut check: impl FnMut() -> bool) -> Result<Duration> {
    let started = Instant::now();
    while started.elapsed() < limit {
        if check() {
            return Ok(started.elapsed());
        }
        std::thread::sleep(POLL);
    }
    Err(format!("{what}: not within {limit:?}"))
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Keys, addresses and council halves; returns the `validators.txt` path.
fn keys(work: &Path, opts: &Opts) -> Result<PathBuf> {
    let pass = work.join("passphrase");
    std::fs::write(&pass, PASSPHRASE).map_err(|e| format!("passphrase: {e}"))?;
    let bond = opts.bond_coins * maya_amount::ONE_COIN;
    let mut lines = vec!["# label key operator bond".to_owned()];
    for i in 0..VALIDATORS {
        let dir = work.join(format!("v{i}"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let key = devnet::output(
            Command::new(node_bin())
                .arg("--generate-validator-key")
                .arg(dir.join("validator.key"))
                .arg("--validator-passphrase-file")
                .arg(&pass),
        )?;
        let wallet = devnet::output(
            Command::new(devnet::bin("l1-wallet"))
                .env("L1_WALLET_PASSWORD", PASSPHRASE)
                .arg("--keystore")
                .arg(dir.join("operator.key"))
                .arg("generate"),
        )?;
        let operator = devnet::field(&wallet, "address").ok_or("l1-wallet printed no address")?;
        lines.push(format!("v{i} {} {operator} {bond}", key.trim()));
    }
    let validators = work.join("validators.txt");
    std::fs::write(&validators, lines.join("\n") + "\n")
        .map_err(|e| format!("validators.txt: {e}"))?;
    let (keys_dir, public) = (work.join("council-keys"), work.join("council-public"));
    std::fs::create_dir_all(&public).map_err(|e| format!("{}: {e}", public.display()))?;
    for label in COUNCIL {
        devnet::output(
            Command::new(devnet::bin("genesis-ceremony"))
                .args([
                    "council",
                    "key",
                    "--label",
                    label,
                    "--passphrase-file",
                    &path(&pass),
                ])
                .args(["--out-dir", &path(&keys_dir)]),
        )?;
        let name = format!("{label}.council.public");
        std::fs::copy(keys_dir.join(&name), public.join(&name))
            .map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(validators)
}

/// `genesis-ceremony` with the decided mainnet values; returns the reserve.
fn ceremony(work: &Path, validators: &Path, bond: u128) -> Result<String> {
    let mining = devnet::root().join("deploy/genesis/mainnet/mining.json");
    let text =
        std::fs::read_to_string(&mining).map_err(|e| format!("{}: {e}", mining.display()))?;
    let reserve = genesis_check::mining_reserve(&text, EPOCH_BLOCKS)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let sheet = devnet::output(
        Command::new(devnet::bin("genesis-ceremony"))
            .args([
                "--chain-id",
                CHAIN_ID,
                "--supply",
                "0",
                "--validators",
                &path(validators),
            ])
            .args(["--mining", &path(&mining), "--mining-reserve", &reserve])
            .args([
                "--council",
                &path(&work.join("council-public")),
                "--council-threshold",
                "2",
            ])
            .args(["--council-max-pause-blocks", MAX_PAUSE_BLOCKS])
            .args(["--min-self-bond", &bond.to_string()])
            .args(["--min-delegation", &maya_amount::ONE_COIN.to_string()])
            .args([
                "--timestamp",
                &now.to_string(),
                "--out-dir",
                &path(&work.join("out")),
            ]),
    )?;
    print!("{sheet}");
    Ok(reserve)
}

fn start(work: &Path, i: u16) -> Result<Child> {
    let dir = work.join(format!("v{i}"));
    let log =
        std::fs::File::create(dir.join("node.log")).map_err(|e| format!("node {i} log: {e}"))?;
    let mut cmd = Command::new(node_bin());
    cmd.arg("--genesis")
        .arg(work.join("out/genesis.json"))
        .arg("--data-dir")
        .arg(&dir)
        .args(["--rpc-addr", &format!("127.0.0.1:{}", RPC_BASE + i)])
        .args(["--p2p-port", &(P2P_BASE + i).to_string()])
        .arg("--validator-key")
        .arg(dir.join("validator.key"))
        .arg("--validator-passphrase-file")
        .arg(work.join("passphrase"))
        .arg("--allow-private-peers");
    for peer in (0..VALIDATORS).filter(|p| *p != i) {
        cmd.args([
            "--bootnode",
            &format!("/ip4/127.0.0.1/tcp/{}", P2P_BASE + peer),
        ]);
    }
    cmd.stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("starting node {i}: {e}"))
}

fn height(i: u16) -> Option<u64> {
    rpc(i, "get_chain_info", &json!([])).ok()?["height"].as_u64()
}

/// Every node holds the same block id at the lowest tip, at or above `min`.
fn agree(min: u64) -> Option<(u64, String)> {
    let low = (0..VALIDATORS)
        .map(height)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min()?;
    if low < min {
        return None;
    }
    let ids: Vec<Value> = (0..VALIDATORS)
        .map(|i| rpc(i, "get_block_by_height", &json!([low])).map(|b| b["header"]["id"].clone()))
        .collect::<Result<_>>()
        .ok()?;
    let first = ids[0].as_str()?.to_owned();
    ids.iter().all(|id| *id == ids[0]).then_some((low, first))
}

fn amount(v: &Value) -> Result<Amount> {
    genesis_check::amount_of(v).ok_or_else(|| format!("{v} is not an amount"))
}

fn balance(i: u16, address: &str) -> Result<Amount> {
    amount(&rpc(i, "get_balance", &json!([address]))?["balance"])
}

/// Node 0's total supply (`get_supply`: accounts, shielded pool and
/// everything staking holds) and the stake its validators hold
/// (`get_validators`: self bond plus delegations).
fn money() -> Result<(Amount, Amount)> {
    let total = amount(&rpc(0, "get_supply", &json!([]))?["total"])?;
    let info = rpc(0, "get_validators", &json!([]))?;
    let rows = info["validators"]
        .as_array()
        .ok_or_else(|| format!("get_validators: {info}"))?;
    let mut staked = Amount::ZERO;
    for v in rows {
        for field in ["self_bond", "delegated"] {
            staked = staked
                .checked_add(amount(&v[field])?)
                .ok_or("stake overflow")?;
        }
    }
    Ok((total, staked))
}

struct Checks(Vec<String>);

impl Checks {
    fn check(&mut self, ok: bool, what: String) {
        println!(
            "ceremony-rehearsal: [{}] {what}",
            if ok { "ok" } else { "FAIL" }
        );
        if !ok {
            self.0.push(what);
        }
    }
}

/// Runs the rehearsal.
///
/// # Errors
///
/// A step that cannot run, or the list of checks that failed.
pub fn run(args: &[String]) -> Result<()> {
    let opts = parse(args)?;
    let work = opts.work.clone();
    rehearse(&opts).map_err(|e| format!("{e} (work dir and logs: {})", work.display()))
}

fn rehearse(opts: &Opts) -> Result<()> {
    let work = &opts.work;
    for port in (0..VALIDATORS).flat_map(|i| [P2P_BASE + i, RPC_BASE + i]) {
        std::net::TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| format!("port {port} is in use: {e}"))?;
    }
    build()?;
    if work.exists() {
        std::fs::remove_dir_all(work).map_err(|e| format!("clearing {}: {e}", work.display()))?;
    }
    std::fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
    let bond = opts.bond_coins * maya_amount::ONE_COIN;
    let validators = keys(work, opts)?;
    let reserve = ceremony(work, &validators, bond)?;
    let genesis = path(&work.join("out/genesis.json"));
    genesis_check::run(std::slice::from_ref(&genesis))?;

    let mut checks = Checks(Vec::new());
    let mut nodes = Procs::default();
    for i in 0..VALIDATORS {
        nodes.push(start(work, i)?);
    }
    let started = Instant::now();
    let mut tip = None;
    let t = wait(
        "seven validators on one chain",
        Duration::from_secs(300),
        || {
            tip = agree(5);
            tip.is_some()
        },
    )?;
    let (h, id) = tip.ok_or("no common tip")?;
    checks.check(
        true,
        format!("7 validators agree on block {id} at height {h}, {t:?} after start"),
    );
    println!(
        "ceremony-rehearsal: bft status {}",
        rpc(0, "get_bft_status", &json!([]))?
    );

    let info = rpc(0, "get_chain_info", &json!([]))?;
    println!("ceremony-rehearsal: genesis id {}", info["genesis"]);
    checks.check(
        info["chain_id"] == CHAIN_ID,
        format!("get_chain_info chain_id {}", info["chain_id"]),
    );

    let expected = amount(&json!(reserve))?
        .checked_add(
            Amount::from_u128(bond)
                .checked_mul(Amount::from_u64(u64::from(VALIDATORS)))
                .ok_or("bonds overflow")?,
        )
        .ok_or("supply overflow")?;
    let supply = rpc(0, "get_supply", &json!([]))?;
    println!("ceremony-rehearsal: get_supply {supply}");
    let (total, staked) = money()?;
    let bonds = Amount::from_u128(bond)
        .checked_mul(Amount::from_u64(u64::from(VALIDATORS)))
        .ok_or("bonds overflow")?;
    checks.check(
        staked == bonds,
        format!("get_validators stake {staked} == 7 x {bond} = {bonds}"),
    );
    // get_supply counted accounts only until 2026-11-09; it now adds what
    // staking holds, so this is the genesis total supply.
    checks.check(
        total == expected,
        format!("get_supply total {total} == reserve {reserve} + 7 x {bond} = {expected}"),
    );

    let text = rpc(0, "get_balance", &json!([RESERVE_ADDRESS]))?["address_text"].clone();
    checks.check(
        text.as_str().is_some_and(|t| t.starts_with("maya1")),
        format!("reserve address_text {text} starts with maya1 (ADR-081: maya-mainnet must be registered in Network::for_chain_id)"),
    );

    if opts.mining {
        mine(work, opts, &expected, &mut checks)?;
    }
    let h = (0..VALIDATORS).filter_map(height).min().unwrap_or(0);
    checks.check(
        agree(h).is_some(),
        format!(
            "all 7 still agree at height {h}, {:?} after start",
            started.elapsed()
        ),
    );
    drop(nodes);
    if checks.0.is_empty() {
        println!("ceremony-rehearsal: PASS");
        Ok(())
    } else {
        Err(format!(
            "ceremony-rehearsal: {} check(s) failed:\n  {}",
            checks.0.len(),
            checks.0.join("\n  ")
        ))
    }
}

/// A CPU miner on node 1; waits for the first paid share, then checks that
/// the payout came from the reserve and nothing was minted.
fn mine(work: &Path, opts: &Opts, supply: &Amount, checks: &mut Checks) -> Result<()> {
    let miner_bin = crate::mining_localnet::build_miner(false)?;
    let log = work.join("miner.jsonl");
    let mut miner = Procs::default();
    miner.push(
        Command::new(miner_bin)
            .args(["--mine", "--json", "--threads", "4", "--rpc"])
            .arg(format!("http://127.0.0.1:{}", RPC_BASE + MINER_NODE))
            .arg("--key-file")
            .arg(work.join("miner.key"))
            .stdout(std::fs::File::create(&log).map_err(|e| e.to_string())?)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting miner: {e}"))?,
    );
    let address = crate::mining_localnet::miner_address(&log)?;
    println!(
        "ceremony-rehearsal: mining opens at height {EPOCH_BLOCKS}; waiting up to {:?}",
        opts.mining_timeout
    );
    let before = balance(0, RESERVE_ADDRESS)?;
    let t = wait("the first paid share", opts.mining_timeout, || {
        balance(0, &address).is_ok_and(|b| b > Amount::ZERO)
    })?;
    drop(miner);
    let paid = balance(0, &address)?;
    let after = balance(0, RESERVE_ADDRESS)?;
    let (total, _) = money()?;
    checks.check(
        paid > Amount::ZERO,
        format!("miner {address} paid {paid} after {t:?}"),
    );
    let fell = before.checked_sub(after).unwrap_or(Amount::ZERO);
    checks.check(
        fell >= paid,
        format!("reserve fell {fell} (>= miner's {paid})"),
    );
    checks.check(
        total == *supply,
        format!("supply after payout {total} == genesis supply {supply}: nothing minted"),
    );
    Ok(())
}
