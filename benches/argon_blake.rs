//! ArgonBlake proof-of-work benchmark.
//!
//! Exists to answer one question with data rather than assertion: does any
//! build-flag or SIMD tuning actually move ArgonBlake's cost?
//!
//! The two stages are benchmarked separately because they have wildly different
//! profiles. BLAKE3 is SIMD-accelerated by default with runtime CPU dispatch
//! (the crate exposes only `no_avx2` / `no_avx512` *opt-outs*), and costs
//! microseconds. Argon2id over 32 MiB is a portable scalar implementation —
//! `argon2 0.5` ships no SIMD feature at all — and costs tens of milliseconds.
//!
//! Splitting them makes the ratio visible, which is the point: tuning the cheap
//! stage cannot move a total the expensive stage dominates.
//!
//! Run with:
//! ```text
//! cargo bench --bench argon_blake
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use custom_l1_node::core::HEADER_LEN;
use custom_l1_node::crypto::argon_blake::argon_blake_hash;
use std::hint::black_box;

fn bench_argon_blake(c: &mut Criterion) {
    let header = [0x5Au8; HEADER_LEN];

    let mut group = c.benchmark_group("argon_blake");
    // Each sample is a 32 MiB memory-hard pass; the default 100 samples would
    // take minutes for no extra precision.
    group.sample_size(20);

    // The full three-stage hash: one proof-of-work attempt.
    group.bench_function("full_hash", |b| {
        b.iter(|| argon_blake_hash(black_box(&header)).expect("hash"));
    });

    // Stage 1 in isolation: the BLAKE3 pre-hash.
    group.bench_function("blake3_prehash_only", |b| {
        b.iter(|| blake3::hash(black_box(&header)));
    });

    group.finish();
}

criterion_group!(benches, bench_argon_blake);
criterion_main!(benches);
