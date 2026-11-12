//! The production node against `spec/tests/sponsored.json` (rules SP-1..SP-6,
//! ADR-135 P-4). The frame and signing-bytes vectors are hand-laid from the
//! spec's byte layouts, not produced by this crate, so an encoder that drifts
//! from the spec fails here with the case id. The behaviour vectors state
//! inputs; their frames carry real signatures, so this file builds them from
//! seeded keys and checks the outcome the vector names.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
use custom_l1_node::core::sponsor_tx::signing_bytes;
use custom_l1_node::core::transaction::{ChainTag, TxOutput};
use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa87, SignatureSuite, SuiteId, suite_address,
};
use maya_dex::Amount;
use primitive_types::U256;
use serde_json::Value;
use tempfile::TempDir;

mod common;

const BASE_FEE: u64 = 2;
const FUNDS: u64 = 100_000_000;
const V4: u64 = 10;
const PAYEE: Address = [0x77; 32];

fn load() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests/sponsored.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("sponsored.json")).unwrap()
}

fn text<'a>(case: &'a Value, field: &str) -> &'a str {
    case[field].as_str().unwrap_or_else(|| panic!("{field}"))
}

fn ok(case: &Value) -> bool {
    case["expect"]["result"] == "ok"
}

fn frame_case(case: &Value) {
    let id = text(case, "id");
    let bytes = hex::decode(text(case, "hex")).unwrap();
    let decoded = Transaction::from_bytes(&bytes);
    if !ok(case) {
        assert!(decoded.is_err(), "{id}: decoded a refused frame");
        return;
    }
    let tx = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
    let e = &case["expect"];
    let sponsorship = tx
        .sponsor
        .as_deref()
        .unwrap_or_else(|| panic!("{id}: no sponsor"));
    assert_eq!(
        tx.inner_bytes()[0].to_string(),
        text(e, "inner_version"),
        "{id}"
    );
    assert_eq!(tx.nonce.to_string(), text(e, "inner_nonce"), "{id}");
    assert_eq!(
        sponsorship.suite.to_byte().to_string(),
        text(e, "suite"),
        "{id}"
    );
    assert_eq!(sponsorship.max_fee.to_string(), text(e, "max_fee"), "{id}");
    assert_eq!(
        sponsorship.valid_until.to_string(),
        text(e, "valid_until"),
        "{id}"
    );
    assert_eq!(tx.to_bytes(), bytes, "{id}: re-encoding differs");
    assert_eq!(bytes.len().to_string(), text(case, "size"), "{id}");
}

fn signing_case(case: &Value) {
    let id = text(case, "id");
    let chain = ChainTag::from_genesis(
        hex::decode(text(case, "chain_tag"))
            .unwrap()
            .try_into()
            .unwrap(),
    );
    let txid: [u8; 32] = hex::decode(text(case, "inner_txid"))
        .unwrap()
        .try_into()
        .unwrap();
    let max_fee = U256::from_dec_str(text(case, "max_fee")).unwrap();
    let valid_until: u64 = text(case, "valid_until").parse().unwrap();
    assert_eq!(
        hex::encode(signing_bytes(&chain, &txid, &max_fee, valid_until)),
        text(&case["expect"], "hex"),
        "{id}"
    );
}

fn fixture() -> (StateDB, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    common::bind(&db);
    let fees = FeeRecord {
        base_fee: Amount::from_u64(BASE_FEE),
        min_base_fee: Amount::from_u64(BASE_FEE),
        target_block_bytes: 1_000_000_000,
        change_denominator: 8,
    };
    db.raw_put_for_test(FEE_KEY, &fees.encode()).unwrap();
    (db, dir)
}

fn fund(db: &StateDB, who: &Address, balance: U256) {
    db.put_account(who, &Account { balance, nonce: 0 }).unwrap();
}

