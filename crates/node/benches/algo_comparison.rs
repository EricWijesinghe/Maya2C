//! Maya2C against other chains' primitives.
//!
//! ```text
//! cargo bench --bench algo_comparison
//! ```
//!
//! # Two tables, because these are two categories
//!
//! The brief asked for one comparison across SHA-256, kHeavyHash, Ed25519, and
//! Maya2C's hybrid stack. Those are not comparable in one table:
//!
//! - **SHA-256 and kHeavyHash are proof-of-work hashes.** They are what a miner
//!   runs billions of times.
//! - **Ed25519 and ML-DSA+SLH-DSA are signature schemes.** They are what a
//!   wallet runs once per transaction and a validator once per verification.
//!
//! A row placing "signature generation" next to SHA-256 would be measuring
//! something Bitcoin does not do. **Bitcoin does not use Ed25519 at all** — its
//! signatures are ECDSA over secp256k1, and Schnorr since Taproot. Ed25519 is
//! here as the fast-classical-signature baseline it actually is, not as
//! Bitcoin's.
//!
//! So: one group for hashes, one for signatures, and no arithmetic across them.
//!
//! # kHeavyHash lives in `benches/crypto.rs`
//!
//! `kaspa-pow` still does not build here (it depends unconditionally on
//! `workflow-core 0.18`). This file used to refuse a reimplementation for want
//! of official vectors to check it against, and that premise was wrong:
//! rusty-kaspa's `matrix.rs` publishes known answers for `heavy_hash` and for
//! `Matrix::generate`. `benches/support/kheavyhash.rs` ports upstream's code and
//! `tests/kheavyhash_reference_tests.rs` checks it against both, and against
//! cSHAKE256 written from NIST SP 800-185 -- the "authority to test against"
//! this comment once said did not exist. The per-nonce row is in the crypto
//! bench; this harness keeps its original comparison.
//!
//! What stays true: `kaspa_hashes::KHeavyHash` alone would be the wrong
//! measurement, being only the cSHAKE256 finalisation; the 64x64 matrix
//! product is the "heavy" in heavy hash, and the reference times both.
//!
//! # Energy figures are modelled, not measured
//!
//! Every joule number this harness prints is `measured seconds x an assumed
//! watts`, and the assumption is printed on the same line as the result.
//!
//! RAPL is not readable without a driver on Windows, so package power is an
//! *input* here, not a reading. A figure derived from an assumption, presented
//! without it, is the part of a comparison that gets quoted and the part that is
//! wrong.
//!
//! **There is no energy-per-transaction figure.** That quantity is
//! `network hashrate x assumed joules-per-hash x hardware mix / transactions`,
//! and two of those three inputs are unobservable — published Bitcoin estimates
//! carry roughly a factor-of-two spread for exactly this reason. Maya2C has no
//! live network at all, so its hashrate is zero and the quotient is undefined.
//! Joules per *operation* is a thing this machine can actually establish.
//!
//! # And no "finalized" transaction
//!
//! Maya2C and Bitcoin have probabilistic finality. There is no finalisation
//! event to measure against — only confirmations accumulating. A per-finalized
//! figure would need a stated confirmation depth, and would then be a statement
//! about that choice rather than about the chain.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

use custom_l1_node::crypto::argon_blake::argon_blake_hash;
use custom_l1_node::crypto::hybrid;
mod common;


/// Assumed package power, in watts, for the joules-per-operation model.
///
/// A whole-package figure for a laptop-class CPU under load. It is an
/// assumption, it is printed next to every number derived from it, and it is
/// the single input a reader should challenge first.
const ASSUMED_PACKAGE_WATTS: f64 = 45.0;

/// Converts a measured duration to modelled joules.
fn joules(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * ASSUMED_PACKAGE_WATTS
}

// ---------------------------------------------------------------------------
// Hashes
// ---------------------------------------------------------------------------

