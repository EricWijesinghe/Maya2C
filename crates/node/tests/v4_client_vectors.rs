//! Client vectors for the v4 transaction forms (ADR-135 I7/I8): one payload per
//! form, its exact bytes, and the words every wallet shows for it.
//!
//! The node is the source of truth. This test builds each form with the node's
//! own types, round-trips it through the node's codec, renders it with
//! `core::describe`, and compares the result with `spec/tests/v4-client-vectors.json`.
//! The Python, TypeScript and WASM SDKs read that same file, so an SDK that
//! encodes a byte differently, or words a form differently, fails its own test
//! against the node's output rather than drifting quietly.
//!
//! Regenerate after an intended change with `MAYA2C_WRITE_VECTORS=1`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::asset_mint_payload::AssetMintAction;
use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::describe::{Review, describe, describe_kind};
use custom_l1_node::core::name_payload::{NameAction, NameUpdate};
use custom_l1_node::core::nft_payload::NftAction;
use custom_l1_node::core::schedule_payload::ScheduleAction;
use custom_l1_node::core::{Transaction, TxKind};
use custom_l1_node::rpc::TransactionInfo;
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite, SuiteId, suite_address};
use primitive_types::U256;
use serde_json::{Value, json};

mod common;

const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../spec/tests/v4-client-vectors.json"
);

/// A symbol as the wire carries it: ASCII, NUL-padded to eight bytes.
fn symbol(text: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..text.len()].copy_from_slice(text.as_bytes());
    out
}

