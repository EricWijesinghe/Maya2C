//! The low-memory ML-DSA-65 against the two things it must equal: NIST's
//! ACVP vectors, and `fips204` byte for byte.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use app_maya2c::lowmem::{self, MuHasher, PUBLIC_KEY_LEN, SECRET_KEY_LEN, SIGNATURE_LEN};
use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer};
use serde_json::Value;

fn vectors(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/crypto-pq/tests/vectors/acvp")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("vendored ACVP file {}: {e}", path.display()));
    serde_json::from_str(&text).expect("json")
}

fn bytes(case: &Value, field: &str) -> Vec<u8> {
    hex::decode(case[field].as_str().expect(field)).expect("hex")
}

/// Every ML-DSA-65 group of `file`, with its cases.
fn ml_dsa_65_groups(file: &Value) -> Vec<&Value> {
    file["testGroups"]
        .as_array()
        .expect("groups")
        .iter()
        .filter(|g| g["parameterSet"] == "ML-DSA-65")
        .collect()
}

#[test]
fn keygen_matches_every_acvp_ml_dsa_65_vector() {
    let file = vectors("ML-DSA-keyGen-FIPS204.json");
    let mut checked = 0;
    for group in ml_dsa_65_groups(&file) {
        for case in group["tests"].as_array().expect("tests") {
            let xi: [u8; 32] = bytes(case, "seed").try_into().expect("32");
            let (mut pk, mut sk) = ([0u8; PUBLIC_KEY_LEN], [0u8; SECRET_KEY_LEN]);
            lowmem::keygen(&xi, &mut pk, &mut sk);
            assert_eq!(pk.to_vec(), bytes(case, "pk"), "tc {}", case["tcId"]);
            assert_eq!(sk.to_vec(), bytes(case, "sk"), "tc {}", case["tcId"]);
            checked += 1;
        }
    }
    assert!(checked > 0, "no ML-DSA-65 keyGen vectors found");
    println!("lowmem keyGen: {checked} ACVP ML-DSA-65 cases");
}

#[test]
fn signing_matches_every_acvp_ml_dsa_65_sig_gen_vector() {
    let file = vectors("ML-DSA-sigGen-FIPS204.json");
    let mut checked = 0;
    for group in ml_dsa_65_groups(&file) {
        let external = group["signatureInterface"] == "external";
        let deterministic = group["deterministic"] == true;
        if group["preHash"] != "pure" && group["preHash"] != "none" {
            continue; // HashML-DSA is not what a transaction uses
        }
        for case in group["tests"].as_array().expect("tests") {
            let sk: [u8; SECRET_KEY_LEN] = bytes(case, "sk").try_into().expect("sk");
            let message = bytes(case, "message");
            let rnd: [u8; 32] = if deterministic {
                [0; 32]
            } else {
                bytes(case, "rnd").try_into().expect("rnd")
            };
            let mut mu = if external {
                MuHasher::external(&sk, &bytes(case, "context"))
            } else {
                MuHasher::internal(&sk)
            };
            mu.update(&message);
            let mut sig = [0u8; SIGNATURE_LEN];
            lowmem::sign_mu(&sk, &mu.finish(), &rnd, &mut sig).expect("sign");
            assert_eq!(
                sig.to_vec(),
                bytes(case, "signature"),
                "tc {}",
                case["tcId"]
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no ML-DSA-65 sigGen vectors found");
    println!("lowmem sigGen: {checked} ACVP ML-DSA-65 cases");
}

/// A deterministic stream of test inputs, from BLAKE3 in XOF mode.
fn stream(label: &str, index: u32, out: &mut [u8]) {
    let mut hasher = blake3::Hasher::new_derive_key("app-maya2c lowmem test inputs");
    hasher.update(label.as_bytes());
    hasher.update(&index.to_le_bytes());
    hasher.finalize_xof().fill(out);
}

#[test]
fn keys_and_signatures_equal_fips204_byte_for_byte() {
    const CASES: u32 = 64;
    for index in 0..CASES {
        let mut xi = [0u8; 32];
        stream("xi", index, &mut xi);
        let mut message = vec![0u8; (index as usize * 37) % 3000];
        stream("message", index, &mut message);

        let (expected_pk, expected_sk) = ml_dsa_65::KG::keygen_from_seed(&xi);
        let (mut pk, mut sk) = ([0u8; PUBLIC_KEY_LEN], [0u8; SECRET_KEY_LEN]);
        lowmem::keygen(&xi, &mut pk, &mut sk);
        assert_eq!(pk, expected_pk.clone().into_bytes(), "pk {index}");
        assert_eq!(sk, expected_sk.clone().into_bytes(), "sk {index}");

        // Deterministic, empty context: the wallet's signing rule.
        let expected = expected_sk
            .try_sign_with_seed(&[0; 32], &message, b"")
            .expect("fips204 sign");
        let mut sig = [0u8; SIGNATURE_LEN];
        lowmem::sign(&sk, &message, b"", &[0; 32], &mut sig).expect("sign");
        assert_eq!(sig, expected, "signature {index}");
    }
}

#[test]
fn a_message_streamed_in_pieces_signs_the_same_as_whole() {
    let mut xi = [0u8; 32];
    stream("xi", 1_000, &mut xi);
    let (mut pk, mut sk) = ([0u8; PUBLIC_KEY_LEN], [0u8; SECRET_KEY_LEN]);
    lowmem::keygen(&xi, &mut pk, &mut sk);
    let mut message = vec![0u8; 2_100];
    stream("message", 1_000, &mut message);

    let mut whole = [0u8; SIGNATURE_LEN];
    lowmem::sign(&sk, &message, b"", &[0; 32], &mut whole).expect("whole");

    // As APDUs deliver it: 255-byte chunks.
    let mut mu = MuHasher::external(&sk, b"");
    for chunk in message.chunks(255) {
        mu.update(chunk);
    }
    let mut streamed = [0u8; SIGNATURE_LEN];
    lowmem::sign_mu(&sk, &mu.finish(), &[0; 32], &mut streamed).expect("streamed");
    assert_eq!(whole, streamed);
}
