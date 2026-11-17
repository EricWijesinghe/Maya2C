//! Stealth addresses over ML-KEM-768 — ADR-102.
//!
//! Each test fails if the property it names breaks: the round trip, the wrong
//! view key, unlinkability of two receipts, malformed records, tweak binding,
//! and the conformance vectors in `tests/vectors/stealth/derivation-v1.json`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_crypto_pq::stealth::{
    ANNOUNCEMENT_LEN, META_ADDRESS_LEN, ScanSeed, StealthAnnouncement, StealthAuth, StealthError,
    StealthMetaAddress, Tweak, ViewKey, derive_tag_and_tweak, one_time_address,
};
use maya_crypto_pq::suite::{
    MasterSeed, MlDsa65, SignatureSuite, SuiteError, SuiteId, suite_address,
};
use serde_json::Value;

type SpendKey = <MlDsa65 as SignatureSuite>::SigningKey;

/// Stand-in for the chain-tagged signing bytes the transaction layer supplies.
const MSG: &[u8] = b"maya2c test: chain-tagged signing bytes";

fn recipient(n: u8) -> (SpendKey, ViewKey) {
    let sk = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([n; 32]));
    let a = suite_address(SuiteId::MlDsa65, &MlDsa65::public_key(&sk));
    (sk, ViewKey::from_seed(&ScanSeed::from_bytes([n; 64]), a))
}

#[test]
fn a_payment_round_trips_from_meta_address_to_spend() {
    let (sk, view) = recipient(1);
    let meta = StealthMetaAddress::from_bytes(&view.meta_address().to_bytes()).unwrap();
    assert_eq!(meta, view.meta_address());

    let pay = meta.pay();
    let ann = StealthAnnouncement::from_bytes(&pay.announcement.to_bytes()).unwrap();
    assert_eq!(ann, pay.announcement);

    let outputs = [[0xaa; 32], pay.one_time_address, [0xbb; 32]];
    let found = view
        .scan(&ann, &outputs)
        .expect("recipient detects its payment");
    assert_eq!(found.output_index, 1);
    assert_eq!(found.one_time_address, pay.one_time_address);

    let auth = StealthAuth::sign::<MlDsa65>(&sk, &found.tweak, MSG).unwrap();
    assert_eq!(auth.verify(MSG).unwrap(), pay.one_time_address);
}

#[test]
fn the_view_key_restored_from_its_seed_detects_the_same_payment() {
    let (_, view) = recipient(2);
    let pay = view.meta_address().pay();
    let restored = ViewKey::from_seed(
        &ScanSeed::from_bytes([2; 64]),
        *view.meta_address().spend_address(),
    );
    let found = restored
        .scan(&pay.announcement, &[pay.one_time_address])
        .unwrap();
    assert_eq!(found.one_time_address, pay.one_time_address);
}

#[test]
fn a_wrong_view_key_finds_nothing() {
    let (_, alice) = recipient(3);
    let (_, bob) = recipient(4);
    // Bob's scan key paired with Alice's spend address: the case where only `dk` is wrong.
    let bob_dk_alice_a = ViewKey::from_seed(
        &ScanSeed::from_bytes([4; 64]),
        *alice.meta_address().spend_address(),
    );
    let meta = alice.meta_address();
    for _ in 0..256 {
        let pay = meta.pay();
        let outputs = [pay.one_time_address];
        assert!(bob.scan(&pay.announcement, &outputs).is_none());
        assert!(bob_dk_alice_a.scan(&pay.announcement, &outputs).is_none());
        assert!(alice.scan(&pay.announcement, &outputs).is_some());
    }
}

