//! The production node against `spec/tests/names.json` (rules NM-1 and NM-2,
//! ADR-135 P-3). The vectors are hand-laid from the spec's byte layouts, not
//! produced by this crate. NOT VERIFIED until CI runs this file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use custom_l1_node::core::TxKind;
use custom_l1_node::core::codec::ByteReader;
use serde_json::Value;

fn load() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests/names.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("names.json")).unwrap()
}

#[test]
fn every_name_form_matches_its_vector() {
    let doc = load();
    let cases = doc["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
        let mut reader = ByteReader::new(&bytes);
        let decoded = TxKind::decode(&mut reader).and_then(|kind| reader.finish().map(|()| kind));
        if case["expect"]["result"] == "error" {
            assert!(decoded.is_err(), "{id}: decoded a form the rules refuse");
            continue;
        }
        let kind = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(matches!(kind, TxKind::Name(_)), "{id}");
        let mut again = Vec::new();
        kind.encode_into(&mut again);
        assert_eq!(again, bytes, "{id}: re-encoding differs");
        let size: usize = case["size"].as_str().unwrap().parse().unwrap();
        assert_eq!(bytes.len(), size, "{id}");
    }
}
