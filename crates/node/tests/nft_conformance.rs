//! The production node against `spec/tests/nfts.json` (rules NF-1..NF-7,
//! ADR-135 P-2). The vectors are hand-laid from the spec's byte layouts and
//! the collection ids from the BLAKE3 reference implementation, not produced
//! by this crate, so an encoder that drifts from the spec fails here with the
//! case id. `apply` cases run real blocks: each step is one block at the
//! case's height, every step but the last must apply, and the last must give
//! the case's result.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::nft_payload::NftAction;
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::nft::{NftCollection, NftItem, collection_id};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use primitive_types::U256;
use serde_json::Value;

mod common;

fn load() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests/nfts.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("nfts.json")).unwrap()
}

fn ok(case: &Value) -> bool {
    case["expect"]["result"] == "ok"
}

fn num(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}

fn bytes32(v: &Value) -> [u8; 32] {
    hex::decode(v.as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap()
}

fn encoding_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
    let mut reader = ByteReader::new(&bytes);
    let decoded = TxKind::decode(&mut reader).and_then(|kind| reader.finish().map(|()| kind));
    if case["expect"]["result"] == "error" {
        assert!(decoded.is_err(), "{id}: decoded a truncated form");
        return;
    }
    let kind = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
    assert!(matches!(kind, TxKind::Nft(_)), "{id}");
    let mut again = Vec::new();
    kind.encode_into(&mut again);
    assert_eq!(again, bytes, "{id}: re-encoding differs");
    let size: usize = case["size"].as_str().unwrap().parse().unwrap();
    assert_eq!(bytes.len(), size, "{id}");
}

fn collection_id_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let derived = collection_id(&bytes32(&case["creator"]), num(&case["nonce"]));
    assert_eq!(
        hex::encode(derived),
        case["expect"]["id"].as_str().unwrap(),
        "{id}"
    );
}

/// NF-3/NF-6: a record decodes to its fields and re-encodes to its bytes; a
/// record of any other length (a URI tacked on) or version is refused.
fn record_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
    let e = &case["expect"];
    match case["record"].as_str().unwrap() {
        "collection" => {
            let decoded = NftCollection::decode(&bytes);
            if !ok(case) {
                assert!(decoded.is_err(), "{id}: accepted a bad record");
                return;
            }
            let r = decoded.unwrap_or_else(|err| panic!("{id}: {err}"));
            assert_eq!(r.creator, bytes32(&e["creator"]), "{id}");
            assert_eq!(r.max_items, num(&e["max_items"]), "{id}");
            assert_eq!(r.minted, num(&e["minted"]), "{id}");
            assert_eq!(r.metadata_hash, bytes32(&e["metadata_hash"]), "{id}");
            assert_eq!(r.encode(), bytes, "{id}: re-encoding differs");
        }
        "item" => {
            let decoded = NftItem::decode(&bytes);
            if !ok(case) {
                assert!(decoded.is_err(), "{id}: accepted a bad record");
                return;
            }
            let r = decoded.unwrap_or_else(|err| panic!("{id}: {err}"));
            assert_eq!(r.owner, bytes32(&e["owner"]), "{id}");
            assert_eq!(r.metadata_hash, bytes32(&e["metadata_hash"]), "{id}");
            assert_eq!(r.encode(), bytes, "{id}: re-encoding differs");
        }
        other => panic!("{id}: unknown record {other}"),
    }
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
}

const HASH: [u8; 32] = [0xA1; 32];

fn step_kind(actors: &Actors, step: &Value, collection: [u8; 32]) -> TxKind {
    let action = match step["kind"].as_str().unwrap() {
        "create" => NftAction::CreateCollection {
            max_items: num(&step["max_items"]),
            metadata_hash: HASH,
        },
        "mint" => NftAction::MintItem {
            collection,
            metadata_hash: HASH,
        },
        "transfer" => NftAction::TransferItem {
            collection,
            index: num(&step["index"]),
            to: actors.key(step["to"].as_str().unwrap()).address(),
        },
        "burn" => NftAction::BurnItem {
            collection,
            index: num(&step["index"]),
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
    TxKind::Nft(action)
}

fn block_of(tx: Transaction) -> Block {
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
        vec![tx],
    )
}

fn apply_case(actors: &Actors, case: &Value) {
    let id = case["id"].as_str().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    for key in &actors.keys {
        let funded = Account {
            balance: U256::from(1_000u64),
            nonce: 0,
        };
        db.put_account(&key.address(), &funded).unwrap();
    }
    let context =
        BlockContext::at_height(num(&case["height"])).with_nft_activation(num(&case["activation"]));
    // Every vector's collection is k0's first, at nonce 0.
    let collection = collection_id(&actors.keys[0].address(), 0);
    let steps = case["steps"].as_array().unwrap();
    let mut nonces = BTreeMap::<String, u64>::new();
    for (index, step) in steps.iter().enumerate() {
        let from = step["from"].as_str().unwrap().to_string();
        let nonce = nonces.entry(from.clone()).or_default();
        let mut tx = Transaction::with_kind(step_kind(actors, step, collection), *nonce);
        tx.sign(actors.key(&from), &common::test_chain()).unwrap();
        let result = db.apply_block(&block_of(tx), context);
        if index + 1 < steps.len() {
            result.unwrap_or_else(|e| panic!("{id}: setup step {index}: {e}"));
            *nonce += 1;
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
    let e = &case["expect"];
    let record = db
        .get_nft_collection(&collection)
        .unwrap()
        .expect("collection");
    assert_eq!(record.minted, num(&e["minted"]), "{id}");
    for (index, owner) in e["items"].as_object().unwrap() {
        let index: u64 = index.parse().unwrap();
        let item = db.get_nft_item(&collection, index).unwrap();
        match owner.as_str() {
            None => {
                assert_eq!(item, None, "{id}: item {index} should not exist");
                for key in &actors.keys {
                    assert!(
                        !db.nft_owner_index_contains(&key.address(), &collection, index)
                            .unwrap(),
                        "{id}: a burned item kept an owner row"
                    );
                }
            }
            Some(name) => {
                let owner: Address = actors.key(name).address();
                assert_eq!(item.expect("item").owner, owner, "{id}: item {index}");
                assert!(
                    db.nft_owner_index_contains(&owner, &collection, index)
                        .unwrap(),
                    "{id}: item {index} missing its owner row"
                );
            }
        }
    }
}

#[test]
fn every_unique_asset_vector_holds() {
    let doc = load();
    let cases = doc["cases"].as_array().expect("cases");
    assert!(cases.len() >= 20, "the vector file lost cases");
    let actors = Actors {
        keys: [
            signing_key_from_seed(&[1; 32]).unwrap(),
            signing_key_from_seed(&[2; 32]).unwrap(),
        ],
    };
    for case in cases {
        match case["type"].as_str().unwrap_or("encoding") {
            "encoding" => encoding_case(case),
            "collection_id" => collection_id_case(case),
            "record" => record_case(case),
            "apply" => apply_case(&actors, case),
            other => panic!("unknown case type {other}"),
        }
    }
}
