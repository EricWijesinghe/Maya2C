//! `maya2c` — the developer CLI.
//!
//! ```text
//! maya2c dev [--watch contract.wasm] [--accounts N] [--dir DIR]
//! maya2c replay --from URL --genesis FILE <height> [--snapshot-depth N]
//! maya2c fork   --from URL --genesis FILE [--rpc-port P] [--snapshot-depth N]
//! maya2c debug <contract.wasm> [--input HEX] [--caller HEX] [--height N]
//!              [--gas N] [--script "n;n;s;b;e"]
//! maya2c dap                 # Debug Adapter Protocol on stdio, for editors
//! maya2c custody-report <ADDR>... --from H --to H [--csv]
//! ```

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use maya2c_cli::CallSpec;

#[derive(Parser)]
#[command(name = "maya2c", version, about = "Maya2C developer CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show an account's vault: policy, requests, and what it protects.
    Vault {
        /// The account, hex.
        address: String,
        /// Node JSON-RPC.
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        rpc: String,
    },
    /// A custodian's statement: every balance movement of the given accounts
    /// over a height range, reconciled, anchored to block ids and digested.
    CustodyReport {
        /// Accounts, hex.
        #[arg(required = true)]
        addresses: Vec<String>,
        /// First block covered (at least 1).
        #[arg(long)]
        from: u64,
        /// Last block covered.
        #[arg(long)]
        to: u64,
        /// Emit CSV movements instead of the JSON statement.
        #[arg(long)]
        csv: bool,
        /// Node JSON-RPC.
        #[arg(long, default_value = "http://127.0.0.1:8545")]
        rpc: String,
    },
    /// Serve the Debug Adapter Protocol on stdin/stdout, so an editor (the
    /// VS Code extension in `editors/vscode`) drives the time-travel debugger.
    Dap,
    /// Re-execute a block of another network locally; succeeds only if the
    /// state root it declares is reproduced.
    Replay {
        /// Source node JSON-RPC.
        #[arg(long)]
        from: String,
        /// The network's genesis file.
        #[arg(long)]
        genesis: PathBuf,
        /// Block height to replay.
        height: u64,
        /// Working directory, wiped on start.
        #[arg(long, default_value = ".maya2c-replay")]
        dir: PathBuf,
        /// Start from the source's snapshot at least this deep instead of
        /// executing every block from genesis.
        #[arg(long)]
        snapshot_depth: Option<u64>,
    },
    /// Copy another network's state locally and extend it with local blocks.
    Fork {
        /// Source node JSON-RPC.
        #[arg(long)]
        from: String,
        /// The network's genesis file.
        #[arg(long)]
        genesis: PathBuf,
        /// Working directory, wiped on start.
        #[arg(long, default_value = ".maya2c-fork")]
        dir: PathBuf,
        /// Local JSON-RPC port.
        #[arg(long, default_value_t = 8546)]
        rpc_port: u16,
        /// As for `replay`.
        #[arg(long)]
        snapshot_depth: Option<u64>,
    },
    /// A local chain with pre-funded accounts; redeploys `--watch` on save.
    Dev {
        /// Contract to deploy now and on every change.
        #[arg(long)]
        watch: Option<PathBuf>,
        /// Pre-funded accounts.
        #[arg(long, default_value_t = 5)]
        accounts: usize,
        /// Working directory, wiped on start.
        #[arg(long, default_value = ".maya2c-dev")]
        dir: PathBuf,
        /// JSON-RPC port.
        #[arg(long, default_value_t = 8545)]
        rpc_port: u16,
        /// P2P port.
        #[arg(long, default_value_t = 30333)]
        p2p_port: u16,
        /// Explorer port (needs `explorer` next to `maya2c`).
        #[arg(long, default_value_t = 3000)]
        explorer_port: u16,
    },
    /// Run a contract call once and step through it, forward and backward.
    Debug {
        /// The contract's wasm.
        wasm: PathBuf,
        /// Call input, hex.
        #[arg(long, default_value = "")]
        input: String,
        /// The signer the contract sees, hex (32 bytes).
        #[arg(long)]
        caller: Option<String>,
        /// Block height the contract sees.
        #[arg(long, default_value_t = 1)]
        height: u64,
        /// Gas limit.
        #[arg(long, default_value_t = 10_000_000)]
        gas: u64,
        /// Commands separated by `;`, instead of an interactive prompt.
        #[arg(long)]
        script: Option<String>,
    },
}

