//! `maya2c dev` — the instant local loop (Master Prompt 24 §2).
//!
//! One command: a one-validator DAG-BFT chain with pre-funded accounts, an
//! optional local explorer, and a contract that redeploys every time its
//! `.wasm` changes on disk. The chain has no fees and no staking, so a deploy
//! is one signed transaction and nothing else.
//!
//! Throwaway by construction: the directory is wiped on start, the keystores
//! share one printed password, and nothing binds beyond 127.0.0.1.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, anyhow, bail};
use custom_l1_node::core::payload::{ContractDeploy, derive_contract_id};
use custom_l1_node::core::{Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use l1_wallet::client::NodeClient;
use serde_json::json;

/// Password of every dev keystore. Dev keys fund nothing real.
pub const DEV_PASSWORD: &str = "maya2c-dev";
/// Balance of each pre-funded account, base units.
pub const DEV_BALANCE: u64 = 1_000_000_000;
/// The default `chain_id`.
pub const CHAIN_ID: &str = "maya2c-dev";
const POLL: Duration = Duration::from_millis(50);
const WATCH_POLL: Duration = Duration::from_millis(300);
const BOOT_TIMEOUT: Duration = Duration::from_secs(30);
const DEPLOY_TIMEOUT: Duration = Duration::from_secs(30);
const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// What `maya2c dev` should do.
#[derive(Clone, Debug)]
pub struct Options {
    /// Working directory; wiped on start.
    pub dir: PathBuf,
    /// Pre-funded accounts to create.
    pub accounts: usize,
    /// A contract to deploy now and on every change.
    pub watch: Option<PathBuf>,
    /// JSON-RPC port.
    pub rpc_port: u16,
    /// P2P port.
    pub p2p_port: u16,
    /// Explorer port, if an `explorer` binary sits next to `maya2c`.
    pub explorer_port: Option<u16>,
    /// Directory holding `maya2c-node` (and `explorer`).
    pub bin_dir: PathBuf,
    /// Stop after this many deployments (tests); `None` runs until killed.
    pub stop_after_deploys: Option<usize>,
    /// Extra `maya2c-node` flags, e.g. `--snapshot-interval 5` so the chain
    /// can be forked or replayed from.
    pub node_args: Vec<String>,
    /// The genesis `chain_id`: two dev chains side by side are two networks.
    pub chain_id: String,
    /// Turn the fee market on with this initial (and minimum) base fee per
    /// byte. `None` is a fee-free chain, the default for `maya2c dev`.
    pub base_fee: Option<u64>,
}

/// What happened, measured.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Process start to the node answering JSON-RPC.
    pub rpc_ready: Duration,
    /// Process start to block 1.
    pub first_block: Duration,
    /// Pre-funded account addresses, hex.
    pub accounts: Vec<String>,
    /// Each deployment: contract id (hex) and save-to-deployed time.
    pub deployments: Vec<(String, Duration)>,
}

/// Kills the node (and explorer) when dropped.
pub struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            // Already exited is fine.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn prepare(o: &Options) -> anyhow::Result<(Vec<(HybridSigningKey, String)>, PathBuf)> {
    if o.dir.exists() {
        std::fs::remove_dir_all(&o.dir).with_context(|| format!("clearing {}", o.dir.display()))?;
    }
    let node_dir = o.dir.join("node");
    std::fs::create_dir_all(&node_dir)?;
    let validator = Command::new(o.bin_dir.join(format!("maya2c-node{EXE}")))
        .arg("--generate-validator-key")
        .arg(node_dir.join("validator.key"))
        .output()
        .context("running maya2c-node --generate-validator-key")?;
    if !validator.status.success() {
        bail!("maya2c-node --generate-validator-key failed");
    }
    let mut accounts = Vec::with_capacity(o.accounts);
    for i in 0..o.accounts {
        let key = generate_signing_key().map_err(|e| anyhow!("keygen: {e}"))?;
        let path = o.dir.join(format!("account-{i}.key"));
        l1_wallet::keystore::save(&path, &l1_wallet::keystore::encrypt(&key, DEV_PASSWORD)?)?;
        let address = hex::encode(key.address());
        accounts.push((key, address));
    }
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut bft = json!({ "validators": [String::from_utf8_lossy(&validator.stdout).trim()], "anchor_timeout_ms": 500 });
    if let Some(fee) = o.base_fee {
        bft["fees"] = json!({ "initial_base_fee": fee, "min_base_fee": fee,
            "target_block_bytes": 2_621_440, "change_denominator": 8 });
    }
    let genesis = json!({
        "chain_id": o.chain_id, "timestamp": now, "difficulty_bits": 0, "pow_limit_bits": 0,
        "allocations": accounts.iter().map(|(_, a)| json!({ "address": a, "balance": DEV_BALANCE })).collect::<Vec<_>>(),
        "bft": bft
    });
    let genesis_path = o.dir.join("genesis.json");
    std::fs::write(&genesis_path, genesis.to_string())?;
    Ok((accounts, genesis_path))
}

fn spawn_node(o: &Options, genesis: &Path) -> anyhow::Result<Child> {
    let node_dir = o.dir.join("node");
    let log = std::fs::File::create(o.dir.join("node.log"))?;
    Command::new(o.bin_dir.join(format!("maya2c-node{EXE}")))
        .arg("--genesis")
        .arg(genesis)
        .arg("--data-dir")
        .arg(&node_dir)
        .args([
            "--rpc-addr",
            &format!("127.0.0.1:{}", o.rpc_port),
            "--p2p-port",
            &o.p2p_port.to_string(),
        ])
        .arg("--validator-key")
        .arg(node_dir.join("validator.key"))
        .args(&o.node_args)
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("starting maya2c-node")
}

