//! `threat-firewall`: enforces a co-located node's active threat indicators
//! until interrupted.
//!
//! Needs `CAP_NET_ADMIN` for the `nftables` and `iptables` backends and holds
//! no chain key. Start with `--backend dry-run` on a new host: it prints what it
//! would change and touches nothing.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use maya_threat_firewall::rpc::RpcSource;
use maya_threat_firewall::{
    Backend, CommandSink, DryRunSink, FirewallSink, Result as FirewallResult, StepReport, Worker,
};

/// Seconds between ticks unless `--poll-secs` says otherwise. Short against a
/// block, so a new indicator is enforced within the block that recorded it.
const DEFAULT_POLL_SECS: u64 = 5;

/// The nftables table unless `--nft-table` says otherwise.
const DEFAULT_NFT_TABLE: &str = "maya2c";

struct Args {
    rpc: String,
    backend: String,
    nft_table: String,
    poll_secs: u64,
}

fn usage() -> &'static str {
    "usage: threat-firewall --rpc <URL> --backend <nftables|iptables|dry-run> \
     [--nft-table NAME] [--poll-secs N]"
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut rpc = None;
    let mut backend = None;
    let mut nft_table = DEFAULT_NFT_TABLE.to_owned();
    let mut poll_secs = DEFAULT_POLL_SECS;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{flag} needs a value\n{}", usage()))
        };
        match flag.as_str() {
            "--rpc" => rpc = Some(value()?),
            "--backend" => backend = Some(value()?),
            "--nft-table" => nft_table = value()?,
            "--poll-secs" => poll_secs = value()?.parse()?,
            other => return Err(format!("unknown argument {other}\n{}", usage()).into()),
        }
    }
    if poll_secs == 0 {
        return Err("--poll-secs must be at least 1".into());
    }
    Ok(Args {
        rpc: rpc.ok_or_else(|| format!("--rpc is required\n{}", usage()))?,
        backend: backend.ok_or_else(|| format!("--backend is required\n{}", usage()))?,
        nft_table,
        poll_secs,
    })
}

fn sink(args: &Args) -> Result<Box<dyn FirewallSink>, Box<dyn Error>> {
    Ok(match args.backend.as_str() {
        "nftables" => Box::new(CommandSink::new(Backend::nftables(&args.nft_table)?)),
        "iptables" => Box::new(CommandSink::new(Backend::Iptables)),
        "dry-run" => Box::new(DryRunSink::default()),
        other => return Err(format!("unknown backend {other}\n{}", usage()).into()),
    })
}

fn report(outcome: FirewallResult<StepReport>) {
    match outcome {
        Ok(step) => {
            for ip in &step.blocked {
                eprintln!("height {}: blocked {ip}", step.height);
            }
            for ip in &step.unblocked {
                eprintln!("height {}: unblocked {ip}", step.height);
            }
            for failure in &step.failures {
                eprintln!("height {}: {failure}", step.height);
            }
        }
        Err(error) => eprintln!("{error}"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let source = Arc::new(RpcSource::connect(&args.rpc)?);
    let mut worker = Worker::new(source, sink(&args)?);

    let (stop, shutdown) = watch::channel(false);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = stop.send(true);
        }
    });
    worker
        .run(Duration::from_secs(args.poll_secs), shutdown, report)
        .await;
    Ok(())
}
