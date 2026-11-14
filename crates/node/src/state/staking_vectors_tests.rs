//! `spec/tests/staking.json` (STK-1, STK-2, STK-8): the byte layout of every
//! staking record kind, and the staking layer's root over a small fixed state
//! that holds one of each.
//!
//! Every other staking test round-trips through the node's own encoder, so an
//! accidental change to a layout would pass them all while two versions of
//! the node computed different roots. These are fixed bytes: a layout change
//! fails here, and must be made in the spec and the vectors on purpose.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use maya_staking::{Amount, Params, Status, ValidatorRecord};
use serde_json::Value;
use tempfile::TempDir;

use crate::state::db::{Overlay, StateDB};
use crate::state::proof::StateLayer;
use crate::state::staking::{Meta, STATE_KEY, put_amount};
use crate::state::staking_records::{
    PayoutJob, StoredDelegation, StoredValidator, decode_unbonding, delegation_key, payout_key,
    release_key, unbonding_key, validator_key,
};

const VECTORS: &str = include_str!("../../../../spec/tests/staking.json");

fn field<'a>(case: &'a Value, name: &str) -> &'a str {
    case["fields"][name]
        .as_str()
        .unwrap_or_else(|| panic!("{}: field {name}", case["id"]))
}

fn num(case: &Value, name: &str) -> u64 {
    field(case, name).parse().unwrap()
}

fn amt(case: &Value, name: &str) -> Amount {
    Amount::from_u64(num(case, name))
}

fn id(case: &Value, name: &str) -> [u8; 32] {
    hex::decode(field(case, name)).unwrap().try_into().unwrap()
}

fn status(case: &Value) -> Status {
    match field(case, "status") {
        "bonded" => Status::Bonded,
        "tombstoned" => Status::Tombstoned,
        jailed => Status::Jailed {
            until: jailed.strip_prefix("jailed:").unwrap().parse().unwrap(),
        },
    }
}

fn counts(case: &Value, name: &str) -> BTreeMap<[u8; 32], u64> {
    case["fields"][name]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                hex::decode(k).unwrap().try_into().unwrap(),
                v.as_str().unwrap().parse().unwrap(),
            )
        })
        .collect()
}

/// `(key, value)` the case's fields encode to, checking the value decodes
/// back to the same fields.
fn encoded(case: &Value) -> (Vec<u8>, Vec<u8>) {
    match case["kind"].as_str().unwrap() {
        "meta" => {
            let meta = Meta {
                params: Params::DEVNET,
                epoch: num(case, "epoch"),
                active: case["fields"]["active"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| {
                        hex::decode(v.as_str().unwrap())
                            .unwrap()
                            .try_into()
                            .unwrap()
                    })
                    .collect(),
                epoch_blocks: num(case, "epoch_blocks"),
                last_round: num(case, "last_round"),
                expected: counts(case, "expected"),
                authored: counts(case, "authored"),
                delegation_records: num(case, "delegation_records"),
            };
            let value = meta.encode();
            assert_eq!(Meta::decode(&value).unwrap(), meta);
            (STATE_KEY.to_vec(), value)
        }
        "validator" => {
            let v = StoredValidator {
                record: ValidatorRecord {
                    operator: id(case, "operator"),
                    self_bond: amt(case, "self_bond"),
                    delegated: amt(case, "delegated"),
                    commission_bps: u16::try_from(num(case, "commission_bps")).unwrap(),
                    status: status(case),
                },
                pending_epoch: num(case, "pending_epoch"),
                pending_self: amt(case, "pending_self"),
                pending_delegated: amt(case, "pending_delegated"),
                delegations: num(case, "delegations"),
                unbonding: amt(case, "unbonding"),
            };
            let value = v.encode();
            assert_eq!(StoredValidator::decode(&value).unwrap(), v);
            (validator_key(&id(case, "validator")), value)
        }
        "delegation" => {
            let d = StoredDelegation {
                amount: amt(case, "amount"),
                pending: amt(case, "pending"),
                pending_epoch: num(case, "pending_epoch"),
                next_payout_epoch: num(case, "next_payout_epoch"),
            };
            let value = d.encode();
            assert_eq!(StoredDelegation::decode(&value).unwrap(), d);
            (
                delegation_key(&id(case, "delegator"), &id(case, "validator")),
                value,
            )
        }
        "unbonding" => {
            let mut value = Vec::new();
            put_amount(&mut value, amt(case, "amount"));
            assert_eq!(decode_unbonding(&value).unwrap(), amt(case, "amount"));
            let key = (
                id(case, "validator"),
                num(case, "release_epoch"),
                id(case, "owner"),
            );
            (unbonding_key(&key), value)
        }
        "release" => {
            let key = (
                id(case, "validator"),
                num(case, "release_epoch"),
                id(case, "owner"),
            );
            (release_key(&key), Vec::new())
        }
        "payout" => {
            let cursor = match field(case, "cursor") {
                "none" => None,
                hex => Some(hex::decode(hex).unwrap().try_into().unwrap()),
            };
            let job = PayoutJob {
                rest: amt(case, "rest"),
                total: amt(case, "total"),
                owed: amt(case, "owed"),
                remaining: num(case, "remaining"),
                cursor,
            };
            let value = job.encode();
            assert_eq!(PayoutJob::decode(&value).unwrap(), job);
            (
                payout_key(&id(case, "validator"), num(case, "epoch")),
                value,
            )
        }
        other => panic!("unknown record kind {other}"),
    }
}

