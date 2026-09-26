//! Quorum-certificate weight and verification throughput (Master Prompt 13
//! §3-4).
//!
//! Option (a) of the brief — a plain list of 2f+1 signatures with a compact
//! signer bitmap — measured with real ML-DSA signatures: certificate bytes,
//! serial verify time, and verify time across every core. Options (b)
//! STARK-aggregated and (c) hash-based multisignatures are not built; the
//! output says so rather than leaving a gap. Also: verifications per second
//! per core for every suite, the number ingest capacity is sized from.
//!
//! `cargo bench -p maya-crypto-pq --bench certificate`

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Instant;

use maya_crypto_pq::suite::{
    Ed25519, HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa65, MlDsa87, SignatureSuite,
    SlhDsaSha2_128s, SlhDsaShake256f, SuiteId, verify,
};

fn per_core<S: SignatureSuite>(id: SuiteId, n: usize) -> (f64, usize, usize) {
    let sk = S::signing_key_from_seed(&MasterSeed::from_bytes([7; 32]));
    let pk = S::public_key(&sk);
    let msg = b"maya2c vote: round 42, digest 0x...";
    let sig = S::sign(&sk, msg).unwrap();
    let t = Instant::now();
    for _ in 0..n {
        verify(id, &pk, msg, &sig).unwrap();
    }
    (n as f64 / t.elapsed().as_secs_f64(), pk.len(), sig.len())
}

struct Cert {
    bitmap: Vec<u8>,
    sigs: Vec<Vec<u8>>,
}

fn certificate<S: SignatureSuite>(id: SuiteId, validators: usize, threads: usize) {
    let quorum = 2 * ((validators - 1) / 3) + 1;
    let keys: Vec<_> = (0..validators)
        .map(|i| {
            let mut seed = [0u8; 32];
            seed[..8].copy_from_slice(&(i as u64).to_le_bytes());
            let sk = S::signing_key_from_seed(&MasterSeed::from_bytes(seed));
            let pk = S::public_key(&sk);
            (sk, pk)
        })
        .collect();
    let digest = [0xABu8; 32];
    let cert = Cert {
        bitmap: {
            let mut b = vec![0u8; validators.div_ceil(8)];
            for i in 0..quorum {
                b[i / 8] |= 1 << (i % 8);
            }
            b
        },
        sigs: keys[..quorum]
            .iter()
            .map(|(sk, _)| S::sign(sk, &digest).unwrap())
            .collect(),
    };
    let bytes = cert.bitmap.len() + cert.sigs.iter().map(Vec::len).sum::<usize>();
    // Verifiers hold public keys only; signing keys never cross a thread.
    let pks: Vec<Vec<u8>> = keys.into_iter().map(|(_, pk)| pk).collect();
    let t = Instant::now();
    for (i, s) in cert.sigs.iter().enumerate() {
        verify(id, &pks[i], &digest, s).unwrap();
    }
    let serial = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let chunk = quorum.div_ceil(threads);
    std::thread::scope(|scope| {
        for (c, part) in cert.sigs.chunks(chunk).enumerate() {
            let pks = &pks;
            scope.spawn(move || {
                for (j, s) in part.iter().enumerate() {
                    verify(id, &pks[c * chunk + j], &digest, s).unwrap();
                }
            });
        }
    });
    let parallel = t.elapsed().as_secs_f64() * 1e3;
    println!(
        "{:<22} n={validators:>3} quorum={quorum:>3}  {:>8} B ({:>6.1} KiB)  verify serial {serial:>7.2} ms  {threads} cores {parallel:>7.2} ms",
        format!("{id:?}"),
        bytes,
        bytes as f64 / 1024.0
    );
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    println!("== verifications per second, one core ==");
    let rows = [
        (
            "Ed25519 (classical)",
            per_core::<Ed25519>(SuiteId::Ed25519, 2_000),
        ),
        ("ML-DSA-65", per_core::<MlDsa65>(SuiteId::MlDsa65, 2_000)),
        ("ML-DSA-87", per_core::<MlDsa87>(SuiteId::MlDsa87, 2_000)),
        (
            "SLH-DSA-SHA2-128s",
            per_core::<SlhDsaSha2_128s>(SuiteId::SlhDsaSha2_128s, 200),
        ),
        (
            "SLH-DSA-SHAKE-256f",
            per_core::<SlhDsaShake256f>(SuiteId::SlhDsaShake256f, 50),
        ),
        (
            "Hybrid 65 + 128s",
            per_core::<HybridMlDsa65SlhDsa128s>(SuiteId::HybridMlDsa65SlhDsa128s, 200),
        ),
    ];
    for (name, (rate, pk, sig)) in rows {
        println!("{name:<22} {rate:>10.0} verify/s   pk {pk:>5} B   sig {sig:>6} B");
    }
    println!("\n== option (a): list of 2f+1 signatures + signer bitmap, {cores} cores ==");
    for n in [100usize, 200, 400] {
        certificate::<MlDsa65>(SuiteId::MlDsa65, n, cores);
        certificate::<MlDsa87>(SuiteId::MlDsa87, n, cores);
    }
    println!(
        "\noption (b) STARK-aggregated certificate: NOT BUILT (needs an ML-DSA verifier circuit in zk-stark)"
    );
    println!("option (c) hash-based multisignature: NOT BUILT (RESEARCH; no standardised scheme)");
}
