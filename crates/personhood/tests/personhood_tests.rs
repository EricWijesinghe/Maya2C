//! Proof-of-personhood (Master Prompt 6 §7) on synthetic iris codes: noise
//! the code corrects, people it does not confuse, proofs that expire and do
//! not replay, and verifications at scale with nothing biometric published.

#![allow(clippy::unwrap_used)]

use maya_personhood::{
    Challenge, PROOF_TTL_MS, PersonhoodError, Secret, TEMPLATE_BITS, TEMPLATE_BYTES, Template,
    Verifier, enroll, prove,
};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};

fn template(rng: &mut ChaCha20Rng) -> Template {
    let mut t = [0u8; TEMPLATE_BYTES];
    rng.fill_bytes(&mut t);
    Template(t)
}

/// A fresh scan of the same eye: each bit flipped independently with
/// probability `per_mille / 1000`.
fn rescan(t: &Template, per_mille: u32, rng: &mut ChaCha20Rng) -> Template {
    let mut scan = t.0;
    for i in 0..TEMPLATE_BITS {
        if rng.next_u32() % 1_000 < per_mille {
            scan[i / 8] ^= 1 << (i % 8);
        }
    }
    Template(scan)
}

fn challenge(n: u64) -> Challenge {
    let mut nonce = [0u8; 32];
    nonce[..8].copy_from_slice(&n.to_le_bytes());
    Challenge {
        verifier: [0xAA; 32],
        nonce,
        issued_ms: 1_000 * n,
    }
}

#[test]
fn genuine_scans_match_and_other_people_do_not() {
    const TRIALS: u64 = 200;
    let mut rng = ChaCha20Rng::seed_from_u64(1);
    let alice = template(&mut rng);
    let (helper, commitment) = enroll(&alice, &Secret([7; 16]));
    let mut verifier = Verifier::default();
    let mut n = 0;
    for per_mille in [50, 100, 150, 200] {
        let mut accepted = 0;
        for _ in 0..TRIALS {
            n += 1;
            let c = challenge(n);
            if let Ok(proof) = prove(&helper, &rescan(&alice, per_mille, &mut rng), &c, &mut rng) {
                verifier
                    .verify(&commitment, &c, &proof, c.issued_ms + 10)
                    .unwrap();
                accepted += 1;
            }
        }
        println!("bit-flip rate {per_mille}/1000: {accepted}/{TRIALS} genuine scans accepted");
        if per_mille <= 100 {
            assert_eq!(accepted, TRIALS, "the code corrects 10% noise");
        }
    }
    // Other people: independent templates sit near 50% distance.
    for _ in 0..TRIALS {
        n += 1;
        let c = challenge(n);
        let mallory = template(&mut rng);
        assert_eq!(
            prove(&helper, &mallory, &c, &mut rng),
            Err(PersonhoodError::NoMatch)
        );
    }
}

#[test]
fn a_proof_is_bound_to_its_challenge_and_expires_and_does_not_replay() {
    let mut rng = ChaCha20Rng::seed_from_u64(2);
    let alice = template(&mut rng);
    let (helper, commitment) = enroll(&alice, &Secret([9; 16]));
    let (_, bob) = enroll(&template(&mut rng), &Secret([3; 16]));
    let c = challenge(5);
    let proof = prove(&helper, &rescan(&alice, 50, &mut rng), &c, &mut rng).unwrap();
    let mut verifier = Verifier::default();
    // Late: its own verifier, since a verifier's clock never runs back.
    let stale = c.issued_ms + PROOF_TTL_MS + 1;
    assert_eq!(
        Verifier::default().verify(&commitment, &c, &proof, stale),
        Err(PersonhoodError::Stale)
    );
    assert_eq!(
        verifier.verify(&commitment, &c, &proof, c.issued_ms - 1),
        Err(PersonhoodError::Stale)
    );
    assert_eq!(
        verifier.verify(&bob, &c, &proof, c.issued_ms),
        Err(PersonhoodError::BadProof),
        "another person's commitment"
    );
    let other = Challenge {
        nonce: [1; 32],
        ..c
    };
    assert_eq!(
        verifier.verify(&commitment, &other, &proof, c.issued_ms),
        Err(PersonhoodError::BadProof),
        "another challenge"
    );
    verifier
        .verify(&commitment, &c, &proof, c.issued_ms + PROOF_TTL_MS)
        .unwrap();
    assert_eq!(
        verifier.verify(&commitment, &c, &proof, c.issued_ms + 1),
        Err(PersonhoodError::Replayed)
    );
}

