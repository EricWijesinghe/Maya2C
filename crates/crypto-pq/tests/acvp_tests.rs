//! NIST ACVP known-answer tests for every FIPS parameter set the suites and
//! KEMs compile in — ADR-007, ADR-009.
//!
//! Round-trips prove a scheme agrees with itself; these prove it agrees with
//! NIST. The vectors are a trimmed copy of `usnistgov/ACVP-Server`'s
//! `internalProjection.json` files (which carry the expected answers), at the
//! commit named in `tests/vectors/acvp/MANIFEST.txt` with the SHA-256 of each
//! untrimmed upstream file. Trimming kept only our parameter sets, dropped
//! HashML-DSA / HashSLH-DSA (`preHash`) and external-μ groups (no suite uses
//! them), and capped cases per group.
//!
//! Signature *verification* runs through `maya_crypto_pq::suite::
//! verify_with_context`, the crate's own entry point. Key generation and
//! signing run against the pinned backends in the exact modes the suites call
//! (seeded keygen, deterministic and hedged signing, pure and internal
//! interfaces), because ACVP hands over expanded secret keys that no suite
//! API accepts.

#![allow(deprecated)] // ACVP supplies expanded keys; the backends deprecate that form.

use maya_crypto_pq::suite::{self, SuiteId};
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/tests/vectors/acvp/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).expect("vector JSON")
}

fn hex(v: &Value, field: &str) -> Vec<u8> {
    let s = v[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing field {field}"));
    hex::decode(s).expect("hex")
}

/// Every group of `file` whose parameter set is `ps`, with its tests.
fn groups<'a>(file: &'a Value, ps: &str) -> Vec<&'a Value> {
    file["testGroups"]
        .as_array()
        .expect("testGroups")
        .iter()
        .filter(|g| g["parameterSet"] == ps)
        .collect()
}

fn cases(group: &Value) -> &Vec<Value> {
    group["tests"].as_array().expect("tests")
}

/// FIPS 204/205 external "pure" framing: `0 ‖ len(ctx) ‖ ctx ‖ M`.
fn pure_prefix(ctx: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8, u8::try_from(ctx.len()).expect("ctx ≤ 255")];
    out.extend_from_slice(ctx);
    out
}

// ------------------------------------------------------------------ ML-DSA

fn ml_dsa_keygen<P: ml_dsa::MlDsaParams>(ps: &str) -> usize {
    let file = load("ML-DSA-keyGen-FIPS204.json");
    let mut n = 0;
    for g in groups(&file, ps) {
        for t in cases(g) {
            let seed: [u8; 32] = hex(t, "seed").try_into().expect("32-byte seed");
            let key = ml_dsa::SigningKey::<P>::from_seed(&seed.into());
            let expanded = key.expanded_key();
            assert_eq!(
                expanded.verifying_key().encode().as_slice(),
                hex(t, "pk"),
                "{ps} tc {}",
                t["tcId"]
            );
            assert_eq!(
                expanded.to_expanded().as_slice(),
                hex(t, "sk"),
                "{ps} tc {}",
                t["tcId"]
            );
            n += 1;
        }
    }
    n
}

fn ml_dsa_siggen<P: ml_dsa::MlDsaParams>(ps: &str) -> usize {
    let file = load("ML-DSA-sigGen-FIPS204.json");
    let mut n = 0;
    for g in groups(&file, ps) {
        let external = g["signatureInterface"] == "external";
        let deterministic = g["deterministic"].as_bool().expect("deterministic");
        for t in cases(g) {
            let sk_bytes = hex(t, "sk");
            let sk = ml_dsa::ExpandedSigningKey::<P>::from_expanded(
                sk_bytes.as_slice().try_into().expect("expanded sk length"),
            );
            let message = hex(t, "message");
            let rnd: [u8; 32] = if deterministic {
                [0u8; 32]
            } else {
                hex(t, "rnd").try_into().expect("32-byte rnd")
            };
            let prefix = if external {
                pure_prefix(&hex(t, "context"))
            } else {
                Vec::new()
            };
            let sig = sk.sign_internal(&[&prefix, &message], &rnd.into());
            assert_eq!(
                sig.encode().as_slice(),
                hex(t, "signature"),
                "{ps} tc {}",
                t["tcId"]
            );
            n += 1;
        }
    }
    n
}

