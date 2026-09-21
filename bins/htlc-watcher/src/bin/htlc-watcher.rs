//! `htlc-watcher`: settles the journal's swaps until interrupted.
//!
//! Registering a swap — `Worker::initiate`, `accept_response`, `respond` — is a
//! library call for now; this binary runs what the journal already holds. The
//! signing key comes from an encrypted wallet keystore unlocked with
//! `MAYA_HTLC_WATCHER_PASSWORD`, as the pool's treasury does. An initiator's
//! swap secrets are hex files passed with `--secret`: they are exactly as
//! sensitive as the keystore and are **not** encrypted, so keep them on the
//! same footing as the password. A file holds `<family>:<64 hex>`, the family
//! one of `sha3-256`, `blake3`, `sha256` (hash-lock preimages) or `lattice`
//! (a lattice seed); bare hex is read as `lattice`, the only kind there was.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use zeroize::Zeroizing;

use maya_htlc_lattice::{
    HashFunction, LatticeSecret, PREIMAGE_BYTES, Preimage, SEED_BYTES, SwapSecret,
};
use maya_htlc_watcher::rpc::RpcChain;
use maya_htlc_watcher::{Journal, Margins, Result as WatcherResult, StepReport, Worker};

/// Environment variable holding the keystore password.
const PASSWORD_ENV: &str = "MAYA_HTLC_WATCHER_PASSWORD";

/// Seconds between ticks unless `--poll-secs` says otherwise.
const DEFAULT_POLL_SECS: u64 = 10;

struct Args {
    journal: PathBuf,
    keystore: PathBuf,
    maya_rpc: String,
    counterparty_rpc: String,
    poll_secs: u64,
    margins: Margins,
    secrets: Vec<PathBuf>,
}

fn usage() -> String {
    format!(
        "usage: htlc-watcher --journal <PATH> --keystore <PATH> --maya-rpc <URL> \
         --counterparty-rpc <URL> [--poll-secs N] [--maya-confirmations N] \
         [--counterparty-confirmations N] [--submission-blocks N] [--secret <PATH>]...\n\
         The keystore password is read from {PASSWORD_ENV}."
    )
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut journal = None;
    let mut keystore = None;
    let mut maya_rpc = None;
    let mut counterparty_rpc = None;
    let mut poll_secs = DEFAULT_POLL_SECS;
    let mut margins = Margins::default();
    let mut secrets = Vec::new();

    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let mut value = || argv.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--journal" => journal = Some(PathBuf::from(value()?)),
            "--keystore" => keystore = Some(PathBuf::from(value()?)),
            "--maya-rpc" => maya_rpc = Some(value()?),
            "--counterparty-rpc" => counterparty_rpc = Some(value()?),
            "--poll-secs" => poll_secs = value()?.parse()?,
            "--maya-confirmations" => margins.maya_confirmations = value()?.parse()?,
            "--counterparty-confirmations" => {
                margins.counterparty_confirmations = value()?.parse()?;
            }
            "--submission-blocks" => margins.submission_blocks = value()?.parse()?,
            "--secret" => secrets.push(PathBuf::from(value()?)),
            "--help" | "-h" => return Err(usage().into()),
            other => return Err(format!("unknown flag {other}\n{}", usage()).into()),
        }
    }
    let missing = |name: &str| format!("{name} is required\n{}", usage());
    Ok(Args {
        journal: journal.ok_or_else(|| missing("--journal"))?,
        keystore: keystore.ok_or_else(|| missing("--keystore"))?,
        maya_rpc: maya_rpc.ok_or_else(|| missing("--maya-rpc"))?,
        counterparty_rpc: counterparty_rpc.ok_or_else(|| missing("--counterparty-rpc"))?,
        poll_secs,
        margins,
        secrets,
    })
}

/// Reads a hex secret straight into its zeroizing wrapper.
fn load_secret(path: &PathBuf) -> Result<SwapSecret, Box<dyn Error>> {
    let text = Zeroizing::new(std::fs::read_to_string(path)?);
    let (family, hex_text) = text
        .trim()
        .split_once(':')
        .unwrap_or(("lattice", text.trim()));
    // Each family's own length: a lattice seed and a preimage are both 32
    // bytes today, and nothing should rely on that staying true.
    let expected = if family == "lattice" {
        SEED_BYTES
    } else {
        PREIMAGE_BYTES
    };
    if hex_text.len() != 2 * expected {
        return Err(format!(
            "{}: a {family} secret is {} hex characters",
            path.display(),
            2 * expected
        )
        .into());
    }
    let function = match family {
        "lattice" => {
            return Ok(SwapSecret::Lattice(LatticeSecret::generate(|buf| {
                hex::decode_to_slice(hex_text, buf)
            })?));
        }
        "sha3-256" => HashFunction::Sha3_256,
        "blake3" => HashFunction::Blake3,
        "sha256" => HashFunction::Sha256,
        other => {
            return Err(format!("{}: unknown secret family {other:?}", path.display()).into());
        }
    };
    let mut bytes = Zeroizing::new([0u8; PREIMAGE_BYTES]);
    hex::decode_to_slice(hex_text, bytes.as_mut_slice())?;
    Ok(SwapSecret::Hash {
        function,
        preimage: Preimage::new(*bytes),
    })
}

fn print_tick(tick: WatcherResult<StepReport>) {
    match tick {
        Ok(report) => {
            for (id, action) in &report.actions {
                eprintln!("swap {}: {action:?}", hex::encode(id));
            }
            for (id, failure) in &report.failures {
                eprintln!("swap {}: FAILED {failure}", hex::encode(id));
            }
        }
        Err(error) => eprintln!("tick failed: {error}"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let password = Zeroizing::new(
        std::env::var(PASSWORD_ENV).map_err(|_| format!("{PASSWORD_ENV} is not set"))?,
    );
    let key = l1_wallet::keystore::decrypt(&std::fs::read(&args.keystore)?, &password)?;

    let maya = Arc::new(RpcChain::connect(&args.maya_rpc)?);
    let counterparty = Arc::new(RpcChain::connect(&args.counterparty_rpc)?);
    let journal = Journal::open(&args.journal)?;
    let mut worker = Worker::new(maya, counterparty, key, args.margins, journal);
    for path in &args.secrets {
        worker.add_secret(load_secret(path)?)?;
    }

    let (stop, shutdown) = watch::channel(false);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = stop.send(true);
        }
    });
    worker
        .run(Duration::from_secs(args.poll_secs), shutdown, print_tick)
        .await?;
    Ok(())
}
