//! Fee conversion rounding and fee-charge atomicity (Master Prompt 22 §3).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_amount::Amount;
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId};
use maya_smart_account::account::AccountError;
use maya_smart_account::fees::{FeeSpec, Paymaster, TOKEN_MARGIN_BPS, TokenPrice};
use maya_smart_account::op::{Action, Op};
use maya_smart_account::{AccountId, Registry, key_hash};

type Sk = <MlDsa65 as SignatureSuite>::SigningKey;

fn amt(n: u64) -> Amount {
    Amount::from_u64(n)
}

fn price(native: u64, token: u64) -> TokenPrice {
    TokenPrice {
        native: amt(native),
        token: amt(token),
    }
}

fn ceil_div(a: u128, b: u128) -> u128 {
    a.div_ceil(b)
}

#[test]
fn token_cost_matches_exact_double_ceiling_over_a_grid() {
    // Reference: base = ceil(fee * token / native); cost = ceil(base * (10_000 + margin) / 10_000).
    let factor = u128::from(10_000 + TOKEN_MARGIN_BPS);
    for native in 1..=9u64 {
        for token in 0..=9u64 {
            for fee in 0..=200u64 {
                let base = ceil_div(u128::from(fee) * u128::from(token), u128::from(native));
                let want = ceil_div(base * factor, 10_000);
                let got = price(native, token).token_cost(amt(fee)).unwrap();
                assert_eq!(got, Amount::from_u128(want), "fee {fee} at {token}/{native}");
            }
        }
    }
}

#[test]
fn token_cost_never_underpays_the_native_fee() {
    // cost / token >= fee / native, i.e. cost * native >= fee * token * (1 + margin).
    let factor = u128::from(10_000 + TOKEN_MARGIN_BPS);
    for (native, token) in [(1, 2), (3, 1), (7, 5), (1_000_003, 999_983)] {
        for fee in [1u64, 2, 3, 10, 99, 12_345, 1_000_000_007] {
            let cost = price(native, token).token_cost(amt(fee)).unwrap();
            let cost = cost.to_u128().unwrap();
            assert!(
                cost * u128::from(native) * 10_000 >= u128::from(fee) * u128::from(token) * factor,
                "underpaid: fee {fee} at {token}/{native}"
            );
        }
    }
}

#[test]
fn a_tiny_fee_still_costs_at_least_one_token_unit() {
    // 1 native = 1 token / 3: floor would give 0 and a free fee.
    assert_eq!(price(3, 1).token_cost(amt(1)), Some(amt(2)), "1 -> 1 -> ceil(1.05) = 2");
    assert_eq!(price(1_000, 1).token_cost(amt(1)), Some(amt(2)));
    assert_eq!(price(3, 1).token_cost(amt(0)), Some(amt(0)), "no fee, no cost");
}

#[test]
fn zero_native_price_has_no_cost_and_overflow_is_none_not_a_wrap() {
    assert_eq!(price(0, 5).token_cost(amt(1)), None, "division by zero");
    assert_eq!(price(1, 1).token_cost(Amount::MAX), None, "margin pushes past MAX");
    assert_eq!(
        TokenPrice {
            native: amt(1),
            token: Amount::MAX
        }
        .token_cost(amt(2)),
        None
    );
}

#[test]
fn finding_a_zero_token_price_makes_the_token_fee_free() {
    // FINDING (post-genesis review, not changed here): the price check only
    // rejects native == 0. A price with token == 0 converts every fee to 0
    // token units, so a payer would cover any native fee for nothing. This pins
    // the current behaviour; an oracle must never publish it, and a fix belongs
    // with the fee-semantics owner. If it is fixed, flip this assertion.
    assert_eq!(price(1, 0).token_cost(amt(1_000_000)), Some(amt(0)));
}

#[test]
fn a_product_wider_than_256_bits_is_computed_not_overflowed() {
    let two_100 = Amount::from_u128(1 << 100);
    let two_200 = two_100.checked_mul(two_100).unwrap();
    // fee * token = 2^300 does not fit; the quotient 2^200 does.
    let p = TokenPrice {
        native: two_100,
        token: two_100,
    };
    let cost = p.token_cost(two_200).unwrap();
    let five_percent = two_200.mul_div(amt(5), amt(100)).unwrap();
    assert!(cost >= two_200.checked_add(five_percent).unwrap().saturating_sub(amt(1)));
    assert!(cost <= two_200.checked_add(five_percent).unwrap().checked_add(amt(2)).unwrap());
}

