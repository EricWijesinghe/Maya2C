//! SIM: EEG authentication end to end on synthetic recordings (Master
//! Prompt 6 §7): the EDF parser, a genuine user, an impostor, a replayed
//! recording, and proofs that expire after three seconds and do not replay.

#![allow(clippy::unwrap_used)]

use maya_bci_sim::sim::Subject;
use maya_bci_sim::{BciError, SIM, edf, enroll, features, respond, stimulus_hz};
use maya_personhood::{Challenge, PROOF_TTL_MS, PersonhoodError, Secret, Verifier};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::SeedableRng;

fn challenge(n: u8, issued_ms: u64) -> Challenge {
    Challenge {
        verifier: [0xAA; 32],
        nonce: [n; 32],
        issued_ms,
    }
}

fn differing_bits(a: &[u8], b: &[u8]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

#[test]
fn the_edf_parser_round_trips_and_refuses_inconsistent_files() {
    println!("{SIM}");
    let samples = vec![
        (0..512)
            .map(|t| f64::from(t % 50) - 25.0)
            .collect::<Vec<f64>>();
        2
    ];
    let bytes = edf::write(&["Fp1", "O1"], 256, &samples, 100.0);
    let parsed = edf::parse(&bytes).unwrap();
    assert_eq!(parsed.signals.len(), 2);
    assert_eq!(parsed.signals[1].label, "O1");
    assert!((parsed.signals[0].rate_hz - 256.0).abs() < 1e-9);
    let worst = parsed.signals[0]
        .samples
        .iter()
        .zip(&samples[0])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f64::max);
    assert!(worst < 0.01, "16-bit quantization only: {worst}");
    assert!(
        edf::parse(&bytes[..bytes.len() - 1]).is_err(),
        "a record cut short"
    );
    assert!(edf::parse(&bytes[..300]).is_err(), "a header cut short");
    let mut lying = bytes.clone();
    lying[252..256].copy_from_slice(b"3   ");
    assert!(
        edf::parse(&lying).is_err(),
        "signal count disagrees with the header size"
    );
}

#[test]
fn sessions_of_one_subject_agree_and_subjects_differ() {
    let alice = Subject::new(1);
    let t = |s: &Subject, session| {
        features::template(&edf::parse(&s.record(session, None)).unwrap())
            .unwrap()
            .0
    };
    let a1 = t(&alice, 1);
    let same: Vec<u32> = (2..8).map(|s| differing_bits(&a1, &t(&alice, s))).collect();
    let other: Vec<u32> = (2..8)
        .map(|id| differing_bits(&a1, &t(&Subject::new(id), 1)))
        .collect();
    println!(
        "{SIM}: of 2,048 template bits, sessions of one subject differ in {same:?}; other subjects in {other:?}"
    );
    assert!(same.iter().all(|d| *d < 150));
    assert!(other.iter().all(|d| *d > 700));
}

#[test]
fn a_live_user_authenticates_and_a_replay_or_an_impostor_does_not() {
    let mut rng = ChaCha20Rng::seed_from_u64(9);
    let alice = Subject::new(1);
    let (helper, commitment) = enroll(&alice.record(1, None), &Secret([3; 16])).unwrap();
    let mut verifier = Verifier::default();

    // Live: a fresh session showing the challenged stimulus.
    let c = challenge(1, 10_000);
    let live = alice.record(2, Some(stimulus_hz(&c)));
    let proof = respond(&helper, &live, &c, &mut rng).unwrap();
    verifier
        .verify(&commitment, &c, &proof, c.issued_ms + 500)
        .unwrap();
    assert_eq!(
        verifier.verify(&commitment, &c, &proof, c.issued_ms + 600),
        Err(PersonhoodError::Replayed)
    );

    // Replay: the same recording against a challenge for another stimulus.
    let next = challenge(2, 20_000);
    assert!((stimulus_hz(&next) - stimulus_hz(&c)).abs() > 0.5);
    assert!(matches!(
        respond(&helper, &live, &next, &mut rng),
        Err(BciError::NotLive(_))
    ));
    // No stimulus at all: not live either.
    assert!(matches!(
        respond(&helper, &alice.record(3, None), &next, &mut rng),
        Err(BciError::NotLive(_))
    ));

    // An impostor, live and looking at the right stimulus: not Alice.
    let mallory = Subject::new(77).record(1, Some(stimulus_hz(&next)));
    assert_eq!(
        respond(&helper, &mallory, &next, &mut rng),
        Err(BciError::Personhood(PersonhoodError::NoMatch))
    );

    // A genuine proof presented after three seconds is stale.
    let late = challenge(3, 30_000);
    let proof = respond(
        &helper,
        &alice.record(4, Some(stimulus_hz(&late))),
        &late,
        &mut rng,
    )
    .unwrap();
    assert_eq!(
        Verifier::default().verify(
            &commitment,
            &late,
            &proof,
            late.issued_ms + PROOF_TTL_MS + 1
        ),
        Err(PersonhoodError::Stale)
    );
}
