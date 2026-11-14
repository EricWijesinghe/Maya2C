//! The production node against `spec/tests/names.json` (rules NM-1..NM-11,
//! ADR-135 P-3). The vectors are hand-laid from the spec's byte layouts and
//! the commitments from the BLAKE3 reference implementation, not produced by
//! this crate. `apply` cases run real blocks: each step is one block at its
//! own height, every step but the last must apply, and the last must give
//! the case's result; the expected state is then read back.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::name_payload::{NameAction, NameUpdate};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::governance::ParameterTable;
use custom_l1_node::state::invariant_guard::{BreakerRecord, Invariant, Module, guard_key};
use custom_l1_node::state::names::name_commitment;
use custom_l1_node::state::shielded::FEE_SINK;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_governance::params::ParameterKey;
use primitive_types::U256;
use serde_json::Value;

mod common;

/// Salt of every commitment in the apply vectors.
const SALT: [u8; 32] = [0x5A; 32];

fn load() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests/names.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("names.json")).unwrap()
}

fn ok(case: &Value) -> bool {
    case["expect"]["result"] == "ok"
}

fn num(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}

fn text(v: &Value) -> &[u8] {
    v.as_str().unwrap().as_bytes()
}

fn encoding_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
    let mut reader = ByteReader::new(&bytes);
    let decoded = TxKind::decode(&mut reader).and_then(|kind| reader.finish().map(|()| kind));
    if case["expect"]["result"] == "error" {
        assert!(decoded.is_err(), "{id}: decoded a form the rules refuse");
        return;
    }
    let kind = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
    assert!(matches!(kind, TxKind::Name(_)), "{id}");
    let mut again = Vec::new();
    kind.encode_into(&mut again);
    assert_eq!(again, bytes, "{id}: re-encoding differs");
    let size: usize = case["size"].as_str().unwrap().parse().unwrap();
    assert_eq!(bytes.len(), size, "{id}");
}

fn commitment_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let sender: [u8; 32] = hex::decode(case["sender"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let salt: [u8; 32] = hex::decode(case["salt"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let derived = name_commitment(text(&case["label"]), &sender, &salt);
    assert_eq!(
        hex::encode(derived),
        case["expect"]["commitment"].as_str().unwrap(),
        "{id}"
    );
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
        self.key(name).address()
    }
}

fn label_of(step: &Value) -> Vec<u8> {
    text(&step["label"]).to_vec()
}

fn step_kind(actors: &Actors, step: &Value) -> TxKind {
    let from = actors.address(step["from"].as_str().unwrap());
    let to = |field: &str| actors.address(step[field].as_str().unwrap());
    let periods = || u8::try_from(num(&step["periods"])).unwrap();
    let action = match step["kind"].as_str().unwrap() {
        "commit" => NameAction::Commit {
            commitment: name_commitment(&label_of(step), &from, &SALT),
        },
        "register" => NameAction::Register {
            label: label_of(step),
            salt: SALT,
            periods: periods(),
            target: to("target"),
        },
        "renew" => NameAction::Renew {
            label: label_of(step),
            periods: periods(),
        },
        "set_target" => NameAction::Update {
            label: label_of(step),
            op: NameUpdate::SetTarget(to("to")),
        },
        "set_reverse" => NameAction::Update {
            label: label_of(step),
            op: NameUpdate::SetReverse,
        },
        "transfer" => NameAction::Update {
            label: label_of(step),
            op: NameUpdate::Transfer(to("to")),
        },
        other => panic!("unknown step {other}"),
    };
    TxKind::Name(action)
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

/// Steps that write state directly, as a passed proposal or a tripped guard
/// would have: `None` when the step is a transaction.
fn direct_step(db: &StateDB, step: &Value, height: u64) -> Option<()> {
    match step["kind"].as_str().unwrap() {
        "breaker" => {
            let tripped = BreakerRecord {
                module: Module::Names,
                invariant: Invariant::PoolValuePerShare,
                tripped_at: height,
                until: num(&step["until"]),
            };
            db.raw_put_for_test(&guard_key(Module::Names), &tripped.encode())
                .unwrap();
        }
        "release_reserved" => {
            let table = ParameterTable::new().with(
                ParameterKey::NameReservedLabels,
                maya_dex::Amount::from_u64(0),
            );
            db.raw_put_for_test(b"g:params", &table.encode()).unwrap();
        }
        _ => return None,
    }
    Some(())
}

fn check_expect(db: &StateDB, actors: &Actors, case: &Value) {
    let id = case["id"].as_str().unwrap();
    let e = &case["expect"];
    if let Some(name) = e.get("name") {
        let label = text(&name["label"]);
        let record = db.get_name(label).unwrap();
        if name.get("absent").is_some() {
            assert_eq!(record, None, "{id}: the name should not exist");
        } else {
            let record = record.unwrap_or_else(|| panic!("{id}: no name record"));
            assert_eq!(
                record.owner,
                actors.address(name["owner"].as_str().unwrap()),
                "{id}"
            );
            assert_eq!(
                record.target,
                actors.address(name["target"].as_str().unwrap()),
                "{id}"
            );
            assert_eq!(
                record.expires_at,
                num(&name["expires_at"]),
                "{id}: expires_at"
            );
        }
    }
    if let Some(reverse) = e.get("reverse").and_then(Value::as_object) {
        for (who, label) in reverse {
            let got = db.reverse_name(&actors.address(who)).unwrap();
            let want = label
                .as_str()
                .map(|l| *blake3::hash(l.as_bytes()).as_bytes());
            assert_eq!(got, want, "{id}: reverse record of {who}");
        }
    }
    if let Some(c) = e.get("commitment") {
        let from = actors.address(c["from"].as_str().unwrap());
        let commitment = name_commitment(text(&c["label"]), &from, &SALT);
        let got = db.commitment_height(&from, &commitment).unwrap();
        assert_eq!(
            got,
            c["height"].as_str().map(|h| h.parse().unwrap()),
            "{id}: commitment row"
        );
        // The expiry index row (`nm:x:`) lives and dies with the commitment
        // row: checked at every height this sender committed this label.
        for step in case["steps"].as_array().unwrap() {
            let same = step["from"] == c["from"] && step["label"] == c["label"];
            if step["kind"] == "commit" && same {
                let at = num(&step["height"]);
                assert_eq!(
                    db.commitment_indexed(&from, &commitment, at).unwrap(),
                    got == Some(at),
                    "{id}: expiry index row of the commit at {at}"
                );
            }
        }
    }
    if let Some(burned) = e.get("burned") {
        let sink = db.get_account(&FEE_SINK).unwrap().balance;
        assert_eq!(
            sink,
            U256::from_dec_str(burned.as_str().unwrap()).unwrap(),
            "{id}: burned"
        );
    }
}

fn apply_case(actors: &Actors, case: &Value) {
    let id = case["id"].as_str().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    // A hundred coins each (10^24 base units a coin): ten years of a 3-byte name.
    let funds = U256::from(10u64).pow(U256::from(26));
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
    let activation = num(&case["activation"]);
    let steps = case["steps"].as_array().unwrap();
    let mut nonces = BTreeMap::<String, u64>::new();
    for (index, step) in steps.iter().enumerate() {
        let height = num(&step["height"]);
        if direct_step(&db, step, height).is_some() {
            continue;
        }
        let context = BlockContext::at_height(height).with_names_activation(activation);
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
fn every_name_vector_holds() {
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
            "commitment" => commitment_case(case),
            "apply" => apply_case(&actors, case),
            other => panic!("unknown case type {other}"),
        }
    }
}
