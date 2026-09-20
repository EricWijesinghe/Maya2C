//! What a reporter sends, and what the collector is willing to believe.
//!
//! # Every field here is a claim
//!
//! A telemetry report is unauthenticated data from a stranger. Nothing in it
//! is verified against the chain, and nothing in it should be presented as if
//! it were. A miner can claim any hash rate; a node can claim any height.
//!
//! Two consequences the collector actually implements:
//!
//! - **Bounds on every number** ([`MAX_HASH_RATE`], [`MAX_PEERS`]). Not to
//!   make the claims true, but to stop one absurd report from making every
//!   other number on the dashboard unreadable — a single `u64::MAX` hash rate
//!   turns a chart into a flat line at zero.
//! - **One report per reporter.** Reports are keyed by [`ReporterId`] and
//!   replace rather than accumulate, so a reporter cannot inflate the network
//!   total by sending the same figure a thousand times.
//!
//! What that still permits: a thousand fabricated reporter ids, each claiming
//! a plausible hash rate. The dashboard is a picture of what nodes *say*, and
//! `docs/telemetry.md` says so where an operator will read it. The chain's own
//! difficulty is the number that cannot be faked, and it is shown alongside
//! for exactly that reason.

use crate::region::Region;

/// Largest hash rate a single reporter may claim, in hashes per second.
///
/// 10 TH/s. Roughly three orders of magnitude above what one machine does at
/// this algorithm, so a real large farm reporting as one unit still fits,
/// while `u64::MAX` does not.
pub const MAX_HASH_RATE: u64 = 10_000_000_000_000;

/// Largest peer count a node may claim.
pub const MAX_PEERS: u32 = 10_000;

/// Longest a reporter's self-assigned name may be.
///
/// Rendered on a public page, so it is bounded and character-checked. An
/// unbounded name is a paragraph of someone else's text in everybody's
/// browser.
pub const MAX_NAME_LEN: usize = 32;

/// How long a report stays in the snapshot before the reporter is considered
/// gone.
///
/// Reporters do not say goodbye — they crash, get unplugged, or lose a
/// network. Without an expiry the dashboard's total only ever grows and the
/// number becomes the sum of everyone who has ever mined.
pub const REPORT_TTL_SECONDS: u64 = 300;

/// A reporter's stable identity.
///
/// Self-assigned and unverified: it distinguishes reporters from each other
/// so their reports replace rather than accumulate, and it does nothing else.
/// It is deliberately not derived from an address or an IP — an id that
/// identified a person would make this a tracking database.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ReporterId(String);

impl ReporterId {
    /// Accepts an id of 1..=[`MAX_NAME_LEN`] characters from
    /// `[A-Za-z0-9_-]`.
    ///
    /// # Errors
    ///
    /// [`ReportError::BadReporterId`] otherwise. The character set is narrow
    /// on purpose: this string is a map key, a log field, and a line on a
    /// public page, and the set that is safe in all three is small.
    pub fn parse(input: &str) -> Result<Self, ReportError> {
        let ok = (1..=MAX_NAME_LEN).contains(&input.len())
            && input
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');

        if ok {
            Ok(Self(input.to_string()))
        } else {
            Err(ReportError::BadReporterId)
        }
    }

    /// The id as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What is wrong with a report.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    /// The id was empty, too long, or held characters outside `[A-Za-z0-9_-]`.
    #[error("reporter id must be 1-{MAX_NAME_LEN} characters of [A-Za-z0-9_-]")]
    BadReporterId,

    /// A claimed hash rate above [`MAX_HASH_RATE`].
    #[error("hash rate above {MAX_HASH_RATE} h/s is not a claim this collector accepts")]
    ImplausibleHashRate,

    /// A claimed peer count above [`MAX_PEERS`].
    #[error("peer count above {MAX_PEERS} is not a claim this collector accepts")]
    ImplausiblePeerCount,

    /// A device or client string outside the accepted character set.
    #[error("{field} must be at most {MAX_NAME_LEN} printable ASCII characters")]
    BadLabel {
        /// Which field.
        field: &'static str,
    },
}

/// What a miner sends.
///
/// Deliberately narrow. A miner knows its hash rate, what it is running on,
/// and nothing else worth collecting — a field for "wallet address" or
/// "location" would be a field somebody fills in.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MinerReport {
    /// Self-assigned identity, so repeat reports replace rather than add.
    pub reporter: ReporterId,
    /// Claimed hash rate in hashes per second.
    pub hash_rate: u64,
    /// Backend the miner is using — `cpu`, `cuda`, `wgpu`.
    pub backend: String,
    /// Device description, as the backend reports it.
    pub device: String,
}

/// What a node sends.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NodeReport {
    /// Self-assigned identity.
    pub reporter: ReporterId,
    /// Best height this node has.
    pub height: u64,
    /// Peers it is connected to.
    pub peers: u32,
    /// Milliseconds between a block's header timestamp and this node
    /// accepting it, for the most recent block.
    ///
    /// The propagation figure. It is a difference of two clocks that are not
    /// synchronised and one of which a miner writes freely — see the chain's
    /// ninth invariant — so it is indicative, never a measurement. The
    /// dashboard labels it as a median across reporters for that reason: one
    /// node's skew shows up as an outlier rather than as the number.
    pub propagation_ms: Option<u64>,
    /// Client version string.
    pub client: String,
}

