//! Kani harnesses. Compiled only under `cargo kani -p maya-staking`.
//!
//! Scalar properties get unbounded proofs. The state-machine properties are
//! checked on one validator with one delegation — enough for CBMC to explore
//! every amount, not enough to call the collections proved; the scenario test
//! (`tests/scenario.rs`) covers a thousand validators by execution.

use crate::Params;
use crate::params::{BPS, bps_of};
use crate::types::{Effect, Staking};

#[kani::proof]
fn bps_of_never_exceeds_the_amount() {
    let amount: u64 = kani::any();
    let bps: u16 = kani::any();
    kani::assume(u64::from(bps) <= BPS);
    assert!(bps_of(amount, bps) <= amount);
}

#[kani::proof]
#[kani::unwind(4)]
fn a_double_sign_slash_conserves_value() {
    let bond: u64 = kani::any();
    let delegation: u64 = kani::any();
    let params = Params::DEVNET;
    kani::assume(bond >= params.min_self_bond && bond < u64::MAX / 4);
    kani::assume(delegation >= params.min_delegation && delegation < u64::MAX / 4);
    let mut s = Staking::new(params).unwrap();
    let _ = s.register([1; 32], [9; 32], bond, 0).unwrap();
    let _ = s.delegate([2; 32], [9; 32], delegation).unwrap();
    let before = s.held();
    let effects = s.slash_double_sign([9; 32]).unwrap();
    let burned: u128 = effects
        .iter()
        .map(|e| match e {
            Effect::Burn(b) => u128::from(*b),
            _ => 0,
        })
        .sum();
    assert!(s.held() + burned == before);
}
