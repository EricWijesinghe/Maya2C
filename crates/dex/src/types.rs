//! Identifiers, units, and the scale every price is quoted in.

use maya_amount::Amount;

/// Identifier of a tradable asset.
///
/// The native coin is [`NATIVE_ASSET`], all zeros. Every other asset is a
/// 32-byte value the node derives; this crate never inspects one beyond
/// comparing it for equality and ordering, so it does not need to know how.
pub type AssetId = [u8; 32];

/// The native coin.
///
/// All zeros rather than a derived value, so the node can route it to the
/// existing account record instead of the multi-asset keyspace without a table
/// lookup. Nothing derived can collide with it: a BLAKE3 output of all zeros is
/// not a value anyone can produce.
pub const NATIVE_ASSET: AssetId = [0u8; 32];

/// Identifier of a trading pair.
pub type PairId = [u8; 32];

/// Fixed-point scale for prices.
///
/// A price is quote base-units per [`PRICE_SCALE`] base base-units, and like
/// every amount it is 256-bit (ADR-046). Every `price * amount` is formed as a
/// 512-bit product and reduced by [`crate::math::mul_div_floor`], so it cannot
/// overflow (a documented exception to the 256-bit rule, DECISIONS 2026-11-01).
/// `1e24` matches the 24 decimals of a coin (ADR-068): a price resolves one
/// base unit per coin, so a cheap asset still has an exact price. The bound:
/// a spot price `reserve_quote * 1e24 / reserve_base` is at most
/// 2^104 * 2^80 = 2^184, well inside 256 bits.
///
/// It is *not* a decimals constant for assets. Every amount in this crate, and
/// everywhere in the node, is in base units.
pub const PRICE_SCALE: Amount = Amount::from_u128(1_000_000_000_000_000_000_000_000);

/// Largest total supply an asset may register: the largest reserve whose
/// scaled price `reserve * PRICE_SCALE` still fits 256 bits (about 2^176).
///
/// Without it a 2^200-unit asset paired against one base unit overflows
/// [`crate::amm::Pool::spot_price`], every batch on that pair refunds, and the
/// pair id (which the attacker took first) is frozen (#279 security review).
/// It is the whole 256-bit range the price path can carry, not a lower guess.
pub const MAX_ASSET_SUPPLY: Amount = match Amount::MAX.checked_div(PRICE_SCALE) {
    Some(max) => max,
    None => panic!("PRICE_SCALE is non-zero"),
};

/// Which way value moves through a pair.
///
/// A pair has a canonical `base` and `quote` asset, fixed when the pool is
/// created and ordered by the node so that one pair of assets yields one pair
/// identifier. Everything downstream is expressed relative to that ordering
/// rather than to whichever asset a particular trader happened to name first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    /// Selling base, receiving quote.
    BaseToQuote,
    /// Selling quote, receiving base.
    QuoteToBase,
}

impl Direction {
    /// The opposite direction.
    #[must_use]
    pub const fn flip(self) -> Self {
        match self {
            Self::BaseToQuote => Self::QuoteToBase,
            Self::QuoteToBase => Self::BaseToQuote,
        }
    }

    /// Stable byte tag for the wire encoding. Never renumber — it is consensus.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::BaseToQuote => 0,
            Self::QuoteToBase => 1,
        }
    }

    /// Decodes a wire tag.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::BaseToQuote),
            1 => Some(Self::QuoteToBase),
            _ => None,
        }
    }
}
