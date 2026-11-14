//! The production node against `spec/tests/schedules.json` (rules SC-1..SC-7,
//! ADR-135 P-7). The vectors are hand-laid from the spec's byte layouts and
//! the schedule ids from the BLAKE3 reference implementation, not produced by
//! this crate. `apply` cases run real blocks: each step is one block at its
//! own height (or a direct write a passed proposal or a tripped guard would
//! have left), every step but the last must apply, and the last must give
//! the case's result; the expected state is then read back.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::schedule_payload::ScheduleAction;
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::governance::ParameterTable;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use custom_l1_node::state::invariant_guard::{BreakerRecord, Invariant, Module, guard_key};
use custom_l1_node::state::scheduled::{ScheduleRecord, schedule_id};
use custom_l1_node::state::shielded::FEE_SINK;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_governance::params::ParameterKey;
use primitive_types::U256;
use serde_json::Value;

mod common;

/// The payee every apply vector pays unless it names another.
const PAYEE: Address = [0x42; 32];

fn load() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests/schedules.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("schedules.json")).unwrap()
}

fn ok(case: &Value) -> bool {
    case["expect"]["result"] == "ok"
}

fn num(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}

fn amount(v: &Value) -> U256 {
    U256::from_dec_str(v.as_str().unwrap()).unwrap()
}

fn encoding_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
    let size: usize = case["size"].as_str().unwrap().parse().unwrap();
    assert_eq!(bytes.len(), size, "{id}");
    let mut reader = ByteReader::new(&bytes);
    let decoded = TxKind::decode(&mut reader).and_then(|kind| reader.finish().map(|()| kind));
    if case["expect"]["result"] == "error" {
        assert!(decoded.is_err(), "{id}: decoded a form the rules refuse");
        return;
    }
    let kind = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
    assert!(matches!(kind, TxKind::Schedule(_)), "{id}");
    let mut again = Vec::new();
    kind.encode_into(&mut again);
    assert_eq!(again, bytes, "{id}: re-encoding differs");
}

fn id_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let payer: [u8; 32] = hex::decode(case["payer"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let derived = schedule_id(&payer, num(&case["nonce"]));
    assert_eq!(
        hex::encode(derived),
        case["expect"]["id"].as_str().unwrap(),
        "{id}"
    );
}

fn record_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
    let decoded = ScheduleRecord::decode(&bytes);
    if !ok(case) {
        assert!(decoded.is_err(), "{id}: accepted a bad record");
        return;
    }
    let r = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
    let e = &case["expect"];
    assert_eq!(hex::encode(r.payer), e["payer"].as_str().unwrap(), "{id}");
    assert_eq!(hex::encode(r.payee), e["payee"].as_str().unwrap(), "{id}");
    assert_eq!(r.amount, amount(&e["amount"]), "{id}");
    assert_eq!(r.next_due, num(&e["next_due"]), "{id}");
    assert_eq!(r.interval, num(&e["interval"]), "{id}");
    assert_eq!(u64::from(r.remaining), num(&e["remaining"]), "{id}");
    assert_eq!(r.escrow, amount(&e["escrow"]), "{id}");
    assert_eq!(u64::from(r.missed), num(&e["missed"]), "{id}");
    assert_eq!(r.encode(), bytes, "{id}: re-encoding differs");
}

struct Actors {
    keys: [HybridSigningKey; 2],
}

impl Actors {
    fn key(&self, name: &str) -> &HybridSigningKey {
        match name {
            "k0" => &self.keys[0],
            "k1" => &self.keys[1],
            other => panic!("unknown actor {other}"),
        }
    }

    fn address(&self, name: &str) -> Address {
        match name {
            "payee" => PAYEE,
            "collector" => FEE_COLLECTOR,
            "sink" => FEE_SINK,
            other => self.key(other).address(),
        }
    }
}

