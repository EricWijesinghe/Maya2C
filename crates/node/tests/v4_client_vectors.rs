//! Client vectors for the v4 transaction forms (ADR-135 I7/I8): one payload per
//! form, its exact bytes, and the words every wallet shows for it.
//!
//! The node is the source of truth. This test builds each form with the node's
//! own types, round-trips it through the node's codec, renders it with
//! `core::describe`, and compares the result with `spec/tests/v4-client-vectors.json`.
//! The Python, TypeScript and `sdk-ffi` tests read that same file, so an SDK
//! that encodes a byte differently, or words a form differently, fails its own
//! test against the node's output rather than drifting quietly.
//!
//! Besides the happy path, the file carries the edges: `decode_only` payloads
//! the node decodes but no canonical encoder writes (symbols outside `[A-Z0-9]`,
//! read the way `describe` reads them), `refused_payloads` the node's decoder
//! refuses, and `refused_fields` every SDK encoder must refuse.
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

fn vector(label: &str, kind: &TxKind, fields: Value) -> Value {
    let d = describe_kind(kind);
    json!({
        "label": label,
        "fields": fields,
        "payload": h(&payload(kind)),
        "summary": d.summary,
        "careful": d.review == Review::Careful,
        "details": d.details.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
    })
}

/// Symbol bytes the decoder accepts and no canonical encoder writes: UTF-8
/// outside `[A-Z0-9]`, invalid UTF-8, a NUL inside. Each is shown the way
/// `describe` shows it (UTF-8 lossy over all eight bytes, then trailing NULs
/// trimmed), so every SDK must read it the same way.
fn odd_symbols() -> Vec<[u8; 8]> {
    vec![
        *b"G\xc3\x96LD\0\0\0",
        *b"\xffAB\0\0\0\0\0",
        *b"\xe2\x82AB\0\0\0\0",
        *b"AB\0CD\0\0\0",
    ]
}

fn odd_symbol_kind(symbol: [u8; 8]) -> TxKind {
    TxKind::AssetMint(AssetMintAction::RegisterCapped {
        symbol,
        total_supply: U256::from(1_u64),
        cap: U256::from(2_u64),
        authority: [0xA1; 32],
    })
}

fn decode_only() -> Vec<Value> {
    odd_symbols()
        .into_iter()
        .map(|raw| {
            let shown = String::from_utf8_lossy(&raw)
                .trim_end_matches('\0')
                .to_owned();
            let fields = json!({"symbol": shown, "total_supply": "1", "cap": "2", "authority": h(&[0xA1; 32])});
            vector("register_capped_asset", &odd_symbol_kind(raw), fields)
        })
        .collect()
}

/// Payloads the node's decoder refuses, with why.
fn refused_payloads() -> Vec<(&'static str, Vec<u8>)> {
    let name_op = |op: u8| {
        let mut buf = payload(&TxKind::Name(NameAction::Update {
            label: b"alice".to_vec(),
            op: NameUpdate::SetReverse,
        }));
        *buf.last_mut().unwrap() = op;
        buf
    };
    let label_bytes = |label: &[u8]| {
        let mut buf = vec![65_u8, u8::try_from(label.len()).unwrap()];
        buf.extend_from_slice(label);
        buf.push(1);
        buf
    };
    vec![
        ("name op 0x00 does not exist", name_op(0x00)),
        ("name op 0x02 (stealth) is not in this build", name_op(0x02)),
        ("name op 0x05 (stealth) is not in this build", name_op(0x05)),
        ("an uppercase label", label_bytes(b"Alice")),
        ("a non-ASCII label", label_bytes("al\u{e9}x".as_bytes())),
    ]
}

/// Fields every SDK encoder must refuse rather than write some payload.
fn refused_fields() -> Value {
    let a = h(&[0xA1; 32]);
    json!([
        {"why": "an op that is a prototype key", "label": "update_name", "fields": {"label": "alice", "op": "constructor"}},
        {"why": "an op that is a prototype key", "label": "update_name", "fields": {"label": "alice", "op": "toString"}},
        {"why": "an op that is a prototype key", "label": "update_name", "fields": {"label": "alice", "op": "__proto__"}},
        {"why": "a stealth op", "label": "update_name", "fields": {"label": "alice", "op": "set_stealth"}},
        {"why": "a form that is a prototype key", "label": "constructor", "fields": {}},
        {"why": "a form that is a prototype key", "label": "hasOwnProperty", "fields": {}},
        {"why": "non-ASCII digits", "label": "burn_asset", "fields": {"asset": a, "amount": "\u{661}\u{662}"}},
        {"why": "a superscript digit", "label": "burn_asset", "fields": {"asset": a, "amount": "\u{b2}"}},
        {"why": "a sign", "label": "burn_asset", "fields": {"asset": a, "amount": "+1"}},
        {"why": "an empty integer", "label": "burn_asset", "fields": {"asset": a, "amount": ""}},
        {"why": "uppercase hex", "label": "renounce_mint", "fields": {"asset": "A1".repeat(32)}},
        {"why": "hex with spaces", "label": "renounce_mint", "fields": {"asset": format!("{} {}", "a1".repeat(16), "a1".repeat(16))}},
        {"why": "a NUL inside a symbol", "label": "register_capped_asset", "fields": {"symbol": "GO\u{0}LD", "total_supply": "1", "cap": "2", "authority": a}},
        {"why": "a non-ASCII symbol", "label": "register_capped_asset", "fields": {"symbol": "G\u{d6}LD", "total_supply": "1", "cap": "2", "authority": a}},
        {"why": "a lowercase symbol", "label": "register_capped_asset", "fields": {"symbol": "gold", "total_supply": "1", "cap": "2", "authority": a}},
        {"why": "a symbol of nine bytes", "label": "register_capped_asset", "fields": {"symbol": "GOLDGOLDG", "total_supply": "1", "cap": "2", "authority": a}},
    ])
}

fn vector_file() -> Value {
    let vectors: Vec<Value> = samples()
        .into_iter()
        .map(|(label, kind, fields)| vector(label, &kind, fields))
        .collect();
    let refused: Vec<Value> = refused_payloads()
        .into_iter()
        .map(|(why, bytes)| json!({"why": why, "payload": h(&bytes)}))
        .collect();
    json!({
        "about": "ADR-135 v4 transaction payloads (TxKind tag || fields), generated by crates/node/tests/v4_client_vectors.rs from the node's own codec and core::describe. Integers are decimal strings; bytes are lowercase hex; labels are ASCII; a symbol reads as UTF-8 (lossy) with trailing NULs trimmed. decode_only: payloads that decode but that no canonical encoder writes. refused_payloads: the node's decoder refuses them. refused_fields: every SDK encoder must refuse them. Regenerate with MAYA2C_WRITE_VECTORS=1; never edit by hand.",
        "vectors": vectors,
        "decode_only": decode_only(),
        "refused_payloads": refused,
        "refused_fields": refused_fields(),
    })
}

#[test]
fn the_edge_vectors_hold_against_the_node() {
    for raw in odd_symbols() {
        let kind = odd_symbol_kind(raw);
        assert_eq!(decode(&payload(&kind)), kind, "decodes as written");
        assert!(
            custom_l1_node::state::asset::validate_symbol(&raw).is_err(),
            "{raw:?} must not be a symbol a canonical encoder writes"
        );
    }
    for (why, bytes) in refused_payloads() {
        let mut reader = ByteReader::new(&bytes);
        let read = TxKind::decode(&mut reader).and_then(|_| reader.finish());
        assert!(read.is_err(), "{why}: the decoder must refuse it");
    }
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
