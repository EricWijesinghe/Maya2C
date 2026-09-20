//! Real proofs: an ONNX file in, a verifying proof out, and every way the
//! verifier must say no.
//!
//! `circuit_tests.rs` checks the constraints with `MockProver`. This file runs
//! the actual KZG prover and verifier, which is where encoding, transcript and
//! parsing bugs live — none of which a mock can see.

use std::path::PathBuf;

use maya_zkml::circuit::{Witness, public_inputs};
use maya_zkml::error::ZkmlError;
use maya_zkml::model::QuantizedMlp;
use maya_zkml::srs::{self, K};
use maya_zkml::verify::{MAX_PROOF_BYTES, MAX_PUBLIC_INPUTS, MAX_VK_BYTES, model_id, verify};
use maya_zkml_prover::onnx;
use maya_zkml_prover::prove::{ProvingSetup, keygen, prove, prove_witness};
use sha2::{Digest, Sha256};

/// SHA-256 of the committed fixture. If `make_zkml_fixture.py` changes the
/// model, this changes, and the expected-output file must be regenerated with
/// it — the two are only meaningful as a pair.
const CLASSIFIER_SHA256: &str = "3eadf1eb248ec0b33dfbf96f22ef8ef178b8e9ba4da0bf1082bde3bfccc7b24f";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../node/tests/fixtures/zkml")
        .join(name)
}

struct Case {
    x: Vec<i8>,
    logits: Vec<i64>,
    class: usize,
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(fixture("classifier.expected.json")).expect("json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("parse");
    let ints = |v: &serde_json::Value| -> Vec<i64> {
        v.as_array()
            .expect("array")
            .iter()
            .map(|n| n.as_i64().expect("int"))
            .collect()
    };
    json["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|c| Case {
            x: ints(&c["x"]).into_iter().map(|v| v as i8).collect(),
            logits: ints(&c["logits"]),
            class: c["class"].as_u64().expect("class") as usize,
        })
        .collect()
}

fn model() -> QuantizedMlp {
    onnx::load(fixture("classifier.onnx")).expect("fixture loads")
}

fn public(x: &[i8], class: usize) -> Vec<i64> {
    x.iter()
        .map(|&v| i64::from(v))
        .chain([class as i64])
        .collect()
}

/// One setup and one proof, shared by the tests that only need *a* valid proof.
fn proved() -> (ProvingSetup, Vec<u8>, Vec<i64>) {
    let setup = keygen(&model()).expect("keygen");
    let case = &cases()[0];
    let (proof, class) = prove(&setup, &case.x).expect("prove");
    (setup, proof, public(&case.x, class))
}

#[test]
fn the_fixture_is_the_one_the_generator_wrote() {
    let bytes = std::fs::read(fixture("classifier.onnx")).expect("read");
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(digest, CLASSIFIER_SHA256);
}

#[test]
fn numpy_tract_and_the_model_agree_on_every_case() {
    // Three evaluators that share no code: numpy wrote the JSON, tract reads the
    // ONNX graph, and `QuantizedMlp::evaluate` is what the circuit constrains.
    let model = model();
    for case in cases() {
        let (tract_class, tract_logits) =
            onnx::evaluate_with_tract(fixture("classifier.onnx"), &case.x).expect("tract runs");
        let ours = model.evaluate(&case.x).expect("evaluate");

        assert_eq!(
            tract_logits, case.logits,
            "tract vs numpy logits for {:?}",
            case.x
        );
        assert_eq!(
            ours.logits, case.logits,
            "model vs numpy logits for {:?}",
            case.x
        );
        assert_eq!(
            tract_class, case.class,
            "tract vs numpy class for {:?}",
            case.x
        );
        assert_eq!(
            ours.class, case.class,
            "model vs numpy class for {:?}",
            case.x
        );
    }
}

#[test]
fn every_case_proves_and_verifies() {
    let setup = keygen(&model()).expect("keygen");
    for case in cases() {
        let (proof, class) = prove(&setup, &case.x).expect("prove");
        assert_eq!(class, case.class);
        assert_eq!(
            verify(setup.verifying_key(), &public(&case.x, class), &proof),
            Ok(true),
            "honest proof refused for {:?}",
            case.x
        );
    }
}

#[test]
fn a_proof_does_not_verify_for_another_class_or_input() {
    let (setup, proof, honest) = proved();
    let inputs = honest.len() - 1;

    let mut wrong_class = honest.clone();
    wrong_class[inputs] = (wrong_class[inputs] + 1) % 3;
    assert_eq!(
        verify(setup.verifying_key(), &wrong_class, &proof),
        Ok(false)
    );

    let mut wrong_input = honest.clone();
    wrong_input[0] = if wrong_input[0] == 0 { 1 } else { 0 };
    assert_eq!(
        verify(setup.verifying_key(), &wrong_input, &proof),
        Ok(false)
    );

    // Dropping the class entirely leaves that instance row zero — which is a
    // different claim, not an absent one.
    assert_eq!(
        verify(setup.verifying_key(), &honest[..inputs], &proof),
        Ok(false)
    );
}

