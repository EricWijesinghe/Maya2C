//! How much stack ML-DSA-65 needs — measured on the host, stated as such.
//!
//! A Ledger app's SRAM, from the SDK's own link scripts
//! (`ledger_secure_sdk_sys-1.16.4/devices/*/<device>_layout.ld`), is 40 KiB
//! on Nano S Plus and Apex P, 36 KiB on Stax and Flex, 28 KiB on Nano X —
//! statics, heap and stack together.
//!
//! There is no device and no emulator here, so this measures the next best
//! thing: the smallest thread stack, to 1 KiB, on which each operation
//! completes on this x86-64 host. The test re-runs its own binary with
//! `MEMORY_PROBE=<op>:<bytes>`; the child runs the operation on a thread of
//! exactly that stack and exits 0, or overflows and dies. A bisection over
//! child exit codes needs no unsafe stack painting. The figure is a host
//! figure: Cortex-M frames are smaller (32-bit words, no red zone), so it
//! bounds the device's need from above rather than predicting it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

use app_maya2c::{lowmem, suite};
use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer, Verifier};

const PROBE_ENV: &str = "MAYA_MEMORY_PROBE";
const LOW: usize = 4 * 1024;
const HIGH: usize = 1024 * 1024;
const GRAIN: usize = 1024;

/// The operations probed. `fips204-*` is the stock crate, `lowmem-*` this
/// crate's low-memory ML-DSA-65 (`src/lowmem/`).
const OPERATIONS: [&str; 6] = [
    "baseline",
    "fips204-keygen",
    "fips204-sign",
    "fips204-verify",
    "lowmem-keygen",
    "lowmem-sign",
];

/// Runs `op` on a thread of `stack` bytes. Inputs are prepared on the calling
/// thread and moved in, so only the operation itself is measured.
fn run_on_stack(op: &str, stack: usize) {
    let chain_key = [0x42; 32];
    let message = b"maya2c ledger stack probe".to_vec();
    let (public, secret) = suite::keypair_from_chain_key(&chain_key);
    let signature = suite::sign(&secret, &message).expect("sign");
    let sk: [u8; lowmem::SECRET_KEY_LEN] = *secret;
    drop(secret);
    let op = op.to_owned();
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || match op.as_str() {
            // A thread that does nothing: what the harness itself costs.
            "baseline" => {}
            "fips204-keygen" => {
                let _ = ml_dsa_65::KG::keygen_from_seed(&chain_key);
            }
            "fips204-sign" => {
                let key = ml_dsa_65::PrivateKey::try_from_bytes(sk).expect("sk");
                let _ = key
                    .try_sign_with_seed(&[0; 32], &message, b"")
                    .expect("sign");
            }
            "fips204-verify" => {
                let key = ml_dsa_65::PublicKey::try_from_bytes(public).expect("pk");
                assert!(key.verify(&message, &signature, b""));
            }
            "lowmem-keygen" => {
                let mut pk = [0u8; lowmem::PUBLIC_KEY_LEN];
                let mut out = [0u8; lowmem::SECRET_KEY_LEN];
                lowmem::keygen(&chain_key, &mut pk, &mut out);
            }
            "lowmem-sign" => {
                let mut sig = [0u8; lowmem::SIGNATURE_LEN];
                lowmem::sign(&sk, &message, b"", &[0; 32], &mut sig).expect("sign");
            }
            other => panic!("unknown probe {other}"),
        })
        .expect("thread")
        .join()
        .expect("operation");
}

/// Whether `op` completes on a `stack`-byte thread, decided by a child.
fn fits(op: &str, stack: usize) -> bool {
    Command::new(std::env::current_exe().expect("exe"))
        .args(["--exact", "probe_child", "--nocapture", "--test-threads=1"])
        .env(PROBE_ENV, format!("{op}:{stack}"))
        .output()
        .expect("spawn child")
        .status
        .success()
}

/// Smallest stack, to `GRAIN`, on which `op` completes.
fn minimum_stack(op: &str) -> usize {
    assert!(fits(op, HIGH), "{op} does not fit even {HIGH} bytes");
    let (mut low, mut high) = (LOW, HIGH);
    while high - low > GRAIN {
        let mid = (low + high) / 2;
        if fits(op, mid) {
            high = mid;
        } else {
            low = mid;
        }
    }
    high
}

/// The child half: does nothing unless the probe variable is set.
#[test]
fn probe_child() {
    let Ok(spec) = std::env::var(PROBE_ENV) else {
        return;
    };
    let (op, stack) = spec.split_once(':').expect("op:bytes");
    run_on_stack(op, stack.parse().expect("bytes"));
}

#[test]
fn report_ml_dsa_65_stack_high_water() {
    if std::env::var(PROBE_ENV).is_ok() {
        return; // a child running another probe
    }
    let mut lines = Vec::new();
    for op in OPERATIONS {
        let bytes = minimum_stack(op);
        lines.push(format!("{op}: {} KiB", bytes / 1024));
    }
    // Printed, not asserted against the device budget: the number is the
    // finding, and `docs/ledger-feasibility.md` records it.
    println!(
        "ledger-maya2c: ML-DSA-65 minimum thread stack on this host ({}, {} build, \
         {GRAIN}-byte resolution): {}",
        std::env::consts::OS,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        lines.join(", ")
    );
}