fn hashes(c: &mut Criterion) {
    let mut group = c.benchmark_group("pow-hash");
    // Per-hash, so the reported figure is comparable across algorithms whose
    // input sizes differ.
    group.throughput(Throughput::Elements(1));

    let input = [0x42u8; 80];

    // Bitcoin's proof of work is SHA-256 applied twice.
    group.bench_function("sha256d (Bitcoin)", |b| {
        use sha2::{Digest, Sha256};
        b.iter(|| {
            let first = Sha256::digest(black_box(&input));
            black_box(Sha256::digest(first))
        });
    });

    // Maya2C before DAG_ACTIVATION_HEIGHT. 32 MiB of Argon2id per attempt, so
    // this is roughly six orders of magnitude slower than SHA-256 per hash —
    // which is the design, not a defect. Sample size is dropped because the
    // default 100 would take minutes.
    group.sample_size(10);
    group.bench_function("argon-blake (Maya2C, pre-fork)", |b| {
        b.iter(|| black_box(argon_blake_hash(black_box(&input))));
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Signatures
// ---------------------------------------------------------------------------

fn signatures(c: &mut Criterion) {
    let message = b"maya2c comparative benchmark";

    // --- Ed25519, the fast classical baseline ---
    //
    // Not Bitcoin's. Bitcoin uses ECDSA over secp256k1 and Schnorr since
    // Taproot; Ed25519 is here because it is the signature scheme a
    // post-quantum one is usually measured against.
    let ed_signing = {
        use ed25519_dalek::SigningKey;
        SigningKey::from_bytes(&[7u8; 32])
    };
    let ed_verifying = ed_signing.verifying_key();
    let ed_signature = {
        use ed25519_dalek::Signer as _;
        ed_signing.sign(message, &common::test_chain())
    };

    // --- Maya2C's hybrid pair ---
    let hybrid_signing =
        hybrid::signing_key_from_seed(&[7u8; 32]).expect("a fixed seed derives a key");
    let hybrid_public = hybrid_signing.public_key();
    let hybrid_verifying =
        hybrid::HybridVerifyingKey::from_public_key(&hybrid_public).expect("own key verifies");
    let hybrid_signature = hybrid_signing.sign(message).expect("signing succeeds");

    let mut group = c.benchmark_group("signature-generation");
    group.bench_function("ed25519", |b| {
        use ed25519_dalek::Signer as _;
        b.iter(|| black_box(ed_signing.sign(black_box(message, &common::test_chain()))));
    });

    // SLH-DSA-SHA2-128s signing dominates the pair and is measured in hundreds
    // of milliseconds even optimised, so the default sample size would take
    // minutes. See CLAUDE.md invariant 2 for why the instantiating crate is
    // the one that has to be optimised.
    group.sample_size(10);
    group.bench_function("ml-dsa-65 + slh-dsa (Maya2C)", |b| {
        b.iter(|| black_box(hybrid_signing.sign(black_box(message, &common::test_chain())).expect("signs")));
    });
    group.finish();

    let mut group = c.benchmark_group("signature-verification");
    group.bench_function("ed25519", |b| {
        use ed25519_dalek::Verifier as _;
        b.iter(|| black_box(ed_verifying.verify(black_box(message), &ed_signature)));
    });
    group.bench_function("ml-dsa-65 + slh-dsa (Maya2C)", |b| {
        b.iter(|| {
            black_box(
                hybrid_verifying
                    .verify(black_box(message), &hybrid_signature)
                    .is_ok(),
            )
        });
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// Sizes, which cost bandwidth and storage forever
// ---------------------------------------------------------------------------

/// Not a timing benchmark — a table.
///
/// Bytes matter more than microseconds for a chain: a signature is stored by
/// every archival node for the life of the network and shipped to every peer
/// that syncs. A scheme twice as fast and four times as large is usually the
/// worse trade, and a comparison that reported only time would hide that.
fn sizes(_c: &mut Criterion) {
    println!();
    println!("Signature and key sizes");
    println!("=======================");
    println!();
    println!("  {:<32} {:>10} {:>10}", "scheme", "pubkey B", "sig B");
    println!("  {:<32} {:>10} {:>10}", "ed25519", 32, 64);
    println!(
        "  {:<32} {:>10} {:>10}",
        "ml-dsa-65 (FIPS 204)",
        1952,
        // Re-exported through `crypto::keys` rather than `hybrid`, which keeps
        // its own copy private.
        custom_l1_node::crypto::keys::SIGNATURE_LENGTH
    );
    println!(
        "  {:<32} {:>10} {:>10}",
        "slh-dsa-sha2-128s (FIPS 205)",
        32,
        hybrid::SLH_DSA_SIGNATURE_LENGTH
    );
    println!(
        "  {:<32} {:>10} {:>10}",
        "Maya2C hybrid (both required)",
        hybrid::HYBRID_PUBLIC_KEY_LEN,
        hybrid::HYBRID_SIGNATURE_LENGTH
    );
    println!();
    println!(
        "  Maya2C's signature is {:.0}x ed25519's, and both halves must verify:",
        hybrid::HYBRID_SIGNATURE_LENGTH as f64 / 64.0
    );
    println!("  there is no mode in which one suffices. See docs/hybrid-signatures.md.");
    println!();
    println!("Energy model");
    println!("============");
    println!();
    println!("  Joules per operation = measured seconds x {ASSUMED_PACKAGE_WATTS} W (ASSUMED).");
    println!("  RAPL is not readable without a driver on this platform, so package");
    println!("  power is an input, not a reading. Challenge this number first.");
    println!();
    println!(
        "  Example: a 1 ms operation models to {:.4} J.",
        joules(Duration::from_millis(1))
    );
    println!();
    println!("  There is deliberately NO energy-per-transaction figure. That needs");
    println!("  network hashrate and a hardware mix, neither observable, and Maya2C");
    println!("  has no live network — its hashrate is zero and the quotient undefined.");
    println!();
    println!("Elsewhere: kHeavyHash (Kaspa)");
    println!("=============================");
    println!();
    println!("  Timed in benches/crypto.rs against a port checked by Kaspa's own");
    println!("  known answers and by cSHAKE256 from NIST SP 800-185. A reference,");
    println!("  not Kaspa's miner (upstream uses assembly Keccak; miners use GPUs).");
    println!();
}

criterion_group!(benches, hashes, signatures, sizes);
criterion_main!(benches);
