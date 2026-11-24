//! The upgrade record has room to grow (ADR-155, G21 and NEW-3): an upgrade
//! names the hash of the release that implements it, and the feature set is
//! 128 bits wide, so features past bit 63 need no new record kind.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::governance_payload::ScheduleUpgrade;
use custom_l1_node::state::StateDB;
use custom_l1_node::state::upgrades::version_key;
use custom_l1_node::upgrade::{FeatureSet, ProtocolUpgrade};
use tempfile::TempDir;

mod common;

const RELEASE: [u8; 32] = [0x5a; 32];

#[test]
fn a_feature_set_holds_bits_past_sixty_three() {
    let wide = FeatureSet(FeatureSet::DVP | 1 << 100);
    assert!(wide.contains(1 << 100));
    assert!(!wide.is_known(), "bit 100 names no feature in this build");
    assert_eq!(wide.unimplemented(), 1 << 100);
}

#[test]
fn a_scheduled_upgrade_names_its_release_and_round_trips() {
    let upgrade = ScheduleUpgrade {
        version: 3,
        activation_height: 1_000_000,
        features: FeatureSet(FeatureSet::DVP | 1 << 127),
        binary_hash: RELEASE,
        sunsets: vec![],
    };
    let mut bytes = Vec::new();
    upgrade.encode_into(&mut bytes);
    // version, height, 16-byte features, 32-byte release hash, sunset count.
    assert_eq!(bytes.len(), 4 + 8 + 16 + 32 + 8);
    let mut reader = ByteReader::new(&bytes);
    assert_eq!(ScheduleUpgrade::decode(&mut reader).unwrap(), upgrade);
    reader.finish().unwrap();
}

#[test]
fn a_genesis_upgrade_reads_its_release_hash_as_hex_and_defaults_to_none() {
    let named: ProtocolUpgrade = serde_json::from_str(&format!(
        r#"{{"version": 2, "activation_height": 10, "features": 1, "binary_hash": "{}"}}"#,
        hex::encode(RELEASE)
    ))
    .unwrap();
    assert_eq!(named.binary_hash, RELEASE);
    assert_eq!(named.features, FeatureSet(FeatureSet::DVP));
    let unnamed: ProtocolUpgrade =
        serde_json::from_str(r#"{"version": 2, "activation_height": 10}"#).unwrap();
    assert_eq!(unnamed.binary_hash, [0; 32], "zero: no release named");
    let wide: ProtocolUpgrade = serde_json::from_str(
        r#"{"version": 2, "activation_height": 10, "features": 340282366920938463463374607431768211455}"#,
    )
    .unwrap();
    assert_eq!(wide.features, FeatureSet(u128::MAX));
}

#[test]
fn a_stored_record_round_trips_and_one_of_another_layout_is_refused() {
    let entry = ProtocolUpgrade {
        version: 2,
        activation_height: 500,
        features: FeatureSet(1 << 90),
        binary_hash: RELEASE,
    };
    let dir = TempDir::new().unwrap();
    let state = StateDB::open(dir.path()).unwrap();
    let record = common::version_record(&entry);
    assert_eq!(record.len(), 61, "layout, version, height, features, hash");
    state.raw_put_for_test(&version_key(2), &record).unwrap();
    assert_eq!(
        state.effective_schedule().unwrap().upgrades.entries(),
        &[entry],
        "the release hash and the high bit survive the store"
    );
    let mut other_layout = record;
    other_layout[0] = 2;
    state
        .raw_put_for_test(&version_key(2), &other_layout)
        .unwrap();
    assert!(state.effective_schedule().is_err());
}
