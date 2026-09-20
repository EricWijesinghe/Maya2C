//! RFC 9381 Appendix B.3 test vectors for `ECVRF-EDWARDS25519-SHA512-TAI`.
//!
//! # Why this file is the important one
//!
//! Everything else in this crate is checked against itself. A round-trip test
//! proves that `verify` agrees with `prove`, which is exactly what a subtly
//! wrong implementation of both would also do.
//!
//! This is not hypothetical. The first version of this implementation used
//! suite octet `0x04` instead of `0x03` — the octet belonging to
//! `ECVRF-EDWARDS25519-SHA512-ELL2`, the same curve and hash under a different
//! hash-to-curve map. Every round trip passed. Every proof it produced was
//! self-consistent and would have been rejected by every other implementation
//! of the suite it claimed to be. These vectors are what found it.
//!
//! Consensus code with no second implementation on this chain to disagree with
//! it has to be checked from outside the repository. This is the outside.
//!
//! The three secret keys are RFC 8032's ed25519 TEST 1, 2, and 3, reused by
//! RFC 9381 so the two documents' vectors can be read against each other.
//!
//! Source: <https://www.rfc-editor.org/rfc/rfc9381.txt>, Appendix B.3,
//! Examples 16 through 18.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vrf::ecvrf::{PROOF_LEN, VrfProof, proof_to_hash, prove, verify};
use maya_vrf::keys::VrfSecretKey;

/// One RFC 9381 Appendix B.3 vector.
struct Vector {
    name: &'static str,
    secret: &'static str,
    public: &'static str,
    alpha: &'static [u8],
    proof: &'static str,
    beta: &'static str,
}

const VECTORS: &[Vector] = &[
    Vector {
        name: "Example 16",
        secret: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        public: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        alpha: b"",
        proof: "8657106690b5526245a92b003bb079ccd1a92130477671f6fc01ad16f26f7\
                23f26f8a57ccaed74ee1b190bed1f479d9727d2d0f9b005a6e456a35d4fb0daab1\
                268a1b0db10836d9826a528ca76567805",
        beta: "90cf1df3b703cce59e2a35b925d411164068269d7b2d29f3301c03dd757\
               876ff66b71dda49d2de59d03450451af026798e8f81cd2e333de5cdf4f3e140fdd\
               8ae",
    },
    Vector {
        name: "Example 17",
        secret: "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        public: "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        alpha: b"\x72",
        proof: "f3141cd382dc42909d19ec5110469e4feae18300e94f304590abdced48aed\
                5933bf0864a62558b3ed7f2fea45c92a465301b3bbf5e3e54ddf2d935be3b67926\
                da3ef39226bbc355bdc9850112c8f4b02",
        beta: "eb4440665d3891d668e7e0fcaf587f1b4bd7fbfe99d0eb2211ccec90496\
               310eb5e33821bc613efb94db5e5b54c70a848a0bef4553a41befc57663b56373a5\
               031",
    },
    Vector {
        name: "Example 18",
        secret: "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
        public: "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        alpha: b"\xaf\x82",
        proof: "9bc0f79119cc5604bf02d23b4caede71393cedfbb191434dd016d30177ccb\
                f8096bb474e53895c362d8628ee9f9ea3c0e52c7a5c691b6c18c9979866568add7\
                a2d41b00b05081ed0f58ee5e31b3a970e",
        beta: "645427e5d00c62a23fb703732fa5d892940935942101e456ecca7bb217c\
               61c452118fec1219202a0edcf038bb6373241578be7217ba85a2687f7a0310b2df\
               19f",
    },
];

/// Strips the line-continuation indentation out of the hex literals above.
fn decode(text: &str) -> Vec<u8> {
    let stripped: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    hex::decode(stripped).expect("hex literal")
}

#[test]
fn the_public_key_derivation_matches_rfc_8032() {
    // Shared with RFC 8032, so a failure here is a broken key expansion rather
    // than a broken VRF. Worth separating: from further down, the two look the
    // same.
    for vector in VECTORS {
        let secret = VrfSecretKey::from_bytes(&decode(vector.secret)).expect("secret key");
        assert_eq!(
            secret.public_key().compressed.to_vec(),
            decode(vector.public),
            "{}: derived public key does not match",
            vector.name
        );
    }
}

#[test]
fn proving_reproduces_the_rfc_proof_byte_for_byte() {
    // The strongest statement available: not "a proof that verifies" but *the*
    // proof, which pins the nonce derivation, both domain separators, the
    // challenge truncation, and the endianness of every integer.
    for vector in VECTORS {
        let secret = VrfSecretKey::from_bytes(&decode(vector.secret)).expect("secret key");
        let proof = prove(&secret, vector.alpha).expect("prove");
        assert_eq!(
            hex::encode(proof.as_bytes()),
            hex::encode(decode(vector.proof)),
            "{}: proof does not match the RFC",
            vector.name
        );
    }
}

#[test]
fn the_output_matches_the_rfc_beta() {
    for vector in VECTORS {
        let proof = VrfProof::from_slice(&decode(vector.proof)).expect("proof");
        assert_eq!(
            hex::encode(proof_to_hash(&proof).expect("output")),
            hex::encode(decode(vector.beta)),
            "{}: output does not match the RFC",
            vector.name
        );
    }
}

#[test]
fn verifying_the_rfc_proofs_returns_the_rfc_output() {
    for vector in VECTORS {
        let secret = VrfSecretKey::from_bytes(&decode(vector.secret)).expect("secret key");
        let public = secret.public_key();
        let proof = VrfProof::from_slice(&decode(vector.proof)).expect("proof");

        let output = verify(&public, vector.alpha, &proof).expect("verify");
        assert_eq!(
            hex::encode(output),
            hex::encode(decode(vector.beta)),
            "{}: verified output does not match the RFC",
            vector.name
        );
    }
}

#[test]
fn a_proof_is_exactly_eighty_bytes() {
    // Pinned as a literal rather than computed, so a change to the challenge
    // width fails here instead of silently changing every stored proof.
    assert_eq!(PROOF_LEN, 80);
    for vector in VECTORS {
        assert_eq!(decode(vector.proof).len(), PROOF_LEN, "{}", vector.name);
    }
}

#[test]
fn a_proof_does_not_verify_under_the_wrong_input() {
    // The vectors prove the happy path agrees with the RFC. This is the other
    // half: the same proof under a different alpha must fail, or "verified"
    // would mean nothing.
    for vector in VECTORS {
        let secret = VrfSecretKey::from_bytes(&decode(vector.secret)).expect("secret key");
        let public = secret.public_key();
        let proof = VrfProof::from_slice(&decode(vector.proof)).expect("proof");

        let mut wrong = vector.alpha.to_vec();
        wrong.push(0x00);
        assert!(
            verify(&public, &wrong, &proof).is_err(),
            "{}: a proof verified under an input it was not made for",
            vector.name
        );
    }
}

#[test]
fn a_proof_does_not_verify_under_another_key() {
    for (index, vector) in VECTORS.iter().enumerate() {
        let other = &VECTORS[(index + 1) % VECTORS.len()];
        let public = VrfSecretKey::from_bytes(&decode(other.secret))
            .expect("secret key")
            .public_key();
        let proof = VrfProof::from_slice(&decode(vector.proof)).expect("proof");

        assert!(
            verify(&public, vector.alpha, &proof).is_err(),
            "{}: a proof verified under somebody else's key",
            vector.name
        );
    }
}