fn debug(
    wasm: &PathBuf,
    input: &str,
    caller: Option<&str>,
    height: u64,
    gas: u64,
    script: Option<&str>,
) -> anyhow::Result<()> {
    let spec = CallSpec {
        wasm: std::fs::read(wasm).with_context(|| format!("reading {}", wasm.display()))?,
        input: maya2c_cli::parse_hex("--input", input)?,
        caller: caller.map(maya2c_cli::parse_caller).transpose()?,
        height,
        gas,
    };
    let mut session = maya2c_cli::record_spec(&spec)?;
    println!("{}", session.command("list").unwrap_or_default());
    if let Some(script) = script {
        for line in script.split(';') {
            match session.command(line.trim()) {
                Some(out) => println!("> {}\n{out}", line.trim()),
                None => break,
            }
        }
        return Ok(());
    }
    let stdin = std::io::stdin();
    loop {
        print!("(maya2c debug) ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            return Ok(());
        }
        match session.command(line.trim()) {
            Some(out) => println!("{out}"),
            None => return Ok(()),
        }
    }
}

fn replay(source: &maya2c_cli::fork::Source, height: u64) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let handle = runtime.handle().clone();
    let r = maya2c_cli::fork::replay(source, height, handle)?;
    println!("replayed block {} ({})", r.height, r.block_id);
    println!("  state root {} — reproduced", r.state_root);
    for txid in &r.txids {
        println!("  tx {txid}");
    }
    for c in &r.changes {
        println!("  {}  {} -> {}", hex::encode(c.address), c.before, c.after);
    }
    Ok(())
}

async fn fork(source: maya2c_cli::fork::Source, rpc_port: u16) -> anyhow::Result<()> {
    let from = source.url.clone();
    let fork = maya2c_cli::fork::start(source, ([127, 0, 0, 1], rpc_port).into()).await?;
    println!("forked {from} at height {}", fork.forked_at);
    println!(
        "  rpc http://{} — local blocks every {:?} when the mempool is not empty",
        fork.server.address,
        maya2c_cli::fork::FORK_BLOCK_INTERVAL
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn vault(rpc: &str, address: &str) -> anyhow::Result<()> {
    match maya2c_cli::vault::status(rpc, address).await? {
        None => println!("{address} has no vault: its transfers are instant and final."),
        Some(v) => println!("{}", serde_json::to_string_pretty(&v)?),
    }
    println!(
        "
{}",
        maya2c_cli::vault::RISK_LABEL
    );
    Ok(())
}

async fn custody_report(
    rpc: &str,
    addresses: &[String],
    from: u64,
    to: u64,
    csv: bool,
) -> anyhow::Result<()> {
    let report = maya2c_cli::custody_report::fetch(rpc, addresses, from, to).await?;
    if csv {
        print!("{}", report.csv());
    } else {
        println!("{}", String::from_utf8(report.canonical()?)?);
    }
    eprintln!("sha256 {}", report.digest()?);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Dap => maya2c_cli::dap::serve(std::io::stdin().lock(), std::io::stdout().lock()),
        Command::CustodyReport {
            addresses,
            from,
            to,
            csv,
            rpc,
        } => tokio::runtime::Runtime::new()?
            .block_on(custody_report(&rpc, &addresses, from, to, csv)),
        Command::Vault { address, rpc } => {
            tokio::runtime::Runtime::new()?.block_on(vault(&rpc, &address))
        }
        Command::Replay {
            from,
            genesis,
            height,
            dir,
            snapshot_depth,
        } => replay(
            &maya2c_cli::fork::Source {
                url: from,
                genesis,
                dir,
                snapshot_depth,
            },
            height,
        ),
        Command::Fork {
            from,
            genesis,
            dir,
            rpc_port,
            snapshot_depth,
        } => tokio::runtime::Runtime::new()?.block_on(fork(
            maya2c_cli::fork::Source {
                url: from,
                genesis,
                dir,
                snapshot_depth,
            },
            rpc_port,
        )),
        Command::Dev {
            watch,
            accounts,
            dir,
            rpc_port,
            p2p_port,
            explorer_port,
        } => {
            let bin_dir = std::env::current_exe()?
                .parent()
                .map(std::path::Path::to_path_buf)
                .context("locating the maya2c executable")?;
            let options = maya2c_cli::dev::Options {
                dir,
                accounts,
                watch,
                rpc_port,
                p2p_port,
                explorer_port: Some(explorer_port),
                bin_dir,
                stop_after_deploys: None,
                node_args: Vec::new(),
                chain_id: maya2c_cli::dev::CHAIN_ID.to_string(),
                base_fee: None,
            };
            tokio::runtime::Runtime::new()?.block_on(maya2c_cli::dev::run(&options))?;
            Ok(())
        }
        Command::Debug {
            wasm,
            input,
            caller,
            height,
            gas,
            script,
        } => debug(
            &wasm,
            &input,
            caller.as_deref(),
            height,
            gas,
            script.as_deref(),
        ),
    }
}
