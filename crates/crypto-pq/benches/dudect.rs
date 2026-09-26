//! dudect-style timing leakage tests (Reparaz, Balasch, Verbauwhede 2017).
//!
//! Each bench times one operation on inputs from two classes and runs Welch's
//! t-test on the two timing distributions. |t| below ~4.5 means no leak was
//! *detected* at that sample size; above ~10 is a leak. It is evidence, not
//! proof — a leak smaller than the noise floor survives any sample size the
//! run could afford — and the numbers go into `reports/dudect.txt` exactly as
//! printed, not as a pass/fail.
//!
//! Run with `bash scripts/dudect.sh` (all benches, written to
//! `reports/dudect.txt`) or `bash scripts/dudect.sh --filter <name>`. Not
//! `cargo bench`: cargo appends `--bench`, which dudect-bencher rejects.
//!
//! | bench | Left class | Right class | expected |
//! |---|---|---|---|
//! | `control_naive_eq` | equal 4 KiB buffers | differ at byte 0 | **leaks** — proves the harness can see one |
//! | `ct_eq_shared_secret` | equal secrets | differ at byte 0 | no leak |
//! | `ml_kem_768_decaps` | valid ciphertext | random ciphertext (implicit rejection) | no leak |
//! | `hqc_128_decaps` | valid ciphertext | random ciphertext | no leak (draft standard) |
//! | `x_wing_decaps` | valid ciphertext | random ciphertext | no leak |
//! | `ml_dsa_65_sign` | one fixed key | one of 32 other keys | no leak in the key |
//! | `ed25519_sign` | one fixed key | one of 32 other keys | no leak in the key |
//! | `*_decaps_null` | valid ciphertext | another valid ciphertext | no difference — rules out the harness |
//! | `ml_dsa_65_verify` | valid signature | tampered signature | may differ: all inputs are public |
//!
//! SLH-DSA signing is not timed: at milliseconds per signature, the ~10^5
//! samples a meaningful t needs are hours per run, and FIPS 205 signing has no
//! secret-dependent branch to find. That is a gap, stated, not a pass.

use dudect_bencher::{BenchRng, Class, CtRunner, ctbench_main};
use maya_crypto_pq::kem_suite::{self, KemSuite};
use maya_crypto_pq::suite::{self, MasterSeed, SignatureSuite};
use rand::{Rng as _, RngExt as _};

const KEM_SAMPLES: usize = 100_000;
const SIGN_SAMPLES: usize = 40_000;
const CMP_SAMPLES: usize = 200_000;
/// Long enough that a full scan and a first-byte exit differ by far more than
/// timer noise: the control must be *seen* to leak or the harness proves nothing.
const CONTROL_LEN: usize = 4_096;
const KEY_POOL: usize = 32;

/// Draws a class: `true` is Left. (`Class` has no `PartialEq`.)
fn draw(rng: &mut BenchRng) -> (bool, Class) {
    let left = rng.random::<bool>();
    (left, if left { Class::Left } else { Class::Right })
}

fn bytes(rng: &mut BenchRng, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    rng.fill_bytes(&mut out);
    out
}

fn control_naive_eq(runner: &mut CtRunner, rng: &mut BenchRng) {
    let inputs: Vec<(Class, Vec<u8>, Vec<u8>)> = (0..CMP_SAMPLES / 10)
        .map(|_| {
            let a = bytes(rng, CONTROL_LEN);
            let (left, c) = draw(rng);
            let mut b = a.clone();
            if !left {
                b[0] ^= 1;
            }
            (c, a, b)
        })
        .collect();
    for (c, a, b) in inputs {
        runner.run_one(c, || a == b);
    }
}

fn ct_eq_shared_secret(runner: &mut CtRunner, rng: &mut BenchRng) {
    let (dk, ek) = kem_suite::MlKem768::generate().expect("generate");
    let inputs: Vec<_> = (0..KEM_SAMPLES)
        .map(|_| {
            let (ct, sent) = kem_suite::MlKem768::encapsulate(&ek).expect("encapsulate");
            let (left, c) = draw(rng);
            let other = if left {
                kem_suite::MlKem768::decapsulate(&dk, &ct).expect("decapsulate")
            } else {
                kem_suite::MlKem768::encapsulate(&ek)
                    .expect("encapsulate")
                    .1
            };
            (c, sent, other)
        })
        .collect();
    for (c, sent, other) in inputs {
        runner.run_one(c, || sent.ct_eq(&other));
    }
}

