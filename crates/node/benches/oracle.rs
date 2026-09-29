//! Oracle costs, and the sizes that bound them.
//!
//! Three numbers decide whether this subsystem belongs in a block:
//!
//! 1. **VRF verification.** Every node checks the beacon proof in every block,
//!    forever. If it is not cheap, running a node is not cheap.
//! 2. **Feed verification.** A quorum of post-quantum signatures is the single
//!    most expensive thing a transaction can ask a node to do, and
//!    `MAX_FEED_SUBMISSIONS_PER_BLOCK` is only a defensible number if this one
//!    is known.
//! 3. **Feed size.** ML-DSA and SLH-DSA do not aggregate, so a submission grows
//!    linearly in the quorum. This is not measured — it is arithmetic on
//!    constants — but it is *reported* here, because it is the constraint that
//!    binds first and the one most likely to be forgotten.
//!
//! Run with:
//! ```text
//! cargo bench --bench oracle
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{Criterion, criterion_group, criterion_main};
use custom_l1_node::crypto::hybrid::{
    HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LENGTH, HybridVerifyingKey, generate_signing_key,
};
use custom_l1_node::oracle::beacon::beacon_alpha;
use custom_l1_node::oracle::feed::{median, observation_bytes};
use maya_vrf::ecvrf::{prove, verify};
use maya_vrf::keys::VrfSecretKey;
use std::hint::black_box;


mod common;
/// Bytes one authority adds to a feed submission.
const OBSERVATION_SIZE: usize = 8 + HYBRID_PUBLIC_KEY_LEN + HYBRID_SIGNATURE_LENGTH;

fn bench_vrf(c: &mut Criterion) {
    let secret = VrfSecretKey::from_seed([7u8; 32]);
    let public = secret.public_key();
    let alpha = beacon_alpha(&[42u8; 32], 1_000_000);
    let proof = prove(&secret, &alpha).expect("prove");

    let mut group = c.benchmark_group("vrf");

    // Paid once per block by whichever authority is proposing.
    group.bench_function("prove", |b| {
        b.iter(|| black_box(prove(black_box(&secret), black_box(&alpha)).expect("prove")));
    });

    // Paid once per block by *every node, forever*. This is the number that
    // matters.
    group.bench_function("verify", |b| {
        b.iter(|| {
            black_box(
                verify(black_box(&public), black_box(&alpha), black_box(&proof)).expect("verify"),
            )
        });
    });

    group.finish();
}

fn bench_feed(c: &mut Criterion) {
    let key = generate_signing_key().expect("key");
    let message = observation_bytes(&[1u8; 32], 1, 100, 1);
    let signature = key.sign(&message).expect("sign");
    let verifying = HybridVerifyingKey::from_public_key(&key.public_key()).expect("key");

    let mut group = c.benchmark_group("feed");

    // One authority's attestation. A quorum costs this times the threshold,
    // because post-quantum signatures do not aggregate — there is no analogue
    // of a BLS multisignature that would make a quorum constant-cost.
    group.bench_function("verify_one_observation", |b| {
        b.iter(|| {
            verifying
                .verify(black_box(&message), black_box(&signature))
                .expect("verify");
        });
    });

    // The aggregation itself, for scale against the verification above. It is
    // a sort; it is not where the time goes, and showing that is the point.
    let values: Vec<u64> = (0..21u64).map(|index| 100 + index * 7 % 13).collect();
    group.bench_function("median_of_21", |b| {
        b.iter(|| black_box(median(black_box(&values))));
    });

    group.finish();
}

fn report_sizes(c: &mut Criterion) {
    // Not a benchmark. Criterion is simply the thing that runs, and these are
    // the numbers a reader of the timings above needs beside them — the bytes
    // bind before the CPU does, and no timing shows that.
    eprintln!("\n  feed submission size, by quorum (post-quantum signatures do not aggregate):");
    for quorum in [3usize, 5, 11, 21] {
        let bytes = 32 + 8 + 8 + 8 + quorum * OBSERVATION_SIZE;
        eprintln!(
            "    {quorum:>2}-signer quorum: {:>7} bytes ({:.1} KiB), {} per 8 MiB block",
            bytes,
            bytes as f64 / 1024.0,
            (8 * 1024 * 1024) / bytes
        );
    }
    eprintln!(
        "    one observation: {OBSERVATION_SIZE} bytes \
         ({HYBRID_PUBLIC_KEY_LEN} key + {HYBRID_SIGNATURE_LENGTH} signature + 8 value)\n"
    );

    // A single trivial measurement, so the group is not empty.
    c.bench_function("observation_bytes", |b| {
        b.iter(|| black_box(observation_bytes(black_box(&[1u8; 32]), 1, 100, 1)));
    });
}

criterion_group!(benches, bench_vrf, bench_feed, report_sizes);
criterion_main!(benches);
