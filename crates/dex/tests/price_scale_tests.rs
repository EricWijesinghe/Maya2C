//! Price precision at the 24-decimal scale (DECISIONS 2026-11-01).
//!
//! At the old `1e9` scale a pool quoting a billionth of a coin per coin, or
//! less, had a spot price of zero or one: the asset had no usable price. These
//! pin that `1e24` resolves it, and that the worst case still fits 256 bits.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_amount::Amount;
use maya_dex::amm::Pool;
use maya_dex::fees::FeeSchedule;
use maya_dex::types::{MAX_ASSET_SUPPLY, PRICE_SCALE};

/// One coin in base units (ADR-068: 24 decimals).
const COIN: u128 = 1_000_000_000_000_000_000_000_000;

fn pool(base: Amount, quote: Amount) -> Pool {
    Pool {
        reserve_base: base,
        reserve_quote: quote,
        total_shares: Amount::ONE,
        fees: FeeSchedule::standard(),
    }
}

#[test]
fn price_scale_is_one_coin() {
    assert_eq!(PRICE_SCALE, Amount::from_u128(COIN));
}

#[test]
fn a_very_cheap_asset_keeps_an_exact_price() {
    // 1 coin of quote against 3e9 coins of base: 1/3e9 coin per coin.
    let base = Amount::from_u128(3_000_000_000)
        .checked_mul(Amount::from_u128(COIN))
        .unwrap();
    let price = pool(base, Amount::from_u128(COIN))
        .spot_price()
        .expect("price");
    // At 1e9 this floors to 0; at 1e24 it is 333,333,333,333,333.
    assert_eq!(price, Amount::from_u128(333_333_333_333_333));
}

#[test]
fn the_largest_spot_price_fits_256_bits() {
    // Whole supply (21M coins, about 2^104) of quote against one base unit:
    // 2^104 * 2^80 = 2^184, inside 256 bits, so no Overflow.
    let supply = Amount::from_u128(21_000_000)
        .checked_mul(Amount::from_u128(COIN))
        .unwrap();
    let price = pool(Amount::ONE, supply).spot_price().expect("fits");
    assert_eq!(price, supply.checked_mul(PRICE_SCALE).unwrap());
    assert!(
        price.bits() <= 185,
        "bound is 2^184, got {} bits",
        price.bits()
    );
}

#[test]
fn a_pool_at_the_supply_cap_still_has_a_price() {
    // The worst pool registration allows: the cap against one base unit.
    let price = pool(Amount::ONE, MAX_ASSET_SUPPLY)
        .spot_price()
        .expect("fits");
    assert_eq!(price, MAX_ASSET_SUPPLY.checked_mul(PRICE_SCALE).unwrap());
    // One unit past the cap is exactly where the price path would overflow.
    let over = MAX_ASSET_SUPPLY.checked_add(Amount::ONE).unwrap();
    assert!(pool(Amount::ONE, over).spot_price().is_err());
}

#[test]
fn the_cap_covers_every_realistic_asset() {
    // 1e14 whole tokens at 24 decimals fits with room to spare.
    let big = Amount::from_u128(100_000_000_000_000)
        .checked_mul(PRICE_SCALE)
        .unwrap();
    assert!(big < MAX_ASSET_SUPPLY);
    assert!(MAX_ASSET_SUPPLY.bits() >= 176);
}
