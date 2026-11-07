//! Protocol versions and the halt-before-fork rule (Master Prompt 15 §4).
//!
//! A network's upgrades are a schedule of `(version, activation_height)`
//! pairs, set in genesis — the chain's first governance decision — and never
//! inferred from which binary happens to be running. This binary knows the
//! rules of versions up to [`SUPPORTED_PROTOCOL_VERSION`]. Asked to insert a
//! block at a height where the schedule says a later version is in force, it
//! refuses with [`NodeError::UpgradeRequired`] instead of validating that
//! block under rules it does not have — which is how a lagging node would
//! otherwise fork itself off silently, or worse, keep following a minority
//! chain of other lagging nodes.
//!
//! The schedule is part of the genesis id (`GenesisConfig::chain_id_commitment`)
//! whenever it is non-empty, so operators who disagree about it disagree about
//! block zero. An empty schedule leaves every existing id unchanged. Two nodes
//! with different schedules that somehow share a genesis diverge only at the
//! first height where one of them halts, which is the failure this module
//! exists to make loud.
//!
//! Dark features (research code no network runs by default) activate only
//! through this schedule. Each has a protocol version; [`UpgradeSchedule::features`]
//! resolves the schedule to one activation height per feature. Versions must
//! increase with height (see [`UpgradeSchedule::new`]), so features activate in
//! version order. ADR-081 records that choice and the flag-day nature of the
//! header version byte.
//!
//! Version 1 is the protocol as specified in `spec/` v0.1.0.

use serde::{Deserialize, Serialize};

use crate::core::rwa_payload::RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT;
use crate::error::{NodeError, Result};
use crate::state::context::{
    DVP_ACTIVATION_HEIGHT, HTLC_L_ACTIVATION_HEIGHT, IOT_ACTIVATION_HEIGHT,
    STATELESS_ACTIVATION_HEIGHT, THREAT_INTEL_ACTIVATION_HEIGHT, WORK_CLAIM_ACTIVATION_HEIGHT,
    ZKML_ACTIVATION_HEIGHT,
};

/// The newest protocol version whose rules this binary implements.
pub const SUPPORTED_PROTOCOL_VERSION: u32 = RWA_STRICT_VERSION;

/// Protocol version that activates delivery-versus-payment swaps.
pub const DVP_VERSION: u32 = 2;
/// Protocol version that activates `IoT` anchor transactions.
pub const IOT_VERSION: u32 = 3;
/// Protocol version that activates attack attestations.
pub const THREAT_INTEL_VERSION: u32 = 4;
/// Protocol version that makes the accounts root a sparse tree.
pub const STATELESS_VERSION: u32 = 5;
/// Protocol version that credits work claims.
pub const WORK_CLAIM_VERSION: u32 = 6;
/// Protocol version that activates lattice HTLCs.
pub const HTLC_LATTICE_VERSION: u32 = 7;
/// Protocol version that lets contracts verify zkML proofs.
pub const ZKML_VERSION: u32 = 8;
/// Protocol version that requires strictly ascending RWA issuer lists.
pub const RWA_STRICT_VERSION: u32 = 9;

/// The activation height of each dark feature, as a schedule resolves it.
///
/// `Copy`, so it can sit in the chain configuration and every block context.
/// Each field is the compiled-in "never" unless the schedule names that
/// feature's version at or below [`SUPPORTED_PROTOCOL_VERSION`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureActivations {
    /// Delivery-versus-payment swaps.
    pub dvp: u64,
    /// `IoT` anchor transactions.
    pub iot: u64,
    /// Attack attestations.
    pub threat_intel: u64,
    /// The sparse accounts tree.
    pub stateless: u64,
    /// Work-claim credit.
    pub work_claim: u64,
    /// Lattice HTLCs.
    pub htlc_lattice: u64,
    /// zkML proof verification.
    pub zkml: u64,
    /// Strictly ascending RWA issuer lists.
    pub rwa_strict: u64,
}

impl Default for FeatureActivations {
    /// Today's compiled-in heights: every dark feature off.
    fn default() -> Self {
        Self {
            dvp: DVP_ACTIVATION_HEIGHT,
            iot: IOT_ACTIVATION_HEIGHT,
            threat_intel: THREAT_INTEL_ACTIVATION_HEIGHT,
            stateless: STATELESS_ACTIVATION_HEIGHT,
            work_claim: WORK_CLAIM_ACTIVATION_HEIGHT,
            htlc_lattice: HTLC_L_ACTIVATION_HEIGHT,
            zkml: ZKML_ACTIVATION_HEIGHT,
            rwa_strict: RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT,
        }
    }
}

/// One scheduled upgrade.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolUpgrade {
    /// The version that takes effect.
    pub version: u32,
    /// The first height at which it is in force.
    pub activation_height: u64,
}

/// A validated upgrade schedule, ordered by height.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpgradeSchedule {
    upgrades: Vec<ProtocolUpgrade>,
}

impl UpgradeSchedule {
    /// Validates `upgrades`: every version above 1, versions strictly
    /// increasing with strictly increasing heights, none at genesis.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] naming the first entry that breaks the ordering.
    pub fn new(mut upgrades: Vec<ProtocolUpgrade>) -> Result<Self> {
        upgrades.sort_by_key(|u| u.activation_height);
        let mut last = ProtocolUpgrade {
            version: 1,
            activation_height: 0,
        };
        for u in &upgrades {
            if u.version <= last.version || u.activation_height <= last.activation_height {
                return Err(NodeError::Decode(format!(
                    "protocol upgrade to version {} at height {} does not follow version {} at height {}",
                    u.version, u.activation_height, last.version, last.activation_height
                )));
            }
            last = *u;
        }
        Ok(Self { upgrades })
    }

