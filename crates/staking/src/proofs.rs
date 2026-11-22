//! Kani harnesses. Compiled only under `cargo kani -p maya-staking`.
//!
//! Scalar properties get unbounded proofs. The state-machine properties are
//! checked on one validator with one delegation — enough for CBMC to explore
//! every amount, not enough to call the collections proved; the scenario test
//! (`tests/scenario.rs`) covers a thousand validators by execution.

use alloc::collections::BTreeMap;
use alloc::vec;

use maya_amount::Amount;

use crate::Params;
use crate::epoch::Participation;
use crate::params::{BPS, bps_of};
use crate::types::{Effect, Staking};

/// An amount below 2^8, as a full `Amount`. The reward and slash paths divide
/// through a 512-bit product; with full-width symbolic amounts CBMC gave no
/// verdict on that division (reports/kani/2026-11-10-tractable-256.md), so the
/// state-machine harness below is bounded in magnitude, and says so.
fn small_amount() -> Amount {
    Amount::from_u64(u64::from(kani::any::<u8>()))
}

#[kani::proof]
fn bps_of_never_exceeds_the_amount() {
    let amount = Amount(kani::any());
    let bps: u16 = kani::any();
    kani::assume(u64::from(bps) <= BPS);
    assert!(bps_of(amount, bps) <= amount);
}

#[kani::proof]
#[kani::unwind(40)]
fn a_double_sign_slash_conserves_value() {
    let bond = Amount(kani::any());
    let delegation = Amount(kani::any());
    let params = Params::DEVNET;
    let quarter = Amount::MAX.shr(2);
    kani::assume(bond >= params.min_self_bond && bond < quarter);
    kani::assume(delegation >= params.min_delegation && delegation < quarter);
    let mut s = Staking::new(params).unwrap();
    let _ = s.register([1; 32], [9; 32], bond, 0).unwrap();
    let _ = s.delegate([2; 32], [9; 32], delegation).unwrap();
    let before = s.held().unwrap();
    let effects = s.slash_double_sign([9; 32]).unwrap();
    let mut burned = Amount::ZERO;
    for e in &effects {
        if let Effect::Burn(b) = e {
            burned = burned.checked_add(*b).unwrap();
        }
    }
    assert!(s.held().unwrap().checked_add(burned) == Some(before));
}

/// At the two slash rates the launch uses (`Params::DEVNET`, which genesis
/// keeps for both), `bps_of` is exactly the floor of `amount * bps / 10 000`,
/// for every amount below 2^16. Bounded, and said so.
///
/// Exact, unlike `bps_of_never_exceeds_the_amount`, which only bounds the cut:
/// a rule that rounded up, or took the rate from the wrong side of 10 000,
/// fails here and passes the bound. The bounds are where CBMC closes: a
/// symbolic rate gave no verdict in 5 min at 32-bit amounts, and with the rate
/// concrete, 64-bit and 32-bit amounts gave none in 4.5 and 8 min; 16-bit
/// verifies in about 15 s (2026-11-14, reports/kani/2026-11-14-apply-path.md).
#[kani::proof]
#[kani::unwind(41)]
fn bps_of_is_exact_at_the_launch_slash_rates() {
    let amount = u64::from(kani::any::<u16>());
    let p = Params::DEVNET;
    // Two concrete calls, not one symbolic choice: a choice between two
    // constants is a symbolic rate again.
    for bps in [p.downtime_slash_bps, p.double_sign_slash_bps] {
        let exact = u128::from(amount) * u128::from(bps);
        let cut = bps_of(Amount::from_u64(amount), bps).to_u128();
        assert!(cut.is_some());
        if let Some(cut) = cut {
            assert!(cut * u128::from(BPS) <= exact);
            assert!(exact < (cut + 1) * u128::from(BPS));
            kani::cover!(cut * u128::from(BPS) < exact, "a cut is rounded down");
        }
    }
}

/// One validator `[9; 32]` with one delegation, seated with nothing pending:
/// the state `end_epoch` pays rewards in.
///
/// The start state is written by hand (pending maps cleared, `active` set),
/// not reached through a first `end_epoch`, because that call adds a second
/// pass over every collection to a symbolic state CBMC already struggles with.
/// It is reachable: an `end_epoch` after `register` and `delegate` clears both
/// pending maps and seats the ranked eligible set, which with one validator
/// whose bond meets `min_self_bond` is exactly `[id]`.
fn seated(bond: Amount, delegation: Amount, commission: u16) -> Staking {
    let params = Params {
        min_self_bond: Amount::ONE,
        min_delegation: Amount::ONE,
        max_commission_bps: 10_000,
        ..Params::DEVNET
    };
    let id = [9; 32];
    let mut s = Staking::new(params).unwrap();
    let _ = s.register([1; 32], id, bond, commission).unwrap();
    let _ = s.delegate([2; 32], id, delegation).unwrap();
    s.pending_self.clear();
    s.pending_delegations.clear();
    s.active = vec![id];
    s
}