// ---- charge_fee through Registry::apply ----------------------------------

fn key(b: u8) -> (Sk, Vec<u8>) {
    let sk = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([b; 32]));
    let pk = MlDsa65::public_key(&sk);
    (sk, pk)
}

fn setup(balance: u64) -> (Registry, AccountId, (Sk, Vec<u8>), AccountId) {
    let mut r = Registry::new();
    let alice = key(1);
    let a = r.create(SuiteId::MlDsa65, &alice.1, 0, amt(balance));
    let bob = key(2);
    let b = r.create(SuiteId::MlDsa65, &bob.1, 0, amt(0));
    (r, a, alice, b)
}

fn transfer(
    a: AccountId,
    to: AccountId,
    amount: u64,
    fee: FeeSpec,
    (sk, pk): &(Sk, Vec<u8>),
) -> Op {
    let mut o = Op {
        account: a,
        nonce: 0,
        action: Action::Transfer {
            to,
            amount: amt(amount),
        },
        fee,
        signatures: vec![],
    };
    let msg = o.signing_bytes();
    o.signatures = vec![(
        key_hash(SuiteId::MlDsa65, pk),
        MlDsa65::sign(sk, &msg).unwrap(),
    )];
    o
}

/// Everything a refused op must leave alone.
fn snapshot(r: &Registry, a: AccountId, b: AccountId) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        r.get(&a),
        r.get(&b),
        r.token_balances,
        r.fees_collected,
        r.paymasters,
        r.prices
    )
}

fn token_fee(max: u64) -> FeeSpec {
    FeeSpec::Token {
        token: 7,
        max_amount: amt(max),
    }
}

#[test]
fn a_token_fee_with_no_price_is_rejected_and_nothing_changes() {
    let (mut r, a, k, b) = setup(1_000);
    r.token_balances.insert((a, 7), amt(500));
    let before = snapshot(&r, a, b);
    let o = transfer(a, b, 1, token_fee(500), &k);
    assert_eq!(r.apply(&o, 1, amt(100)), Err(AccountError::NoPrice));
    assert_eq!(snapshot(&r, a, b), before, "atomic");
    assert_eq!(r.get(&a).unwrap().nonce, 0, "nonce not consumed");
}

#[test]
fn a_zero_native_price_is_rejected_like_a_missing_one() {
    let (mut r, a, k, b) = setup(1_000);
    r.prices.insert(7, price(0, 5));
    r.token_balances.insert((a, 7), amt(500));
    let before = snapshot(&r, a, b);
    let o = transfer(a, b, 1, token_fee(500), &k);
    assert_eq!(r.apply(&o, 1, amt(100)), Err(AccountError::NoPrice));
    assert_eq!(snapshot(&r, a, b), before);
}

#[test]
fn a_missing_price_leaves_no_phantom_balance_entry() {
    // charge_fee must not create a (account, token) entry before it knows it
    // can charge; the registry snapshot would hide it, so check it directly.
    let (mut r, a, k, b) = setup(1_000);
    let o = transfer(a, b, 1, token_fee(500), &k);
    assert_eq!(r.apply(&o, 1, amt(100)), Err(AccountError::NoPrice));
    assert!(!r.token_balances.contains_key(&(a, 7)));
}

#[test]
fn an_unfunded_token_fee_is_insufficient_without_underflow_or_side_effects() {
    let (mut r, a, k, b) = setup(1_000);
    r.prices.insert(7, price(1, 2));
    r.token_balances.insert((a, 7), amt(209)); // cost is 210
    let before = snapshot(&r, a, b);
    let o = transfer(a, b, 1, token_fee(300), &k);
    assert_eq!(r.apply(&o, 1, amt(100)), Err(AccountError::Insufficient));
    assert_eq!(snapshot(&r, a, b), before);
    r.token_balances.insert((a, 7), amt(210));
    r.apply(&o, 1, amt(100)).unwrap();
    assert_eq!(r.token_balances[&(a, 7)], amt(0), "exactly the rounded-up cost");
    assert_eq!(r.fees_collected, amt(100));
}