#[test]
fn a_proof_does_not_verify_under_another_models_key() {
    let (setup, proof, public) = proved();

    // One bias moved by one. A different model, and so a different key.
    let original = model();
    let b2: Vec<i32> = (0..original.classes())
        .map(|k| original.b2(k) + i32::from(k == 0))
        .collect();
    let other = QuantizedMlp::new(
        original.inputs(),
        original.hidden(),
        original.classes(),
        (0..original.inputs())
            .flat_map(|i| (0..original.hidden()).map(move |j| (i, j)))
            .map(|(i, j)| original.w1(i, j))
            .collect(),
        (0..original.hidden()).map(|j| original.b1(j)).collect(),
        (0..original.hidden())
            .flat_map(|j| (0..original.classes()).map(move |k| (j, k)))
            .map(|(j, k)| original.w2(j, k))
            .collect(),
        b2,
        original.shift(),
    )
    .expect("model");
    let other_setup = keygen(&other).expect("keygen");

    assert_ne!(other_setup.model_id(), setup.model_id());
    assert_eq!(
        verify(other_setup.verifying_key(), &public, &proof),
        Ok(false)
    );
}

#[test]
fn key_generation_is_deterministic() {
    // What lets anyone holding the ONNX file recompute a contract's model id.
    let first = keygen(&model()).expect("keygen");
    let second = keygen(&model()).expect("keygen");
    assert_eq!(first.verifying_key(), second.verifying_key());
    assert_eq!(first.model_id(), model_id(second.verifying_key()));
}

#[test]
fn a_corrupted_proof_is_refused_not_an_error() {
    // A bad proof is an answer. The VM turns `Ok(false)` into 0 and lets the
    // contract decide; an error would trap and fail the whole block.
    let (setup, proof, public) = proved();
    for position in [0, 1, proof.len() / 2, proof.len() - 1] {
        let mut flipped = proof.clone();
        flipped[position] ^= 0x01;
        assert_eq!(
            verify(setup.verifying_key(), &public, &flipped),
            Ok(false),
            "flipped byte {position} accepted"
        );
    }
    assert_eq!(
        verify(setup.verifying_key(), &public, &proof[..proof.len() - 1]),
        Ok(false)
    );
    assert_eq!(verify(setup.verifying_key(), &public, &[]), Ok(false));
}

#[test]
fn a_valid_proof_with_trailing_bytes_is_refused() {
    // It would verify exactly as the proof alone does. Refused because a
    // transaction id covers its bytes, so tolerated padding mints distinct
    // transactions carrying one proof.
    let (setup, mut proof, public) = proved();
    proof.push(0);
    assert_eq!(verify(setup.verifying_key(), &public, &proof), Ok(false));
}

#[test]
fn oversized_buffers_are_errors_before_anything_is_parsed() {
    let (setup, proof, public) = proved();
    let big = vec![0u8; MAX_VK_BYTES + 1];
    assert!(matches!(
        verify(&big, &public, &proof),
        Err(ZkmlError::Oversized {
            what: "verifying key",
            ..
        })
    ));
    let big = vec![0u8; MAX_PROOF_BYTES + 1];
    assert!(matches!(
        verify(setup.verifying_key(), &public, &big),
        Err(ZkmlError::Oversized { what: "proof", .. })
    ));
    let many = vec![0i64; MAX_PUBLIC_INPUTS + 1];
    assert!(matches!(
        verify(setup.verifying_key(), &many, &proof),
        Err(ZkmlError::Oversized {
            what: "public inputs",
            ..
        })
    ));
}

#[test]
fn a_key_for_another_circuit_size_is_refused_before_halo2_sees_it() {
    // `VerifyingKey::read` would build a 2^k evaluation domain from this field.
    // k = 30 must be refused by the header check, not attempted.
    let (setup, proof, public) = proved();
    for k in [K - 1, K + 1, 30, u32::MAX] {
        let mut key = setup.verifying_key().to_vec();
        key[1..5].copy_from_slice(&k.to_le_bytes());
        assert_eq!(
            verify(&key, &public, &proof),
            Err(ZkmlError::MalformedKey(
                "circuit size is not the one the SRS supports"
            )),
            "k = {k}"
        );
    }
}

#[test]
fn a_key_with_trailing_bytes_is_refused() {
    // Otherwise one model would have two ids.
    let (setup, proof, public) = proved();
    let mut key = setup.verifying_key().to_vec();
    key.push(0);
    assert_eq!(
        verify(&key, &public, &proof),
        Err(ZkmlError::MalformedKey("trailing bytes"))
    );
}

