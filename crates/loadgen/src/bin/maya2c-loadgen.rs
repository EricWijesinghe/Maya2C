//! `maya2c-loadgen`: print a workload summary, or stream transactions as
//! tab-separated lines for another tool to consume.
//!
//! `maya2c-loadgen [--seed N] [--txs N] [--contention-ppm N] [--zipf-x100 N] [--emit]`

use maya_loadgen::{HOT_BASE, Kind, Params, Workload};

fn main() -> Result<(), String> {
    let mut p = Params::default();
    let mut txs = 100_000usize;
    let mut emit = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or(format!("{a} needs a value"))?.parse::<u64>().map_err(|e| e.to_string());
        match a.as_str() {
            "--seed" => p.seed = val()?,
            "--txs" => txs = usize::try_from(val()?).map_err(|e| e.to_string())?,
            "--contention-ppm" => p.contention_ppm = u32::try_from(val()?).map_err(|e| e.to_string())?,
            "--zipf-x100" => p.zipf_s_x100 = u32::try_from(val()?).map_err(|e| e.to_string())?,
            "--accounts" => p.accounts = val()?,
            "--emit" => emit = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let mut w = Workload::new(p);
    let mut counts = [0usize; 4];
    let mut hot = 0usize;
    for _ in 0..txs {
        let t = w.next_tx();
        counts[match t.kind {
            Kind::Transfer => 0,
            Kind::TokenTransfer => 1,
            Kind::ContractCall => 2,
            Kind::Deploy => 3,
        }] += 1;
        if t.writes.iter().any(|k| *k >= HOT_BASE && *k < maya_loadgen::CONTRACT_BASE) {
            hot += 1;
        }
        if emit {
            println!("{}\t{:?}\t{}\t{:?}\t{:?}\t{}", t.id, t.kind, t.sender, t.reads, t.writes, t.amount);
        }
    }
    if !emit {
        println!("maya2c-loadgen seed={} txs={txs} accounts={} contention={}ppm zipf_s={}", p.seed, p.accounts, p.contention_ppm, f64::from(p.zipf_s_x100) / 100.0);
        println!("transfer {} token {} call {} deploy {} | touching hot keys {hot}", counts[0], counts[1], counts[2], counts[3]);
    }
    Ok(())
}
