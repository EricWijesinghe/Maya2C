//! `maya2c-audit supply`: recompute supply and emission from a public RPC.
//!
//! Exit codes: 0 every check passed or does not apply; 1 a check failed;
//! 3 nothing failed but something could not be verified; 2 the audit itself
//! could not run (bad flags, unreadable genesis, unreachable node).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use maya2c_audit::genesis::parse_plan;
use maya2c_audit::rpc::HttpRpc;
use maya2c_audit::{AuditError, audit_supply};

/// Exit code for a run that could not start or finish.
const EXIT_ERROR: u8 = 2;

#[derive(Parser)]
#[command(
    name = "maya2c-audit",
    version,
    about = "Verify Maya2C supply from public RPC, without node code"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that no coins were printed and that the mining reserve paid only the scheduled emission.
    Supply(SupplyArgs),
}

#[derive(Args)]
struct SupplyArgs {
    /// Public JSON-RPC endpoint, e.g. https://rpc.example.org/
    #[arg(long)]
    rpc: String,
    /// The genesis file the chain was started from (its JSON, as the node reads it).
    #[arg(long)]
    genesis: PathBuf,
    /// First height to read (default 1). Emission checks need whole epochs inside the window.
    #[arg(long)]
    from: Option<u64>,
    /// Last height to read (default: the tip).
    #[arg(long)]
    to: Option<u64>,
    /// Print the report as JSON instead of text.
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Supply(args) => match run_supply(&args) {
            Ok(code) => ExitCode::from(code),
            Err(error) => {
                eprintln!("maya2c-audit: {error}");
                ExitCode::from(EXIT_ERROR)
            }
        },
    }
}

fn run_supply(args: &SupplyArgs) -> Result<u8, AuditError> {
    let text = std::fs::read_to_string(&args.genesis)
        .map_err(|e| AuditError::GenesisFile(format!("{}: {e}", args.genesis.display())))?;
    let plan = parse_plan(&text)?;
    let rpc = HttpRpc::new(&args.rpc)?;
    let report = audit_supply(&rpc, &plan, args.from, args.to)?;
    if args.json {
        let json =
            serde_json::to_string_pretty(&report).map_err(|e| AuditError::Output(e.to_string()))?;
        println!("{json}");
    } else {
        print!("{}", report.render());
    }
    Ok(u8::try_from(report.exit_code()).unwrap_or(EXIT_ERROR))
}
