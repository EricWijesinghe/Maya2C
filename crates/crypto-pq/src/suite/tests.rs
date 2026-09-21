use super::*;

fn seed(byte: u8) -> MasterSeed {
    MasterSeed::from_bytes([byte; SEED_LEN])
}

/// Every property a suite must have, checked generically.
fn exercise<S: SignatureSuite>() {
    let key = S::signing_key_from_seed(&seed(7));
    let public = S::public_key(&key);
    let signature = S::sign(&key, b"maya2c").expect("sign");
    let info = S::ID.info();

    assert_eq!(
        public.len(),
        info.public_key_len,
        "{:?} public key length",
        S::ID
    );
    assert_eq!(
        signature.len(),
        info.signature_len,
        "{:?} signature length",
        S::ID
    );
    assert_eq!(S::verify(&public, b"maya2c", &signature), Ok(()));
    assert_eq!(verify(S::ID, &public, b"maya2c", &signature), Ok(()));

    // Deterministic: same key, same message, same bytes.
    assert_eq!(S::sign(&key, b"maya2c").expect("sign"), signature);

    // A different message.
    assert_eq!(
        S::verify(&public, b"maya2d", &signature),
        Err(SuiteError::Verification(S::ID))
    );

    // Another key.
    let other = S::public_key(&S::signing_key_from_seed(&seed(8)));
    assert_ne!(other, public);
    assert!(S::verify(&other, b"maya2c", &signature).is_err());

    // A flipped bit in the middle and at each end of the signature.
    for index in [0, signature.len() / 2, signature.len() - 1] {
        let mut tampered = signature.clone();
        tampered[index] ^= 0x01;
        assert!(
            S::verify(&public, b"maya2c", &tampered).is_err(),
            "{:?} bit {index}",
            S::ID
        );
    }

    // Lengths are exact, in both directions.
    let mut long = signature.clone();
    long.push(0);
    assert!(matches!(
        S::verify(&public, b"maya2c", &long),
        Err(SuiteError::SignatureLength { .. })
    ));
    assert!(matches!(
        S::verify(&public[1..], b"maya2c", &signature),
        Err(SuiteError::PublicKeyLength { .. })
    ));
}

#[test]
fn ed25519_behaves_as_a_suite() {
    exercise::<Ed25519>();
}

#[test]
fn ml_dsa_65_behaves_as_a_suite() {
    exercise::<MlDsa65>();
}

#[test]
fn ml_dsa_87_behaves_as_a_suite() {
    exercise::<MlDsa87>();
}

#[test]
fn slh_dsa_sha2_128s_behaves_as_a_suite() {
    exercise::<SlhDsaSha2_128s>();
}

#[test]
fn slh_dsa_shake_256f_behaves_as_a_suite() {
    exercise::<SlhDsaShake256f>();
}

#[test]
fn the_hybrid_behaves_as_a_suite() {
    exercise::<HybridMlDsa65SlhDsa128s>();
}

#[test]
fn the_hybrid_rejects_a_signature_with_either_half_broken() {
    let key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&seed(3));
    let public = HybridMlDsa65SlhDsa128s::public_key(&key);
    let good = HybridMlDsa65SlhDsa128s::sign(&key, b"m").expect("sign");
    let split = ml_dsa::ML_DSA_65_SIGNATURE_LEN;

    let mut lattice_broken = good.clone();
    lattice_broken[10] ^= 1;
    let mut hash_broken = good.clone();
    hash_broken[split + 10] ^= 1;

    for bad in [lattice_broken, hash_broken] {
        assert!(HybridMlDsa65SlhDsa128s::verify(&public, b"m", &bad).is_err());
    }
}

#[test]
fn the_hybrid_rejects_a_valid_half_paired_with_a_foreign_half() {
    let a = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&seed(1));
    let b = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&seed(2));
    let public_a = HybridMlDsa65SlhDsa128s::public_key(&a);
    let sig_a = HybridMlDsa65SlhDsa128s::sign(&a, b"m").expect("sign");
    let sig_b = HybridMlDsa65SlhDsa128s::sign(&b, b"m").expect("sign");
    let split = ml_dsa::ML_DSA_65_SIGNATURE_LEN;

    let mut spliced = sig_a[..split].to_vec();
    spliced.extend_from_slice(&sig_b[split..]);
    assert!(HybridMlDsa65SlhDsa128s::verify(&public_a, b"m", &spliced).is_err());
}

#[test]
fn different_suites_derive_unrelated_keys_from_one_seed() {
    let s = seed(9);
    let ml65 = MlDsa65::public_key(&MlDsa65::signing_key_from_seed(&s));
    let hybrid =
        HybridMlDsa65SlhDsa128s::public_key(&HybridMlDsa65SlhDsa128s::signing_key_from_seed(&s));
    assert_ne!(&hybrid[..ml65.len()], ml65.as_slice());
}

#[test]
fn every_byte_either_names_a_suite_or_is_refused() {
    let mut named = 0;
    for byte in 0..=u8::MAX {
        match SuiteId::try_from(byte) {
            Ok(id) => {
                assert_eq!(id.to_byte(), byte);
                assert_eq!(id.info().id, id);
                named += 1;
            }
            Err(e) => assert_eq!(e, SuiteError::UnknownSuite(byte)),
        }
    }
    assert_eq!(named, SuiteId::ALL.len());
}

#[test]
fn the_registry_rows_are_in_id_order_and_consistent() {
    for (row, id) in REGISTRY.iter().zip(SuiteId::ALL) {
        assert_eq!(row.id, id);
    }
    assert_eq!(
        REGISTRY[1].signature_len, 3309,
        "final FIPS 204, not the 3,293 draft"
    );
    assert!(!SuiteId::Ed25519.info().mainnet_allowed);
    assert_eq!(SuiteId::Ed25519.info().pq_security_bits, 0);
}

#[test]
fn ed25519_keys_round_trip_through_pkcs8() {
    let key = Ed25519::signing_key_from_seed(&seed(4));
    let der = Ed25519::to_pkcs8_der(&key).expect("encode");
    let back = Ed25519::from_pkcs8_der(&der).expect("decode");
    assert_eq!(Ed25519::public_key(&back), Ed25519::public_key(&key));
    assert_eq!(
        Ed25519::from_pkcs8_der(&der[1..]).map(|_| ()),
        Err(SuiteError::KeyEncoding(SuiteId::Ed25519))
    );
}

#[test]
fn ed25519_refuses_a_small_order_public_key() {
    // The identity point: every signature with R = identity and S = 0
    // "verifies" under plain `verify` for some messages; strict refuses it.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let mut signature = [0u8; 64];
    signature[0] = 1;
    assert!(Ed25519::verify(&identity, b"anything", &signature).is_err());
}

#[test]
fn secrets_do_not_print() {
    let text = format!(
        "{:?} {:?}",
        seed(1),
        HybridMlDsa65SlhDsa128s::signing_key_from_seed(&seed(1))
    );
    assert!(text.contains("redacted"));
    assert!(!text.contains("1, 1"));
}