/// The inner frame a behaviour vector names, signed, with its sender funded.
fn inner(db: &StateDB, form: &str, fee_output: bool) -> Transaction {
    let mut outputs = vec![TxOutput {
        amount: U256::one(),
        recipient: PAYEE,
    }];
    if fee_output {
        outputs.push(TxOutput {
            amount: U256::one(),
            recipient: FEE_COLLECTOR,
        });
    }
    let chain = common::test_chain();
    match form {
        "v6" => {
            let key = signing_key_from_seed(&[0x21; 32]).unwrap();
            fund(db, &key.address(), U256::from(FUNDS));
            let mut tx = Transaction::with_kind(
                TxKind::Vault(VaultAction::Configure(VaultConfig {
                    delay_blocks: 100,
                    limit: U256::from(FUNDS),
                    guardians: vec![[0x99; 32]],
                })),
                0,
            );
            tx.outputs = outputs;
            tx.sign(&key, &chain).unwrap();
            tx
        }
        "v7" => {
            let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([0x22; 32]));
            let sender = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&key));
            fund(db, &sender, U256::from(FUNDS));
            let mut tx = Transaction::new(vec![], outputs, 0);
            tx.sign_with_suite::<MlDsa87>(&key, &chain).unwrap();
            tx
        }
        "v8" => {
            let members: Vec<_> = (1..=3u8)
                .map(|i| MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([i; 32])))
                .collect();
            let policy = MultisigPolicy::new(
                2,
                members
                    .iter()
                    .map(|k| PolicyKey {
                        suite: MlDsa87::ID,
                        public_key: MlDsa87::public_key(k),
                    })
                    .collect(),
            )
            .unwrap();
            fund(db, &multisig_address(&policy), U256::from(FUNDS));
            let mut tx = Transaction::new(vec![], outputs, 0);
            tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
            let message = tx.signing_bytes(&chain);
            let approvals = [0u8, 1]
                .iter()
                .map(|&i| Approval {
                    index: i,
                    signature: MlDsa87::sign(&members[usize::from(i)], &message).unwrap(),
                })
                .collect();
            tx.multisig = Some(Box::new(MultisigAuth::new(policy, approvals).unwrap()));
            tx
        }
        other => panic!("unknown inner form {other}"),
    }
}

fn behaviour_case(case: &Value) {
    let id = text(case, "id");
    let (db, _dir) = fixture();
    let sponsor =
        HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes([0x5A; 32]));
    let payer = suite_address(
        SuiteId::HybridMlDsa65SlhDsa128s,
        &HybridMlDsa65SlhDsa128s::public_key(&sponsor),
    );
    let valid_until = V4 + 5;
    let chain = match text(case, "chain") {
        "own" => common::test_chain(),
        _ => ChainTag::from_genesis([0xEE; 32]),
    };
    let base = inner(&db, text(case, "inner"), case["inner_fee_output"] == true);
    let sponsor_with = |max_fee: U256| {
        let mut tx = base.clone();
        tx.sponsor_with::<HybridMlDsa65SlhDsa128s>(&sponsor, max_fee, valid_until, &chain)
            .unwrap();
        tx
    };
    // The outer size does not depend on max_fee's value.
    let required = U256::from(BASE_FEE) * U256::from(sponsor_with(U256::zero()).to_bytes().len());
    let delta: i64 = text(case, "max_fee_delta").parse().unwrap();
    let max_fee = if delta < 0 {
        required - U256::from(delta.unsigned_abs())
    } else {
        required + U256::from(delta.unsigned_abs())
    };
    let frame = sponsor_with(max_fee);
    let short: u64 = text(case, "sponsor_short").parse().unwrap();
    fund(&db, &payer, required - U256::from(short));

    let height = valid_until + text(case, "height_delta").parse::<u64>().unwrap();
    let activation = if case["activated"] == true {
        V4
    } else {
        height + 1
    };
    let context = BlockContext::at_height(height).with_sponsored_fees_activation(activation);
    let block = Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        vec![frame],
    );
    let result = db.apply_block(&block, context);
    if ok(case) {
        result.unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(
            db.get_account(&payer).unwrap().balance,
            U256::zero(),
            "{id}: the sponsor pays exactly required(outer_len)"
        );
    } else {
        let err = result.expect_err(id);
        let reason = text(&case["expect"], "reason");
        assert!(
            err.to_string().to_lowercase().contains(reason),
            "{id}: {err} does not name {reason}"
        );
    }
}

#[test]
fn every_sponsored_vector_holds() {
    let doc = load();
    let cases = doc["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        match text(case, "type") {
            "frame" => frame_case(case),
            "signing-bytes" => signing_case(case),
            "behaviour" => behaviour_case(case),
            other => panic!("{}: unknown type {other}", text(case, "id")),
        }
    }
}
