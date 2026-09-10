//! Where a reporter is, at the only granularity this service keeps.
//!
//! # Country and region, never a coordinate
//!
//! A map of a proof-of-work network is a map of where the hash rate is, and
//! that is operationally interesting and personally sensitive at the same
//! time. A pin on a street is a claim about a person's home; a shaded country
//! is a claim about an economy. The dashboard needs the second and the
//! collector therefore never learns the first.
//!
//! Concretely, three rules, each enforced here rather than left to the UI:
//!
//! 1. **The address is never stored.** A country code is derived at ingest and
//!    the address is dropped in the same function. Nothing downstream can
//!    reconstruct it, because nothing downstream is given it.
//! 2. **There are no coordinates.** Not rounded ones, not "city centre" ones.
//!    A rounded coordinate is still a coordinate, and a UI that receives one
//!    will eventually plot it.
//! 3. **A country with too few reporters is folded into [`Region::OTHER`].**
//!    See [`MIN_REPORTERS`]: "one miner in Liechtenstein" is not aggregate
//!    data, it is an individual with a public dashboard pointing at them.
//!
//! # The country comes from a proxy, not from the reporter
//!
//! A self-reported country is a text field an attacker fills in, and a map
//! built from it looks exactly like a real one. The collector reads a
//! geolocation header set by the reverse proxy in front of it — the same
//! trust boundary, and the same "only when a proxy is actually there" rule,
//! as the faucet's forwarded-address handling.
//!
//! A reporter whose country cannot be established is [`Region::UNKNOWN`],
//! which is counted and shown. An unknown bucket that is visibly large is
//! information; silently dropping those reporters would quietly bias every
//! number on the page.

use std::collections::HashMap;

/// Fewest reporters a country must have before it is named.
///
/// Below this the country is folded into [`Region::OTHER`]. Five is a
/// judgement, not a theorem — it is small enough that the map stays
/// informative and large enough that no single operator is identified by
/// their flag. It is deliberately a constant rather than a configuration key:
/// an operator tuning it to 1 to "see more detail" is the failure this
/// prevents.
pub const MIN_REPORTERS: usize = 5;

/// A place, at country granularity.
///
/// A two-letter ISO 3166-1 alpha-2 code, or one of the two reserved buckets.
/// Stored as a fixed pair of bytes rather than a `String` because it is
/// exactly two characters and a map keyed on it is allocated once per country
/// rather than once per report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Region([u8; 2]);

impl Region {
    /// Reporters whose country could not be established.
    ///
    /// Shown rather than dropped: a large unknown bucket is a fact about the
    /// deployment, and hiding it would bias every percentage on the page.
    pub const UNKNOWN: Self = Self(*b"??");

    /// Countries with fewer than [`MIN_REPORTERS`] reporters, combined.
    pub const OTHER: Self = Self(*b"ZZ");

    /// Parses an ISO 3166-1 alpha-2 code.
    ///
    /// Returns [`Region::UNKNOWN`] for anything else — a missing header, a
    /// proxy that writes `XX`, a lowercase code, a country name. The
    /// alternative is rejecting the whole report over a header nobody
    /// controls, which would lose real hash-rate data to fix a cosmetic
    /// problem.
    #[must_use]
    pub fn parse(code: &str) -> Self {
        let bytes = code.as_bytes();
        if bytes.len() != 2 || !bytes.iter().all(u8::is_ascii_alphabetic) {
            return Self::UNKNOWN;
        }
        let code = [bytes[0].to_ascii_uppercase(), bytes[1].to_ascii_uppercase()];
        // `ZZ` is user-assignable in ISO 3166, so no real country claims it —
        // which is why it is safe to reserve for the folded bucket. A proxy
        // that sends it anyway lands in the bucket it names, which is correct.
        Self(code)
    }

    /// The two-letter code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // The bytes are ASCII by construction: every path either parses two
        // alphabetic bytes or uses one of the two reserved constants.
        std::str::from_utf8(&self.0).unwrap_or("??")
    }

    /// Whether this is a real country rather than a reserved bucket.
    #[must_use]
    pub fn is_country(&self) -> bool {
        *self != Self::UNKNOWN && *self != Self::OTHER
    }
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for Region {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Region {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Ok(Self::parse(&code))
    }
}

