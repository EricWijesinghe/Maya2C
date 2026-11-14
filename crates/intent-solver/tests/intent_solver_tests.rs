//! The intent market's refusals and its settlement rule (P-30, ADR-139).
//!
//! Each test names the attack or mistake it pins: a replayed intent, an
//! expired one, a quote past the limit, a tie that must not depend on arrival
//! order, a refund that must open exactly at the timeout, and arithmetic that
//! must refuse rather than wrap.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, HybridSigningKey, MasterSeed, SignatureSuite,
};
use maya_htlc_lattice::{ClaimOutcome, HashFunction, LockStatus, Preimage, RefundOutcome};
use maya_intent_solver::{
    AssetId, Intent, IntentBook, IntentError, LegState, Quote, SignedIntent, plan_settlement,
};
use primitive_types::U256;

const NETWORK: [u8; 32] = [0x4d; 32];
/// An asset another chain carries; the native coin is [`AssetId::NATIVE`].
const BTC: AssetId = AssetId([0xb7; 32]);
const DEADLINE: u64 = 1_000;
const NOW: u64 = 100;
const MARGIN: u64 = 50;

fn key(seed: u8) -> HybridSigningKey {
    HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]))
}

/// The maker sells up to 500 native base units for 7 units of `BTC`.
fn intent_for(key: &HybridSigningKey, nonce: u64, deadline: u64) -> Intent {
    let public_key = HybridMlDsa65SlhDsa128s::public_key(key);
    Intent {
        network: NETWORK,
        maker: Intent::maker_of(&public_key),
        sell_asset: AssetId::NATIVE,
        max_sell: U256::from(500u64),
        buy_asset: BTC,
        buy_amount: U256::from(7u64),
        buy_recipient: b"bc1q-maker".to_vec(),
        deadline_height: deadline,
        nonce,
    }
}

fn signed(seed: u8, nonce: u64, deadline: u64) -> SignedIntent {
    let k = key(seed);
    SignedIntent::sign(intent_for(&k, nonce, deadline), &k).unwrap()
}

fn quote(intent: &SignedIntent, solver: u8, price: u64, fee: u64) -> Quote {
    Quote {
        intent_id: intent.id(),
        solver: [solver; 32],
        pay_to: vec![solver; 32],
        price: U256::from(price),
        fee: U256::from(fee),
    }
}

#[test]
fn a_signed_intent_verifies_and_a_tampered_one_does_not() {
    let s = signed(1, 0, DEADLINE);
    s.verify(&NETWORK).unwrap();
    let mut tampered = s.clone();
    tampered.intent.max_sell = U256::from(5_000u64);
    assert_eq!(tampered.verify(&NETWORK), Err(IntentError::BadSignature));
    assert_eq!(s.verify(&[0; 32]), Err(IntentError::WrongNetwork));
}

#[test]
fn an_intent_signed_by_another_key_is_refused() {
    let mut s = signed(1, 0, DEADLINE);
    s.public_key = HybridMlDsa65SlhDsa128s::public_key(&key(2));
    assert_eq!(s.verify(&NETWORK), Err(IntentError::MakerMismatch));
}

#[test]
fn a_replayed_intent_is_refused() {
    let mut book = IntentBook::new(NETWORK);
    let s = signed(1, 9, DEADLINE);
    book.post(s.clone(), NOW).unwrap();
    assert_eq!(book.post(s, NOW), Err(IntentError::Replay));
}

#[test]
fn a_new_intent_reusing_a_spent_nonce_is_a_replay_too() {
    let mut book = IntentBook::new(NETWORK);
    book.post(signed(1, 9, DEADLINE), NOW).unwrap();
    // Different content, same (maker, nonce): a nonce is spent once.
    assert_eq!(
        book.post(signed(1, 9, DEADLINE + 1), NOW),
        Err(IntentError::Replay)
    );
    book.post(signed(1, 10, DEADLINE), NOW).unwrap();
}

#[test]
fn an_expired_intent_is_refused() {
    let mut book = IntentBook::new(NETWORK);
    // The deadline height itself is already too late, as for an HTLC claim.
    assert_eq!(book.post(signed(1, 0, NOW), NOW), Err(IntentError::Expired));
    let s = signed(1, 1, NOW + 1);
    book.post(s.clone(), NOW).unwrap();
    assert_eq!(
        book.offer(quote(&s, 3, 400, 0), NOW + 1),
        Err(IntentError::Expired)
    );
}

