//! The FFI describe calls give the node's words for every v4 form and name the
//! sponsor of a wire-v13 frame (ADR-135 I7).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::{ChainTag, Transaction};
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite, SuiteId, suite_address};
use maya_sdk_ffi::{SdkError, describe_payload, describe_transaction};
use primitive_types::U256;
use serde_json::Value;

const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../spec/tests/v4-client-vectors.json"
);

fn vectors() -> Vec<Value> {
    let text = std::fs::read_to_string(VECTORS).expect("vectors");
    serde_json::from_str::<Value>(&text).unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn every_v4_vector_is_described_as_the_node_describes_it() {
    let all = vectors();
    assert_eq!(all.len(), 16);
    for v in all {
        let payload = hex::decode(v["payload"].as_str().unwrap()).unwrap();
        let d = describe_payload(payload.clone()).expect("decodes");
        assert_eq!(d.kind, v["label"].as_str().unwrap());
        assert_eq!(d.summary, v["summary"].as_str().unwrap());
        assert_eq!(d.careful, v["careful"].as_bool().unwrap());
        let details: Vec<(String, String)> =
            d.details.into_iter().map(|x| (x.label, x.value)).collect();
        let want: Vec<(String, String)> = v["details"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into()))
            .collect();
        assert_eq!(details, want, "{}", d.kind);
        assert!(d.sponsor.is_none());
        // Trailing bytes are refused, never ignored.
        let mut long = payload;
        long.push(0);
        assert!(matches!(
            describe_payload(long),
            Err(SdkError::Malformed { .. })
        ));
    }
}

#[test]
fn a_sponsored_frame_names_its_sponsor() {
    let chain = ChainTag::from_genesis([0x5A; 32]);
    let sender = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([1; 32]));
    let sponsor = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([2; 32]));
    let payer = suite_address(SuiteId::MlDsa87, &MlDsa87::public_key(&sponsor));
    let payload = hex::decode(vectors()[1]["payload"].as_str().unwrap()).unwrap();
    let kind = custom_l1_node::core::TxKind::decode(
        &mut custom_l1_node::core::codec::ByteReader::new(&payload),
    )
    .unwrap();
    let mut tx = Transaction::with_kind(kind, 0);
    tx.sign_with_suite::<MlDsa87>(&sender, &chain).unwrap();
    tx.sponsor_with::<MlDsa87>(&sponsor, U256::from(9_000), 77, &chain)
        .unwrap();

    let d = describe_transaction(tx.to_bytes()).expect("v13 decodes");
    assert_eq!(d.kind, "mint_asset");
    assert_eq!(d.sponsor, Some(hex::encode(payer)));
    assert!(d.summary.contains("paid by sponsor"), "{}", d.summary);
    assert!(matches!(
        describe_transaction(vec![0x0D, 0, 0]),
        Err(SdkError::Malformed { .. })
    ));
}
