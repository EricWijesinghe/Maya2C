mod common;

use common::{NOW, card_at, signed_at, signer};
use maya_agent_registry::card::{SignedCard, agent_id_of};
use maya_agent_registry::error::Error;
use primitive_types::U256;

#[test]
fn a_signed_card_verifies_and_names_its_key() {
    let alice = signer(1);
    let card = signed_at(&alice, "translate", NOW);
    assert_eq!(card.verify(NOW), Ok(alice.agent_id()));
}

#[test]
fn a_tampered_name_is_refused() {
    let mut card = signed_at(&signer(1), "translate", NOW);
    card.card.name = "impostor".into();
    assert_eq!(card.verify(NOW), Err(Error::Signature));
}

#[test]
fn a_tampered_expiry_is_refused() {
    let mut card = signed_at(&signer(1), "translate", NOW);
    card.card.expires_at += 1_000_000;
    assert_eq!(card.verify(NOW), Err(Error::Signature));
}

#[test]
fn a_tampered_price_is_refused() {
    let mut card = signed_at(&signer(1), "translate", NOW);
    card.card.prices[0].amount = U256::from(1u64);
    assert_eq!(card.verify(NOW), Err(Error::Signature));
}

#[test]
fn an_expired_card_is_refused_at_and_after_expiry() {
    let card = signed_at(&signer(1), "translate", NOW);
    let expires = card.card.expires_at;
    assert_eq!(
        card.verify(expires - 1),
        Ok(agent_id_of(card.card.public_key.as_slice()))
    );
    assert_eq!(card.verify(expires), Err(Error::Expired));
    assert_eq!(card.verify(expires + 1), Err(Error::Expired));
}

#[test]
fn a_card_cannot_be_signed_with_a_key_it_does_not_name() {
    let alice = signer(1);
    let mallory = signer(2);
    let result = SignedCard::sign(card_at(&alice, "translate", NOW), &mallory);
    assert_eq!(result, Err(Error::KeyMismatch));
}

#[test]
fn a_forged_signature_by_another_key_is_refused() {
    let alice = signer(1);
    let mallory = signer(2);
    // Mallory signs a card naming her own key, then swaps in Alice's card body.
    let mut forged = signed_at(&mallory, "translate", NOW);
    forged.card = card_at(&alice, "translate", NOW);
    assert_eq!(forged.verify(NOW), Err(Error::Signature));
}

#[test]
fn the_agent_key_is_not_the_chain_key_of_the_same_seed() {
    use maya_crypto_pq::suite::{HybridMlDsa65SlhDsa128s, MasterSeed, SignatureSuite};
    let chain_key =
        HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes([1; 32]));
    let chain_public = HybridMlDsa65SlhDsa128s::public_key(&chain_key);
    assert_ne!(signer(1).public_key(), chain_public.as_slice());
}

#[test]
fn a_price_for_an_unlisted_capability_is_out_of_bounds() {
    let mut card = card_at(&signer(1), "translate", NOW);
    card.prices[0].capability = "weather".into();
    assert_eq!(card.canonical(), Err(Error::Bounds));
}

#[test]
fn an_agent_id_is_bound_to_its_key() {
    let alice = signer(1);
    let bob = signer(2);
    let card = signed_at(&alice, "translate", NOW);
    assert_eq!(card.card.agent_id(), alice.agent_id());
    assert_ne!(card.card.agent_id(), bob.agent_id());
}