#[test]
fn a_quote_worse_than_the_limit_is_refused() {
    let mut book = IntentBook::new(NETWORK);
    let s = signed(1, 0, DEADLINE);
    book.post(s.clone(), NOW).unwrap();
    // 499 + 2 > 500: the fee counts against the limit.
    assert_eq!(
        book.offer(quote(&s, 3, 499, 2), NOW),
        Err(IntentError::WorseThanLimit)
    );
    book.offer(quote(&s, 3, 499, 1), NOW).unwrap();
}

#[test]
fn quote_overflow_is_refused_not_wrapped() {
    let mut book = IntentBook::new(NETWORK);
    let s = signed(1, 0, DEADLINE);
    book.post(s.clone(), NOW).unwrap();
    let mut q = quote(&s, 3, 0, 1);
    q.price = U256::MAX;
    // A wrapped sum would be 0 and pass the limit.
    assert_eq!(q.total(), Err(IntentError::Overflow));
    assert_eq!(book.offer(q, NOW), Err(IntentError::Overflow));
}

#[test]
fn best_quote_ties_break_by_solver_id_not_arrival_order() {
    let s = signed(1, 0, DEADLINE);
    let mut forward = IntentBook::new(NETWORK);
    let mut backward = IntentBook::new(NETWORK);
    forward.post(s.clone(), NOW).unwrap();
    backward.post(s.clone(), NOW).unwrap();
    let quotes = [
        quote(&s, 9, 300, 10),
        quote(&s, 4, 305, 5),
        quote(&s, 7, 310, 0),
        quote(&s, 2, 400, 0),
    ];
    for q in &quotes {
        forward.offer(q.clone(), NOW).unwrap();
    }
    for q in quotes.iter().rev() {
        backward.offer(q.clone(), NOW).unwrap();
    }
    let id = s.id();
    let best = forward.best(&id).unwrap();
    // Three quotes total 310; the lowest solver id wins.
    assert_eq!(best.solver, [4; 32]);
    assert_eq!(backward.best(&id).unwrap(), best);
    let order = |b: &IntentBook| -> Vec<u8> {
        b.quotes(&id).unwrap().iter().map(|q| q.solver[0]).collect()
    };
    assert_eq!(order(&forward), vec![4, 7, 9, 2]);
    assert_eq!(order(&forward), order(&backward));
}

#[test]
fn a_solver_can_improve_its_quote_but_not_be_made_worse() {
    let mut book = IntentBook::new(NETWORK);
    let s = signed(1, 0, DEADLINE);
    book.post(s.clone(), NOW).unwrap();
    book.offer(quote(&s, 3, 300, 0), NOW).unwrap();
    // Quotes are not signed, so a worse one in the same name is refused.
    assert_eq!(
        book.offer(quote(&s, 3, 350, 0), NOW),
        Err(IntentError::NotAnImprovement)
    );
    book.offer(quote(&s, 3, 250, 0), NOW).unwrap();
    assert_eq!(book.best(&s.id()).unwrap().price, U256::from(250u64));
    assert_eq!(book.quotes(&s.id()).unwrap().len(), 1);
}

#[test]
fn the_plan_puts_the_solver_timeout_a_margin_before_the_makers() {
    let s = signed(1, 0, DEADLINE);
    let q = quote(&s, 3, 300, 5);
    let plan =
        plan_settlement(&s.intent, &q, HashFunction::Sha256, [0xaa; 32], NOW, MARGIN).unwrap();
    assert_eq!(plan.maker_leg.expiry_height, DEADLINE);
    assert_eq!(plan.solver_leg.expiry_height, DEADLINE - MARGIN);
    assert_eq!(plan.maker_leg.amount, U256::from(305u64));
    assert_eq!(plan.solver_leg.amount, U256::from(7u64));
    assert!(plan.maker_leg.on_maya2c);
    assert!(!plan.solver_leg.on_maya2c);
    assert_eq!(plan.maker_leg.recipient, vec![3; 32]);
    assert_eq!(plan.solver_leg.recipient, b"bc1q-maker".to_vec());
}

#[test]
fn a_plan_with_no_room_or_overflowing_heights_is_refused() {
    let s = signed(1, 0, DEADLINE);
    let q = quote(&s, 3, 300, 0);
    let digest = [0xaa; 32];
    assert_eq!(
        plan_settlement(
            &s.intent,
            &q,
            HashFunction::Sha256,
            digest,
            DEADLINE - MARGIN,
            MARGIN
        ),
        Err(IntentError::NoRoom)
    );
    assert_eq!(
        plan_settlement(
            &s.intent,
            &q,
            HashFunction::Sha256,
            digest,
            NOW,
            DEADLINE + 1
        ),
        Err(IntentError::NoRoom)
    );
    assert_eq!(
        plan_settlement(&s.intent, &q, HashFunction::Sha256, digest, NOW, 0),
        Err(IntentError::NoRoom)
    );
    let far = signed(1, 1, u64::MAX);
    let q = quote(&far, 3, 300, 0);
    assert_eq!(
        plan_settlement(
            &far.intent,
            &q,
            HashFunction::Sha256,
            digest,
            u64::MAX - 2,
            10
        ),
        Err(IntentError::Overflow)
    );
}

