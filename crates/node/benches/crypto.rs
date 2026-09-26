//! Every cryptographic primitive the chain uses, timed on one machine, with
//! the sizes beside the times.
//!
//! ```bash
//! cargo bench --bench crypto                 # measure
//! MAYA_CRYPTO_REPORT=1 cargo bench --bench crypto   # measure and export
//! ```
//!
//! The export writes `reports/crypto-bench.csv` and `reports/crypto-bench.md`
//! from criterion's own `estimates.json`, so the numbers in the report are the
//! numbers the harness measured rather than any retyped by hand. Prose about
//! what they mean goes in `reports/02-crypto.md`.
//!
//! # What is not here
//!
//! **kHeavyHash.** There is no `kheavyhash` crate; Kaspa's `kaspa-pow` does not
//! compile on this toolchain, and implementing it from the specification with no
//! official vector to check against would publish a number misrepresenting
//! another project's performance. `benches/algo_comparison.rs` explains that at
//! length, and it is the same refusal here.
//!
//! **Energy.** Every joule figure would be seconds × an assumed wattage; RAPL
//! is not readable on this machine. Seconds are measured, watts are not.
//!
//! # Reading the signature group
//!
//! `keygen` is derivation from a 32-byte master seed, which is what the wallet
//! and the node actually do — not a fresh random keypair. Signing is
//! deterministic in every suite (invariant: a transaction id hashes the
//! signature), so these are the production paths.

use core::fmt::Write as _;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};

use custom_l1_node::crypto::argon_blake::argon_blake_hash;
use maya_crypto_pq::kem_suite::{
    DualKem768Hqc128, DualKem1024Hqc256, Hqc128, Hqc256, KemSuite, MlKem768, MlKem1024, XWing,
};
use maya_crypto_pq::suite::{
    Ed25519, HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa65, MlDsa87, REGISTRY, SignatureSuite,
    SlhDsaSha2_128s, SlhDsaShake256f,
};

/// The message every signature covers: 256 bytes, the order of magnitude of a
/// transaction's signing bytes without the key material in them.
const MESSAGE: &[u8; 256] = &[0xA5; 256];

/// One seed for every derived key, so `keygen` measures derivation and not the
/// operating system's generator.
const SEED: [u8; 32] = [0x5E; 32];

fn seconds(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    // Slow, hash-based suites dominate the run; 20 samples over 3 seconds is
    // enough to separate milliseconds from microseconds, which is the only
    // distinction this table has to support.
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(3));
}

macro_rules! bench_suite {
    ($group:expr, $suite:ty) => {{
        let name = <$suite as SignatureSuite>::ID.info().name;
        let seed = MasterSeed::from_bytes(SEED);
        let key = <$suite>::signing_key_from_seed(&seed);
        let public = <$suite>::public_key(&key);
        let signature = <$suite>::sign(&key, MESSAGE).expect("sign");

        $group.bench_function(BenchmarkId::new("keygen_from_seed", name), |b| {
            b.iter(|| black_box(<$suite>::signing_key_from_seed(black_box(&seed))));
        });
        $group.bench_function(BenchmarkId::new("sign", name), |b| {
            b.iter(|| black_box(<$suite>::sign(black_box(&key), black_box(MESSAGE))));
        });
        $group.bench_function(BenchmarkId::new("verify", name), |b| {
            b.iter(|| {
                black_box(<$suite>::verify(
                    black_box(&public),
                    black_box(MESSAGE),
                    black_box(&signature),
                ))
            });
        });
    }};
}

fn signatures(c: &mut Criterion) {
    let mut group = c.benchmark_group("signature");
    seconds(&mut group);
    bench_suite!(group, Ed25519);
    bench_suite!(group, MlDsa65);
    bench_suite!(group, MlDsa87);
    bench_suite!(group, SlhDsaSha2_128s);
    bench_suite!(group, SlhDsaShake256f);
    bench_suite!(group, HybridMlDsa65SlhDsa128s);
    group.finish();
}

macro_rules! bench_kem {
    ($group:expr, $kem:ty, $name:literal) => {{
        let (secret, public) = <$kem>::generate().expect("keygen");
        let (ciphertext, _) = <$kem>::encapsulate(&public).expect("encapsulate");

        $group.bench_function(BenchmarkId::new("generate", $name), |b| {
            b.iter(|| black_box(<$kem>::generate()));
        });
        $group.bench_function(BenchmarkId::new("encapsulate", $name), |b| {
            b.iter(|| black_box(<$kem>::encapsulate(black_box(&public))));
        });
        $group.bench_function(BenchmarkId::new("decapsulate", $name), |b| {
            b.iter(|| {
                black_box(<$kem>::decapsulate(
                    black_box(&secret),
                    black_box(&ciphertext),
                ))
            });
        });
    }};
}