    /// The protocol version in force at `height`.
    #[must_use]
    pub fn version_at(&self, height: u64) -> u32 {
        self.upgrades
            .iter()
            .rev()
            .find(|u| u.activation_height <= height)
            .map_or(1, |u| u.version)
    }

    /// The height each dark feature activates at. A version above
    /// [`SUPPORTED_PROTOCOL_VERSION`] contributes nothing: this binary halts at
    /// that version's height instead ([`Self::first_unsupported`]).
    #[must_use]
    pub fn features(&self) -> FeatureActivations {
        let never = FeatureActivations::default();
        let at = |version: u32, default: u64| {
            self.upgrades
                .iter()
                .find(|u| u.version == version)
                .map_or(default, |u| u.activation_height)
        };
        let known = |version: u32, default: u64| {
            if version <= SUPPORTED_PROTOCOL_VERSION {
                at(version, default)
            } else {
                default
            }
        };
        FeatureActivations {
            dvp: known(DVP_VERSION, never.dvp),
            iot: known(IOT_VERSION, never.iot),
            threat_intel: known(THREAT_INTEL_VERSION, never.threat_intel),
            stateless: known(STATELESS_VERSION, never.stateless),
            work_claim: known(WORK_CLAIM_VERSION, never.work_claim),
            htlc_lattice: known(HTLC_LATTICE_VERSION, never.htlc_lattice),
            zkml: known(ZKML_VERSION, never.zkml),
            rwa_strict: known(RWA_STRICT_VERSION, never.rwa_strict),
        }
    }

    /// The first scheduled upgrade this binary does not implement, if any.
    #[must_use]
    pub fn first_unsupported(&self) -> Option<ProtocolUpgrade> {
        self.first_unsupported_by(SUPPORTED_PROTOCOL_VERSION)
    }

    /// [`Self::first_unsupported`] for a binary implementing up to `supported`.
    /// Lets a test model an older binary.
    #[must_use]
    pub fn first_unsupported_by(&self, supported: u32) -> Option<ProtocolUpgrade> {
        self.upgrades
            .iter()
            .copied()
            .find(|u| u.version > supported)
    }

    /// Refuses `height` if the version in force there is newer than this
    /// binary supports.
    ///
    /// # Errors
    ///
    /// [`NodeError::UpgradeRequired`] with the height the unsupported version
    /// activated at, so the message reads "upgrade required before height H".
    pub fn check(&self, height: u64) -> Result<()> {
        refuse_unsupported(self.first_unsupported(), height)
    }
}

/// The halt rule on its own, for callers that keep only
/// [`UpgradeSchedule::first_unsupported`] (the chain's `Copy` config).
///
/// # Errors
///
/// [`NodeError::UpgradeRequired`] when `height` is at or past `unsupported`.
pub fn refuse_unsupported(unsupported: Option<ProtocolUpgrade>, height: u64) -> Result<()> {
    match unsupported {
        Some(u) if height >= u.activation_height => Err(NodeError::UpgradeRequired {
            version: u.version,
            height: u.activation_height,
            supported: SUPPORTED_PROTOCOL_VERSION,
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn up(version: u32, activation_height: u64) -> ProtocolUpgrade {
        ProtocolUpgrade {
            version,
            activation_height,
        }
    }

    #[test]
    fn an_empty_schedule_is_version_one_forever() {
        let s = UpgradeSchedule::default();
        assert_eq!(s.version_at(u64::MAX), 1);
        assert!(s.check(u64::MAX).is_ok());
    }

    #[test]
    fn an_unsupported_version_halts_exactly_at_its_height() {
        let s = UpgradeSchedule::new(vec![up(SUPPORTED_PROTOCOL_VERSION + 1, 100)]).unwrap();
        assert!(s.check(99).is_ok());
        let err = s.check(100).unwrap_err();
        assert!(
            err.to_string()
                .contains("upgrade required before height 100"),
            "{err}"
        );
        assert!(s.check(101).is_err());
    }

    #[test]
    fn out_of_order_schedules_are_rejected() {
        assert!(UpgradeSchedule::new(vec![up(3, 10), up(2, 20)]).is_err());
        assert!(UpgradeSchedule::new(vec![up(2, 10), up(3, 10)]).is_err());
        assert!(UpgradeSchedule::new(vec![up(1, 10)]).is_err());
        assert!(UpgradeSchedule::new(vec![up(2, 0)]).is_err());
        assert_eq!(
            UpgradeSchedule::new(vec![up(3, 20), up(2, 10)])
                .unwrap()
                .version_at(15),
            2
        );
    }

    #[test]
    fn features_resolve_from_their_versions_and_ignore_unimplemented_ones() {
        let s = UpgradeSchedule::new(vec![up(DVP_VERSION, 10), up(ZKML_VERSION, 50)]).unwrap();
        let f = s.features();
        assert_eq!((f.dvp, f.zkml), (10, 50));
        assert_eq!(f.iot, u64::MAX);
        let future = UpgradeSchedule::new(vec![up(SUPPORTED_PROTOCOL_VERSION + 1, 10)]).unwrap();
        assert_eq!(future.features(), FeatureActivations::default());
        assert_eq!(
            UpgradeSchedule::default().features(),
            FeatureActivations::default()
        );
    }

    #[test]
    fn an_older_binary_halts_on_a_version_it_does_not_implement() {
        let s = UpgradeSchedule::new(vec![up(DVP_VERSION, 10)]).unwrap();
        assert!(s.first_unsupported_by(SUPPORTED_PROTOCOL_VERSION).is_none());
        assert_eq!(s.first_unsupported_by(1), Some(up(DVP_VERSION, 10)));
    }
}
