//! Forward-secure seals over 100 simulated epochs.
//!
//! One sealer seals an archive root in every epoch and evolves; one verifier
//! follows along holding only the genesis key. Then the machine is
//! "compromised" at epoch 50: everything the attacker can reach is the live
//! signer, and nothing it can do produces a seal the verifier accepts for an
//! earlier epoch.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_archive::seal::{EpochSigner, KeyChain, Seal, Transition};
use maya_crypto_pq::suite::MasterSeed;

const EPOCHS: u64 = 100;
const COMPROMISED_AT: u64 = 50;

fn root(epoch: u64) -> Vec<u8> {
    format!("bafy-archive-root-{epoch:03}").into_bytes()
}

struct History {
    chain: KeyChain,
    seals: Vec<Seal>,
    transitions: Vec<Transition>,
    live: EpochSigner,
}

/// Seals epochs `0..EPOCHS` and evolves after each, returning the signer
/// left at `EPOCHS`.
fn run() -> History {
    let mut signer = EpochSigner::genesis(&MasterSeed::from_bytes([0x5e; 32]));
    let mut chain = KeyChain::new(signer.public_key().to_vec());
    let mut seals = Vec::new();
    let mut transitions = Vec::new();
    for epoch in 0..EPOCHS {
        assert_eq!(signer.epoch(), epoch);
        let seal = signer.seal(&root(epoch)).expect("seal");
        chain.verify(&seal).expect("fresh seal verifies");
        seals.push(seal);
        let (next, transition) = signer.evolve().expect("evolve");
        chain
            .admit(transition.clone())
            .expect("certified transition");
        transitions.push(transition);
        signer = next;
    }
    History {
        chain,
        seals,
        transitions,
        live: signer,
    }
}

#[test]
fn a_hundred_epochs_seal_evolve_and_verify_from_the_genesis_key_alone() {
    let history = run();
    assert_eq!(history.chain.tip(), EPOCHS);
    assert_eq!(history.live.epoch(), EPOCHS);

    // A verifier built later, from the genesis key and the public
    // transitions only, accepts every seal.
    let genesis = history.chain.key(0).expect("genesis").to_vec();
    let mut fresh = KeyChain::new(genesis);
    for transition in &history.transitions {
        fresh.admit(transition.clone()).expect("replay");
    }
    for seal in &history.seals {
        fresh.verify(seal).expect("historic seal");
    }
    // Every epoch had its own key.
    let keys: std::collections::BTreeSet<_> = (0..=EPOCHS)
        .map(|e| fresh.key(e).expect("key").to_vec())
        .collect();
    assert_eq!(keys.len(), usize::try_from(EPOCHS + 1).expect("fits"));
}

#[test]
fn a_compromise_at_epoch_50_cannot_forge_the_past() {
    let mut signer = EpochSigner::genesis(&MasterSeed::from_bytes([0x5e; 32]));
    let mut chain = KeyChain::new(signer.public_key().to_vec());
    while signer.epoch() < COMPROMISED_AT {
        let (next, transition) = signer.evolve().expect("evolve");
        chain.admit(transition).expect("admit");
        signer = next;
    }
    // The attacker holds `signer`: epoch 50. It seals as 50 …
    let stolen = signer.seal(&root(7)).expect("seal");
    chain
        .verify(&stolen)
        .expect("epoch 50 is the attacker's now");

    // … but a seal claiming epoch 49, or any earlier one, signed by the only
    // key it has, is refused: the signature is not by that epoch's key.
    for past in [0, 1, 25, COMPROMISED_AT - 1] {
        let backdated = Seal {
            epoch: past,
            ..stolen.clone()
        };
        assert!(chain.verify(&backdated).is_err(), "epoch {past}");
    }

    // Nor can it rewrite history by certifying a fresh key for an old epoch:
    // the chain admits only the next epoch, signed by its tip.
    let (_, forged) = EpochSigner::genesis(&MasterSeed::from_bytes([0xee; 32]))
        .evolve()
        .expect("attacker's own chain");
    assert!(chain.admit(forged).is_err(), "epoch 1 is long certified");
}

#[test]
fn tampered_seals_and_transitions_are_refused() {
    let signer = EpochSigner::genesis(&MasterSeed::from_bytes([1; 32]));
    let mut chain = KeyChain::new(signer.public_key().to_vec());
    let seal = signer.seal(&root(0)).expect("seal");

    let mut other_root = seal.clone();
    other_root.root = root(1);
    assert!(chain.verify(&other_root).is_err(), "bound to its root");
    let mut flipped = seal.clone();
    flipped.signature[0] ^= 1;
    assert!(chain.verify(&flipped).is_err());
    let future = Seal { epoch: 3, ..seal };
    assert!(chain.verify(&future).is_err(), "epoch 3 is not certified");

    let (_, mut transition) = signer.evolve().expect("evolve");
    let skipped = Transition {
        epoch: 2,
        ..transition.clone()
    };
    assert!(chain.admit(skipped).is_err(), "no skipping");
    transition.public_key[0] ^= 1;
    assert!(
        chain.admit(transition).is_err(),
        "key bound to its certificate"
    );
    assert_eq!(chain.tip(), 0);
}
