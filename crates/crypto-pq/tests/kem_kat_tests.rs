//! Known answers for the KEMs ACVP does not cover — ADR-009.
//!
//! - **HQC-128 / HQC-256 (draft).** The reference implementation's KATs
//!   (`tests/vectors/hqc/`, provenance in `tests/vectors/SOURCES.md`). The
//!   reference draws its randomness from a SHAKE256 stream over the vector's
//!   seed; this reads the same stream and feeds it to the *seeded* entry
//!   points `kem_suite::Hqc*` call — 32 bytes of key seed, then `k` bytes of
//!   message and 16 of salt — so the vectors pin the code path the suite uses,
//!   not a neighbouring one.
//! - **X-Wing.** The draft's own vectors, through the same seeded entry
//!   points `kem_suite::XWing` calls.
//!
//! ML-KEM's known answers are NIST's, in `acvp_tests.rs`.

use hqc_kem::{HqcKem, HqcParams};
use shake::{ExtendableOutput as _, Update as _, XofReader as _};
use x_wing::{Decapsulate as _, Decapsulator as _, KeyExport as _};

fn vectors_dir() -> String {
    format!("{}/tests/vectors", env!("CARGO_MANIFEST_DIR"))
}

struct Kat {
    count: usize,
    seed: Vec<u8>,
    pk: Vec<u8>,
    sk: Vec<u8>,
    ct: Vec<u8>,
    ss: Vec<u8>,
}

fn parse_rsp(text: &str) -> Vec<Kat> {
    let mut out = Vec::new();
    let mut fields = std::collections::BTreeMap::new();
    let flush = |fields: &mut std::collections::BTreeMap<String, String>, out: &mut Vec<Kat>| {
        if fields.is_empty() {
            return;
        }
        let get = |k: &str| hex::decode(&fields[k]).expect("hex");
        out.push(Kat {
            count: fields["count"].parse().expect("count"),
            seed: get("seed"),
            pk: get("pk"),
            sk: get("sk"),
            ct: get("ct"),
            ss: get("ss"),
        });
        fields.clear();
    };
    for line in text.lines() {
        if line.starts_with("count = ") {
            flush(&mut fields, &mut out);
        }
        if let Some((k, v)) = line.split_once(" = ") {
            fields.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }
    flush(&mut fields, &mut out);
    out
}

/// The reference KAT PRNG: SHAKE256(seed ‖ 0x00).
fn kat_stream(seed: &[u8]) -> shake::Shake256Reader {
    let mut hasher = shake::Shake256::default();
    hasher.update(seed);
    hasher.update(&[0x00]);
    hasher.finalize_xof()
}

fn hqc_kats<P: HqcParams>(file: &str) -> usize {
    let text = std::fs::read_to_string(format!("{}/hqc/{file}", vectors_dir())).expect("rsp");
    let kats = parse_rsp(&text);
    for kat in &kats {
        let mut stream = kat_stream(&kat.seed);
        let mut key_seed = [0u8; 32];
        stream.read(&mut key_seed);
        let (ek, dk) = HqcKem::<P>::generate_key_deterministic(&key_seed);
        assert_eq!(
            ek.as_ref(),
            kat.pk.as_slice(),
            "{file} pk, count {}",
            kat.count
        );
        assert_eq!(
            dk.as_ref(),
            kat.sk.as_slice(),
            "{file} sk, count {}",
            kat.count
        );

        let mut message = vec![0u8; P::params().k];
        let mut salt = [0u8; 16];
        stream.read(&mut message);
        stream.read(&mut salt);
        let (ct, ss) = ek
            .encapsulate_deterministic(&message, &salt)
            .expect("encaps");
        assert_eq!(
            ct.as_ref(),
            kat.ct.as_slice(),
            "{file} ct, count {}",
            kat.count
        );
        assert_eq!(
            ss.as_ref(),
            kat.ss.as_slice(),
            "{file} ss, count {}",
            kat.count
        );
        assert_eq!(
            dk.decapsulate(&ct).as_ref(),
            kat.ss.as_slice(),
            "{file} decaps"
        );
    }
    kats.len()
}

#[test]
fn hqc_128_matches_the_reference_kats() {
    assert_eq!(hqc_kats::<hqc_kem::Hqc128Params>("hqc-1.rsp"), 10);
}

#[test]
fn hqc_256_matches_the_reference_kats() {
    assert_eq!(hqc_kats::<hqc_kem::Hqc256Params>("hqc-5.rsp"), 10);
}

#[test]
fn x_wing_matches_the_draft_vectors() {
    let text = std::fs::read_to_string(format!("{}/xwing/test-vectors.json", vectors_dir()))
        .expect("json");
    let vectors: Vec<serde_json::Value> = serde_json::from_str(&text).expect("parse");
    let field = |v: &serde_json::Value, k: &str| hex::decode(v[k].as_str().expect(k)).expect("hex");
    for v in &vectors {
        let seed: [u8; 32] = field(v, "seed")[..32].try_into().expect("32-byte seed");
        let dk = x_wing::DecapsulationKey::from(seed);
        assert_eq!(dk.as_bytes().as_slice(), field(v, "sk").as_slice());
        let ek = dk.encapsulation_key();
        assert_eq!(ek.to_bytes().as_slice(), field(v, "pk").as_slice());

        let eseed: [u8; 64] = field(v, "eseed").try_into().expect("64-byte eseed");
        let (ct, ss) = ek.encapsulate_deterministic(&eseed.into());
        assert_eq!(ct.as_slice(), field(v, "ct").as_slice());
        assert_eq!(ss.as_slice(), field(v, "ss").as_slice());
        assert_eq!(dk.decapsulate(&ct).as_slice(), field(v, "ss").as_slice());
    }
    assert!(vectors.len() >= 3, "only {} X-Wing vectors", vectors.len());
}
