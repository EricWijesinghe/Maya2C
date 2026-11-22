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

/// One epoch boundary, rewards and the downtime slash together, conserves
/// value: what the module held plus the reward pool equals what it holds
/// afterwards plus every credit and burn the boundary emitted. It never debits
/// an account, and it never fails.
///
/// One validator with one delegation, every amount below 2^8 (see
/// [`small_amount`]), every commission up to 100%, and every participation
/// count below 2^8, so both a paid epoch and a down one are explored. The stake
/// is past its pending epoch and seated, the state `end_epoch` pays rewards in.
/// Bounded, and said so: the collections hold one entry each, and
/// `tests/scenario.rs` covers many validators by execution. `pay_rewards`
/// checks `pool - paid`, so `end_epoch` returning `Err` would mean it paid more
/// than the pool: the `unwrap` is itself the no-overpayment check.
///
/// No verdict yet: still in symbolic execution when stopped at 25 min in WSL
/// (2026-11-23), so it runs only in `kani-heavy.yml`.
#[kani::proof]
#[kani::unwind(40)]
fn an_epoch_boundary_conserves_value() {
    let params = Params {
        min_self_bond: Amount::ONE,
        min_delegation: Amount::ONE,
        max_commission_bps: 10_000,
        ..Params::DEVNET
    };
    let bond = small_amount();
    let delegation = small_amount();
    let pool = small_amount();
    let commission: u16 = kani::any();
    kani::assume(u64::from(commission) <= BPS);
    kani::assume(!bond.is_zero() && !delegation.is_zero());

    let id = [9; 32];
    let mut s = Staking::new(params).unwrap();
    let _ = s.register([1; 32], id, bond, commission).unwrap();
    let _ = s.delegate([2; 32], id, delegation).unwrap();
    s.pending_self.clear();
    s.pending_delegations.clear();
    s.active = vec![id];

    let rounds: u8 = kani::any();
    let authored: u8 = kani::any();
    kani::assume(authored <= rounds);
    let mut authored_by = BTreeMap::new();
    authored_by.insert(id, u64::from(authored));
    let participation = Participation {
        rounds: u64::from(rounds),
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

    kani::cover!(
        after == before && !credited.is_zero() && !burned.is_zero(),
        "a reward is paid and its rounding dust burned"
    );
    kani::cover!(after < before, "a down validator is slashed");
}