fn h(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Every v4 form, with the fields an SDK encodes it from. Integers are decimal
/// strings so a `u64` or `U256` survives JSON in every language.
#[allow(clippy::too_many_lines)] // one entry per form is the point
fn samples() -> Vec<(&'static str, TxKind, Value)> {
    let (a, b, c) = ([0xA1; 32], [0xB2; 32], [0xC3; 32]);
    let big = U256::from_dec_str("1000000000000000000000000").unwrap();
    vec![
        (
            "register_capped_asset",
            TxKind::AssetMint(AssetMintAction::RegisterCapped {
                symbol: symbol("GOLD"),
                total_supply: U256::from(1_000_u64),
                cap: big,
                authority: a,
            }),
            json!({"symbol": "GOLD", "total_supply": "1000", "cap": big.to_string(), "authority": h(&a)}),
        ),
        (
            "mint_asset",
            TxKind::AssetMint(AssetMintAction::Mint {
                asset: b,
                to: c,
                amount: U256::from(250_u64),
            }),
            json!({"asset": h(&b), "to": h(&c), "amount": "250"}),
        ),
        (
            "burn_asset",
            TxKind::AssetMint(AssetMintAction::Burn {
                asset: b,
                amount: U256::from(7_u64),
            }),
            json!({"asset": h(&b), "amount": "7"}),
        ),
        (
            "renounce_mint",
            TxKind::AssetMint(AssetMintAction::Renounce { asset: b }),
            json!({"asset": h(&b)}),
        ),
        (
            "create_collection",
            TxKind::Nft(NftAction::CreateCollection {
                max_items: 10_000,
                metadata_hash: c,
            }),
            json!({"max_items": "10000", "metadata_hash": h(&c)}),
        ),
        (
            "mint_item",
            TxKind::Nft(NftAction::MintItem {
                collection: a,
                metadata_hash: c,
            }),
            json!({"collection": h(&a), "metadata_hash": h(&c)}),
        ),
        (
            "transfer_item",
            TxKind::Nft(NftAction::TransferItem {
                collection: a,
                index: 42,
                to: b,
            }),
            json!({"collection": h(&a), "index": "42", "to": h(&b)}),
        ),
        (
            "burn_item",
            TxKind::Nft(NftAction::BurnItem {
                collection: a,
                index: u64::MAX,
            }),
            json!({"collection": h(&a), "index": u64::MAX.to_string()}),
        ),
        (
            "commit_name",
            TxKind::Name(NameAction::Commit { commitment: c }),
            json!({"commitment": h(&c)}),
        ),
        (
            "register_name",
            TxKind::Name(NameAction::Register {
                label: b"alice".to_vec(),
                salt: b,
                periods: 2,
                target: a,
            }),
            json!({"label": "alice", "salt": h(&b), "periods": "2", "target": h(&a)}),
        ),
        (
            "renew_name",
            TxKind::Name(NameAction::Renew {
                label: b"alice".to_vec(),
                periods: 10,
            }),
            json!({"label": "alice", "periods": "10"}),
        ),
        (
            "update_name",
            TxKind::Name(NameAction::Update {
                label: b"alice".to_vec(),
                op: NameUpdate::SetTarget(c),
            }),
            json!({"label": "alice", "op": "set_target", "address": h(&c)}),
        ),
        (
            "update_name",
            TxKind::Name(NameAction::Update {
                label: b"my-shop-1".to_vec(),
                op: NameUpdate::SetReverse,
            }),
            json!({"label": "my-shop-1", "op": "set_reverse"}),
        ),
        (
            "update_name",
            TxKind::Name(NameAction::Update {
                label: b"alice".to_vec(),
                op: NameUpdate::Transfer(b),
            }),
            json!({"label": "alice", "op": "transfer", "address": h(&b)}),
        ),
        (
            "create_schedule",
            TxKind::Schedule(ScheduleAction::Create {
                payee: b,
                amount: U256::from(1_500_000_u64),
                first_due: 1_000,
                interval: 86_400,
                runs: 12,
            }),
            json!({"payee": h(&b), "amount": "1500000", "first_due": "1000", "interval": "86400", "runs": "12"}),
        ),
        (
            "cancel_schedule",
            TxKind::Schedule(ScheduleAction::Cancel { schedule_id: a }),
            json!({"schedule_id": h(&a)}),
        ),
    ]
}

fn payload(kind: &TxKind) -> Vec<u8> {
    let mut buf = Vec::new();
    kind.encode_into(&mut buf);
    buf
}

fn decode(bytes: &[u8]) -> TxKind {
    let mut reader = ByteReader::new(bytes);
    let kind = TxKind::decode(&mut reader).expect("decodes");
    reader.finish().expect("no trailing bytes");
    kind
}

fn vector_file() -> Value {
    let vectors: Vec<Value> = samples()
        .into_iter()
        .map(|(label, kind, fields)| {
            let d = describe_kind(&kind);
            json!({
                "label": label,
                "fields": fields,
                "payload": h(&payload(&kind)),
                "summary": d.summary,
                "careful": d.review == Review::Careful,
                "details": d.details.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "about": "ADR-135 v4 transaction payloads (TxKind tag || fields), generated by crates/node/tests/v4_client_vectors.rs from the node's own codec and core::describe. Integers are decimal strings; bytes are lowercase hex; labels and symbols are ASCII. Regenerate with MAYA2C_WRITE_VECTORS=1; never edit by hand.",
        "vectors": vectors,
    })
}

#[test]
fn every_v4_form_round_trips_through_the_node_codec() {
    for (label, kind, _) in samples() {
        let bytes = payload(&kind);
        assert_eq!(decode(&bytes), kind, "{label}");
        assert_eq!(kind.label(), label);
        // One byte short is a decode error, never a different form.
        let mut reader = ByteReader::new(&bytes[..bytes.len() - 1]);
        let short = TxKind::decode(&mut reader).and_then(|_| reader.finish());
        assert!(
            short.is_err(),
            "{label}: a truncated payload must not decode"
        );
    }
}

#[test]
fn every_v4_form_renders_a_sentence_and_names_its_addresses_in_full() {
    for (label, kind, _) in samples() {
        let d = describe_kind(&kind);
        assert!(d.summary.ends_with('.'), "{label}: {}", d.summary);
        for (_, value) in &d.details {
            assert!(
                !value.contains('\u{2026}'),
                "{label}: a detail is never abbreviated"
            );
        }
    }
    let give = describe_kind(&samples()[13].1);
    assert!(
        give.details.contains(&("new owner", "b2".repeat(32))),
        "{give:?}"
    );
    assert_eq!(
        give.review,
        Review::Careful,
        "giving a name away is careful"
    );
}

#[test]
fn the_client_vector_file_is_what_the_node_produces() {
    let produced = vector_file();
    if std::env::var_os("MAYA2C_WRITE_VECTORS").is_some() {
        let text = serde_json::to_string_pretty(&produced).unwrap() + "\n";
        std::fs::write(VECTORS, text).expect("write vectors");
    }
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(VECTORS).expect("read vectors"))
            .expect("json");
    assert_eq!(
        on_disk, produced,
        "spec/tests/v4-client-vectors.json is stale: rerun with MAYA2C_WRITE_VECTORS=1"
    );
}

#[test]
fn a_sponsored_frame_names_the_sponsor_that_pays_and_its_ceiling() {
    let sender = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([1; 32]));
    let sponsor = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([2; 32]));
    let sponsor_address = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&sponsor));
    let chain = common::test_chain();
    let mut tx = Transaction::with_kind(samples()[1].1.clone(), 0);
    tx.sign_with_suite::<MlDsa87>(&sender, &chain)
        .expect("sign");
    let alone = describe(&tx);
    tx.sponsor_with::<MlDsa87>(&sponsor, U256::from(9_000_u64), 500, &chain)
        .expect("sponsor");

    let wire = Transaction::from_bytes(&tx.to_bytes()).expect("v13 decodes");
    assert_eq!(wire, tx, "the v13 frame round-trips");
    let d = describe(&wire);
    assert!(d.summary.starts_with(&alone.summary), "{}", d.summary);
    assert!(
        d.summary.contains("paid by sponsor") && d.summary.contains("9,000 base units"),
        "{}",
        d.summary
    );
    assert!(
        d.details
            .contains(&("sponsor", hex::encode(sponsor_address)))
    );
    assert!(
        d.details
            .contains(&("sponsor pays at most", "9,000 base units".into()))
    );
    assert!(d.details.contains(&("sponsored until block", "500".into())));
    assert_eq!(
        d.review, alone.review,
        "a sponsor never changes the review level"
    );

    // The API record the explorer and the gateway pass on carries the same words.
    let info = TransactionInfo::from(&wire);
    let sponsor = info.sponsor.clone().expect("sponsor reported");
    assert_eq!(sponsor.address, hex::encode(sponsor_address));
    assert_eq!(sponsor.max_fee, U256::from(9_000_u64));
    assert_eq!(sponsor.valid_until, 500);
    assert_eq!(info.kind, "mint_asset");
    assert!(
        info.details
            .iter()
            .any(|x| x.label == "asset" && x.value == "b2".repeat(32))
    );
    let json = serde_json::to_string(&info).unwrap();
    assert_eq!(
        serde_json::from_str::<TransactionInfo>(&json).unwrap(),
        info
    );
    // An unsponsored frame reports no sponsor, and an old record still parses.
    assert!(
        TransactionInfo::from(&tx_without_sponsor(&wire))
            .sponsor
            .is_none()
    );
    let mut old: Value = serde_json::from_str(&json).unwrap();
    old.as_object_mut()
        .unwrap()
        .retain(|k, _| k != "details" && k != "sponsor");
    let parsed: TransactionInfo = serde_json::from_value(old).unwrap();
    assert!(parsed.details.is_empty() && parsed.sponsor.is_none());
}

fn tx_without_sponsor(tx: &Transaction) -> Transaction {
    Transaction {
        sponsor: None,
        ..tx.clone()
    }
}
