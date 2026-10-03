//! The production node against `spec/tests/consensus.json`: retargeting
//! (CON-5), the work check (CON-6), fork choice by cumulative work (CON-7),
//! the prune horizon (CON-8) and the verification path for v7/v8 frames
//! (TX-4). The vectors come from `crates/spec-ref`, which shares no code with
//! the node; any disagreement fails with the case id.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use custom_l1_node::consensus::chain::below_prune_horizon;
use custom_l1_node::consensus::{cumulative_work, dag_bft_target, retarget, work_from_target};
use custom_l1_node::core::multisig_tx::MultisigAuth;
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::pow::meets_target;
use maya_crypto_pq::agility::{Network, SuitePolicy};
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite, SuiteId};
use serde_json::Value;

mod common;

fn load() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests/consensus.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn bytes32(v: &Value) -> [u8; 32] {
    hex::decode(v.as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap()
}

fn num(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}

fn pay() -> Transaction {
    Transaction::new(
        Vec::new(),
        vec![TxOutput {
            amount: 1,
            recipient: [0x5a; 32],
        }],
        0,
    )
}

fn key(i: u8) -> <MlDsa87 as SignatureSuite>::SigningKey {
    MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x40 + i; 32]))
}

/// A transaction of `version` whose signature does (or does not) verify.
fn transaction(version: u64, valid: bool) -> Transaction {
    let mut tx = pay();
    match version {
        7 => tx
            .sign_with_suite::<MlDsa87>(&key(0), &common::test_chain())
            .unwrap(),
        8 => {
            let keys: Vec<_> = (1..=3).map(key).collect();
            let listed = keys
                .iter()
                .map(|k| PolicyKey {
                    suite: SuiteId::MlDsa87,
                    public_key: MlDsa87::public_key(k),
                })
                .collect();
            let policy = MultisigPolicy::new(2, listed).unwrap();
            tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
            let message = tx.signing_bytes(&common::test_chain());
            let approvals = (0..2u8)
                .map(|i| Approval {
                    index: i,
                    signature: MlDsa87::sign(&keys[usize::from(i)], &message).unwrap(),
                })
                .collect();
            tx.multisig = Some(Box::new(MultisigAuth::new(policy, approvals).unwrap()));
        }
        _ => {
            let signer = custom_l1_node::crypto::hybrid::signing_key_from_seed(&[7; 32]).unwrap();
            tx.sign(&signer, &common::test_chain()).unwrap();
        }
    }
    if !valid {
        // Change what was signed: the signature no longer covers the frame.
        tx.outputs[0].amount += 1;
    }
    tx
}

fn outcome(ok: bool) -> &'static str {
    if ok { "ok" } else { "error" }
}

fn check(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let expect = &case["expect"];
    let want = expect["result"].as_str().unwrap();
    match case["fn"].as_str().unwrap() {
        "retarget" => {
            let next = retarget(
                &bytes32(&case["previous"]),
                num(&case["timespan"]),
                &bytes32(&case["pow_limit"]),
            );
            if let Some(declared) = case.get("declared") {
                assert_eq!(outcome(next == bytes32(declared)), want, "{id}");
            } else {
                assert_eq!(
                    hex::encode(next),
                    expect["target"].as_str().unwrap(),
                    "{id}"
                );
            }
        }
        "dag_bft_target" => {
            let next = dag_bft_target(&bytes32(&case["parent"]));
            if let Some(declared) = case.get("declared") {
                assert_eq!(outcome(next == bytes32(declared)), want, "{id}");
            } else {
                assert_eq!(
                    hex::encode(next),
                    expect["target"].as_str().unwrap(),
                    "{id}"
                );
            }
        }
        "meets_target" => {
            let ok = meets_target(&bytes32(&case["hash"]), &bytes32(&case["target"]));
            assert_eq!(outcome(ok), want, "{id}");
        }
        "work" => {
            let work = work_from_target(&bytes32(&case["target"])).to_be_bytes();
            assert_eq!(hex::encode(work), expect["work"].as_str().unwrap(), "{id}");
        }
        "fork_choice" => {
            let branch = |k: &str| -> Vec<[u8; 32]> {
                case[k].as_array().unwrap().iter().map(bytes32).collect()
            };
            let (a, b) = (cumulative_work(&branch("a")), cumulative_work(&branch("b")));
            assert_eq!(
                hex::encode(a.to_be_bytes()),
                expect["work_a"].as_str().unwrap(),
                "{id}"
            );
            assert_eq!(
                hex::encode(b.to_be_bytes()),
                expect["work_b"].as_str().unwrap(),
                "{id}"
            );
            assert_eq!(
                if a > b { "a" } else { "b" },
                expect["winner"].as_str().unwrap(),
                "{id}"
            );
        }
        "prune_horizon" => {
            let refused = below_prune_horizon(num(&case["height"]), num(&case["horizon"]));
            assert_eq!(outcome(!refused), want, "{id}");
        }
        "verification" => {
            let tx = transaction(
                case["version"].as_u64().unwrap(),
                case["signature"] == "valid",
            );
            let result = match case["call"].as_str().unwrap() {
                "verify" => tx.verify(&common::test_chain()),
                _ => tx.verify_at(
                    num(&case["height"]),
                    &SuitePolicy::genesis(Network::Mainnet),
                    &common::test_chain(),
                ),
            };
            assert_eq!(outcome(result.is_ok()), want, "{id}: {result:?}");
        }
        other => panic!("{id}: unknown fn {other}"),
    }
}

#[test]
fn consensus_vectors() {
    let doc = load();
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 20, "the consensus vectors are all here");
    cases.iter().for_each(check);
}