#[test]
fn two_payments_to_one_recipient_are_unlinkable_on_chain() {
    let (_, view) = recipient(5);
    let meta = view.meta_address();
    let a = *meta.spend_address();
    let (p1, p2) = (meta.pay(), meta.pay());

    // Nothing an observer sees is shared between the two receipts, and none of it is `A`.
    assert_ne!(p1.one_time_address, p2.one_time_address);
    assert_ne!(p1.announcement.ciphertext(), p2.announcement.ciphertext());
    for p in [&p1, &p2] {
        assert_ne!(p.one_time_address, a);
        let bytes = p.announcement.to_bytes();
        assert!(
            !bytes.windows(32).any(|w| w == a),
            "A appears in the announcement"
        );
        // An observer who guesses `A` and tries public bytes as the tweak cannot confirm it.
        for chunk in p.announcement.ciphertext().as_chunks::<32>().0 {
            assert_ne!(one_time_address(&a, chunk), p.one_time_address);
        }
    }

    // Yet both belong to the recipient, under different tweaks.
    let t1 = view
        .scan(&p1.announcement, &[p1.one_time_address])
        .unwrap()
        .tweak;
    let t2 = view
        .scan(&p2.announcement, &[p2.one_time_address])
        .unwrap()
        .tweak;
    assert_ne!(t1.as_bytes(), t2.as_bytes());
}

#[test]
fn a_malformed_announcement_is_rejected() {
    for len in [0, 1, ANNOUNCEMENT_LEN - 1, ANNOUNCEMENT_LEN + 1] {
        assert_eq!(
            StealthAnnouncement::from_bytes(&vec![0; len]),
            Err(StealthError::AnnouncementLength(len))
        );
    }

    let (_, view) = recipient(6);
    let pay = view.meta_address().pay();
    let outputs = [pay.one_time_address];

    let mut bytes = pay.announcement.to_bytes();
    bytes[0] ^= 1; // tampered ciphertext: implicit rejection, no detection
    assert!(
        view.scan(&StealthAnnouncement::from_bytes(&bytes).unwrap(), &outputs)
            .is_none()
    );

    let mut bytes = pay.announcement.to_bytes();
    bytes[ANNOUNCEMENT_LEN - 1] ^= 1; // flipped view tag
    assert!(
        view.scan(&StealthAnnouncement::from_bytes(&bytes).unwrap(), &outputs)
            .is_none()
    );
}

#[test]
fn a_malformed_meta_address_is_rejected() {
    for len in [0, 32, META_ADDRESS_LEN - 1, META_ADDRESS_LEN + 1] {
        assert_eq!(
            StealthMetaAddress::from_bytes(&vec![0; len]),
            Err(StealthError::MetaAddressLength(len))
        );
    }
    // Coefficients of 0xfff exceed q = 3329: fails the FIPS 203 modulus check.
    assert_eq!(
        StealthMetaAddress::from_bytes(&[0xff; META_ADDRESS_LEN]),
        Err(StealthError::MalformedEncapsulationKey)
    );
}

#[test]
fn a_swapped_tweak_fails_signature_verification() {
    let (sk, view) = recipient(7);
    let meta = view.meta_address();
    let (p1, p2) = (meta.pay(), meta.pay());
    let t1 = view
        .scan(&p1.announcement, &[p1.one_time_address])
        .unwrap()
        .tweak;
    let t2 = view
        .scan(&p2.announcement, &[p2.one_time_address])
        .unwrap()
        .tweak;

    let mut auth = StealthAuth::sign::<MlDsa65>(&sk, &t1, MSG).unwrap();
    auth.tweak = *t2.as_bytes();
    assert_eq!(
        auth.verify(MSG),
        Err(StealthError::Suite(SuiteError::Verification(
            SuiteId::MlDsa65
        )))
    );
}

#[test]
fn a_sender_who_knows_p_and_the_tweak_cannot_spend_from_it() {
    let (alice_sk, alice) = recipient(8);
    let (mallory_sk, _) = recipient(9);
    let pay = alice.meta_address().pay();
    // Stronger than the real sender: Mallory is handed the tweak too.
    let tweak = alice
        .scan(&pay.announcement, &[pay.one_time_address])
        .unwrap()
        .tweak;

    // Her own key signs fine but authorizes a different address.
    let own = StealthAuth::sign::<MlDsa65>(&mallory_sk, &tweak, MSG).unwrap();
    assert_ne!(own.verify(MSG).unwrap(), pay.one_time_address);

    // Claiming Alice's spend key with her own signature does not verify.
    let mut forged = own;
    forged.spend_pk = MlDsa65::public_key(&alice_sk);
    assert!(forged.verify(MSG).is_err());
}