#[test]
fn staking_records_and_layer_root_match_the_spec_vectors() {
    let doc: Value = serde_json::from_str(VECTORS).unwrap();
    let cases = doc["cases"].as_array().unwrap();
    let mut records = BTreeMap::new();
    let mut checked = 0;
    let mut mismatches = Vec::new();
    let refused = |c: &&Value| c["expect"]["result"] == "error";
    for case in cases.iter().filter(refused) {
        let value = hex::decode(case["value"].as_str().unwrap()).unwrap();
        let decoded = match case["kind"].as_str().unwrap() {
            "meta" => Meta::decode(&value).map(drop),
            "validator" => StoredValidator::decode(&value).map(drop),
            "delegation" => StoredDelegation::decode(&value).map(drop),
            "payout" => PayoutJob::decode(&value).map(drop),
            "unbonding" => decode_unbonding(&value).map(drop),
            other => panic!("unknown record kind {other}"),
        };
        assert!(decoded.is_err(), "{} decoded", case["id"]);
        checked += 1;
    }
    for case in cases
        .iter()
        .filter(|c| c["kind"] != "layer_root" && !refused(c))
    {
        let (key, value) = encoded(case);
        let (want_key, want_value) = (
            case["key"].as_str().unwrap(),
            case["value"].as_str().unwrap(),
        );
        if hex::encode(&key) != want_key || hex::encode(&value) != want_value {
            mismatches.push(format!(
                "{}: key {} value {}",
                case["id"],
                hex::encode(&key),
                hex::encode(&value)
            ));
        }
        records.insert(case["id"].as_str().unwrap().to_string(), (key, value));
        checked += 1;
    }
    for case in cases.iter().filter(|c| c["kind"] == "layer_root") {
        let dir = TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        for member in case["records"].as_array().unwrap() {
            let (key, value) = &records[member.as_str().unwrap()];
            db.raw_put_for_test(key, value).unwrap();
        }
        let layer = db
            .state_layers(&Overlay::new())
            .unwrap()
            .into_iter()
            .find(|d| d.layer == StateLayer::Staking)
            .unwrap();
        let state_root = db.state_root().unwrap();
        if hex::encode(layer.root) != case["staking_layer_root"].as_str().unwrap()
            || hex::encode(state_root) != case["state_root"].as_str().unwrap()
        {
            mismatches.push(format!(
                "{}: staking_layer_root {} state_root {}",
                case["id"],
                hex::encode(layer.root),
                hex::encode(state_root)
            ));
        }
        checked += 1;
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    assert!(checked >= 8, "every record kind and the root: {checked}");
}