#[test]
fn damaged_keys_never_verify_and_never_panic() {
    // Keys come from transactions. Every byte position of the real key is
    // flipped in turn, and every truncation tried; each must be an error or a
    // refusal. `verify` catches a halo2 panic and reports it as an error, so a
    // panic here would mean the guard itself failed.
    let (setup, proof, public) = proved();
    let key = setup.verifying_key();
    for position in (0..key.len()).step_by(7) {
        let mut damaged = key.to_vec();
        damaged[position] ^= 0x5a;
        assert_ne!(
            verify(&damaged, &public, &proof),
            Ok(true),
            "byte {position}"
        );
    }
    for length in (0..key.len()).step_by(13) {
        assert_ne!(
            verify(&key[..length], &public, &proof),
            Ok(true),
            "length {length}"
        );
    }
}

#[test]
fn a_lying_witness_never_produces_a_verifying_proof() {
    // Past MockProver: the real prover, handed a witness claiming the wrong
    // class. halo2 may refuse to prove it, or produce a proof the verifier
    // rejects. Either is fine. `Ok(true)` is not.
    let setup = keygen(&model()).expect("keygen");
    let case = &cases()[0];
    let x: Vec<i64> = case.x.iter().map(|&v| i64::from(v)).collect();
    let mut lie = Witness::honest(setup.model(), &x);
    let wrong = (case.class + 1) % 3;
    lie.class = wrong as i64;
    lie.one_hot = (0..3).map(|k| i64::from(k == wrong)).collect();

    if let Ok(proof) = prove_witness(&setup, lie) {
        assert_eq!(
            verify(setup.verifying_key(), &public(&case.x, wrong), &proof),
            Ok(false)
        );
    }
    // And the public-input helper agrees with the instance layout the circuit
    // binds: inputs, then class.
    assert_eq!(public_inputs(&x, wrong as i64).len(), x.len() + 1);
}

#[test]
fn the_importer_refuses_near_misses() {
    for name in ["unsupported-relu.onnx", "unsupported-zero-point.onnx"] {
        assert!(
            matches!(
                onnx::load(fixture(name)),
                Err(ZkmlError::UnsupportedOnnx(_))
            ),
            "{name} imported"
        );
    }
    let truncated = std::env::temp_dir().join("maya-zkml-truncated.onnx");
    let bytes = std::fs::read(fixture("classifier.onnx")).expect("read");
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).expect("write");
    assert!(onnx::load(&truncated).is_err());
    let _ = std::fs::remove_file(truncated);
}

#[test]
fn the_setup_refuses_a_value_bearing_chain() {
    // Checked at compile time: flipping it is a decision somebody writes down,
    // and it should break the build of this test until they also delete this.
    const { assert!(!srs::SRS_IS_TRUSTED) };
    for chain in ["mainnet", "maya-mainnet"] {
        assert_eq!(srs::check_chain(chain), Err(ZkmlError::UntrustedSetup));
    }
    assert_eq!(srs::check_chain("maya-testnet"), Ok(()));
}

#[test]
fn proof_and_key_sizes_are_within_their_bounds_with_room() {
    // The bounds are compiled into the VM's memory accounting. If a halo2
    // upgrade grew either past its bound, every proof would start failing as
    // oversized; this says so first.
    let (setup, proof, _) = proved();
    eprintln!(
        "verifying key {} bytes, proof {} bytes",
        setup.verifying_key().len(),
        proof.len()
    );
    assert!(setup.verifying_key().len() * 2 < MAX_VK_BYTES);
    assert!(proof.len() * 2 < MAX_PROOF_BYTES);
}

#[test]
fn a_second_class_appended_after_the_real_one_is_refused() {
    // The circuit constrains instance rows 0..=n and nothing after. If a
    // verifier ignored the rows past n, a proof of `x -> c` would also verify
    // `[x.., c, junk]`, and a contract reading the class as the *last* public
    // input would take the junk. This verifier does not ignore them:
    // `VerifierSHPLONK` has `QUERY_INSTANCE = false`, so halo2 absorbs every
    // public value into the transcript (`plonk/verifier.rs:84`), and a different
    // vector is a different transcript. These two tests pin that, because a
    // switch to a commitment-querying verifier would quietly lose it.
    let (setup, proof, honest) = proved();
    let fake = (honest[honest.len() - 1] + 1) % 3;
    let mut padded = honest.clone();
    padded.push(fake);
    assert_eq!(verify(setup.verifying_key(), &padded, &proof), Ok(false));
}

#[test]
fn a_zero_appended_after_a_nonzero_class_is_refused() {
    // The narrower case. A trailing zero leaves the instance *polynomial* exactly
    // as it was, so a verifier that bound instances only through a commitment to
    // that polynomial could not tell -- and a contract reading the class as the
    // last public input would read 0. Refused here for the reason above: the
    // values themselves, trailing zero included, go into the transcript.
    let setup = keygen(&model()).expect("keygen");
    let case = cases()
        .into_iter()
        .find(|c| c.class != 0)
        .expect("a case whose class is not 0");
    let (proof, class) = prove(&setup, &case.x).expect("prove");
    let mut padded = public(&case.x, class);
    padded.push(0);
    assert_eq!(verify(setup.verifying_key(), &padded, &proof), Ok(false));
}
