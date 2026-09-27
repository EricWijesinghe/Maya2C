//! Re-sealing archive roots (`docs/resealing.md` Procedure A; Master Prompt 28 §3).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use maya_archive::reseal::{
    Evidence, MAX_LAYERS, Reseal, ResealDue, Resealer, Trust, outer_suite, reseal_if_due, schedule,
    verify,
};
use maya_archive::seal::{EpochSigner, KeyChain, Seal};
use maya_crypto_pq::agility::SuitePolicy;
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SlhDsaSha2_128s, SuiteId};

#[test]
fn layers_nest_verify_and_survive_a_distrusted_generation() {
    let root = b"bafy...archive-root".to_vec();
    let sealer = EpochSigner::genesis(&MasterSeed::from_bytes([1; 32]));
    let chain = KeyChain::new(sealer.public_key().to_vec());
    let original = Evidence::Seal(sealer.seal(&root).unwrap());

    // SHAKE doubted: re-seal under SLH-DSA over SHA-2, then later under a
    // lattice suite.
    let sha2 = Resealer::<SlhDsaSha2_128s>::new(&MasterSeed::from_bytes([2; 32]));
    let lattice = Resealer::<MlDsa87>::new(&MasterSeed::from_bytes([3; 32]));
    let twice = lattice
        .reseal(sha2.reseal(original.clone()).unwrap())
        .unwrap();

    let mut trust = Trust {
        seals: Some(chain),
        resealers: BTreeMap::new(),
    };
    trust
        .resealers
        .insert(SuiteId::SlhDsaSha2_128s, sha2.public_key().to_vec());
    trust
        .resealers
        .insert(SuiteId::MlDsa87, lattice.public_key().to_vec());
    assert_eq!(
        verify(&twice, &root, &trust).unwrap(),
        vec![
            SuiteId::MlDsa87,
            SuiteId::SlhDsaSha2_128s,
            SuiteId::SlhDsaShake256f
        ]
    );

    // A verifier who no longer trusts the original SHAKE seal still
    // verifies through the newer layers.
    let distrusting = Trust {
        seals: None,
        ..trust.clone()
    };
    assert_eq!(
        verify(&twice, &root, &distrusting).unwrap(),
        vec![SuiteId::MlDsa87, SuiteId::SlhDsaSha2_128s]
    );

    // Evidence round-trips through its encoding, byte for byte.
    assert_eq!(Evidence::decode(&twice.encode()).unwrap(), twice);
    assert!(verify(&twice, b"another root", &trust).is_err());
}

#[test]
fn a_tampered_inner_layer_breaks_every_layer_above_it() {
    let root = b"root".to_vec();
    let sealer = EpochSigner::genesis(&MasterSeed::from_bytes([4; 32]));
    let chain = KeyChain::new(sealer.public_key().to_vec());
    let sha2 = Resealer::<SlhDsaSha2_128s>::new(&MasterSeed::from_bytes([5; 32]));
    let mut wrapped = sha2
        .reseal(Evidence::Seal(sealer.seal(&root).unwrap()))
        .unwrap();
    if let Evidence::Reseal(r) = &mut wrapped
        && let Evidence::Seal(s) = &mut r.inner
    {
        s.signature[0] ^= 1; // tamper with the original, keep the wrapper
    }
    let mut trust = Trust::default();
    trust
        .resealers
        .insert(SuiteId::SlhDsaSha2_128s, sha2.public_key().to_vec());
    assert!(
        verify(&wrapped, &root, &trust).is_err(),
        "the re-seal signed the untampered inner bytes"
    );
    trust.seals = Some(chain);
    assert!(verify(&wrapped, &root, &trust).is_err());
    assert!(
        verify(
            &Evidence::Seal(sealer.seal(&root).unwrap()),
            &root,
            &Trust::default()
        )
        .is_err(),
        "nothing trusted"
    );
}