fn kems(c: &mut Criterion) {
    let mut group = c.benchmark_group("kem");
    seconds(&mut group);
    bench_kem!(group, MlKem768, "ML-KEM-768");
    bench_kem!(group, MlKem1024, "ML-KEM-1024");
    bench_kem!(group, Hqc128, "HQC-128 (draft)");
    bench_kem!(group, Hqc256, "HQC-256 (draft)");
    bench_kem!(group, XWing, "X-Wing");
    bench_kem!(group, DualKem768Hqc128, "Dual ML-KEM-768+HQC-128");
    bench_kem!(group, DualKem1024Hqc256, "Dual ML-KEM-1024+HQC-256");
    group.finish();
}

fn hashes(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash");
    seconds(&mut group);
    let header = [0x11u8; custom_l1_node::core::HEADER_LEN];

    group.bench_function("argon_blake (consensus PoW)", |b| {
        b.iter(|| black_box(argon_blake_hash(black_box(&header))));
    });
    group.bench_function("blake3 (64 B)", |b| {
        b.iter(|| black_box(blake3::hash(black_box(&header))));
    });
    group.bench_function("sha256 (64 B)", |b| {
        use sha2::{Digest, Sha256};
        b.iter(|| black_box(Sha256::digest(black_box(header))));
    });
    group.bench_function("sha3-256 (64 B)", |b| {
        use sha3::{Digest, Sha3_256};
        b.iter(|| black_box(Sha3_256::digest(black_box(header))));
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

/// One measured benchmark, as criterion recorded it.
struct Row {
    group: String,
    function: String,
    parameter: String,
    mean_ns: f64,
    median_ns: f64,
    std_dev_ns: f64,
}

/// Every `new/estimates.json` under `dir`, with its labels taken from the path.
fn collect(dir: &Path, prefix: &mut Vec<String>, rows: &mut Vec<Row>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "new" {
            if let Some(row) = read_estimates(&path.join("estimates.json"), prefix) {
                rows.push(row);
            }
            continue;
        }
        if name == "base" || name == "report" || name == "change" {
            continue;
        }
        prefix.push(name);
        collect(&path, prefix, rows);
        prefix.pop();
    }
}

fn read_estimates(path: &Path, prefix: &[String]) -> Option<Row> {
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let point = |key: &str| json[key]["point_estimate"].as_f64().unwrap_or(f64::NAN);
    // Labels come from `benchmark.json` when it is there: criterion lowercases
    // its directory names, and "ML-DSA-65" is not "ml-dsa-65" in a report.
    let labels = path
        .parent()
        .map(|dir| dir.join("benchmark.json"))
        .and_then(|meta| std::fs::read_to_string(meta).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let label = |key: &str, fallback: Option<&String>| {
        labels
            .as_ref()
            .and_then(|json| json[key].as_str())
            .map(str::to_owned)
            .or_else(|| fallback.cloned())
            .unwrap_or_default()
    };
    let (group, rest) = prefix.split_first()?;
    Some(Row {
        group: label("group_id", Some(group)),
        function: label("function_id", rest.first()),
        // A benchmark without a parameter has no third path component.
        parameter: label("value_str", rest.get(1)),
        mean_ns: point("mean"),
        median_ns: point("median"),
        std_dev_ns: point("std_dev"),
    })
}

/// Human units, chosen per row rather than per table: this data spans
/// nanoseconds to tens of milliseconds.
fn human(ns: f64) -> String {
    match ns {
        n if n.is_nan() => "—".to_owned(),
        n if n < 1_000.0 => format!("{n:.0} ns"),
        n if n < 1_000_000.0 => format!("{:.2} µs", n / 1_000.0),
        n => format!("{:.2} ms", n / 1_000_000.0),
    }
}

/// The size table: what each suite and KEM costs on the wire.
fn sizes_markdown() -> String {
    let mut out = String::from(
        "\n## Sizes\n\nSignature suites (`crypto-pq`'s registry, which is also what the wire \
         carries):\n\n| Suite | Id | Public key | Signature | NIST category | PQ bits |\n\
         |---|---|---|---|---|---|\n",
    );
    for info in &REGISTRY {
        let _ = writeln!(
            out,
            "| {} | `0x{:02x}` | {} B | {} B | {} | {} |",
            info.name,
            info.id.to_byte(),
            info.public_key_len,
            info.signature_len,
            if info.nist_category == 0 {
                "—".to_owned()
            } else {
                info.nist_category.to_string()
            },
            info.pq_security_bits,
        );
    }
    out.push_str("\nKEM suites:\n\n| KEM | Encapsulation key | Ciphertext |\n|---|---|---|\n");
    let mut kem = |name: &str, key: usize, ct: usize| {
        let _ = writeln!(out, "| {name} | {key} B | {ct} B |");
    };
    kem(
        "ML-KEM-768",
        MlKem768::ENCAPSULATION_KEY_LEN,
        MlKem768::CIPHERTEXT_LEN,
    );
    kem(
        "ML-KEM-1024",
        MlKem1024::ENCAPSULATION_KEY_LEN,
        MlKem1024::CIPHERTEXT_LEN,
    );
    kem(
        "HQC-128 (draft)",
        Hqc128::ENCAPSULATION_KEY_LEN,
        Hqc128::CIPHERTEXT_LEN,
    );
    kem(
        "HQC-256 (draft)",
        Hqc256::ENCAPSULATION_KEY_LEN,
        Hqc256::CIPHERTEXT_LEN,
    );
    kem(
        "X-Wing",
        XWing::ENCAPSULATION_KEY_LEN,
        XWing::CIPHERTEXT_LEN,
    );
    kem(
        "Dual ML-KEM-768+HQC-128",
        DualKem768Hqc128::ENCAPSULATION_KEY_LEN,
        DualKem768Hqc128::CIPHERTEXT_LEN,
    );
    kem(
        "Dual ML-KEM-1024+HQC-256",
        DualKem1024Hqc256::ENCAPSULATION_KEY_LEN,
        DualKem1024Hqc256::CIPHERTEXT_LEN,
    );
    out
}

fn export(criterion_dir: &Path, reports: &Path) {
    let mut rows = Vec::new();
    collect(criterion_dir, &mut Vec::new(), &mut rows);
    rows.sort_by(|a, b| {
        (&a.group, &a.function, &a.parameter).cmp(&(&b.group, &b.function, &b.parameter))
    });
    if rows.is_empty() {
        eprintln!(
            "crypto bench: nothing to export from {}",
            criterion_dir.display()
        );
        return;
    }

    let mut csv = String::from("group,function,parameter,mean_ns,median_ns,std_dev_ns\n");
    for row in &rows {
        let _ = writeln!(
            csv,
            "{},{},{},{:.1},{:.1},{:.1}",
            row.group, row.function, row.parameter, row.mean_ns, row.median_ns, row.std_dev_ns
        );
    }

    let mut md = format!(
        "# Crypto benchmarks\n\nGenerated by `MAYA_CRYPTO_REPORT=1 cargo bench --bench crypto` \
         on {} ({} rows). Times are criterion's point estimates; \
         `reports/crypto-bench.csv` has the raw nanoseconds.\n\n\
         | Group | Operation | Suite | Mean | Median | Std dev |\n|---|---|---|---|---|---|\n",
        std::env::consts::OS,
        rows.len()
    );
    for row in &rows {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} |",
            row.group,
            row.function,
            if row.parameter.is_empty() {
                "—"
            } else {
                &row.parameter
            },
            human(row.mean_ns),
            human(row.median_ns),
            human(row.std_dev_ns),
        );
    }
    md.push_str(&sizes_markdown());

    if let Err(error) = std::fs::create_dir_all(reports) {
        eprintln!("crypto bench: {}: {error}", reports.display());
        return;
    }
    for (name, body) in [("crypto-bench.csv", csv), ("crypto-bench.md", md)] {
        let path = reports.join(name);
        match std::fs::write(&path, body) {
            Ok(()) => println!("crypto bench: wrote {}", path.display()),
            Err(error) => eprintln!("crypto bench: {}: {error}", path.display()),
        }
    }
}

/// `crates/node/benches/crypto.rs` → the repository root.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn main() {
    // A fixed output directory so the exporter reads the run it just made,
    // wherever `CARGO_TARGET_DIR` points.
    let criterion_dir = repo_root().join("target/criterion-crypto");
    let mut criterion = Criterion::default()
        .output_directory(&criterion_dir)
        .configure_from_args();

    signatures(&mut criterion);
    kems(&mut criterion);
    hashes(&mut criterion);
    criterion.final_summary();

    if std::env::var("MAYA_CRYPTO_REPORT").as_deref() == Ok("1") {
        export(&criterion_dir, &repo_root().join("reports"));
    }
}