#[test]
fn an_external_leg_needs_the_hash_other_chains_can_check() {
    let s = signed(1, 0, DEADLINE);
    let q = quote(&s, 3, 300, 0);
    assert_eq!(
        plan_settlement(&s.intent, &q, HashFunction::Blake3, [0xaa; 32], NOW, MARGIN),
        Err(IntentError::HashNotPortable)
    );
}

#[test]
fn the_happy_path_claims_both_legs_with_one_preimage() {
    let s = signed(1, 0, DEADLINE);
    let preimage = Preimage::new([0x5e; 32]);
    let digest = HashFunction::Sha256.digest(&preimage);
    let plan = plan_settlement(
        &s.intent,
        &quote(&s, 3, 300, 0),
        HashFunction::Sha256,
        digest,
        NOW,
        MARGIN,
    )
    .unwrap();
    let mut maker = LegState::locked(plan.maker_leg.clone());
    let mut solver = LegState::locked(plan.solver_leg.clone());
    let wrong = Preimage::new([0; 32]);
    assert_eq!(
        solver.claim(&plan, &wrong, NOW + 1),
        ClaimOutcome::WrongOpening
    );
    assert_eq!(
        solver.claim(&plan, &preimage, NOW + 1),
        ClaimOutcome::Claimed
    );
    assert_eq!(
        maker.claim(&plan, &preimage, NOW + 2),
        ClaimOutcome::Claimed
    );
    assert_eq!(maker.refund(DEADLINE), RefundOutcome::AlreadySettled);
    assert_eq!(maker.status, LockStatus::Claimed);
}

#[test]
fn the_refund_path_opens_exactly_at_the_timeout() {
    let s = signed(1, 0, DEADLINE);
    let preimage = Preimage::new([0x5e; 32]);
    let digest = HashFunction::Sha256.digest(&preimage);
    let plan = plan_settlement(
        &s.intent,
        &quote(&s, 3, 300, 0),
        HashFunction::Sha256,
        digest,
        NOW,
        MARGIN,
    )
    .unwrap();
    let solver_expiry = plan.solver_leg.expiry_height;
    let mut solver = LegState::locked(plan.solver_leg.clone());
    let mut maker = LegState::locked(plan.maker_leg.clone());
    assert_eq!(solver.refund(solver_expiry - 1), RefundOutcome::NotExpired);
    assert_eq!(solver.refund(solver_expiry), RefundOutcome::Refunded);
    // Refunded once: a late claim and a second refund both do nothing.
    assert_eq!(
        solver.claim(&plan, &preimage, solver_expiry),
        ClaimOutcome::AlreadySettled
    );
    assert_eq!(
        solver.refund(solver_expiry + 1),
        RefundOutcome::AlreadySettled
    );
    // The maker's lock outlives the solver's by the margin, then refunds too.
    assert_eq!(maker.refund(solver_expiry), RefundOutcome::NotExpired);
    assert_eq!(
        maker.claim(&plan, &preimage, DEADLINE),
        ClaimOutcome::Expired
    );
    assert_eq!(maker.refund(DEADLINE), RefundOutcome::Refunded);
}

#[test]
fn malformed_intents_are_refused_before_the_signature_is_checked() {
    let k = key(1);
    let mut i = intent_for(&k, 0, DEADLINE);
    i.buy_asset = i.sell_asset;
    assert_eq!(
        SignedIntent::sign(i, &k).unwrap().verify(&NETWORK),
        Err(IntentError::Malformed("sell and buy assets are the same"))
    );
    let mut i = intent_for(&k, 0, DEADLINE);
    i.max_sell = U256::zero();
    assert!(matches!(
        SignedIntent::sign(i, &k).unwrap().verify(&NETWORK),
        Err(IntentError::Malformed(_))
    ));
    let mut i = intent_for(&k, 0, DEADLINE);
    i.buy_recipient = vec![0; 65];
    assert!(matches!(
        SignedIntent::sign(i, &k).unwrap().verify(&NETWORK),
        Err(IntentError::Malformed(_))
    ));
}
