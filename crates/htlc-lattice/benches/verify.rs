//! What a claim costs a validator: matrix expansion plus the product.
//!
//! The number that decides whether a block full of claims is a problem. A
//! hybrid signature verification is the comparison every transaction already
//! pays.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{Criterion, criterion_group, criterion_main};
use maya_htlc_lattice::{LatticeSecret, Matrix, Opening};
use std::hint::black_box;

fn verify(c: &mut Criterion) {
    let secret = LatticeSecret::from_entropy([7; 32]);
    let commitment = secret.commitment().expect("commit");
    let encoded = secret.opening().encode();

    c.bench_function("expand_matrix", |b| {
        b.iter(|| Matrix::expand(black_box(commitment.seed())));
    });
    c.bench_function("decode_and_verify_claim", |b| {
        b.iter(|| {
            let opening = Opening::decode(black_box(&encoded)).expect("decode");
            commitment.verify(&opening).expect("opens");
        });
    });
}

criterion_group!(benches, verify);
criterion_main!(benches);