async fn wait_until<F, Fut>(deadline: Instant, mut probe: F) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    while !probe().await {
        if Instant::now() > deadline {
            bail!("timed out");
        }
        tokio::time::sleep(POLL).await;
    }
    Ok(())
}

/// Deploys `code` from `key`, waiting until the node has committed it.
async fn deploy(
    client: &NodeClient,
    key: &HybridSigningKey,
    code: Vec<u8>,
) -> anyhow::Result<String> {
    let address = key.address();
    let nonce = client.get_balance(&hex::encode(address)).await?.nonce;
    let id = derive_contract_id(&address, nonce, &code);
    let mut tx = Transaction::with_kind(TxKind::DeployContract(ContractDeploy { code }), nonce);
    tx.sign(key)
        .map_err(|e| anyhow!("signing the deploy: {e}"))?;
    client
        .send_raw_transaction(&hex::encode(tx.to_bytes()))
        .await?;
    let deadline = Instant::now() + DEPLOY_TIMEOUT;
    wait_until(deadline, || async {
        client
            .get_balance(&hex::encode(address))
            .await
            .is_ok_and(|a| a.nonce > nonce)
    })
    .await
    .context("the deploy was not committed")?;
    Ok(hex::encode(id))
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A running local chain: stops when dropped.
pub struct LocalChain {
    /// The node (and explorer) processes.
    pub children: Children,
    /// Pre-funded keys and their hex addresses.
    pub accounts: Vec<(HybridSigningKey, String)>,
    /// A client for the node.
    pub client: NodeClient,
    /// What start-up measured.
    pub report: Report,
}

/// Starts the chain, waits for its first block, and prints how to use it.
///
/// # Errors
///
/// A missing binary, or a node that does not come up.
pub async fn launch(o: &Options) -> anyhow::Result<LocalChain> {
    let started = Instant::now();
    let (accounts, genesis) = prepare(o)?;
    let mut children = Children(vec![spawn_node(o, &genesis)?]);
    let url = format!("http://127.0.0.1:{}", o.rpc_port);
    let client = NodeClient::connect(&url)?;
    let deadline = started + BOOT_TIMEOUT;
    wait_until(deadline, || async {
        client.get_block_by_height(0).await.is_ok()
    })
    .await
    .context("node RPC")?;
    let mut report = Report {
        rpc_ready: started.elapsed(),
        ..Report::default()
    };
    wait_until(deadline, || async {
        client.get_block_by_height(1).await.is_ok()
    })
    .await
    .context("first block")?;
    report.first_block = started.elapsed();
    report.accounts = accounts.iter().map(|(_, a)| a.clone()).collect();
    println!(
        "maya2c dev: chain up — RPC in {:?}, first block in {:?}",
        report.rpc_ready, report.first_block
    );
    println!("  rpc       {url}");
    if let Some(port) = o.explorer_port {
        let explorer = o.bin_dir.join(format!("explorer{EXE}"));
        if explorer.exists() {
            let listen = format!("127.0.0.1:{port}");
            children.0.push(
                Command::new(explorer)
                    .args(["--node-rpc", &url, "--listen", &listen])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?,
            );
            println!("  explorer  http://{listen}");
        }
    }
    println!("  accounts  (keystore password \"{DEV_PASSWORD}\", {DEV_BALANCE} each)");
    for (i, (_, address)) in accounts.iter().enumerate() {
        println!(
            "    {i}  {address}  {}",
            o.dir.join(format!("account-{i}.key")).display()
        );
    }

    Ok(LocalChain {
        children,
        accounts,
        client,
        report,
    })
}

/// Runs the local loop until killed, or until `stop_after_deploys`.
///
/// # Errors
///
/// A missing binary, a node that does not come up, or a failed deploy.
pub async fn run(o: &Options) -> anyhow::Result<Report> {
    let local = launch(o).await?;
    let (client, accounts, mut report) = (&local.client, &local.accounts, local.report.clone());
    let Some(watch) = &o.watch else {
        tokio::signal::ctrl_c().await?;
        return Ok(report);
    };
    let deployer = &accounts
        .first()
        .ok_or_else(|| anyhow!("--watch needs at least one account"))?
        .0;
    let mut seen = None;
    loop {
        let now = modified(watch);
        if now.is_some() && now != seen {
            seen = now;
            let t = Instant::now();
            let code =
                std::fs::read(watch).with_context(|| format!("reading {}", watch.display()))?;
            match deploy(client, deployer, code).await {
                Ok(id) => {
                    println!(
                        "  deployed  {} -> contract {id} in {:?}",
                        watch.display(),
                        t.elapsed()
                    );
                    report.deployments.push((id, t.elapsed()));
                }
                Err(e) => println!("  deploy of {} failed: {e:#}", watch.display()),
            }
            if o.stop_after_deploys
                .is_some_and(|n| report.deployments.len() >= n)
            {
                return Ok(report);
            }
        }
        tokio::select! {
            () = tokio::time::sleep(WATCH_POLL) => {}
            _ = tokio::signal::ctrl_c() => return Ok(report),
        }
    }
}