fn decaps<K: KemSuite>(runner: &mut CtRunner, rng: &mut BenchRng, samples: usize) {
    let (dk, ek) = K::generate().expect("generate");
    // Every input is prepared before the first measurement: interleaving
    // `encapsulate` (which warms the key's cache lines) with the timed
    // `decapsulate` for one class only is exactly the artefact that made the
    // first run report a leak here.
    // Both classes' buffers are allocated by the same statement and filled
    // afterwards. Taking one class straight from `encapsulate` and the other
    // from `bytes` allocates them through different paths, which puts them in
    // different places -- a difference the timer can see that the algorithm
    // does not have. That artefact is worth more than it sounds: with it, HQC
    // measured |t| = 26.9 here.
    let inputs: Vec<(Class, Vec<u8>)> = (0..samples)
        .map(|_| {
            let (left, c) = draw(rng);
            let mut ct = vec![0u8; K::CIPHERTEXT_LEN];
            if left {
                ct.copy_from_slice(&K::encapsulate(&ek).expect("encapsulate").0[..]);
            } else {
                rng.fill_bytes(&mut ct);
            }
            (c, ct)
        })
        .collect();
    for (c, ct) in inputs {
        runner.run_one(c, || K::decapsulate(&dk, &ct).map(|s| s.as_bytes()[0]));
    }
}

/// Null control: both classes are valid ciphertexts. A large |t| here would
/// mean the harness, not the KEM, produces the difference.
fn decaps_null<K: KemSuite>(runner: &mut CtRunner, rng: &mut BenchRng, samples: usize) {
    let (dk, ek) = K::generate().expect("generate");
    let inputs: Vec<(Class, Vec<u8>)> = (0..samples)
        .map(|_| {
            let mut ct = vec![0u8; K::CIPHERTEXT_LEN];
            ct.copy_from_slice(&K::encapsulate(&ek).expect("encapsulate").0[..]);
            (draw(rng).1, ct)
        })
        .collect();
    for (c, ct) in inputs {
        runner.run_one(c, || K::decapsulate(&dk, &ct).map(|s| s.as_bytes()[0]));
    }
}

fn ml_kem_768_decaps_null(runner: &mut CtRunner, rng: &mut BenchRng) {
    decaps_null::<kem_suite::MlKem768>(runner, rng, KEM_SAMPLES);
}

fn hqc_128_decaps_null(runner: &mut CtRunner, rng: &mut BenchRng) {
    decaps_null::<kem_suite::Hqc128>(runner, rng, KEM_SAMPLES / 4);
}

fn ml_kem_768_decaps(runner: &mut CtRunner, rng: &mut BenchRng) {
    decaps::<kem_suite::MlKem768>(runner, rng, KEM_SAMPLES);
}

fn hqc_128_decaps(runner: &mut CtRunner, rng: &mut BenchRng) {
    decaps::<kem_suite::Hqc128>(runner, rng, KEM_SAMPLES / 4);
}

fn x_wing_decaps(runner: &mut CtRunner, rng: &mut BenchRng) {
    decaps::<kem_suite::XWing>(runner, rng, KEM_SAMPLES);
}

fn sign<S: SignatureSuite>(runner: &mut CtRunner, rng: &mut BenchRng) {
    let seed = |rng: &mut BenchRng| {
        let mut s = [0u8; 32];
        rng.fill_bytes(&mut s);
        MasterSeed::from_bytes(s)
    };
    let fixed = S::signing_key_from_seed(&seed(rng));
    let pool: Vec<_> = (0..KEY_POOL)
        .map(|_| S::signing_key_from_seed(&seed(rng)))
        .collect();
    let inputs: Vec<(bool, Class, Vec<u8>)> = (0..SIGN_SAMPLES)
        .map(|_| {
            let (left, c) = draw(rng);
            (left, c, bytes(rng, 64))
        })
        .collect();
    for (i, (left, c, message)) in inputs.into_iter().enumerate() {
        let key = if left { &fixed } else { &pool[i % KEY_POOL] };
        runner.run_one(c, || S::sign(key, &message).map(|s| s[0]));
    }
}

fn ml_dsa_65_sign(runner: &mut CtRunner, rng: &mut BenchRng) {
    sign::<suite::MlDsa65>(runner, rng);
}

fn ed25519_sign(runner: &mut CtRunner, rng: &mut BenchRng) {
    sign::<suite::Ed25519>(runner, rng);
}

fn ml_dsa_65_verify(runner: &mut CtRunner, rng: &mut BenchRng) {
    let key = suite::MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([7; 32]));
    let public = suite::MlDsa65::public_key(&key);
    let inputs: Vec<(Class, Vec<u8>, Vec<u8>)> = (0..SIGN_SAMPLES)
        .map(|_| {
            let message = bytes(rng, 64);
            let mut sig = suite::MlDsa65::sign(&key, &message).expect("sign");
            let (left, c) = draw(rng);
            if !left {
                sig[100] ^= 1;
            }
            (c, message, sig)
        })
        .collect();
    for (c, message, sig) in inputs {
        runner.run_one(c, || {
            suite::MlDsa65::verify(&public, &message, &sig).is_ok()
        });
    }
}

ctbench_main!(
    control_naive_eq,
    ct_eq_shared_secret,
    ml_kem_768_decaps,
    ml_kem_768_decaps_null,
    hqc_128_decaps,
    hqc_128_decaps_null,
    x_wing_decaps,
    ml_dsa_65_sign,
    ed25519_sign,
    ml_dsa_65_verify
);
