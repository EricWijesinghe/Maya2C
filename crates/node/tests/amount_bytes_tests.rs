//! The node converts between `primitive_types::U256` (its balances) and
//! `maya_amount::Amount` (what fee-market, dex and ledger-math compute with)
//! by copying 32 little-endian bytes (ADR-048). This pins that the two types
//! agree on those bytes and on the numbers they mean, at the edges and for
//! random values.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_fee_market::Amount;
use primitive_types::U256;

/// Values checked by the random sweep.
const SWEEP: usize = 20_000;

fn to_amount(v: U256) -> Amount {
    let mut buf = [0u8; 32];
    v.to_little_endian(&mut buf);
    Amount::from_le_bytes(buf)
}

fn to_u256(a: Amount) -> U256 {
    U256::from_little_endian(&a.to_le_bytes())
}

#[test]
fn edges_survive_the_byte_copy_both_ways() {
    for v in [
        U256::zero(),
        U256::one(),
        U256::from(u64::MAX),
        U256::from(u64::MAX) + 1,
        U256::MAX,
        U256::MAX - 1,
    ] {
        assert_eq!(to_u256(to_amount(v)), v, "{v}");
    }
    assert_eq!(to_amount(U256::MAX), Amount::MAX);
    assert_eq!(to_amount(U256::zero()), Amount::ZERO);
}

#[test]
fn both_print_the_same_decimal_text() {
    // The RPC writes U256 amounts as base-10 strings (amount_serde); an
    // Amount printed by fee-market must read back as the same number.
    for v in [U256::zero(), U256::from(1_000_000_000u64), U256::MAX] {
        assert_eq!(to_amount(v).to_string(), v.to_string());
    }
}

/// splitmix64: deterministic, so a failure reproduces; no new dependency.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A random value of a random width, so small and huge values both appear.
fn limbs(state: &mut u64) -> [u64; 4] {
    let keep = usize::try_from(next(state) % 5).expect("< 5");
    let mut l = [next(state), next(state), next(state), next(state)];
    for limb in l.iter_mut().skip(keep) {
        *limb = 0;
    }
    l
}

#[test]
fn random_values_agree_on_bytes_order_and_arithmetic() {
    let mut state = 0x4D41_5941_3243; // "MAYA2C"
    for _ in 0..SWEEP {
        let (x, y) = (limbs(&mut state), limbs(&mut state));
        let (ux, uy) = (U256(x), U256(y));
        let (ax, ay) = (to_amount(ux), to_amount(uy));
        // U256's limbs are little-endian too, so the limbs match directly.
        assert_eq!(ax.0, x);
        assert_eq!(to_u256(ax), ux);
        assert_eq!(ax.cmp(&ay), ux.cmp(&uy));
        assert_eq!(ax.checked_add(ay).map(to_u256), ux.checked_add(uy));
        assert_eq!(ax.checked_sub(ay).map(to_u256), ux.checked_sub(uy));
        assert_eq!(ax.checked_mul(ay).map(to_u256), ux.checked_mul(uy));
        if !uy.is_zero() {
            assert_eq!(ax.checked_div(ay).map(to_u256), Some(ux / uy));
        }
        assert_eq!(ax.to_string(), ux.to_string());
    }
}
