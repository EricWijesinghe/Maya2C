use super::dual::combine_for_test;
use super::*;

fn exercise<K: KemSuite>() {
    let (dk, ek) = K::generate().expect("generate");
    assert_eq!(ek.len(), K::ENCAPSULATION_KEY_LEN, "{:?} ek length", K::ID);

    let (ct, sent) = K::encapsulate(&ek).expect("encapsulate");
    assert_eq!(ct.len(), K::CIPHERTEXT_LEN, "{:?} ct length", K::ID);
    let received = K::decapsulate(&dk, &ct).expect("decapsulate");
    assert!(sent.ct_eq(&received), "{:?} secrets agree", K::ID);

    // Randomized: a second encapsulation to the same key differs.
    let (ct2, sent2) = K::encapsulate(&ek).expect("encapsulate");
    assert_ne!(ct, ct2);
    assert!(!sent.ct_eq(&sent2));

    // Implicit rejection: a tampered ciphertext decapsulates, to a different secret.
    let mut tampered = ct.clone();
    tampered[ct.len() / 2] ^= 1;
    let other = K::decapsulate(&dk, &tampered).expect("implicit rejection, not an error");
    assert!(!other.ct_eq(&sent), "{:?} tamper detected", K::ID);

    // Exact lengths.
    assert_eq!(
        K::encapsulate(&ek[1..]).map(|_| ()),
        Err(KemSuiteError::EncapsulationKey(K::ID))
    );
    assert_eq!(
        K::decapsulate(&dk, &ct[1..]).map(|_| ()),
        Err(KemSuiteError::Ciphertext(K::ID))
    );

    assert!(format!("{sent:?}").contains("redacted"));
}

#[test]
fn ml_kem_768_behaves_as_a_kem() {
    exercise::<MlKem768>();
}

#[test]
fn ml_kem_1024_behaves_as_a_kem() {
    exercise::<MlKem1024>();
}

#[test]
fn hqc_128_behaves_as_a_kem() {
    exercise::<Hqc128>();
}

#[test]
fn hqc_256_behaves_as_a_kem() {
    exercise::<Hqc256>();
}

#[test]
fn x_wing_behaves_as_a_kem() {
    exercise::<XWing>();
}

#[test]
fn dual_kem_768_hqc_128_behaves_as_a_kem() {
    exercise::<DualKem768Hqc128>();
}

#[test]
fn dual_kem_1024_hqc_256_behaves_as_a_kem() {
    exercise::<DualKem1024Hqc256>();
}

#[test]
fn corrupting_only_the_hqc_half_changes_the_dual_secret() {
    let (dk, ek) = DualKem768Hqc128::generate().expect("generate");
    let (ct, sent) = DualKem768Hqc128::encapsulate(&ek).expect("encapsulate");
    let mut tampered = ct.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1; // inside ct_hqc, the ML-KEM half is untouched
    let got = DualKem768Hqc128::decapsulate(&dk, &tampered).expect("decapsulate");
    assert!(!got.ct_eq(&sent));
}

#[test]
fn the_combiner_depends_on_every_input() {
    let a = SharedSecret::from_slice(&[1; 32]);
    let b = SharedSecret::from_slice(&[2; 32]);
    let z = SharedSecret::from_slice(&[0; 32]);
    let id = KemId::DualKem768Hqc128;
    let base = combine_for_test(id, [&a, &b], [b"ct_a", b"ct_b"], [b"ek_a", b"ek_b"]);

    // Knowing one component secret is not enough: replacing either changes it.
    let variants = [
        combine_for_test(id, [&z, &b], [b"ct_a", b"ct_b"], [b"ek_a", b"ek_b"]),
        combine_for_test(id, [&a, &z], [b"ct_a", b"ct_b"], [b"ek_a", b"ek_b"]),
        combine_for_test(id, [&a, &b], [b"ct_x", b"ct_b"], [b"ek_a", b"ek_b"]),
        combine_for_test(id, [&a, &b], [b"ct_a", b"ct_x"], [b"ek_a", b"ek_b"]),
        combine_for_test(id, [&a, &b], [b"ct_a", b"ct_b"], [b"ek_x", b"ek_b"]),
        combine_for_test(id, [&a, &b], [b"ct_a", b"ct_b"], [b"ek_a", b"ek_x"]),
        combine_for_test(
            KemId::DualKem1024Hqc256,
            [&a, &b],
            [b"ct_a", b"ct_b"],
            [b"ek_a", b"ek_b"],
        ),
    ];
    for (i, v) in variants.iter().enumerate() {
        assert!(!v.ct_eq(&base), "input {i} did not affect the output");
    }
}

#[test]
fn published_sizes_match_adr_009() {
    assert_eq!(
        (MlKem768::ENCAPSULATION_KEY_LEN, MlKem768::CIPHERTEXT_LEN),
        (1184, 1088)
    );
    assert_eq!(
        (MlKem1024::ENCAPSULATION_KEY_LEN, MlKem1024::CIPHERTEXT_LEN),
        (1568, 1568)
    );
    assert_eq!(
        (Hqc128::ENCAPSULATION_KEY_LEN, Hqc128::CIPHERTEXT_LEN),
        (2241, 4433)
    );
    assert_eq!(
        (Hqc256::ENCAPSULATION_KEY_LEN, Hqc256::CIPHERTEXT_LEN),
        (7237, 14_421)
    );
    assert_eq!(
        (XWing::ENCAPSULATION_KEY_LEN, XWing::CIPHERTEXT_LEN),
        (1216, 1120)
    );
    assert_eq!(DualKem768Hqc128::ENCAPSULATION_KEY_LEN, 1184 + 2241);
    assert_eq!(DualKem768Hqc128::CIPHERTEXT_LEN, 1088 + 4433);
}
