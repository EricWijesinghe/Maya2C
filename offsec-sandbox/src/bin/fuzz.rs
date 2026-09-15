//! The fuzzing entry point.
//!
//! ```text
//! fuzz <surface> [--time SECONDS | --iters N] [--seed N] [--out DIR]
//! ```
//!
//! Without `--features libafl` this runs the deterministic random-search loop in
//! [`maya_offsec_sandbox::runner`]. With it, the same mutators, oracle and
//! triage are driven by LibAFL's coverage-guided engine, sharing the `fuzz/`
//! corpora.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use maya_offsec_sandbox::runner::{Budget, Config, run};
use maya_offsec_sandbox::Surface;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(config) = parse(&args) else {
        eprintln!(
            "usage: fuzz <tx|wasm|handshake|block> [--time SECONDS | --iters N] [--seed N] [--out DIR]"
        );
        return ExitCode::from(2);
    };

    #[cfg(feature = "libafl")]
    {
        return maya_offsec_sandbox::engine::run(&config);
    }
    #[cfg(not(feature = "libafl"))]
    {
        match run(&config) {
            Ok(report) => {
                println!(
                    "{}: {} inputs, {} distinct finding(s)",
                    config.surface.name(),
                    report.executed,
                    report.findings.len()
                );
                for finding in &report.findings {
                    println!("  {}", finding.signature());
                }
                if report.findings.is_empty() {
                    ExitCode::SUCCESS
                } else {
                    // A finding is a non-zero exit, so CI notices.
                    ExitCode::FAILURE
                }
            }
            Err(error) => {
                eprintln!("run failed: {error}");
                ExitCode::FAILURE
            }
        }
    }
}

fn parse(args: &[String]) -> Option<Config> {
    let surface = Surface::parse(args.first()?)?;
    let mut budget = Budget::Iterations(100_000);
    let mut seed = 0u64;
    let mut out_dir = PathBuf::from("findings");

    let mut i = 1;
    while i < args.len() {
        let value = args.get(i + 1)?;
        match args[i].as_str() {
            "--time" => budget = Budget::Time(Duration::from_secs(value.parse().ok()?)),
            "--iters" => budget = Budget::Iterations(value.parse().ok()?),
            "--seed" => seed = value.parse().ok()?,
            "--out" => out_dir = PathBuf::from(value),
            _ => return None,
        }
        i += 2;
    }
    Some(Config {
        surface,
        seed,
        budget,
        out_dir,
    })
}
