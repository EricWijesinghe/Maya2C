//! Price feeds: what a quorum agreed, and when.
//!
//! ## Why the median
//!
//! A mean is a weighted vote, and one compromised authority holds a weight
//! bounded only by the number it is willing to sign. A median is not: until a
//! liar holds a majority of the quorum, moving the median requires moving the
//! honest values, which is to say it requires being right.
//!
//! With an even count there is no single middle, and the two candidates are
//! averaged in most textbook definitions. Not here — averaging introduces a
//! division, a division introduces rounding, and a rounding rule in a price is
//! a systematic bias in whichever direction it falls. The lower of the two
//! middles is taken instead: deterministic, exact, and conservative in the same
//! direction every time.
//!
//! ## One scale, chain-wide
//!
//! Every price is an integer in units of `1 / FEED_SCALE`. There is no
//! per-feed decimals field, for the reason [`crate::rpc::market`] already gives
//! about supply: saying "whole units" in one place and "base units" everywhere
//! else is how a number ends up wrong by a power of ten. A feed that cannot be
//! expressed at this scale is a feed this oracle does not carry.
//!
//! ## What a signature commits to
//!
//! The feed, the round, the value, and the height the authority observed at —
//! under a domain of its own. All four matter. Without the feed id a signature
//! for MAYA/USD is a signature for BTC/USD; without the round it replays
//! forever; without the height a stale observation is indistinguishable from a
//! fresh one; and without the domain separator it is a transaction signature.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Fixed-point scale for every price: nine decimal places.
///
/// The same scale the trading engine quotes at, chosen for the same reason and
/// deliberately *not* imported from it. A shared constant would couple the
/// oracle to the DEX so that neither could change its precision without the
/// other, and they are answerable to different things — the DEX's scale is a
/// property of its curve arithmetic, this one is a property of what a price
/// feed can express.
pub const FEED_SCALE: u64 = 1_000_000_000;

/// Bytes in a feed's human-readable name, right-padded with zeros.
pub const FEED_NAME_LEN: usize = 16;

/// Encoded size of a [`FeedRecord`].
pub const FEED_RECORD_LEN: usize = FEED_NAME_LEN + 8 + 8 + 8 + 1;

/// Domain separator for what an authority signs.
///
/// Distinct from the transaction domain, so an observation can never be
/// replayed as a transaction signature or the reverse.
const OBSERVATION_DOMAIN: &[u8] = b"maya-oracle.feed-observation.v1";

/// Domain separator for deriving a feed identifier.
const FEED_ID_DOMAIN: &str = "maya-oracle feed id v1";

/// The current state of one price feed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedRecord {
    /// Human-readable name, such as `MAYA/USD`, zero-padded.
    pub name: [u8; FEED_NAME_LEN],
    /// The agreed price, in units of `1 / FEED_SCALE`.
    pub value: u64,
    /// Round this value came from. Strictly increasing.
    pub round: u64,
    /// Height at which the chain accepted it.
    ///
    /// **This is the clock.** Not a timestamp: block timestamps on this chain
    /// are used only for difficulty retargeting and carry no validity rule at
    /// all, so a miner may write whatever it likes in one. A freshness check
    /// against a timestamp would read as safety and provide none. See
    /// `docs/oracle.md`.
    pub updated_height: u64,
    /// How many authorities signed the round this value came from.
    pub observations: u8,
}

impl FeedRecord {
    /// How many blocks old this value is at `height`.
    ///
    /// Saturating, so a record somehow ahead of the chain reads as fresh rather
    /// than as enormously stale. That case means a reorg is in progress, and
    /// reporting a wrapped age would turn it into a wrong answer rather than a
    /// conservative one.
    #[must_use]
    pub const fn age(&self, height: u64) -> u64 {
        height.saturating_sub(self.updated_height)
    }

    /// Whether this value is no older than `max_age` blocks at `height`.
    #[must_use]
    pub const fn is_fresh(&self, height: u64, max_age: u64) -> bool {
        self.age(height) <= max_age
    }

    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(FEED_RECORD_LEN);
        buf.extend_from_slice(&self.name);
        buf.extend_from_slice(&self.value.to_le_bytes());
        buf.extend_from_slice(&self.round.to_le_bytes());
        buf.extend_from_slice(&self.updated_height.to_le_bytes());
        buf.push(self.observations);
        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is truncated or carries
    /// trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let record = Self {
            name: reader.read_array::<FEED_NAME_LEN>()?,
            value: reader.read_u64()?,
            round: reader.read_u64()?,
            updated_height: reader.read_u64()?,
            observations: reader.read_u8()?,
        };
        reader.finish()?;
        Ok(record)
    }

    /// Hashes the record into a Merkle leaf.
    #[must_use]
    pub fn leaf(&self, feed_id: &[u8; 32]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-oracle feed leaf v1");
        hasher.update(feed_id);
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }
}

/// Validates a feed name.
///
/// Uppercase ASCII, digits, and `/` only, at least one character, no interior
/// padding. The same rule an asset symbol gets and for the same reason: a name
/// carrying control characters or a look-alike glyph is a tool aimed at
/// whichever interface renders it.
///
/// # Errors
///
/// Returns [`NodeError::InvalidFeedName`] describing what was wrong.
pub fn validate_name(name: &[u8; FEED_NAME_LEN]) -> Result<()> {
    let length = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(FEED_NAME_LEN);
    if length == 0 {
        return Err(NodeError::InvalidFeedName {
            reason: "empty".to_string(),
        });
    }

    for (index, byte) in name.iter().enumerate() {
        let permitted = if index < length {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'/'
        } else {
            // One name, one encoding: everything after the first zero must also
            // be zero.
            *byte == 0
        };
        if !permitted {
            return Err(NodeError::InvalidFeedName {
                reason: format!("byte {index} is not permitted"),
            });
        }
    }
    Ok(())
}

