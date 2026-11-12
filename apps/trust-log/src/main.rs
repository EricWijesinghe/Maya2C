//! `maya2c-trust`: sign and check release statements, incident records and
//! the foundation canary.
//!
//! ```text
//! maya2c-trust keygen --out SEED_FILE
//! maya2c-trust release sign   --key SEED --tag TAG --sums SHA256SUMS --out STATEMENT
//! maya2c-trust release verify --pubkey PK --tag TAG --sums SHA256SUMS --statement STATEMENT [ANCHOR]
//! maya2c-trust incident append --key SEED --dir DIR --draft DRAFT.json
//! maya2c-trust incident verify --pubkey PK --dir DIR [ANCHOR]
//! maya2c-trust canary sign  --key SEED --rpc URL --statement-file TEXT --out CANARY
//! maya2c-trust canary check --pubkey PK --rpc URL --canary CANARY --max-age-blocks N [ANCHOR]
//!
//! ANCHOR = --rpc URL --anchor-height H --anchor-txid TXID
//! ```
//!
//! Every check exits 1 on a mismatch and prints why; nothing is installed,
//! deleted or stopped. Seeds are read from files, never flags, and never
//! printed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use maya_trust_log::anchor::{self, Anchor, Location};
use maya_trust_log::chain::Rpc;
use maya_trust_log::key::{self, SignerKey};
use maya_trust_log::{Error, Result, canary, incident, release};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Parser)]
#[command(
    name = "maya2c-trust",
    about = "Signed, chain-anchored foundation statements"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate an ML-DSA-87 seed file and print its public key and address.
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// release-log-anchor.
    #[command(subcommand)]
    Release(ReleaseCmd),
    /// signed-incidents.
    #[command(subcommand)]
    Incident(IncidentCmd),
    /// foundation-canary.
    #[command(subcommand)]
    Canary(CanaryCmd),
}

