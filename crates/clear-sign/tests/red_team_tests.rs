//! Red-team cases for `review`: what a drainer does once it knows how the
//! check works. Each case was tried against the reviewer as it stood; the two
//! marked FOUND failed before the fix in the same change.

#![allow(clippy::unwrap_used)]

use maya_amount::Amount;
use maya_clear_sign::{Address, Context, Effect, Warning, review};

const NATIVE: Address = [0; 32];
const TOKEN: Address = [0x70; 32];
const THIEF: Address = [0xBA; 32];
const FRIEND: Address = [0xC0; 32];
const DEX: Address = [0xE0; 32];

fn ctx(native: u64, token: u64) -> Context {
    Context {
        contacts: [FRIEND].into(),
        verified: [DEX].into(),
        balances: vec![
            (NATIVE, Amount::from_u64(native)),
            (TOKEN, Amount::from_u64(token)),
        ],
    }
}

fn send(token: Address, to: Address, amount: u64) -> Effect {
    Effect::Transfer {
        token,
        to,
        amount: Amount::from_u64(amount),
    }
}

/// FOUND: the claim and the simulation were compared as sets, so a
/// simulation sending the claimed transfer twice compared equal to it.
#[test]
fn a_transfer_the_simulation_repeats_but_the_claim_lists_once_is_a_mismatch() {
    let once = [send(NATIVE, FRIEND, 10)];
    let twice = [send(NATIVE, FRIEND, 10), send(NATIVE, FRIEND, 10)];
    let w = review(&once, &twice, &ctx(1_000, 0));
    assert_eq!(w.first(), Some(&Warning::ClaimMismatch), "{w:?}");
    // And the other way: claiming more than happens is a mismatch too.
    let w = review(&twice, &once, &ctx(1_000, 0));
    assert_eq!(w.first(), Some(&Warning::ClaimMismatch), "{w:?}");
}

/// FOUND: each transfer was checked against the balance alone, so ten
/// honest-looking 10% transfers emptied the account with no drain warning.
#[test]
fn a_drain_split_into_small_transfers_is_still_a_drain() {
    let split: Vec<Effect> = (0..10).map(|_| send(NATIVE, THIEF, 100)).collect();
    // The drainer claims exactly what it does: no mismatch to catch it.
    let w = review(&split, &split, &ctx(1_000, 0));
    assert!(w.contains(&Warning::DrainsBalance), "{w:?}");
    assert!(!w.contains(&Warning::ClaimMismatch), "{w:?}");
    // Two transfers of 45% each: 90% in total, exactly at the boundary.
    let pair = [send(NATIVE, THIEF, 450), send(NATIVE, FRIEND, 450)];
    assert!(review(&pair, &pair, &ctx(1_000, 0)).contains(&Warning::DrainsBalance));
}

#[test]
fn outflow_is_summed_per_token_never_across_tokens() {
    // 60% of the coin and 60% of a token: neither balance is drained.
    let two = [send(NATIVE, FRIEND, 600), send(TOKEN, FRIEND, 60)];
    assert_eq!(review(&two, &two, &ctx(1_000, 100)), Vec::<Warning>::new());
    // Just under 90% across two transfers of one token: no drain.
    let under = [send(NATIVE, FRIEND, 449), send(NATIVE, FRIEND, 450)];
    assert_eq!(
        review(&under, &under, &ctx(1_000, 0)),
        Vec::<Warning>::new()
    );
}

#[test]
fn a_split_drain_whose_sum_passes_2_pow_256_is_a_drain_not_a_wrap() {
    let max = Amount([u64::MAX; 4]);
    let huge = Effect::Transfer {
        token: NATIVE,
        to: THIEF,
        amount: max,
    };
    // Each alone is the whole balance; the sum would wrap to below it.
    let both = [huge.clone(), huge];
    let c = Context {
        balances: vec![(NATIVE, max)],
        ..Context::default()
    };
    assert!(review(&both, &both, &c).contains(&Warning::DrainsBalance));
}

#[test]
fn the_claim_may_list_effects_in_any_order() {
    let swap = [
        Effect::Call {
            contract: DEX,
            method: "swap".into(),
        },
        send(NATIVE, DEX, 5),
    ];
    let reversed = [swap[1].clone(), swap[0].clone()];
    assert_eq!(
        review(&reversed, &swap, &ctx(1_000, 0)),
        Vec::<Warning>::new()
    );
}

#[test]
fn an_empty_claim_hides_nothing() {
    let sim = [send(NATIVE, FRIEND, 1)];
    assert_eq!(review(&[], &sim, &ctx(1_000, 0)), [Warning::ClaimMismatch]);
    // A claim of effects that never happen is also a mismatch.
    assert_eq!(review(&sim, &[], &ctx(1_000, 0)), [Warning::ClaimMismatch]);
    assert_eq!(review(&[], &[], &ctx(1_000, 0)), Vec::<Warning>::new());
}

#[test]
fn the_claim_never_softens_a_warning_on_the_simulation() {
    // The app claims a harmless call; the simulation is an unlimited
    // approval to an unknown spender plus an upgrade. Every warning comes
    // from the simulation, sorted most severe first, each once.
    let claim = [Effect::Call {
        contract: DEX,
        method: "claim_airdrop".into(),
    }];
    let approve = Effect::Approve {
        token: TOKEN,
        spender: THIEF,
        amount: Amount([0, 0, 0, 1]),
    };
    let sim = [approve.clone(), approve, Effect::Upgrade { contract: DEX }];
    assert_eq!(
        review(&claim, &sim, &ctx(1_000, 100)),
        [
            Warning::ClaimMismatch,
            Warning::ControlChange,
            Warning::UnlimitedApproval,
            Warning::UnverifiedSpender,
        ]
    );
}

#[test]
fn a_balance_the_wallet_does_not_know_raises_no_drain() {
    // Zero or unknown balance: the drain check has nothing to compare with;
    // the transfer is still reviewed for its recipient.
    let unknown = [send([0x99; 32], THIEF, u64::MAX)];
    assert_eq!(
        review(&unknown, &unknown, &ctx(1_000, 0)),
        Vec::<Warning>::new()
    );
}
