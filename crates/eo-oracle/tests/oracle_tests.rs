//! The earth-observation oracle (Master Prompt 6 §8) on real Sentinel-2 L2A
//! pixels: one field in California's Central Valley, June 2025, from three
//! satellites (2A, 2B, 2C), fetched by `eo-fetch` and committed as a fixture.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};
use maya_eo_oracle::{OracleError, Policy, Report, Round, data_commitment, field_ndvi};
use serde_json::Value;

const FIELD: [u8; 32] = [0xF1; 32];
const WINDOW: (u64, u64, u64) = (100, 200, 300);

struct Scene {
    id: String,
    red: Vec<u16>,
    nir: Vec<u16>,
}

fn scenes() -> Vec<Scene> {
    let doc: Value = serde_json::from_str(include_str!("fixtures/field.json")).unwrap();
    let band = |s: &Value, k: &str| -> Vec<u16> {
        s[k].as_array()
            .unwrap()
            .iter()
            .map(|v| u16::try_from(v.as_u64().unwrap()).unwrap())
            .collect()
    };
    doc["scenes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| Scene {
            id: s["id"].as_str().unwrap().to_owned(),
            red: band(s, "red"),
            nir: band(s, "nir"),
        })
        .collect()
}

struct Reporter {
    id: [u8; 32],
    key: <MlDsa65 as SignatureSuite>::SigningKey,
}

fn reporters(n: u8) -> (Vec<Reporter>, BTreeMap<[u8; 32], Vec<u8>>) {
    let rs: Vec<Reporter> = (0..n)
        .map(|i| Reporter {
            id: [i + 1; 32],
            key: MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([i + 40; 32])),
        })
        .collect();
    let registry = rs
        .iter()
        .map(|r| (r.id, MlDsa65::public_key(&r.key)))
        .collect();
    (rs, registry)
}

fn report(scene: &Scene, ndvi_bps: i32) -> Report {
    Report {
        round: 7,
        field: FIELD,
        ndvi_bps,
        data: data_commitment(&scene.red, &scene.nir),
        scene: scene.id.clone(),
    }
}

#[test]
fn real_sentinel_2_pixels_give_a_green_field_and_three_satellites_agree() {
    let values: Vec<i32> = scenes()
        .iter()
        .map(|s| field_ndvi(&s.red, &s.nir).unwrap())
        .collect();
    println!("field NDVI (bps) from Sentinel-2C, 2B, 2A, June 2025: {values:?}");
    // Dense irrigated vegetation, and three satellites within 0.1 of each other.
    assert!(
        values.iter().all(|v| (8_000..9_200).contains(v)),
        "{values:?}"
    );
    assert!(values.iter().max().unwrap() - values.iter().min().unwrap() < 1_000);
}