/// Folds countries below [`MIN_REPORTERS`] into [`Region::OTHER`].
///
/// Applied at the point the snapshot is built rather than at ingest, because
/// a country can cross the threshold in either direction as miners come and
/// go: a country folded at ingest could never be un-folded, and one that was
/// large yesterday would keep its name today with two reporters left.
///
/// [`Region::UNKNOWN`] is never folded. It is not a country and it carries no
/// identifying information — it is the count of reporters the proxy could not
/// place, and suppressing it would hide the size of the gap.
#[must_use]
pub fn fold_small<T, F>(
    counts: HashMap<Region, (usize, T)>,
    combine: F,
) -> HashMap<Region, (usize, T)>
where
    T: Default,
    F: Fn(&mut T, T),
{
    let mut folded: HashMap<Region, (usize, T)> = HashMap::new();

    for (region, (reporters, value)) in counts {
        let key = if region.is_country() && reporters < MIN_REPORTERS {
            Region::OTHER
        } else {
            region
        };

        let entry = folded.entry(key).or_insert_with(|| (0, T::default()));
        entry.0 += reporters;
        combine(&mut entry.1, value);
    }

    folded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_code_is_upper_cased() {
        assert_eq!(Region::parse("de").as_str(), "DE");
        assert_eq!(Region::parse("US").as_str(), "US");
        assert_eq!(Region::parse("Jp").as_str(), "JP");
    }

    #[test]
    fn anything_that_is_not_a_code_is_unknown() {
        // A missing header, a proxy that writes a placeholder, a country
        // name. None of these should cost the report — the hash rate in it is
        // real whether or not the geolocation worked.
        for input in ["", "U", "USA", "Germany", "12", "U1", "  "] {
            assert_eq!(Region::parse(input), Region::UNKNOWN, "{input:?}");
        }
    }

    #[test]
    fn the_reserved_buckets_are_not_countries() {
        assert!(!Region::UNKNOWN.is_country());
        assert!(!Region::OTHER.is_country());
        assert!(Region::parse("FR").is_country());
    }

    #[test]
    fn a_country_below_the_floor_is_folded() {
        // The privacy property. One miner in a small country is an
        // individual, and a public map that names them is a public map
        // pointing at them.
        let mut counts = HashMap::new();
        counts.insert(Region::parse("US"), (40usize, 4_000u64));
        counts.insert(Region::parse("LI"), (1, 100));
        counts.insert(Region::parse("MC"), (2, 200));

        let folded = fold_small(counts, |total, value| *total += value);

        assert_eq!(folded.get(&Region::parse("US")), Some(&(40, 4_000)));
        assert!(!folded.contains_key(&Region::parse("LI")));
        assert!(!folded.contains_key(&Region::parse("MC")));
        assert_eq!(
            folded.get(&Region::OTHER),
            Some(&(3, 300)),
            "the folded reporters and their hash rate are kept, only the name is dropped"
        );
    }

    #[test]
    fn folding_conserves_the_totals() {
        // Folding hides which country, never how much. A map whose parts do
        // not sum to the network total is a map somebody will chase a bug in.
        let mut counts = HashMap::new();
        for (index, code) in ["US", "DE", "LI", "MC", "SM", "AD"].iter().enumerate() {
            counts.insert(Region::parse(code), (index + 1, (index as u64 + 1) * 10));
        }
        let before: (usize, u64) = counts
            .values()
            .fold((0, 0), |acc, (n, v)| (acc.0 + n, acc.1 + v));

        let folded = fold_small(counts, |total, value| *total += value);
        let after: (usize, u64) = folded
            .values()
            .fold((0, 0), |acc, (n, v)| (acc.0 + n, acc.1 + v));

        assert_eq!(before, after);
    }

    #[test]
    fn the_unknown_bucket_is_never_folded() {
        // It is not a country and identifies nobody. Folding it into `OTHER`
        // would hide how much of the map is guesswork.
        let mut counts = HashMap::new();
        counts.insert(Region::UNKNOWN, (1usize, 50u64));
        counts.insert(Region::parse("US"), (10, 1_000));

        let folded = fold_small(counts, |total, value| *total += value);
        assert_eq!(folded.get(&Region::UNKNOWN), Some(&(1, 50)));
    }

    #[test]
    fn a_country_exactly_at_the_floor_keeps_its_name() {
        let mut counts = HashMap::new();
        counts.insert(Region::parse("SE"), (MIN_REPORTERS, 500u64));

        let folded = fold_small(counts, |total, value| *total += value);
        assert_eq!(
            folded.get(&Region::parse("SE")),
            Some(&(MIN_REPORTERS, 500))
        );
    }

    #[test]
    fn a_region_round_trips_through_json() {
        let json = serde_json::to_string(&Region::parse("br")).expect("serialize");
        assert_eq!(json, r#""BR""#);
        assert_eq!(
            serde_json::from_str::<Region>(&json).expect("deserialize"),
            Region::parse("BR")
        );
    }
}