#[derive(Subcommand)]
enum ReleaseCmd {
    Sign {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        sums: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    Verify {
        #[arg(long)]
        pubkey: PathBuf,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        sums: PathBuf,
        #[arg(long)]
        statement: PathBuf,
        #[command(flatten)]
        anchor: AnchorArgs,
    },
}

#[derive(Subcommand)]
enum IncidentCmd {
    Append {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        draft: PathBuf,
    },
    Verify {
        #[arg(long)]
        pubkey: PathBuf,
        #[arg(long)]
        dir: PathBuf,
        #[command(flatten)]
        anchor: AnchorArgs,
    },
}

#[derive(Subcommand)]
enum CanaryCmd {
    Sign {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        rpc: String,
        #[arg(long)]
        statement_file: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    Check {
        #[arg(long)]
        pubkey: PathBuf,
        #[arg(long)]
        rpc: String,
        #[arg(long)]
        canary: PathBuf,
        #[arg(long)]
        max_age_blocks: u64,
        #[arg(long)]
        anchor_height: Option<u64>,
        #[arg(long)]
        anchor_txid: Option<String>,
    },
}

/// Where the record's anchor landed. All three or none.
#[derive(Args)]
struct AnchorArgs {
    #[arg(long)]
    rpc: Option<String>,
    #[arg(long)]
    anchor_height: Option<u64>,
    #[arg(long)]
    anchor_txid: Option<String>,
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("maya2c-trust: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Keygen { out } => {
            let (k, seed) = SignerKey::generate()?;
            key::write_seed(&out, &seed)?;
            println!("public key: {}", hex::encode(k.public_key()));
            println!("address:    {}", hex::encode(k.address()));
            Ok(())
        }
        Command::Release(cmd) => run_release(cmd),
        Command::Incident(cmd) => run_incident(cmd),
        Command::Canary(cmd) => run_canary(cmd),
    }
}

fn run_release(cmd: ReleaseCmd) -> Result<()> {
    match cmd {
        ReleaseCmd::Sign {
            key,
            tag,
            sums,
            out,
        } => {
            let (statement, anchor) = release::sign(&SignerKey::load(&key)?, &tag, &read(&sums)?)?;
            write_json(&out, &statement)?;
            println!("{}", anchor.instructions());
            Ok(())
        }
        ReleaseCmd::Verify {
            pubkey,
            tag,
            sums,
            statement,
            anchor,
        } => {
            let pinned = read_pubkey(&pubkey)?;
            let found = release::verify(&read_json(&statement)?, &pinned, &tag, &read(&sums)?)
                .inspect_err(|e| eprintln!("WARNING: do not install {tag}: {e}"))?;
            check_anchor(&anchor, &found, &pinned)?;
            println!("release {tag}: SHA256SUMS matches the signed statement");
            Ok(())
        }
    }
}

fn run_incident(cmd: IncidentCmd) -> Result<()> {
    match cmd {
        IncidentCmd::Append { key, dir, draft } => {
            let (path, anchor) =
                incident::append(&dir, &SignerKey::load(&key)?, read_json(&draft)?)?;
            println!("wrote {}", path.display());
            println!("{}", anchor.instructions());
            Ok(())
        }
        IncidentCmd::Verify {
            pubkey,
            dir,
            anchor,
        } => {
            let pinned = read_pubkey(&pubkey)?;
            let records = incident::load(&dir)?;
            let head = incident::verify_chain(&records, &pinned)?;
            check_anchor(
                &anchor,
                &Anchor::new(incident::SCHEMA.to_owned(), head)?,
                &pinned,
            )?;
            println!(
                "incident log: {} records, head {}",
                records.len(),
                hex::encode(head)
            );
            Ok(())
        }
    }
}

fn run_canary(cmd: CanaryCmd) -> Result<()> {
    match cmd {
        CanaryCmd::Sign {
            key,
            rpc,
            statement_file,
            out,
        } => {
            let text =
                std::fs::read_to_string(&statement_file).map_err(|e| io(&statement_file, &e))?;
            let (c, anchor) = canary::sign(&SignerKey::load(&key)?, &Rpc::new(&rpc)?, &text)?;
            write_json(&out, &c)?;
            println!("{}", anchor.instructions());
            Ok(())
        }
        CanaryCmd::Check {
            pubkey,
            rpc,
            canary: path,
            max_age_blocks,
            anchor_height,
            anchor_txid,
        } => {
            let pinned = read_pubkey(&pubkey)?;
            let c: maya_trust_log::signed::Signed<canary::Canary> = read_json(&path)?;
            let found = canary::check(&c, &pinned, &Rpc::new(&rpc)?, max_age_blocks)?;
            let args = AnchorArgs {
                rpc: Some(rpc),
                anchor_height,
                anchor_txid,
            };
            check_anchor(&args, &found, &pinned)?;
            println!(
                "canary: valid, names block {} ({})",
                c.body.block_height, c.body.block_id
            );
            Ok(())
        }
    }
}

/// Checks the anchor when its location is given, and says so when it is not.
fn check_anchor(args: &AnchorArgs, expected: &Anchor, pinned: &[u8]) -> Result<()> {
    match (&args.rpc, args.anchor_height, &args.anchor_txid) {
        (Some(rpc), Some(height), Some(txid)) => {
            let at = Location {
                height,
                txid: txid.clone(),
            };
            anchor::verify(&Rpc::new(rpc)?, expected, &key::address_of(pinned), &at)?;
            println!("anchored at height {height} in tx {txid}");
            Ok(())
        }
        (_, None, None) => {
            println!(
                "NOT CHECKED: no --anchor-height/--anchor-txid, so the on-chain anchor was not verified"
            );
            Ok(())
        }
        _ => Err(Error::Malformed(
            "--rpc, --anchor-height and --anchor-txid go together".into(),
        )),
    }
}

fn io(path: &Path, e: &std::io::Error) -> Error {
    Error::Io(format!("{}: {e}", path.display()))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| io(path, &e))
}

fn read_pubkey(path: &Path) -> Result<Vec<u8>> {
    let text = std::fs::read_to_string(path).map_err(|e| io(path, &e))?;
    hex::decode(text.trim()).map_err(|e| Error::Malformed(format!("{}: {e}", path.display())))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|e| io(path, &e))?;
    serde_json::from_str(&text).map_err(|e| Error::Malformed(format!("{}: {e}", path.display())))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(|e| Error::Malformed(e.to_string()))?;
    std::fs::write(path, text).map_err(|e| io(path, &e))
}