/// Anything a reporter can send.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Report {
    /// A miner's hash rate.
    Miner(MinerReport),
    /// A node's view of the chain.
    Node(NodeReport),
}

impl Report {
    /// Who sent it.
    #[must_use]
    pub fn reporter(&self) -> &ReporterId {
        match self {
            Self::Miner(report) => &report.reporter,
            Self::Node(report) => &report.reporter,
        }
    }

    /// Checks the claims are within the bounds the collector will accept.
    ///
    /// # Errors
    ///
    /// The [`ReportError`] naming the field. Rejecting rather than clamping:
    /// a clamped report is a number the reporter did not send, presented as
    /// though they had.
    pub fn validate(&self) -> Result<(), ReportError> {
        match self {
            Self::Miner(report) => {
                if report.hash_rate > MAX_HASH_RATE {
                    return Err(ReportError::ImplausibleHashRate);
                }
                check_label("backend", &report.backend)?;
                check_label("device", &report.device)
            }
            Self::Node(report) => {
                if report.peers > MAX_PEERS {
                    return Err(ReportError::ImplausiblePeerCount);
                }
                check_label("client", &report.client)
            }
        }
    }
}

/// A report as the collector stored it.
///
/// The reporter's address is **not** a field, and that is the point. It is
/// used to derive [`Stored::region`] at ingest and dropped in the same
/// function — see `region.rs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// The report itself.
    pub report: Report,
    /// Country, at the only granularity kept.
    pub region: Region,
    /// Seconds since the Unix epoch when it arrived.
    pub received_at: u64,
}

impl Stored {
    /// Whether this report is still within [`REPORT_TTL_SECONDS`] of `now`.
    #[must_use]
    pub fn is_live(&self, now: u64) -> bool {
        // A report from the future is a reporter with a fast clock, not a
        // reason to drop them from the network's total.
        now.saturating_sub(self.received_at) < REPORT_TTL_SECONDS
    }
}

/// Bounds a free-text label that will be rendered on a public page.
fn check_label(field: &'static str, value: &str) -> Result<(), ReportError> {
    let ok =
        value.len() <= MAX_NAME_LEN && value.bytes().all(|b| b.is_ascii_graphic() || b == b' ');

    if ok {
        Ok(())
    } else {
        Err(ReportError::BadLabel { field })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn miner(hash_rate: u64) -> Report {
        Report::Miner(MinerReport {
            reporter: ReporterId::parse("rig-1").expect("id"),
            hash_rate,
            backend: "wgpu".to_string(),
            device: "NVIDIA GeForce RTX 4090".to_string(),
        })
    }

    #[test]
    fn a_plausible_miner_report_validates() {
        miner(120_000_000).validate().expect("valid");
    }

    #[test]
    fn an_absurd_hash_rate_is_rejected_not_clamped() {
        // Clamping would put a number on the page the reporter never sent.
        // Rejecting says what happened.
        assert_eq!(
            miner(u64::MAX).validate(),
            Err(ReportError::ImplausibleHashRate)
        );
    }

    #[test]
    fn a_reporter_id_is_narrow_on_purpose() {
        // It is a map key, a log field, and a line on a public page. The set
        // that is safe in all three is small.
        for good in ["a", "rig-1", "RIG_2", &"x".repeat(MAX_NAME_LEN)] {
            ReporterId::parse(good).unwrap_or_else(|_| panic!("{good} should parse"));
        }
        for bad in [
            "",
            &"x".repeat(MAX_NAME_LEN + 1),
            "rig 1",
            "rig/1",
            "<script>",
            "rig\n1",
        ] {
            assert_eq!(
                ReporterId::parse(bad),
                Err(ReportError::BadReporterId),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_device_label_cannot_smuggle_markup_or_a_newline() {
        // Rendered on a public page. An escaping bug downstream should not be
        // the only thing standing between a reporter and everyone's browser.
        let report = Report::Miner(MinerReport {
            reporter: ReporterId::parse("rig-1").expect("id"),
            hash_rate: 1,
            backend: "wgpu".to_string(),
            device: "line\nbreak".to_string(),
        });
        assert_eq!(
            report.validate(),
            Err(ReportError::BadLabel { field: "device" })
        );
    }

    #[test]
    fn a_report_expires_so_the_total_can_go_down() {
        // Reporters crash rather than saying goodbye. Without expiry the
        // network total becomes the sum of everyone who has ever mined.
        let stored = Stored {
            report: miner(1),
            region: Region::UNKNOWN,
            received_at: 1_000,
        };
        assert!(stored.is_live(1_000));
        assert!(stored.is_live(1_000 + REPORT_TTL_SECONDS - 1));
        assert!(!stored.is_live(1_000 + REPORT_TTL_SECONDS));
    }

    #[test]
    fn a_reporter_with_a_fast_clock_is_still_live() {
        let stored = Stored {
            report: miner(1),
            region: Region::UNKNOWN,
            received_at: 5_000,
        };
        assert!(
            stored.is_live(1_000),
            "a clock skew is not a reason to drop a reporter from the total"
        );
    }

    #[test]
    fn a_report_round_trips_through_json() {
        // The wire format the miners emit. A tag change here silently stops
        // every deployed miner from being counted.
        let json = serde_json::to_string(&miner(42)).expect("serialize");
        assert!(json.contains(r#""kind":"miner""#), "{json}");
        assert_eq!(
            serde_json::from_str::<Report>(&json).expect("deserialize"),
            miner(42)
        );
    }
}
