//! A validator that double-signs and then unbonds everything must still be
//! slashed (self-review 2026-10-25, finding 1). Before the fix the empty record
//! was dropped at the next boundary, `slash_double_sign` answered
//! `UnknownValidator`, and the unbonding entries were released unslashed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_staking::{Amount, Effect, Params, Participation, StakeError, Staking};

const OPERATOR: [u8; 32] = [0xAA; 32];
const OFFENDER: [u8; 32] = [1; 32];
const BOND: u64 = 1_000_000;

fn amount(n: u64) -> Amount {
    Amount::from_u64(n)
}

fn registered() -> Staking {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(OPERATOR, OFFENDER, amount(BOND), 0).unwrap();
    s
}

fn totals(effects: &[Effect]) -> (u128, u128) {
    let mut burned = 0;
    let mut credited = 0;
    for e in effects {
        match e {
            Effect::Burn(a) => burned += a.to_u128().unwrap(),
            Effect::Credit(_, a) => credited += a.to_u128().unwrap(),
            Effect::Debit(..) => {}
        }
    }
    (burned, credited)
}

/// Runs `n` empty epochs and returns every effect they produced.
fn epochs(s: &mut Staking, n: u64) -> Vec<Effect> {
    (0..n)
        .flat_map(|_| {
            s.end_epoch(&Participation::default(), Amount::ZERO)
                .unwrap()
                .effects
        })
        .collect()
}

#[test]
fn a_fully_unbonded_offender_is_still_slashed_and_the_rest_is_conserved() {
    let mut s = registered();
    s.unbond(OPERATOR, OFFENDER, amount(BOND)).unwrap();
    let mut effects = epochs(&mut s, 1); // the boundary that used to drop the record

    let slash = s
        .slash_double_sign(OFFENDER)
        .expect("evidence against a validator with funds still unbonding must land");
    effects.extend(slash);
    effects.extend(epochs(&mut s, u64::from(Params::DEVNET.unbonding_epochs) + 1));

    let (burned, credited) = totals(&effects);
    let cut = u128::from(Params::DEVNET.double_sign_slash_bps) * u128::from(BOND) / 10_000;
    assert_eq!(burned, cut, "the slash takes its share of the unbonding funds");
    assert_eq!(credited, u128::from(BOND) - cut, "the rest is released, no more");
}

#[test]
fn an_exiting_validator_cannot_re_register_until_its_funds_are_out() {
    let mut s = registered();
    s.unbond(OPERATOR, OFFENDER, amount(BOND)).unwrap();
    epochs(&mut s, 1);
    assert_eq!(
        s.register(OPERATOR, OFFENDER, amount(BOND), 0).unwrap_err(),
        StakeError::AlreadyRegistered,
        "a new registration would share the old unbonding entries' slashing"
    );
    epochs(&mut s, u64::from(Params::DEVNET.unbonding_epochs) + 1);
    s.register(OPERATOR, OFFENDER, amount(BOND), 0)
        .expect("once everything is released the id is free again");
}

#[test]
fn an_honest_full_exit_is_released_in_full() {
    let mut s = registered();
    s.unbond(OPERATOR, OFFENDER, amount(BOND)).unwrap();
    let effects = epochs(&mut s, u64::from(Params::DEVNET.unbonding_epochs) + 1);
    assert_eq!(totals(&effects), (0, u128::from(BOND)));
}