fn ml_dsa_sigver(id: SuiteId, ps: &str) -> usize {
    let file = load("ML-DSA-sigVer-FIPS204.json");
    let mut n = 0;
    for g in groups(&file, ps)
        .into_iter()
        .filter(|g| g["signatureInterface"] == "external")
    {
        for t in cases(g) {
            let got = suite::verify_with_context(
                id,
                &hex(t, "pk"),
                &hex(t, "message"),
                &hex(t, "context"),
                &hex(t, "signature"),
            );
            let expected = t["testPassed"].as_bool().expect("testPassed");
            assert_eq!(
                got.is_ok(),
                expected,
                "{ps} tc {} ({})",
                t["tcId"],
                t["reason"]
            );
            n += 1;
        }
    }
    n
}

#[test]
fn ml_dsa_65_matches_nist() {
    let n = ml_dsa_keygen::<ml_dsa::MlDsa65>("ML-DSA-65")
        + ml_dsa_siggen::<ml_dsa::MlDsa65>("ML-DSA-65")
        + ml_dsa_sigver(SuiteId::MlDsa65, "ML-DSA-65");
    assert!(n >= 40, "only {n} ML-DSA-65 cases ran");
}

#[test]
fn ml_dsa_87_matches_nist() {
    let n = ml_dsa_keygen::<ml_dsa::MlDsa87>("ML-DSA-87")
        + ml_dsa_siggen::<ml_dsa::MlDsa87>("ML-DSA-87")
        + ml_dsa_sigver(SuiteId::MlDsa87, "ML-DSA-87");
    assert!(n >= 40, "only {n} ML-DSA-87 cases ran");
}

// ----------------------------------------------------------------- SLH-DSA

fn slh_keygen<P: slh_dsa::ParameterSet>(ps: &str) -> usize {
    let file = load("SLH-DSA-keyGen-FIPS205.json");
    let mut n = 0;
    for g in groups(&file, ps) {
        for t in cases(g) {
            let sk = slh_dsa::SigningKey::<P>::slh_keygen_internal(
                &hex(t, "skSeed"),
                &hex(t, "skPrf"),
                &hex(t, "pkSeed"),
            );
            assert_eq!(sk.to_vec(), hex(t, "sk"), "{ps} tc {}", t["tcId"]);
            assert_eq!(sk.as_ref().to_vec(), hex(t, "pk"), "{ps} tc {}", t["tcId"]);
            n += 1;
        }
    }
    n
}

fn slh_siggen<P: slh_dsa::ParameterSet>(ps: &str) -> usize {
    let file = load("SLH-DSA-sigGen-FIPS205.json");
    let mut n = 0;
    for g in groups(&file, ps) {
        let external = g["signatureInterface"] == "external";
        let deterministic = g["deterministic"].as_bool().expect("deterministic");
        for t in cases(g) {
            let sk = slh_dsa::SigningKey::<P>::try_from(hex(t, "sk").as_slice()).expect("sk");
            let message = hex(t, "message");
            let opt_rand = (!deterministic).then(|| hex(t, "additionalRandomness"));
            let prefix = if external {
                pure_prefix(&hex(t, "context"))
            } else {
                Vec::new()
            };
            let sig = sk.slh_sign_internal(&[&prefix, &message], opt_rand.as_deref());
            assert_eq!(sig.to_vec(), hex(t, "signature"), "{ps} tc {}", t["tcId"]);
            n += 1;
        }
    }
    n
}

fn slh_sigver(id: SuiteId, ps: &str) -> usize {
    let file = load("SLH-DSA-sigVer-FIPS205.json");
    let mut n = 0;
    for g in groups(&file, ps)
        .into_iter()
        .filter(|g| g["signatureInterface"] == "external")
    {
        for t in cases(g) {
            let got = suite::verify_with_context(
                id,
                &hex(t, "pk"),
                &hex(t, "message"),
                &hex(t, "context"),
                &hex(t, "signature"),
            );
            let expected = t["testPassed"].as_bool().expect("testPassed");
            // A wrong-length signature is a legitimate NIST negative; the
            // suite reports it as a length error rather than a verification
            // failure, and either is a rejection.
            assert_eq!(
                got.is_ok(),
                expected,
                "{ps} tc {} ({})",
                t["tcId"],
                t["reason"]
            );
            n += 1;
        }
    }
    n
}

#[test]
fn slh_dsa_sha2_128s_matches_nist() {
    let n = slh_keygen::<slh_dsa::Sha2_128s>("SLH-DSA-SHA2-128s")
        + slh_siggen::<slh_dsa::Sha2_128s>("SLH-DSA-SHA2-128s")
        + slh_sigver(SuiteId::SlhDsaSha2_128s, "SLH-DSA-SHA2-128s");
    assert!(n >= 30, "only {n} SLH-DSA-SHA2-128s cases ran");
}

