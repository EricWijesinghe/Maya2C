//! `cargo xtask pgo` — profile-guided optimisation of the execution path,
//! measured (Master Prompt 12).
//!
//! The workload is `crates/node/examples/bft_tps.rs`: signature-verified,
//! executed, committed, final transactions per second through four in-process
//! DAG-BFT validators. Three builds of it under the `perf` profile, each in
//! its own target directory so no artifact is shared:
//!
//! 1. plain, run `--runs` times — the baseline;
//! 2. instrumented (`-Cprofile-generate`), run once to collect a profile;
//! 3. optimised with the merged profile (`-Cprofile-use`), run `--runs` times.
//!
//! It prints both medians and the ratio. It changes no build profile: whether
//! a release ships with PGO is a decision for the release pipeline, made on
//! this number.
//!
//! ```text
//! cargo xtask pgo [--runs 3] [--work D:/Temp/maya-pgo] [--accounts 2000] [--per-account 5]
//! ```

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

struct Options {
    runs: usize,
    work: PathBuf,
    accounts: String,
    per_account: String,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        runs: 3,
        work: PathBuf::from("D:/Temp/maya-pgo"),
        accounts: "2000".into(),
        per_account: "5".into(),
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--runs" => o.runs = value.parse().map_err(|e| format!("--runs: {e}"))?,
            "--work" => o.work = PathBuf::from(value),
            "--accounts" => o.accounts.clone_from(value),
            "--per-account" => o.per_account.clone_from(value),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(o)
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// Builds the example into `target` with `rustflags`, returning the binary.
fn build(target: &Path, rustflags: &str) -> Result<PathBuf, String> {
    let status = Command::new(cargo())
        .args([
            "build",
            "--profile",
            "perf",
            "-p",
            "custom-l1-node",
            "--example",
            "bft_tps",
        ])
        .env("CARGO_TARGET_DIR", target)
        .env("RUSTFLAGS", rustflags)
        .current_dir(crate::workspace_root())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("cargo build: {e}"))?;
    if !status.success() {
        return Err(format!("build with RUSTFLAGS={rustflags:?} failed"));
    }
    Ok(target
        .join("perf")
        .join("examples")
        .join(format!("bft_tps{EXE}")))
}

/// Runs the workload once and returns transactions per second.
fn run_once(exe: &Path, o: &Options) -> Result<f64, String> {
    let out = Command::new(exe)
        .args([&o.accounts, &o.per_account])
        .output()
        .map_err(|e| format!("running {}: {e}", exe.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    // bft_tps prints "... <n> tx/s ..."; take the number before "tx/s".
    text.split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[1].starts_with("tx/s"))
        .and_then(|w| w[0].replace(',', "").parse().ok())
        .ok_or_else(|| format!("no tx/s in bft_tps output:\n{text}"))
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[xs.len() / 2]
}

fn llvm_profdata() -> Result<PathBuf, String> {
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .current_dir(crate::workspace_root())
        .output()
        .map_err(|e| format!("rustc --print sysroot: {e}"))?;
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let host = Command::new("rustc")
        .args(["-vV"])
        .output()
        .map_err(|e| e.to_string())?;
    let host = String::from_utf8_lossy(&host.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .ok_or("rustc -vV printed no host")?;
    let tool = sysroot
        .join("lib/rustlib")
        .join(host)
        .join("bin")
        .join(format!("llvm-profdata{EXE}"));
    tool.exists()
        .then_some(tool)
        .ok_or_else(|| "llvm-profdata missing: rustup component add llvm-tools".to_string())
}

/// Entry point.
///
/// # Errors
///
/// A failed build or run, or no `llvm-profdata`.
pub fn run(args: &[String]) -> Result<(), String> {
    let o = options(args)?;
    let profdata = llvm_profdata()?;
    let _ = std::fs::remove_dir_all(o.work.join("profiles"));
    let raw = o.work.join("profiles");
    let merged = o.work.join("merged.profdata");

    println!("pgo: baseline build");
    let plain = build(&o.work.join("plain"), "")?;
    let base: Vec<f64> = (0..o.runs)
        .map(|_| run_once(&plain, &o))
        .collect::<Result<_, _>>()?;

    println!("pgo: instrumented build and training run");
    let instrumented = build(
        &o.work.join("instrumented"),
        &format!("-Cprofile-generate={}", raw.display()),
    )?;
    run_once(&instrumented, &o)?;
    let status = Command::new(&profdata)
        .arg("merge")
        .arg("-o")
        .arg(&merged)
        .arg(&raw)
        .status()
        .map_err(|e| format!("llvm-profdata: {e}"))?;
    if !status.success() {
        return Err("llvm-profdata merge failed".into());
    }

    println!("pgo: optimised build");
    let opt = build(
        &o.work.join("optimised"),
        &format!("-Cprofile-use={}", merged.display()),
    )?;
    let pgo: Vec<f64> = (0..o.runs)
        .map(|_| run_once(&opt, &o))
        .collect::<Result<_, _>>()?;

    let (b, p) = (median(base.clone()), median(pgo.clone()));
    println!(
        "pgo: bft_tps {} accounts x {} (perf profile), {} runs each",
        o.accounts, o.per_account, o.runs
    );
    println!("  baseline  {base:?} tx/s  median {b:.0}");
    println!("  pgo       {pgo:?} tx/s  median {p:.0}");
    println!("  ratio     {:.3}", p / b);
    Ok(())
}
