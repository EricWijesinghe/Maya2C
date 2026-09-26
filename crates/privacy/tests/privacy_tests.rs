#![allow(clippy::unwrap_used, clippy::cast_precision_loss)]

use std::time::Instant;

use maya_privacy::{PrivacyError, ViewingKey, disclosure_key, open, open_disclosed, seal};
use maya_zk_stark::sanctions::{self, SanctionsList};

#[test]
fn the_account_viewing_key_opens_its_envelopes_and_no_one_elses() {
    let (alice, bob) = (ViewingKey::generate(), ViewingKey::generate());
    let env = seal(b"pay 250 to supplier 7, invoice 2026-114", &alice.ek);
    assert_eq!(
        open(&env, &alice).unwrap(),
        b"pay 250 to supplier 7, invoice 2026-114"
    );
    assert_eq!(open(&env, &bob), Err(PrivacyError::CannotOpen));
}

#[test]
fn a_disclosure_key_opens_one_transaction_and_not_the_accounts_others() {
    let alice = ViewingKey::generate();
    let (tax, salary) = (
        seal(b"sale: 1,000", &alice.ek),
        seal(b"salary: 4,000", &alice.ek),
    );
    let k = disclosure_key(&tax, &alice);
    assert_eq!(
        open_disclosed(&tax, &k).unwrap(),
        b"sale: 1,000",
        "the authority reads the disclosed transaction"
    );
    assert_eq!(
        open_disclosed(&salary, &k),
        Err(PrivacyError::CannotOpen),
        "and nothing else in the account"
    );
}

#[test]
fn a_tampered_envelope_does_not_open() {
    let alice = ViewingKey::generate();
    let mut bytes = seal(b"amount 10", &alice.ek).to_bytes();
    let last = bytes.len() - 1;
    bytes[last] ^= 1; // one bit of the sealed body
    let env = maya_privacy::Envelope::from_bytes(&bytes).unwrap();
    assert_eq!(open(&env, &alice), Err(PrivacyError::CannotOpen));
}

/// Association-set proof, "funds are not from a flagged set": the existing
/// STARK non-membership proof, measured on this machine (not a phone).
#[test]
fn not_from_a_flagged_set_proof_is_measured() {
    let flagged: Vec<[u8; 32]> = (0..1_000u32)
        .map(|i| *blake3::hash(&i.to_le_bytes()).as_bytes())
        .collect();
    let list = SanctionsList::build(flagged.iter().copied()).unwrap();
    let root = list.root().unwrap();
    let me = *blake3::hash(b"a clean deposit").as_bytes();
    let w = list.absence_witness(&me).unwrap();
    let t = Instant::now();
    let proof = sanctions::prove(&w, &root).unwrap();
    let prove_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    sanctions::verify(&proof, &root).unwrap();
    let verify_ms = t.elapsed().as_secs_f64() * 1e3;
    println!(
        "non-membership in a 1,000-entry flagged set: prove {prove_ms:.0} ms, verify {verify_ms:.1} ms, proof {} bytes ({}; desktop 4 vCPU, not a phone)",
        proof.as_bytes().len(),
        if cfg!(debug_assertions) {
            "debug assertions on"
        } else {
            "optimized"
        }
    );
    assert!(
        list.absence_witness(&flagged[3]).is_err(),
        "a flagged deposit cannot produce a witness"
    );
}
