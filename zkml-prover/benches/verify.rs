//! How long `host_verify_zkml_proof` takes, measured rather than claimed.
//!
//! The brief's budget is 10 ms. What is timed is exactly what the host function
//! does per call — parse the verifying key from bytes, then verify — because a
//! contract passes the key in on every call and the node caches nothing across
//! transactions. The one-off cost of deriving the SRS is reported separately:
//! it is paid once per process, not per proof.
//!
//! Run with `cargo bench -p maya-zkml-prover --bench verify`. The number in
//! `docs/zkml.md` came from this.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use maya_zkml::{srs, verify};
use maya_zkml_prover::onnx;
use maya_zkml_prover::prove::{keygen, prove};

fn bench(c: &mut Criterion) {
    let started = Instant::now();
    let _ = srs::params();
    eprintln!("SRS derivation (once per process): {:?}", started.elapsed());

    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/zkml/classifier.onnx");
    let setup = keygen(&onnx::load(path).expect("fixture")).expect("keygen");
    let x = [127i8, -128, 5, 0];
    let (proof, class) = prove(&setup, &x).expect("prove");
    let public: Vec<i64> = x
        .iter()
        .map(|&v| i64::from(v))
        .chain([class as i64])
        .collect();
    assert_eq!(
        verify::verify(setup.verifying_key(), &public, &proof),
        Ok(true)
    );

    c.bench_function("verify (parse key + verify proof)", |b| {
        b.iter(|| {
            verify::verify(
                black_box(setup.verifying_key()),
                black_box(&public),
                black_box(&proof),
            )
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