#[test]
fn malformed_and_over_deep_evidence_is_refused() {
    let seal = Evidence::Seal(Seal {
        epoch: 0,
        root: vec![1],
        signature: vec![2],
    });
    let bytes = seal.encode();
    assert!(
        Evidence::decode(&bytes[..bytes.len() - 1]).is_err(),
        "truncated"
    );
    assert!(
        Evidence::decode(&[bytes.clone(), vec![0]].concat()).is_err(),
        "trailing"
    );
    assert!(Evidence::decode(&[9]).is_err(), "unknown tag");
    let mut deep = seal;
    for _ in 0..=MAX_LAYERS {
        deep = Evidence::Reseal(Box::new(Reseal {
            suite: SuiteId::MlDsa87,
            root: vec![1],
            inner: deep,
            signature: vec![],
        }));
    }
    assert!(Evidence::decode(&deep.encode()).is_err(), "nesting bound");
}

#[test]
fn a_deprecation_schedules_the_reseal_and_only_an_active_successor_takes_it() {
    use maya_crypto_pq::agility::Network;
    let root = b"root".to_vec();
    let sealer = EpochSigner::genesis(&MasterSeed::from_bytes([6; 32]));
    let original = Evidence::Seal(sealer.seal(&root).unwrap());
    let lattice = Resealer::<MlDsa87>::new(&MasterSeed::from_bytes([7; 32]));
    let policy = SuitePolicy::genesis(Network::Mainnet);
    assert_eq!(schedule(&original, &policy, 10), ResealDue::NotDue);
    let same = reseal_if_due(&lattice, original.clone(), &policy, 10).unwrap();
    assert_eq!(same, original, "nothing due, nothing added");

    // Governance deprecates SHAKE-256f at height 100, sunset at 100 + window.
    let doubted = policy
        .with_deprecation(SuiteId::SlhDsaShake256f, 100, 1_000_000, false)
        .unwrap();
    let sunset = doubted
        .deprecation(SuiteId::SlhDsaShake256f)
        .unwrap()
        .sunset_height;
    assert_eq!(schedule(&original, &doubted, 99), ResealDue::NotDue);
    assert_eq!(
        schedule(&original, &doubted, 100),
        ResealDue::Due {
            sunset_height: sunset
        }
    );
    assert_eq!(schedule(&original, &doubted, sunset), ResealDue::Overdue);

    let resealed = reseal_if_due(&lattice, original.clone(), &doubted, 150).unwrap();
    assert_eq!(outer_suite(&resealed), SuiteId::MlDsa87);
    assert_eq!(schedule(&resealed, &doubted, sunset), ResealDue::NotDue);

    // A successor under the doubted suite itself is refused.
    let shake =
        Resealer::<maya_crypto_pq::suite::SlhDsaShake256f>::new(&MasterSeed::from_bytes([8; 32]));
    assert!(reseal_if_due(&shake, original, &doubted, 150).is_err());
}

#[test]
fn padding_inside_a_layer_is_refused_and_layers_are_capped_on_write() {
    let root = b"root".to_vec();
    let sealer = EpochSigner::genesis(&MasterSeed::from_bytes([10; 32]));
    let lattice = Resealer::<MlDsa87>::new(&MasterSeed::from_bytes([11; 32]));
    let original = Evidence::Seal(sealer.seal(&root).unwrap());
    let wrapped = lattice.reseal(original.clone()).unwrap();

    // Re-encode by hand with two bytes of padding after the inner evidence,
    // inside its length-prefixed field.
    let Evidence::Reseal(r) = &wrapped else {
        unreachable!()
    };
    let padded_inner = [original.encode(), vec![0xEE, 0xEE]].concat();
    let field = |b: &[u8]| [&u32::try_from(b.len()).unwrap().to_le_bytes()[..], b].concat();
    let padded = [
        vec![1, r.suite.to_byte()],
        field(&r.root),
        field(&padded_inner),
        field(&r.signature),
    ]
    .concat();
    assert!(
        Evidence::decode(&padded).is_err(),
        "one encoding per evidence"
    );
    assert_eq!(Evidence::decode(&wrapped.encode()).unwrap(), wrapped);

    // Stacked to the cap, one more layer is refused rather than written
    // undecodable.
    let mut evidence = original;
    for _ in 0..MAX_LAYERS {
        evidence = lattice.reseal(evidence).unwrap();
    }
    assert_eq!(evidence.layers(), MAX_LAYERS);
    assert!(Evidence::decode(&evidence.encode()).is_ok());
    assert!(lattice.reseal(evidence).is_err());
}