#[test]
fn a_spend_signature_is_bound_to_the_message() {
    let (sk, view) = recipient(10);
    let pay = view.meta_address().pay();
    let tweak = view
        .scan(&pay.announcement, &[pay.one_time_address])
        .unwrap()
        .tweak;
    let auth = StealthAuth::sign::<MlDsa65>(&sk, &tweak, MSG).unwrap();
    assert!(auth.verify(b"a different transaction").is_err());
    // And a plain suite signature over the same bytes is not a stealth spend.
    let plain = MlDsa65::sign(&sk, MSG).unwrap();
    let replay = StealthAuth {
        signature: plain,
        ..auth
    };
    assert!(replay.verify(MSG).is_err());
}

#[test]
fn secrets_do_not_print_their_material() {
    let (_, view) = recipient(11);
    let pay = view.meta_address().pay();
    let tweak = view
        .scan(&pay.announcement, &[pay.one_time_address])
        .unwrap()
        .tweak;
    for s in [
        format!("{view:?}"),
        format!("{:?}", ScanSeed::from_bytes([11; 64])),
        format!("{tweak:?}"),
    ] {
        assert!(s.contains("redacted"), "{s}");
    }
    static_assertions::assert_not_impl_any!(Tweak: Clone, Copy, core::fmt::Display);
    static_assertions::assert_not_impl_any!(ScanSeed: Clone, Copy, core::fmt::Display);
    static_assertions::assert_not_impl_any!(ViewKey: Clone, Copy, core::fmt::Display);
}

// ------------------------------------------------------- conformance vectors

const VECTORS: &str = "tests/vectors/stealth/derivation-v1.json";
/// ML-KEM-768 keyGen cases whose `d ‖ z` seed the vector scan keys.
const ACVP_TC_IDS: [u64; 3] = [26, 27, 28];

fn path(rel: &str) -> String {
    format!("{}/{rel}", env!("CARGO_MANIFEST_DIR"))
}

fn hexf(v: &Value, field: &str) -> Vec<u8> {
    hex::decode(
        v[field]
            .as_str()
            .unwrap_or_else(|| panic!("missing {field}")),
    )
    .expect("hex")
}

fn acvp_keygen_case(tc_id: u64) -> Value {
    let text =
        std::fs::read_to_string(path("tests/vectors/acvp/ML-KEM-keyGen-FIPS203.json")).unwrap();
    let file: Value = serde_json::from_str(&text).unwrap();
    file["testGroups"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| g["parameterSet"] == "ML-KEM-768")
        .flat_map(|g| g["tests"].as_array().unwrap())
        .find(|t| t["tcId"] == tc_id)
        .unwrap_or_else(|| panic!("ACVP ML-KEM-768 tcId {tc_id} missing"))
        .clone()
}

/// ML-KEM-768 deterministic encapsulation with message `m` (FIPS 203 §6.2
/// internal), so the vector's `ct` and `ss` are reproducible.
fn encapsulate_with(seed: &[u8; 64], m: &[u8; 32]) -> ([u8; 1088], [u8; 32]) {
    let dk = ml_kem::ml_kem_768::DecapsulationKey::from_seed((*seed).into());
    let (ct, ss) = dk
        .encapsulation_key()
        .encapsulate_deterministic(&(*m).into());
    (
        ct.as_slice().try_into().unwrap(),
        ss.as_slice().try_into().unwrap(),
    )
}

fn case_inputs(i: usize) -> ([u8; 64], [u8; 32], [u8; 32]) {
    let t = acvp_keygen_case(ACVP_TC_IDS[i]);
    let mut seed = hexf(&t, "d");
    seed.extend_from_slice(&hexf(&t, "z"));
    let m = [0x40 + u8::try_from(i).unwrap(); 32];
    let a = [0x10 * (u8::try_from(i).unwrap() + 1); 32];
    (seed.try_into().unwrap(), m, a)
}

