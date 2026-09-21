//! Writes raw bytes for the offline batteries (`scripts/entropy_battery.sh`).
//!
//! ```text
//! entropy-dump <source> <bytes> <out-file | ->
//!   source: pool | os | rdseed | sim-thermal | sim-microvoltage | sim-brownian | sim-homodyne-qrng
//! ```
//!
//! `-` writes to stdout, for piping into `dieharder -g 200`; a closed pipe
//! ends the run quietly, since that is how Dieharder says it has enough.
//!
//! `pool` is the DRBG output — what keys are made from. The others are the
//! raw source samples, *before* the DRBG, which is where a bad source shows.
//! SIM sources print a warning: their output is OS randomness shaped by a
//! model, so a passing battery says nothing about any physical device.

use std::io::Write as _;
use std::process::ExitCode;

use maya_entropy::EntropySource;
use maya_entropy::pool::EntropyPool;
use maya_entropy::sources::{OsSource, RdSeedSource, sim};
use maya_entropy::tamper::TamperLine;

const CHUNK: usize = 1 << 16;

fn source(name: &str) -> Result<Box<dyn EntropySource>, String> {
    Ok(match name {
        "os" => Box::new(OsSource),
        "rdseed" => Box::new(RdSeedSource::detect().ok_or("rdseed not available on this CPU")?),
        "sim-thermal" => Box::new(sim::ThermalNoise::typical()),
        "sim-microvoltage" => Box::new(sim::MicroVoltage::typical()),
        "sim-brownian" => Box::new(sim::Brownian::typical()),
        "sim-homodyne-qrng" => Box::new(sim::HomodyneQrng::typical()),
        other => return Err(format!("unknown source {other}")),
    })
}

fn run(args: &[String]) -> Result<(), String> {
    let [name, bytes, path] = args else {
        return Err("usage: entropy-dump <source> <bytes> <out-file | ->".into());
    };
    let total: usize = bytes.parse().map_err(|e| format!("bytes: {e}"))?;
    let sink: Box<dyn std::io::Write> = if path == "-" {
        Box::new(std::io::stdout().lock())
    } else {
        Box::new(std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?)
    };
    let mut file = std::io::BufWriter::new(sink);
    let mut buf = vec![0u8; CHUNK];
    let mut pool = (name == "pool").then(|| EntropyPool::new(b"entropy-dump", TamperLine::new()));
    let mut raw = if pool.is_none() {
        Some(source(name)?)
    } else {
        None
    };
    if let Some(s) = &raw {
        eprintln!(
            "entropy-dump: {} ({:?}), declared {} mbit/byte",
            s.name(),
            s.class(),
            s.min_entropy_millibits()
        );
    }
    let mut written = 0;
    while written < total {
        let n = CHUNK.min(total - written);
        match (&mut pool, &mut raw) {
            (Some(p), _) => p.fill(&mut buf[..n]).map_err(|e| e.to_string())?,
            (None, Some(s)) => s.fill(&mut buf[..n]).map_err(|e| e.to_string())?,
            (None, None) => unreachable!("one of the two is always set"),
        }
        match file.write_all(&buf[..n]) {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
            other => other.map_err(|e| e.to_string())?,
        }
        written += n;
    }
    file.flush().map_err(|e| e.to_string())?;
    eprintln!("entropy-dump: wrote {written} bytes to {path}");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("entropy-dump: {e}");
            ExitCode::FAILURE
        }
    }
}