/// Renders a feed name as text, stopping at the padding.
#[must_use]
pub fn name_text(name: &[u8; FEED_NAME_LEN]) -> String {
    let length = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(FEED_NAME_LEN);
    String::from_utf8_lossy(&name[..length]).into_owned()
}

/// Derives a feed identifier from its name.
///
/// From the name alone, with no creator and no nonce, so that `MAYA/USD` is one
/// feed chain-wide and a contract can compute the identifier it needs without
/// consulting a registry. The cost of that choice is that a name is a scarce
/// global — which is correct here, because two feeds both calling themselves
/// `MAYA/USD` is precisely the ambiguity a contract must not have to resolve.
#[must_use]
pub fn derive_feed_id(name: &[u8; FEED_NAME_LEN]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(FEED_ID_DOMAIN);
    hasher.update(name);
    *hasher.finalize().as_bytes()
}

/// The bytes an authority signs to attest one observation.
#[must_use]
pub fn observation_bytes(
    feed_id: &[u8; 32],
    round: u64,
    value: u64,
    observed_height: u64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(OBSERVATION_DOMAIN.len() + 32 + 24);
    buf.extend_from_slice(OBSERVATION_DOMAIN);
    buf.extend_from_slice(feed_id);
    buf.extend_from_slice(&round.to_le_bytes());
    buf.extend_from_slice(&value.to_le_bytes());
    buf.extend_from_slice(&observed_height.to_le_bytes());
    buf
}

/// The median of a set of observed values.
///
/// Sorts a copy, so the caller's order — which is whatever order the
/// submitter chose — cannot change the result.
///
/// Returns `None` for an empty set.
#[must_use]
pub fn median(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    // The lower middle on an even count. See the module documentation: the
    // average of the two middles would introduce a rounding rule, and a
    // rounding rule in a price is a bias.
    Some(sorted[(sorted.len() - 1) / 2])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_median_ignores_the_order_it_was_given() {
        let ascending = [1u64, 2, 3, 4, 5];
        let descending = [5u64, 4, 3, 2, 1];
        let shuffled = [3u64, 5, 1, 4, 2];
        assert_eq!(median(&ascending), Some(3));
        assert_eq!(median(&descending), Some(3));
        assert_eq!(median(&shuffled), Some(3));
    }

    #[test]
    fn an_even_count_takes_the_lower_middle_rather_than_averaging() {
        assert_eq!(median(&[10, 20]), Some(10));
        assert_eq!(median(&[1, 2, 3, 4]), Some(2));
    }

    #[test]
    fn one_liar_cannot_move_the_median_of_an_honest_majority() {
        // The property the whole aggregation rests on. A mean of these is
        // 3689348814741910323; the median is unmoved.
        let honest = [100u64, 101, 102, 103];
        let mut with_liar = honest.to_vec();
        with_liar.push(u64::MAX);
        assert_eq!(median(&with_liar), Some(102));

        let mut with_low_liar = honest.to_vec();
        with_low_liar.push(0);
        assert_eq!(median(&with_low_liar), Some(101));
    }

    #[test]
    fn an_empty_set_has_no_median() {
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn a_feed_name_must_be_renderable_and_unambiguous() {
        assert!(validate_name(b"MAYA/USD\0\0\0\0\0\0\0\0").is_ok());
        assert!(validate_name(b"BTC/USD\0\0\0\0\0\0\0\0\0").is_ok());
        // Lowercase, punctuation, and interior padding are all refused; the
        // last is the subtle one, because it needs no unusual characters.
        assert!(validate_name(b"maya/usd\0\0\0\0\0\0\0\0").is_err());
        assert!(validate_name(b"MAYA-USD\0\0\0\0\0\0\0\0").is_err());
        assert!(validate_name(b"MAYA\0USD\0\0\0\0\0\0\0\0").is_err());
        assert!(validate_name(&[0u8; FEED_NAME_LEN]).is_err());
    }

    #[test]
    fn a_feed_identifier_is_a_function_of_the_name_alone() {
        // So a contract can derive the identifier it needs without a registry
        // lookup, and so two nodes never disagree about which feed is which.
        let name = *b"MAYA/USD\0\0\0\0\0\0\0\0";
        assert_eq!(derive_feed_id(&name), derive_feed_id(&name));
        assert_ne!(
            derive_feed_id(&name),
            derive_feed_id(b"BTC/USD\0\0\0\0\0\0\0\0\0")
        );
    }

    #[test]
    fn changing_any_signed_field_changes_the_bytes_that_were_signed() {
        let base = observation_bytes(&[1u8; 32], 1, 100, 10);
        assert_ne!(base, observation_bytes(&[2u8; 32], 1, 100, 10));
        assert_ne!(base, observation_bytes(&[1u8; 32], 2, 100, 10));
        assert_ne!(base, observation_bytes(&[1u8; 32], 1, 101, 10));
        assert_ne!(base, observation_bytes(&[1u8; 32], 1, 100, 11));
    }

    #[test]
    fn freshness_is_measured_in_blocks_and_saturates_backwards() {
        let record = FeedRecord {
            updated_height: 100,
            ..FeedRecord::default()
        };
        assert_eq!(record.age(100), 0);
        assert_eq!(record.age(105), 5);
        // A record ahead of the chain — a reorg in progress — reads as fresh
        // rather than as a wrapped, enormous age.
        assert_eq!(record.age(99), 0);

        assert!(record.is_fresh(105, 5));
        assert!(!record.is_fresh(106, 5));
    }
}