fn step_kind(actors: &Actors, step: &Value) -> TxKind {
    let action = match step["kind"].as_str().unwrap() {
        "create" => ScheduleAction::Create {
            payee: actors.address(step["payee"].as_str().unwrap_or("payee")),
            amount: amount(&step["amount"]),
            first_due: num(&step["first_due"]),
            interval: num(&step["interval"]),
            runs: u32::try_from(num(&step["runs"])).unwrap(),
        },
        "cancel" => ScheduleAction::Cancel {
            schedule_id: schedule_id(&actors.address("k0"), num(&step["schedule"])),
        },
        "vault" => {
            return TxKind::Vault(Box::new(VaultAction::Configure(VaultConfig {
                delay_blocks: 10,
                limit: U256::from(1),
                guardians: vec![[0x99; 32]],
            })));
        }
        other => panic!("unknown step {other}"),
    };
    TxKind::Schedule(action)
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn set_run_fee(db: &StateDB, fee: &Value) {
    let table = ParameterTable::new().with(
        ParameterKey::ScheduleRunFee,
        maya_dex::Amount::from_u64(num(fee)),
    );
    db.raw_put_for_test(b"g:params", &table.encode()).unwrap();
}

/// A write a passed proposal or a tripped guard would have left; `false`
/// when the step is a block.
fn direct_step(db: &StateDB, step: &Value) -> bool {
    match step["kind"].as_str().unwrap() {
        "set_run_fee" => set_run_fee(db, &step["fee"]),
        "breaker" => {
            let tripped = BreakerRecord {
                module: Module::Scheduled,
                invariant: Invariant::PoolValuePerShare,
                tripped_at: num(&step["height"]),
                until: num(&step["until"]),
            };
            db.raw_put_for_test(&guard_key(Module::Scheduled), &tripped.encode())
                .unwrap();
        }
        _ => return false,
    }
    true
}

fn check_expect(db: &StateDB, actors: &Actors, case: &Value) {
    let id = case["id"].as_str().unwrap();
    let e = &case["expect"];
    if let Some(s) = e.get("schedule") {
        let sid = schedule_id(&actors.address("k0"), num(&s["nonce"]));
        let record = db.get_schedule(&sid).unwrap();
        if s.get("absent").is_some() {
            assert_eq!(record, None, "{id}: the schedule should be closed");
        } else {
            let r = record.unwrap_or_else(|| panic!("{id}: no schedule"));
            assert_eq!(
                u64::from(r.remaining),
                num(&s["remaining"]),
                "{id}: remaining"
            );
            assert_eq!(r.escrow, amount(&s["escrow"]), "{id}: escrow");
            assert_eq!(r.next_due, num(&s["next_due"]), "{id}: next_due");
            assert_eq!(u64::from(r.missed), num(&s["missed"]), "{id}: missed");
            assert!(
                db.is_due_indexed(r.next_due, &sid).unwrap(),
                "{id}: due row"
            );
        }
    }
    if let Some(balances) = e.get("balances").and_then(Value::as_object) {
        for (who, want) in balances {
            let got = db.get_account(&actors.address(who)).unwrap().balance;
            assert_eq!(got, amount(want), "{id}: balance of {who}");
        }
    }
}

fn apply_case(actors: &Actors, case: &Value) {
    let id = case["id"].as_str().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    let funds = amount(&case["funds"]);
    for key in &actors.keys {
        db.put_account(
            &key.address(),
            &Account {
                balance: funds,
                nonce: 0,
            },
        )
        .unwrap();
    }
    // "default" leaves the governed parameter unset: its activation default.
    if case["run_fee"] != "default" {
        set_run_fee(&db, &case["run_fee"]);
    }
    let activation = num(&case["activation"]);
    let steps = case["steps"].as_array().unwrap();
    let mut nonces = BTreeMap::<String, u64>::new();
    for (index, step) in steps.iter().enumerate() {
        if direct_step(&db, step) {
            continue;
        }
        let context =
            BlockContext::at_height(num(&step["height"])).with_scheduled_activation(activation);
        let (txs, from) = if step["kind"] == "empty" {
            (vec![], None)
        } else {
            let from = step["from"].as_str().unwrap().to_string();
            let nonce = *nonces.get(&from).unwrap_or(&0);
            let mut tx = Transaction::with_kind(step_kind(actors, step), nonce);
            tx.sign(actors.key(&from), &common::test_chain()).unwrap();
            (vec![tx], Some(from))
        };
        let result = db.apply_block(&block_of(txs), context);
        if index + 1 < steps.len() {
            result.unwrap_or_else(|e| panic!("{id}: setup step {index}: {e}"));
            if let Some(from) = from {
                *nonces.entry(from).or_default() += 1;
            }
            continue;
        }
        if !ok(case) {
            let Err(err) = result else {
                panic!("{id}: the last step was applied");
            };
            // The reason, when the vector names one: without it a refusal for an
            // unrelated failure would pass as this rule's.
            if let Some(reason) = case["expect"]["error"].as_str() {
                let err = err.to_string();
                assert!(
                    err.contains(reason),
                    "{id}: refused for `{err}`, not `{reason}`"
                );
            }
            return;
        }
        result.unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    check_expect(&db, actors, case);
}

#[test]
fn every_schedule_vector_holds() {
    let doc = load();
    let cases = doc["cases"].as_array().expect("cases");
    assert!(cases.len() >= 30, "the vector file lost cases");
    let actors = Actors {
        keys: [
            signing_key_from_seed(&[1; 32]).unwrap(),
            signing_key_from_seed(&[2; 32]).unwrap(),
        ],
    };
    for case in cases {
        match case["type"].as_str().unwrap_or("encoding") {
            "encoding" => encoding_case(case),
            "schedule_id" => id_case(case),
            "record" => record_case(case),
            "apply" => apply_case(&actors, case),
            other => panic!("unknown case type {other}"),
        }
    }
}
