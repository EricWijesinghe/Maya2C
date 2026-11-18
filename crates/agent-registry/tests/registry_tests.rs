mod common;

use common::{NOW, card_at, signed_at, signer};
use maya_agent_registry::card::SignedCard;
use maya_agent_registry::error::Error;
use maya_agent_registry::limit::FixedWindow;
use maya_agent_registry::registry::Registry;

#[test]
fn a_published_card_is_found_by_capability() {
    let mut registry = Registry::default();
    let alice = signer(1);
    let bob = signer(2);
    assert!(
        registry
            .publish(signed_at(&alice, "translate", NOW), NOW)
            .is_ok()
    );
    assert!(
        registry
            .publish(signed_at(&bob, "translate", NOW), NOW)
            .is_ok()
    );

    assert_eq!(registry.search("translate", NOW).len(), 2);
    assert!(registry.search("weather", NOW).is_empty());
    assert_eq!(
        registry
            .get(&alice.agent_id(), NOW)
            .map(|c| c.card.name.clone()),
        Some("translator".into())
    );
}

#[test]
fn an_expired_card_is_not_served() {
    let mut registry = Registry::default();
    let alice = signer(1);
    let card = signed_at(&alice, "translate", NOW);
    let expires = card.card.expires_at;
    registry.publish(card, NOW).expect("valid when published");

    assert!(registry.search("translate", expires).is_empty());
    assert!(registry.get(&alice.agent_id(), expires).is_none());
}

#[test]
fn a_card_older_than_the_held_one_is_refused() {
    let mut registry = Registry::default();
    let alice = signer(1);
    registry
        .publish(signed_at(&alice, "translate", NOW), NOW)
        .expect("first card");
    let older = signed_at(&alice, "translate", NOW - 300);
    assert_eq!(registry.publish(older, NOW), Err(Error::Stale));
}

#[test]
fn a_card_issued_in_the_future_is_refused() {
    let mut registry = Registry::default();
    let future = signed_at(&signer(1), "translate", NOW + 10_000);
    assert_eq!(registry.publish(future, NOW), Err(Error::Bounds));
}

#[test]
fn an_expired_card_is_refused_at_publish() {
    let mut registry = Registry::default();
    let alice = signer(1);
    let card = signed_at(&alice, "translate", NOW);
    let expires = card.card.expires_at;
    assert_eq!(registry.publish(card, expires), Err(Error::Expired));
}

#[test]
fn a_tampered_card_is_refused_at_publish() {
    let mut registry = Registry::default();
    let mut card: SignedCard = signed_at(&signer(1), "translate", NOW);
    card.card = card_at(&signer(2), "translate", NOW);
    assert_eq!(registry.publish(card, NOW), Err(Error::Signature));
}

#[test]
fn the_fixed_window_admits_its_budget_then_resets() {
    let mut limiter = FixedWindow::new(60, 2);
    assert!(limiter.admit("10.0.0.1", 1_000));
    assert!(limiter.admit("10.0.0.1", 1_001));
    assert!(!limiter.admit("10.0.0.1", 1_002));
    // Another client has its own budget.
    assert!(limiter.admit("10.0.0.2", 1_002));
    // The next window starts afresh.
    assert!(limiter.admit("10.0.0.1", 1_060));
}
