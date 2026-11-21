//! `maya2c-validator-ledger`: prints the public validator ledger of one
//! endpoint. Read-only, one shot.
//!
//! ```text
//! maya2c-validator-ledger --node https://rpc.example/rpc [--epochs 8] [--json]
//! ```

use std::process::ExitCode;

use maya_sentinel::rpc;

const USAGE: &str = "usage: maya2c-validator-ledger --node URL [--epochs 8] [--json]";
const DEFAULT_EPOCHS: u64 = 8;
/// Upper bound on `--epochs`: each is one RPC call against a rate-limited endpoint.
const MAX_EPOCHS: u64 = 256;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("maya2c-validator-ledger: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<(), String> {
    let mut url = None;
    let mut epochs = DEFAULT_EPOCHS;
    let mut as_json = false;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--json" => as_json = true,
            "--node" => url = Some(it.next().ok_or("--node needs a URL")?),
            "--epochs" => {
                let v = it.next().ok_or("--epochs needs a number")?;
                epochs = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=MAX_EPOCHS).contains(n))
                    .ok_or_else(|| format!("--epochs expects 1..={MAX_EPOCHS}, got {v}"))?;
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let url = url.ok_or("--node URL is required")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("--node must start with http:// or https://".into());
    }
    let client = rpc::client(&url)?;
    let ledger = maya_validator_ledger::build(&client, epochs).await?;
    if as_json {
        println!("{}", maya_validator_ledger::to_json(&ledger));
    } else {
        print!("{}", maya_validator_ledger::render_text(&ledger));
    }
    Ok(())
}