#[test]
fn a_token_cost_above_the_quoted_maximum_is_refused() {
    let (mut r, a, k, b) = setup(1_000);
    r.prices.insert(7, price(1, 2));
    r.token_balances.insert((a, 7), amt(1_000));
    let before = snapshot(&r, a, b);
    let o = transfer(a, b, 1, token_fee(209), &k);
    assert_eq!(r.apply(&o, 1, amt(100)), Err(AccountError::FeeAboveMaximum));
    assert_eq!(snapshot(&r, a, b), before);
    assert!(r.apply(&transfer(a, b, 1, token_fee(210), &k), 1, amt(100)).is_ok());
}

#[test]
fn a_native_fee_beyond_the_balance_is_insufficient_not_an_underflow() {
    let (mut r, a, k, b) = setup(50);
    let before = snapshot(&r, a, b);
    let fee = FeeSpec::Native { max_fee: amt(1_000) };
    let o = transfer(a, b, 1, fee, &k);
    assert_eq!(r.apply(&o, 1, amt(51)), Err(AccountError::Insufficient));
    assert_eq!(snapshot(&r, a, b), before);
}

#[test]
fn a_failing_action_after_the_fee_refunds_the_fee() {
    // Fee takes the whole balance, so the transfer then fails: the fee must
    // not stay charged, and fees_collected must not count it.
    let (mut r, a, k, b) = setup(100);
    let before = snapshot(&r, a, b);
    let o = transfer(a, b, 1, FeeSpec::Native { max_fee: amt(100) }, &k);
    assert_eq!(r.apply(&o, 1, amt(100)), Err(AccountError::Insufficient));
    assert_eq!(snapshot(&r, a, b), before);
    assert_eq!(r.get(&a).unwrap().nonce, 0);

    // Same for a token fee and a sponsored fee.
    r.prices.insert(7, price(1, 1));
    r.token_balances.insert((a, 7), amt(1_000));
    let before = snapshot(&r, a, b);
    let o = transfer(a, b, 1_000_000, token_fee(1_000), &k);
    assert_eq!(r.apply(&o, 1, amt(10)), Err(AccountError::Insufficient));
    assert_eq!(snapshot(&r, a, b), before, "token fee refunded");

    let (_, ppk) = key(11);
    let pm = r.create(SuiteId::MlDsa65, &ppk, 0, amt(0));
    r.paymasters.insert(
        pm,
        Paymaster {
            sponsored: vec![a],
            per_account_daily: amt(1_000),
            budget: amt(1_000),
            ..Paymaster::default()
        },
    );
    let before = snapshot(&r, a, b);
    let o = transfer(a, b, 1_000_000, FeeSpec::Sponsored { paymaster: pm }, &k);
    assert_eq!(r.apply(&o, 1, amt(10)), Err(AccountError::Insufficient));
    assert_eq!(snapshot(&r, a, b), before, "sponsor budget and daily tally restored");
    assert_eq!(r.paymasters[&pm].budget, amt(1_000));
    assert!(r.paymasters[&pm].spent.is_empty());
}

#[test]
fn an_unknown_paymaster_is_refused_unchanged() {
    let (mut r, a, k, b) = setup(1_000);
    let before = snapshot(&r, a, b);
    let o = transfer(a, b, 1, FeeSpec::Sponsored { paymaster: [9; 32] }, &k);
    assert_eq!(r.apply(&o, 1, amt(10)), Err(AccountError::SponsorRefused));
    assert_eq!(snapshot(&r, a, b), before);
}

#[test]
fn a_native_fee_exactly_at_the_maximum_is_allowed_and_counted() {
    let (mut r, a, k, b) = setup(1_000);
    let o = transfer(a, b, 10, FeeSpec::Native { max_fee: amt(40) }, &k);
    r.apply(&o, 1, amt(40)).unwrap();
    assert_eq!(r.get(&a).unwrap().balance, amt(1_000 - 40 - 10));
    assert_eq!(r.fees_collected, amt(40));
}
