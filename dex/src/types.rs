//! Identifiers, units, and the scale every price is quoted in.

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
/// A price is quote base-units per [`PRICE_SCALE`] base base-units. `1e9` is
/// chosen so that `price * amount` stays inside `u128` for every `u64` amount
/// (`2^64 * 2^64 * 10^9` does not, but `amount * price` does — the products are
/// always formed as `u128 * u128` and reduced by [`crate::math::mul_div_floor`]
/// before they can grow), and so that a price can express nine decimal digits
/// of precision, which is more than the eight the smallest sensible base unit
/// needs.
///
/// It is *not* a decimals constant for assets. Every amount in this crate, and
/// everywhere in the node, is in base units.
pub const PRICE_SCALE: u128 = 1_000_000_000;

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