#[test]
fn a_round_finalizes_at_the_median_and_a_lying_reporter_is_struck() {
    let scenes = scenes();
    let (rs, registry) = reporters(4);
    let mut round = Round::new(7, FIELD, WINDOW, registry);
    let honest: Vec<i32> = scenes
        .iter()
        .map(|s| field_ndvi(&s.red, &s.nir).unwrap())
        .collect();
    for (r, (scene, v)) in rs.iter().zip(scenes.iter().zip(&honest)) {
        let rep = report(scene, *v);
        round
            .submit(r.id, rep.clone(), &rep.sign(&r.key).unwrap(), 150)
            .unwrap();
    }
    // The fourth reporter claims drought on the same real pixels.
    let lie = report(&scenes[0], 3_000);
    round
        .submit(rs[3].id, lie.clone(), &lie.sign(&rs[3].key).unwrap(), 150)
        .unwrap();
    assert!(
        matches!(round.finalize(250), Err(OracleError::Phase(_))),
        "not before the window closes"
    );

    // Disputes open the committed pixels; wrong pixels are not evidence.
    assert_eq!(
        round.open(rs[3].id, &scenes[1].red, &scenes[1].nir, 250),
        Err(OracleError::NotTheCommittedData)
    );
    assert_eq!(
        round.open(rs[3].id, &scenes[0].red, &scenes[0].nir, 250),
        Ok(true),
        "the lie is struck"
    );
    assert_eq!(
        round.open(rs[0].id, &scenes[0].red, &scenes[0].nir, 250),
        Ok(false),
        "the honest report stands"
    );
    assert_eq!(
        round.open(rs[0].id, &scenes[0].red, &scenes[0].nir, 251),
        Ok(false),
        "opening again changes nothing"
    );
    for (r, scene) in rs.iter().zip(&scenes).skip(1) {
        assert_eq!(round.open(r.id, &scene.red, &scene.nir, 260), Ok(false));
    }
    assert!(
        matches!(
            round.open(rs[1].id, &scenes[1].red, &scenes[1].nir, 300),
            Err(OracleError::Phase(_))
        ),
        "not after the window"
    );

    let mut sorted = honest.clone();
    sorted.sort_unstable();
    let final_ndvi = round.finalize(300).unwrap();
    assert_eq!(
        final_ndvi, sorted[1],
        "the median of the three honest reports"
    );
    println!(
        "final NDVI {final_ndvi} bps from {:?}; struck {}",
        honest,
        round.struck().len()
    );

    // Parametric insurance: a drought trigger does not pay on a green field;
    // a trigger above the observed value does.
    assert_eq!(
        Policy {
            field: FIELD,
            trigger_bps: 5_000,
            payout: 1_000
        }
        .settle(final_ndvi),
        0
    );
    assert_eq!(
        Policy {
            field: FIELD,
            trigger_bps: final_ndvi + 1,
            payout: 1_000
        }
        .settle(final_ndvi),
        1_000
    );
}

#[test]
fn signatures_phases_and_source_counts_are_enforced() {
    let scenes = scenes();
    let (rs, registry) = reporters(3);
    let mut round = Round::new(7, FIELD, WINDOW, registry);
    let rep = report(
        &scenes[0],
        field_ndvi(&scenes[0].red, &scenes[0].nir).unwrap(),
    );
    let sig = rep.sign(&rs[0].key).unwrap();
    assert_eq!(
        round.submit(rs[0].id, rep.clone(), &sig, 50),
        Err(OracleError::Phase(
            "reports are accepted only while the round is open"
        ))
    );
    assert_eq!(
        round.submit(rs[1].id, rep.clone(), &sig, 150),
        Err(OracleError::BadSignature),
        "another reporter's signature"
    );
    assert_eq!(
        round.submit([9; 32], rep.clone(), &sig, 150),
        Err(OracleError::UnknownReporter)
    );
    round.submit(rs[0].id, rep.clone(), &sig, 150).unwrap();
    assert_eq!(
        round.submit(rs[0].id, rep, &sig, 151),
        Err(OracleError::Duplicate)
    );
    assert_eq!(round.finalize(300), Err(OracleError::TooFewSources(0)));
}

#[test]
fn a_commitment_nobody_opens_counts_for_nothing() {
    // Three reporters commit; only two open. A report resting on pixels that
    // do not exist cannot be disputed, so it must not count at all.
    let scenes = scenes();
    let (rs, registry) = reporters(3);
    let mut round = Round::new(7, FIELD, WINDOW, registry);
    for (r, scene) in rs.iter().zip(&scenes) {
        let rep = report(scene, field_ndvi(&scene.red, &scene.nir).unwrap());
        round
            .submit(r.id, rep.clone(), &rep.sign(&r.key).unwrap(), 150)
            .unwrap();
    }
    for (r, scene) in rs.iter().zip(&scenes).take(2) {
        round.open(r.id, &scene.red, &scene.nir, 250).unwrap();
    }
    assert_eq!(round.finalize(300), Err(OracleError::TooFewSources(2)));
}
