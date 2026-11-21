//! HW-3 G0 harness (ADR-137): the same batches through the CPU verifier and
//! the GPU-assisted split path, every item compared, both timed.
//!
//! Prints the commit, hardware, OS and command with the numbers (Production
//! Standing Orders). Without an adapter, or built without `--features gpu`,
//! the GPU lines say MISSING. Exits non-zero on any mismatch.
//!
//! The CPU figure is `maya-crypto-pq` on every core over distinct signatures,
//! so a verified-signature cache would never hit; that is stated in the output.

#![allow(clippy::cast_precision_loss)] // reporting rates, never deciding anything

use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};
use maya_wgpu_verify::Item;
use maya_wgpu_verify::mldsa;

/// ADR-137's G0 exit test: batch sizes 32 to 469 (469 is the most hybrid
/// signatures one block's byte cap admits, invariant 33).
const BATCH_SIZES: [usize; 5] = [32, 64, 128, 256, 469];
/// Timed repetitions per figure; the mean is printed.
const REPEATS: u32 = 5;

fn main() -> ExitCode {
    print_header();
    let items = signed(BATCH_SIZES[BATCH_SIZES.len() - 1]);
    for &n in &BATCH_SIZES {
        let (elapsed, verdicts) = timed(|| parallel(&items[..n], mldsa::cpu_verdict));
        assert!(
            verdicts.iter().all(|&v| v),
            "a generated signature failed to verify"
        );
        println!(
            "cpu  batch {n:>3}: {:>10.0} verifies/s  (maya-crypto-pq, {} threads, distinct signatures: no cache hits)",
            rate(n, elapsed),
            threads()
        );
    }
    gpu_section(&items)
}

#[cfg(not(feature = "gpu"))]
fn gpu_section(_: &[Item]) -> ExitCode {
    println!("gpu: MISSING (built without --features gpu)");
    ExitCode::SUCCESS
}

#[cfg(feature = "gpu")]
fn gpu_section(items: &[Item]) -> ExitCode {
    use maya_wgpu_verify::gpu::GpuLattice;
    use maya_wgpu_verify::lattice::{LatticeBackend, MAX_DISPATCH_ITEMS, lattice_on_cpu};

    let adapters = GpuLattice::adapters();
    if adapters.is_empty() {
        println!("gpu: MISSING (no adapter)");
        return ExitCode::SUCCESS;
    }
    let mut mismatches = 0usize;
    for (index, info) in adapters.iter().enumerate() {
        let mut gpu = match GpuLattice::new(index) {
            Ok(gpu) => gpu,
            Err(e) => {
                println!("gpu {index} {}: MISSING (could not open: {e})", info.name);
                continue;
            }
        };
        println!(
            "gpu {index}: {}{}",
            gpu.name(),
            if info.software {
                "  [software adapter]"
            } else {
                ""
            }
        );
        for &n in &BATCH_SIZES {
            let batch = &items[..n];
            let (t_prep, prepared) = timed(|| parallel(batch, |i| mldsa::prepare(i).ok()));
            let prepared: Vec<_> = prepared.into_iter().flatten().collect();
            let inputs: Vec<_> = prepared.iter().map(|p| (*p.input).clone()).collect();
            let (t_gpu, outputs) = timed(|| {
                inputs
                    .chunks(MAX_DISPATCH_ITEMS)
                    .map(|c| gpu.run(c))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|v| v.concat())
            });
            let outputs = match outputs {
                Ok(o) => o,
                Err(e) => {
                    println!("  batch {n:>3}: MISSING (gpu failed: {e})");
                    break;
                }
            };
            let (t_cpu_lattice, reference) = timed(|| parallel(&inputs, lattice_on_cpu));
            let pairs: Vec<_> = prepared.iter().zip(&outputs).collect();
            let (t_fin, finished) = timed(|| parallel(&pairs, |(p, o)| mldsa::finish(p, o)));
            let verdicts = parallel(batch, mldsa::cpu_verdict);
            let wrong = (0..n)
                .filter(|&i| outputs[i] != reference[i] || finished[i] != verdicts[i])
                .count();
            mismatches += wrong;
            println!(
                "  batch {n:>3}: {:>10.0} verifies/s assisted  (prepare {:?}, gpu lattice {:?} vs cpu lattice {:?}, finish {:?}); mismatches {wrong}",
                rate(n, t_prep + t_gpu + t_fin),
                t_prep,
                t_gpu,
                t_cpu_lattice,
                t_fin,
            );
        }
    }
    if mismatches == 0 {
        ExitCode::SUCCESS
    } else {
        println!("FAIL: {mismatches} mismatches");
        ExitCode::FAILURE
    }
}

fn print_header() {
    let run = |cmd: &str, args: &[&str]| {
        Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "MISSING".to_owned())
    };
    let cpu = std::env::var("PROCESSOR_IDENTIFIER").ok().or_else(|| {
        std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|m| m.trim().to_owned())
        })
    });
    let kernel = if cfg!(windows) {
        run("cmd", &["/c", "ver"])
    } else {
        run("uname", &["-r"])
    };
    println!("HW-3 G0 differential harness (ADR-137). The CPU verdict is the only verdict.");
    println!("commit:   {}", run("git", &["rev-parse", "HEAD"]));
    println!(
        "cpu:      {} ({} threads)",
        cpu.unwrap_or_else(|| "MISSING".to_owned()),
        threads()
    );
    println!(
        "os:       {} {} / {kernel}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!(
        "command:  {}",
        std::env::args().collect::<Vec<_>>().join(" ")
    );
    println!("repeats:  mean of {REPEATS}");
    if cfg!(debug_assertions) {
        println!("WARNING: debug build; rerun with --release before reading any rate");
    }
}

fn signed(n: usize) -> Vec<Item> {
    let keys: Vec<_> = (0u8..8)
        .map(|seed| MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32])))
        .collect();
    (0..n)
        .map(|i| {
            let key = &keys[i % keys.len()];
            let message = format!("hw-3 g0 bench item {i}").into_bytes();
            Item {
                public_key: MlDsa65::public_key(key),
                signature: MlDsa65::sign(key, &message).expect("signing a generated message"),
                message,
                context: Vec::new(),
            }
        })
        .collect()
}

/// Mean wall time of [`REPEATS`] runs, and the last run's result.
fn timed<R>(mut f: impl FnMut() -> R) -> (Duration, R) {
    let start = Instant::now();
    let mut last = f();
    for _ in 1..REPEATS {
        last = f();
    }
    (start.elapsed() / REPEATS, last)
}

fn rate(n: usize, elapsed: Duration) -> f64 {
    n as f64 / elapsed.as_secs_f64()
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// `f` over `items` on every core, results in order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let chunk = items.len().div_ceil(threads()).max(1);
    let f = &f;
    std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|c| s.spawn(move || c.iter().map(f).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("worker thread"))
            .collect()
    })
}
