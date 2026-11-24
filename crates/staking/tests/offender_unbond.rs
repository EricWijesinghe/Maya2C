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
    // Four bystanders keep the committee floor without the offender, so its
    // exit starts at once (CON-07 holds an exit that would break the floor).
    for n in 0..u8::try_from(maya_staking::MIN_COMMITTEE).unwrap() {
        s.register([0xC0 | n; 32], [0x10 | n; 32], amount(BOND), 0)
            .unwrap();
    }
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
    effects.extend(epochs(&mut s, Params::DEVNET.unbonding_epochs + 1));

    let (burned, credited) = totals(&effects);
    let cut = u128::from(Params::DEVNET.double_sign_slash_bps) * u128::from(BOND) / 10_000;
    assert_eq!(
        burned, cut,
        "the slash takes its share of the unbonding funds"
    );
    assert_eq!(
        credited,
        u128::from(BOND) - cut,
        "the rest is released, no more"
    );
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
    epochs(&mut s, Params::DEVNET.unbonding_epochs + 1);
    s.register(OPERATOR, OFFENDER, amount(BOND), 0)
        .expect("once everything is released the id is free again");
}

#[test]
fn an_honest_full_exit_is_released_in_full() {
    let mut s = registered();
    s.unbond(OPERATOR, OFFENDER, amount(BOND)).unwrap();
    let effects = epochs(&mut s, Params::DEVNET.unbonding_epochs + 1);
    assert_eq!(totals(&effects), (0, u128::from(BOND)));
}

#[test]
fn a_zero_amount_unbond_does_not_keep_an_emptied_record_alive() {
    let mut s = registered();
    s.unbond(OPERATOR, OFFENDER, amount(BOND)).unwrap();
    // Empty entries could once be pushed to extend the record's life past the
    // real funds; they are now refused outright (pre-audit S-001).
    for _ in 0..3 {
        assert!(s.unbond(OPERATOR, OFFENDER, Amount::ZERO).is_err());
        epochs(&mut s, 2);
    }
    epochs(&mut s, Params::DEVNET.unbonding_epochs + 1);
    s.register(OPERATOR, OFFENDER, amount(BOND), 0)
        .expect("only unbonding funds pin a record, not empty entries");
}

#[test]
fn nobody_can_delegate_into_an_emptied_record_to_pin_it() {
    let mut s = registered();
    s.unbond(OPERATOR, OFFENDER, amount(BOND)).unwrap();
    epochs(&mut s, 1);
    assert_eq!(
        s.delegate([0xBB; 32], OFFENDER, amount(BOND)).unwrap_err(),
        StakeError::UnknownValidator
    );
}

#[test]
fn a_delegators_unbonding_funds_are_cut_when_the_validator_is_slashed_after_exit() {
    let mut s = registered();
    let delegator = [0xBB; 32];
    s.delegate(delegator, OFFENDER, amount(BOND)).unwrap();
    s.undelegate(delegator, OFFENDER, amount(BOND)).unwrap();
    s.unbond(OPERATOR, OFFENDER, amount(BOND)).unwrap();
    let mut effects = epochs(&mut s, 1);
    effects.extend(s.slash_double_sign(OFFENDER).unwrap());
    effects.extend(epochs(&mut s, Params::DEVNET.unbonding_epochs + 1));
    let cut = u128::from(Params::DEVNET.double_sign_slash_bps) * u128::from(BOND) / 10_000;
    let (burned, credited) = totals(&effects);
    // Both the operator's and the delegator's unbonding funds lose the same share.
    assert_eq!(burned, 2 * cut);
    assert_eq!(credited, 2 * (u128::from(BOND) - cut));
}

/// STK-21 (Eric 2026-11-24, third round 4): evidence of an offence in epoch
/// `e` is admissible in epochs `e ..= e + unbonding_epochs - 1`, exactly as
/// long as a stake that backed epoch `e` can still be unbonding. A validator
/// that leaves in the offence epoch itself is reached by evidence filed in
/// the last epoch before its funds are released.
#[test]
fn evidence_filed_in_the_last_epoch_of_the_unbonding_period_reaches_the_unbonded_funds() {
    let window = Params::DEVNET.unbonding_epochs;
    let mut s = registered();
    let offence = s.epoch;
    s.unbond(OPERATOR, OFFENDER, amount(BOND)).unwrap();
    let mut effects = epochs(&mut s, window - 1);
    assert_eq!(s.epoch, offence + window - 1, "the last admissible epoch");
    effects.extend(
        s.report_double_sign(OFFENDER, offence)
            .expect("evidence filed inside the unbonding period must land"),
    );
    effects.extend(epochs(&mut s, 2));
    let cut = u128::from(Params::DEVNET.double_sign_slash_bps) * u128::from(BOND) / 10_000;
    assert_eq!(totals(&effects), (cut, u128::from(BOND) - cut));
}

/// One epoch later the period is over: the evidence is refused and nothing
/// changes.
#[test]
fn evidence_older_than_the_unbonding_period_is_refused() {
    let window = Params::DEVNET.unbonding_epochs;
    let mut s = registered();
    let offence = s.epoch;
    epochs(&mut s, window);
    let before = s.clone();
    assert_eq!(
        s.report_double_sign(OFFENDER, offence).unwrap_err(),
        StakeError::EvidenceOutsideWindow
    );
    assert_eq!(s, before, "a refusal changes nothing");
    // The epoch after the offence is still inside its own window.
    let burned = s.report_double_sign(OFFENDER, offence + 1).unwrap();
    assert!(matches!(burned.as_slice(), [Effect::Burn(a)] if !a.is_zero()));
}

#[test]
fn evidence_naming_an_epoch_not_yet_reached_is_refused() {
    let mut s = registered();
    let next = s.epoch + 1;
    assert_eq!(
        s.report_double_sign(OFFENDER, next).unwrap_err(),
        StakeError::EvidenceOutsideWindow
    );
}
