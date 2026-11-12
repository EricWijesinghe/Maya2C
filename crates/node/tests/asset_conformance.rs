//! The production node against `spec/tests/assets.json` (rules AS-1..AS-7,
//! ADR-135 P-1). The vectors are hand-laid from the spec's byte layouts, not
//! produced by this crate, so an encoder that drifts from the spec fails here
//! with the case id.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use custom_l1_node::core::asset_mint_payload::AssetMintAction;
use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::asset::{AssetRecordV2, NO_AUTHORITY};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB, derive_asset_id};
use primitive_types::U256;
use serde_json::Value;

mod common;

const SYMBOL: [u8; 8] = *b"CAP\0\0\0\0\0";

fn load() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests/assets.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("assets.json")).unwrap()
}

fn amount(v: &Value) -> U256 {
    U256::from_dec_str(v.as_str().unwrap()).unwrap()
}

fn num(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}

fn ok(case: &Value) -> bool {
    case["expect"]["result"] == "ok"
}

fn encoding_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
    let mut reader = ByteReader::new(&bytes);
    let decoded = TxKind::decode(&mut reader).and_then(|kind| reader.finish().map(|()| kind));
    if !ok(case) {
        assert!(decoded.is_err(), "{id}: decoded a truncated form");
        return;
    }
    let kind = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
    assert!(matches!(kind, TxKind::AssetMint(_)), "{id}");
    let mut again = Vec::new();
    kind.encode_into(&mut again);
    assert_eq!(again, bytes, "{id}: re-encoding differs");
    assert_eq!(
        bytes.len(),
        usize::try_from(num(&case["size"])).unwrap(),
        "{id}"
    );
}

fn record_case(case: &Value) {
    let id = case["id"].as_str().unwrap();
    let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
    let decoded = AssetRecordV2::decode(&bytes);
    if !ok(case) {
        assert!(decoded.is_err(), "{id}: accepted a bad record");
        return;
    }
    let r = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
    let e = &case["expect"];
    assert_eq!(r.base.total_supply, amount(&e["supply"]), "{id}");
    assert_eq!(r.cap, amount(&e["cap"]), "{id}");
    assert_eq!(
        hex::encode(r.authority),
        e["authority"].as_str().unwrap(),
        "{id}"
    );
}

struct Actors {
    k0: HybridSigningKey,
    k1: HybridSigningKey,
}

impl Actors {
    fn key(&self, name: &str) -> &HybridSigningKey {
        match name {
            "k0" => &self.k0,
            "k1" => &self.k1,
            other => panic!("unknown actor {other}"),
        }
    }

    fn address(&self, name: &str, sender: &HybridSigningKey) -> Address {
        match name {
            "self" => sender.address(),
            "none" => NO_AUTHORITY,
            other => self.key(other).address(),
        }
    }
}

fn step_kind(actors: &Actors, step: &Value, asset: [u8; 32]) -> TxKind {
    let from = actors.key(step["from"].as_str().unwrap());
    let action = match step["kind"].as_str().unwrap() {
        "register" => AssetMintAction::RegisterCapped {
            symbol: SYMBOL,
            total_supply: amount(&step["supply"]),
            cap: amount(&step["cap"]),
            authority: actors.address(step["authority"].as_str().unwrap(), from),
        },
        "mint" => AssetMintAction::Mint {
            asset,
            to: actors.address(step["to"].as_str().unwrap(), from),
            amount: amount(&step["amount"]),
        },
        "burn" => AssetMintAction::Burn {
            asset,
            amount: amount(&step["amount"]),
        },
        "renounce" => AssetMintAction::Renounce { asset },
        other => panic!("unknown step {other}"),
    };
    TxKind::AssetMint(action)
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
    for key in [&actors.k0, &actors.k1] {
        let funded = Account {
            balance: U256::from(1_000u64),
            nonce: 0,
        };
        db.put_account(&key.address(), &funded).unwrap();
    }
    let context = BlockContext::at_height(num(&case["height"]))
        .with_asset_mint_activation(num(&case["activation"]));
    // Every vector registers first, as k0 at nonce 0.
    let asset = derive_asset_id(&actors.k0.address(), 0, &SYMBOL);
    let steps = case["steps"].as_array().unwrap();
    let mut nonces = std::collections::BTreeMap::<String, u64>::new();
    for (index, step) in steps.iter().enumerate() {
        let from = step["from"].as_str().unwrap().to_string();
        let nonce = nonces.entry(from.clone()).or_default();
        let mut tx = Transaction::with_kind(step_kind(actors, step, asset), *nonce);
        tx.sign(actors.key(&from), &common::test_chain()).unwrap();
        let result = db.apply_block(&block_of(tx), context);
        if index + 1 < steps.len() {
            result.unwrap_or_else(|e| panic!("{id}: setup step {index}: {e}"));
            *nonce += 1;
            continue;
        }
        if !ok(case) {
            assert!(result.is_err(), "{id}: the last step was applied");
            return;
        }
        result.unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    let r = db.get_capped_asset(&asset).unwrap().expect("registered");
    let e = &case["expect"];
    assert_eq!(r.base.total_supply, amount(&e["supply"]), "{id}");
    assert_eq!(r.cap, amount(&e["cap"]), "{id}");
    let authority = actors.address(e["authority"].as_str().unwrap(), &actors.k0);
    assert_eq!(r.authority, authority, "{id}");
}

#[test]
fn asset_mint_vectors() {
    let doc = load();
    let actors = Actors {
        k0: signing_key_from_seed(&[1; 32]).unwrap(),
        k1: signing_key_from_seed(&[2; 32]).unwrap(),
    };
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 20, "the vector file lost cases");
    for case in cases {
        match case["type"].as_str().unwrap() {
            "encoding" => encoding_case(case),
            "record" => record_case(case),
            "apply" => apply_case(&actors, case),
            other => panic!("unknown case type {other}"),
        }
    }
}