#[test]
fn derivation_matches_the_conformance_vectors() {
    let text = std::fs::read_to_string(path(VECTORS)).expect("vector file");
    let file: Value = serde_json::from_str(&text).unwrap();
    let cases = file["cases"].as_array().unwrap();
    assert_eq!(cases.len(), ACVP_TC_IDS.len());
    for (i, c) in cases.iter().enumerate() {
        let seed: [u8; 64] = hexf(c, "scan_seed").try_into().unwrap();
        let m: [u8; 32] = hexf(c, "m").try_into().unwrap();
        let a: [u8; 32] = hexf(c, "spend_address").try_into().unwrap();
        assert_eq!((seed, m, a), case_inputs(i), "case {i} inputs drifted");

        // The scan key's ek is NIST's for that seed.
        let view = ViewKey::from_seed(&ScanSeed::from_bytes(seed), a);
        let acvp = acvp_keygen_case(ACVP_TC_IDS[i]);
        assert_eq!(
            view.meta_address().to_bytes()[32..],
            hexf(&acvp, "ek")[..],
            "case {i} ek"
        );

        let (ct, ss) = encapsulate_with(&seed, &m);
        assert_eq!(ct[..], hexf(c, "ct")[..], "case {i} ct");
        assert_eq!(ss[..], hexf(c, "ss")[..], "case {i} ss");
        let (tag, tweak) = derive_tag_and_tweak(&ss, &ct);
        assert_eq!(u64::from(tag), c["tag"].as_u64().unwrap(), "case {i} tag");
        assert_eq!(tweak.as_bytes()[..], hexf(c, "tweak")[..], "case {i} tweak");
        let p = one_time_address(&a, tweak.as_bytes());
        assert_eq!(p[..], hexf(c, "one_time_address")[..], "case {i} P");

        let mut ann = ct.to_vec();
        ann.push(tag);
        assert_eq!(ann, hexf(c, "announcement"), "case {i} announcement bytes");
        let ann = StealthAnnouncement::from_bytes(&ann).unwrap();
        assert_eq!(view.scan(&ann, &[p]).unwrap().one_time_address, p);

        // Negative vector: the same announcement with its tag flipped is not detected.
        let mut bad = ann.to_bytes();
        bad[ANNOUNCEMENT_LEN - 1] ^= 0x01;
        assert!(
            view.scan(&StealthAnnouncement::from_bytes(&bad).unwrap(), &[p])
                .is_none()
        );
    }
}

/// Rewrites the vector file. Run only when the derivation is changed on purpose
/// (which is a new `v2` domain, not an edit of `v1`):
/// `cargo test -p maya-crypto-pq --test stealth_tests -- --ignored regenerate`
#[test]
#[ignore = "writes tests/vectors/stealth/derivation-v1.json"]
fn regenerate_conformance_vectors() {
    let cases: Vec<Value> = (0..ACVP_TC_IDS.len())
        .map(|i| {
            let (seed, m, a) = case_inputs(i);
            let (ct, ss) = encapsulate_with(&seed, &m);
            let (tag, tweak) = derive_tag_and_tweak(&ss, &ct);
            let mut ann = ct.to_vec();
            ann.push(tag);
            serde_json::json!({
                "acvp_keygen_tc_id": ACVP_TC_IDS[i],
                "scan_seed": hex::encode(seed),
                "m": hex::encode(m),
                "spend_address": hex::encode(a),
                "ct": hex::encode(ct),
                "ss": hex::encode(ss),
                "tag": tag,
                "tweak": hex::encode(tweak.as_bytes()),
                "one_time_address": hex::encode(one_time_address(&a, tweak.as_bytes())),
                "announcement": hex::encode(ann),
            })
        })
        .collect();
    let file = serde_json::json!({
        "spec": "ADR-102 stealth derivation v1; docs/stealth-addresses.md",
        "kdf": "k = SHAKE256(\"maya2c/stealth/v1\" || ss || ct)[0..64]; tag = k[0]; tweak = k[32..64]",
        "address": "P = BLAKE3-derive-key(\"maya2c/stealth/addr/v1\", A || tweak)",
        "scan_seed_source": "d || z of ML-KEM-768 keyGen cases in tests/vectors/acvp/ML-KEM-keyGen-FIPS203.json",
        "encapsulation": "FIPS 203 ML-KEM.Encaps_internal(ek, m)",
        "cases": cases,
    });
    let out = path(VECTORS);
    std::fs::create_dir_all(std::path::Path::new(&out).parent().unwrap()).unwrap();
    std::fs::write(&out, serde_json::to_string_pretty(&file).unwrap() + "\n").unwrap();
}