fn verifications(count: u64) {
    let mut rng = ChaCha20Rng::seed_from_u64(3);
    let alice = template(&mut rng);
    let (helper, commitment) = enroll(&alice, &Secret([5; 16]));
    let mut verifier = Verifier::default();
    let started = std::time::Instant::now();
    let mut rescans = 0u64;
    for n in 1..=count {
        let c = challenge(n);
        // A false reject (about 2e-5 per scan at 5% noise) is met the way a
        // device meets it: scan again.
        let proof = loop {
            match prove(&helper, &rescan(&alice, 50, &mut rng), &c, &mut rng) {
                Ok(proof) => break proof,
                Err(PersonhoodError::NoMatch) if rescans < count / 100 => rescans += 1,
                Err(e) => panic!("verification {n}: {e}"),
            }
        };
        // What leaves the device is the commitment (once) and the proof; no
        // 8-byte run of the template appears in either.
        if n == 1 {
            let published = [&commitment.0[..], &proof].concat();
            assert!(
                alice
                    .0
                    .windows(8)
                    .all(|w| !published.windows(8).any(|p| p == w))
            );
        }
        verifier
            .verify(&commitment, &c, &proof, c.issued_ms + 1)
            .unwrap();
    }
    println!(
        "{count} private verifications in {:?} (scan, correct, derive key, sign, verify); {rescans} false rejects, each met by a rescan",
        started.elapsed()
    );
}

#[test]
fn a_thousand_private_verifications() {
    verifications(1_000);
}

/// The brief's figure. Run for the report: `cargo test --release -p
/// maya-personhood -- --ignored --nocapture`.
#[test]
#[ignore = "100,000 ML-DSA signatures; run in release for the report"]
fn a_hundred_thousand_private_verifications() {
    verifications(100_000);
}

#[test]
fn the_replay_set_forgets_what_has_expired() {
    let mut rng = ChaCha20Rng::seed_from_u64(4);
    let alice = template(&mut rng);
    let (helper, commitment) = enroll(&alice, &Secret([1; 16]));
    let mut verifier = Verifier::default();
    for n in 1..=20 {
        let c = challenge(n); // issued 1 s apart
        let proof = prove(&helper, &rescan(&alice, 50, &mut rng), &c, &mut rng).unwrap();
        verifier
            .verify(&commitment, &c, &proof, c.issued_ms)
            .unwrap();
    }
    assert!(
        verifier.remembered() <= 4,
        "only the last TTL of challenges: {}",
        verifier.remembered()
    );
    // A late proof for an old challenge is stale even under an earlier clock.
    let old = challenge(3);
    let proof = prove(&helper, &rescan(&alice, 50, &mut rng), &old, &mut rng).unwrap();
    assert_eq!(
        verifier.verify(&commitment, &old, &proof, old.issued_ms + 1),
        Err(PersonhoodError::Stale)
    );
    // The same nonce from another verifier is another challenge.
    let other = Challenge {
        verifier: [0xBB; 32],
        ..challenge(20)
    };
    let proof = prove(&helper, &rescan(&alice, 50, &mut rng), &other, &mut rng).unwrap();
    verifier
        .verify(&commitment, &other, &proof, other.issued_ms)
        .unwrap();
}