/// Runs one epoch boundary and returns `(held before, held after, credited,
/// burned)`, after asserting that it never debits an account and that held +
/// pool before equals held + credits + burns after. `pay_rewards` checks
/// `pool - paid`, so `end_epoch` returning `Err` would mean it paid more than
/// the pool: the `unwrap` is itself the no-overpayment check.
fn conserving_epoch(
    s: &mut Staking,
    rounds: u64,
    authored: u64,
    pool: Amount,
) -> (Amount, Amount, Amount, Amount) {
    let mut authored_by = BTreeMap::new();
    authored_by.insert([9; 32], authored);
    let participation = Participation {
        rounds,
        expected: BTreeMap::new(),
        authored: authored_by,
    };
    let before = s.held().unwrap();
    let out = s.end_epoch(&participation, pool).unwrap();
    let mut credited = Amount::ZERO;
    let mut burned = Amount::ZERO;
    for e in &out.effects {
        match e {
            Effect::Credit(_, a) => credited = credited.checked_add(*a).unwrap(),
            Effect::Burn(b) => burned = burned.checked_add(*b).unwrap(),
            Effect::Debit(..) => panic!("an epoch boundary debited an account"),
        }
    }
    let after = s.held().unwrap();
    let outgoing = after
        .checked_add(credited)
        .and_then(|x| x.checked_add(burned));
    assert!(before.checked_add(pool) == outgoing);
    (before, after, credited, burned)
}

/// One epoch boundary, rewards and the downtime slash together, conserves
/// value (see [`conserving_epoch`]).
///
/// One validator with one delegation (see [`seated`]), every amount below 2^8
/// (see [`small_amount`]), every commission up to 100%, and every participation
/// count below 2^8, so both a paid epoch and a down one are explored. Bounded,
/// and said so: the collections hold one entry each, and `tests/scenario.rs`
/// covers many validators by execution. The two harnesses after it split this
/// one into a paid and a down epoch with the rates and counts concrete.
///
/// No verdict yet: still in symbolic execution when stopped at 25 min in WSL
/// (2026-11-23), so it runs only in `kani-heavy.yml`.
#[kani::proof]
#[kani::unwind(40)]
fn an_epoch_boundary_conserves_value() {
    let bond = small_amount();
    let delegation = small_amount();
    let pool = small_amount();
    let commission: u16 = kani::any();
    kani::assume(u64::from(commission) <= BPS);
    kani::assume(!bond.is_zero() && !delegation.is_zero());
    let mut s = seated(bond, delegation, commission);

    let rounds: u8 = kani::any();
    let authored: u8 = kani::any();
    kani::assume(authored <= rounds);
    let (before, after, credited, burned) =
        conserving_epoch(&mut s, u64::from(rounds), u64::from(authored), pool);

    kani::cover!(
        after == before && !credited.is_zero() && !burned.is_zero(),
        "a reward is paid and its rounding dust burned"
    );
    kani::cover!(after < before, "a down validator is slashed");
}

/// A paid epoch conserves value and leaves the stake untouched: the validator
/// led its one slot, so there is no slash, and the pool leaves as credits plus
/// a dust burn. Amounts below 2^8, commission fixed at `Params::DEVNET`'s
/// maximum (2,000 bps). Bounded, and said so.
#[kani::proof]
#[kani::unwind(40)]
fn a_paid_epoch_conserves_value() {
    let bond = small_amount();
    let delegation = small_amount();
    let pool = small_amount();
    kani::assume(!bond.is_zero() && !delegation.is_zero());
    let mut s = seated(bond, delegation, Params::DEVNET.max_commission_bps);

    let (before, after, credited, burned) = conserving_epoch(&mut s, 1, 1, pool);
    assert!(after == before);
    assert!(credited.checked_add(burned) == Some(pool));
    kani::cover!(!credited.is_zero(), "a reward is paid");
    kani::cover!(
        !credited.is_zero() && !burned.is_zero(),
        "a reward is paid and its rounding dust burned"
    );
}

/// A down epoch conserves value: the validator missed its one slot, so the
/// boundary slashes it at `Params::DEVNET`'s downtime rate (100 bps), pays
/// nothing, and burns the pool with the cut. Stake below 2^16 (so a 1% cut can
/// be non-zero), pool below 2^8. Bounded, and said so.
#[kani::proof]
#[kani::unwind(40)]
fn a_down_epoch_slashes_and_conserves_value() {
    let bond = Amount::from_u64(u64::from(kani::any::<u16>()));
    let delegation = Amount::from_u64(u64::from(kani::any::<u16>()));
    let pool = small_amount();
    kani::assume(!bond.is_zero() && !delegation.is_zero());
    let mut s = seated(bond, delegation, 0);

    let (before, after, credited, _) = conserving_epoch(&mut s, 1, 0, pool);
    assert!(credited.is_zero());
    assert!(after <= before);
    kani::cover!(after < before, "a down validator is slashed");
}

/// A double-sign slash conserves value, as `a_double_sign_slash_conserves_value`
/// above, with the bond and delegation below 2^16 (the full-width version gave
/// no verdict in 15 min) and `Params::DEVNET`'s minimums and launch rate
/// (5,000 bps), on the real state machine. Bounded, and said so.
#[kani::proof]
#[kani::unwind(40)]
fn a_bounded_double_sign_slash_conserves_value() {
    let bond = Amount::from_u64(u64::from(kani::any::<u16>()));
    let delegation = Amount::from_u64(u64::from(kani::any::<u16>()));
    let params = Params::DEVNET;
    kani::assume(bond >= params.min_self_bond && delegation >= params.min_delegation);
    let mut s = Staking::new(params).unwrap();
    let _ = s.register([1; 32], [9; 32], bond, 0).unwrap();
    let _ = s.delegate([2; 32], [9; 32], delegation).unwrap();
    let before = s.held().unwrap();
    let effects = s.slash_double_sign([9; 32]).unwrap();
    let mut burned = Amount::ZERO;
    for e in &effects {
        match e {
            Effect::Burn(b) => burned = burned.checked_add(*b).unwrap(),
            _ => panic!("a slash moved value to an account"),
        }
    }
    assert!(s.held().unwrap().checked_add(burned) == Some(before));
    kani::cover!(!burned.is_zero(), "a slash burns stake");
}
