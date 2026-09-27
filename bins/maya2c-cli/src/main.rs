//! `maya2c` — the developer CLI.
//!
//! ```text
//! maya2c dev [--watch contract.wasm] [--accounts N] [--dir DIR]
//! maya2c debug <contract.wasm> [--input HEX] [--caller HEX] [--height N]
//!              [--gas N] [--script "n;n;s;b;e"]
//! ```

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use maya_vm::host::MemoryState;

/// The contract id a debugged call runs as; storage is keyed by it.
const DEBUG_CONTRACT: [u8; 32] = [0xDB; 32];

#[derive(Parser)]
#[command(name = "maya2c", version, about = "Maya2C developer CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
    let bytes = std::fs::read(wasm).with_context(|| format!("reading {}", wasm.display()))?;
    let input = hex::decode(input.trim_start_matches("0x")).context("--input is not hex")?;
    let mut state = MemoryState::at_height(height);
    if let Some(c) = caller {
        let raw = hex::decode(c.trim_start_matches("0x")).context("--caller is not hex")?;
        state.caller = Some(
            raw.try_into()
                .map_err(|_| anyhow::anyhow!("--caller must be 32 bytes"))?,
        );
    }
    let (mut session, _) = maya2c_cli::record(&bytes, DEBUG_CONTRACT, &input, gas, state)?;
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

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
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
