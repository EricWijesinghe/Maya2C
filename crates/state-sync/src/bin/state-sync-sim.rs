//! `state-sync-sim [--accounts N] [--peers P] [--malicious M] [--uplink-mbps B]`
//!
//! Master Prompt 14's 100-million-account sync: run in release. Virtual time
//! is the network model's; CPU times are real.

#![allow(clippy::cast_precision_loss)]

use maya_state_sync::sim::{Setup, simulate};
use maya_state_sync::{ACCOUNT_BYTES, SyntheticState};

fn main() -> Result<(), String> {
    let (mut accounts, mut peers, mut malicious, mut mbps) =
        (100_000_000u64, 32u16, 4u16, 1_000u64);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v: u64 = args
            .next()
            .ok_or(format!("{a} needs a value"))?
            .parse()
            .map_err(|e| format!("{e}"))?;
        match a.as_str() {
            "--accounts" => accounts = v,
            "--peers" => peers = u16::try_from(v).map_err(|e| e.to_string())?,
            "--malicious" => malicious = u16::try_from(v).map_err(|e| e.to_string())?,
            "--uplink-mbps" => mbps = v,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let state = SyntheticState {
        accounts,
        per_chunk: 10_000,
    };
    let setup = Setup {
        peers,
        malicious: (1..=malicious).collect(),
        per_peer: 4,
        uplink_bps: mbps * 1_000_000,
        seed: 14,
    };
    println!(
        "[SIM network] state sync: {accounts} accounts ({:.2} GB), {} chunks, {peers} peers ({malicious} malicious), {mbps} Mbit/s uplink each",
        (accounts as f64 * ACCOUNT_BYTES as f64) / 1e9,
        state.chunks()
    );
    let r = simulate(state, &setup);
    println!("virtual sync time     {:.1} s", r.virtual_secs);
    println!(
        "downloaded            {:.2} GB ({:.2} MB wasted on bad chunks)",
        r.bytes as f64 / 1e9,
        r.wasted as f64 / 1e6
    );
    println!("banned peers          {:?}", r.banned);
    println!(
        "manifest build (CPU)  {:.1} s   joiner verify (CPU) {:.1} s",
        r.manifest_cpu_secs, r.verify_cpu_secs
    );
    println!("balances imported     {}", r.balance_sum);
    Ok(())
}