#[test]
fn slh_dsa_shake_256f_matches_nist() {
    let n = slh_keygen::<slh_dsa::Shake256f>("SLH-DSA-SHAKE-256f")
        + slh_siggen::<slh_dsa::Shake256f>("SLH-DSA-SHAKE-256f")
        + slh_sigver(SuiteId::SlhDsaShake256f, "SLH-DSA-SHAKE-256f");
    assert!(n >= 20, "only {n} SLH-DSA-SHAKE-256f cases ran");
}

// ------------------------------------------------------------------ ML-KEM

/// A macro rather than a generic function: `ml-kem`'s parameter traits live
/// in a private module, so the bounds cannot be named from outside the crate.
macro_rules! ml_kem_acvp {
    ($P:ty, $ps:literal) => {{
        use ml_kem::ExpandedKeyEncoding as _;
        use ml_kem::kem::{Decapsulate as _, KeyExport as _};
        type P = $P;
        let ps = $ps;

        let mut n = 0;
        let keygen = load("ML-KEM-keyGen-FIPS203.json");
        for g in groups(&keygen, ps) {
            for t in cases(g) {
                let mut seed = hex(t, "d");
                seed.extend_from_slice(&hex(t, "z"));
                let dk = ml_kem::DecapsulationKey::<P>::from_seed(
                    seed.as_slice().try_into().expect("64"),
                );
                assert_eq!(
                    dk.encapsulation_key().to_bytes().as_slice(),
                    hex(t, "ek"),
                    "{ps} tc {}",
                    t["tcId"]
                );
                assert_eq!(
                    dk.to_expanded_bytes().as_slice(),
                    hex(t, "dk"),
                    "{ps} tc {}",
                    t["tcId"]
                );
                n += 1;
            }
        }

        let ed = load("ML-KEM-encapDecap-FIPS203.json");
        for g in groups(&ed, ps) {
            for t in cases(g) {
                match g["function"].as_str().expect("function") {
                    "encapsulation" => {
                        let ek = ml_kem::EncapsulationKey::<P>::new(
                            hex(t, "ek").as_slice().try_into().expect("ek len"),
                        )
                        .expect("valid ek");
                        let m: [u8; 32] = hex(t, "m").try_into().expect("m");
                        let (c, k) = ek.encapsulate_deterministic(&m.into());
                        assert_eq!(c.as_slice(), hex(t, "c"), "{ps} tc {}", t["tcId"]);
                        assert_eq!(k.as_slice(), hex(t, "k"), "{ps} tc {}", t["tcId"]);
                    }
                    "decapsulation" => {
                        let dk = ml_kem::DecapsulationKey::<P>::from_expanded_bytes(
                            hex(t, "dk").as_slice().try_into().expect("dk len"),
                        )
                        .expect("valid dk");
                        let k = dk.decapsulate(hex(t, "c").as_slice().try_into().expect("c len"));
                        assert_eq!(k.as_slice(), hex(t, "k"), "{ps} tc {}", t["tcId"]);
                    }
                    "encapsulationKeyCheck" => {
                        let bytes = hex(t, "ek");
                        let ok = bytes
                            .as_slice()
                            .try_into()
                            .ok()
                            .is_some_and(|b| ml_kem::EncapsulationKey::<P>::new(b).is_ok());
                        assert_eq!(
                            ok,
                            t["testPassed"].as_bool().expect("testPassed"),
                            "{ps} tc {}",
                            t["tcId"]
                        );
                    }
                    "decapsulationKeyCheck" => {
                        let bytes = hex(t, "dk");
                        let ok = bytes.as_slice().try_into().ok().is_some_and(|b| {
                            ml_kem::DecapsulationKey::<P>::from_expanded_bytes(b).is_ok()
                        });
                        assert_eq!(
                            ok,
                            t["testPassed"].as_bool().expect("testPassed"),
                            "{ps} tc {}",
                            t["tcId"]
                        );
                    }
                    other => panic!("unexpected ML-KEM function {other}"),
                }
                n += 1;
            }
        }
        n
    }};
}

#[test]
fn ml_kem_768_matches_nist() {
    let n: usize = ml_kem_acvp!(ml_kem::MlKem768, "ML-KEM-768");
    assert!(n >= 50, "only {n} ML-KEM-768 cases ran");
}

#[test]
fn ml_kem_1024_matches_nist() {
    let n: usize = ml_kem_acvp!(ml_kem::MlKem1024, "ML-KEM-1024");
    assert!(n >= 50, "only {n} ML-KEM-1024 cases ran");
}
