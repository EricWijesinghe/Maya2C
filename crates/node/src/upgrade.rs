//! Protocol versions, feature sets, and the halt-before-fork rule (Master
//! Prompt 15 §4, ADR-082).
//!
//! The chain's upgrade schedule is a list of `(version, activation_height,
//! features)` entries. Versions are the rules epoch and only ever increase;
//! heights only ever increase. Each entry names the dark features it switches
//! on ([`FeatureSet`]), so any feature can join any later version. The list is
//! the genesis entries plus the entries governance or the security council
//! writes on chain ([`crate::state::upgrades`]). Nothing is inferred from which
//! binary happens to be running.
//!
//! This binary implements the rules of versions up to
//! [`SUPPORTED_PROTOCOL_VERSION`]. Asked to insert a block at a height where
//! the schedule says a later version is in force, it refuses with
//! [`NodeError::UpgradeRequired`] instead of validating that block under rules
//! it does not have, which is how a lagging node would otherwise fork itself
//! off silently.
//!
//! Version 1 is the protocol as specified in `spec/` v0.1.0. Version 2 is the
//! first epoch whose binaries implement the dark-feature gates and the
//! on-chain schedule. Version 3 is the first whose binaries implement account
//! authority (ADR-086); an entry switching on [`FeatureSet::ACCOUNT_AUTHORITY`]
//! names version 3 so that every older binary halts at its height.

use serde::{Deserialize, Serialize};

use crate::core::rwa_payload::RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT;
use crate::error::{NodeError, Result};
use crate::state::context::{
    ACCOUNT_AUTHORITY_ACTIVATION_HEIGHT, ACCOUNT_POLICY_ACTIVATION_HEIGHT, DVP_ACTIVATION_HEIGHT,
    HTLC_L_ACTIVATION_HEIGHT, IOT_ACTIVATION_HEIGHT, STATELESS_ACTIVATION_HEIGHT,
    THREAT_INTEL_ACTIVATION_HEIGHT, WORK_CLAIM_ACTIVATION_HEIGHT, ZKML_ACTIVATION_HEIGHT,
};

/// The newest protocol version whose rules this binary implements.
pub const SUPPORTED_PROTOCOL_VERSION: u32 = 3;

/// The dark features an upgrade entry can switch on, as a bitmask. The bit
/// positions are consensus: never renumber, only append.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureSet(pub u64);

impl FeatureSet {
    /// Delivery-versus-payment swaps.
    pub const DVP: u64 = 1 << 0;
    /// `IoT` anchor transactions.
    pub const IOT: u64 = 1 << 1;
    /// Attack attestations.
    pub const THREAT_INTEL: u64 = 1 << 2;
    /// The sparse accounts tree.
    pub const STATELESS: u64 = 1 << 3;
    /// Work-claim credit.
    pub const WORK_CLAIM: u64 = 1 << 4;
    /// Lattice HTLCs.
    pub const HTLC_LATTICE: u64 = 1 << 5;
    /// zkML proof verification.
    pub const ZKML: u64 = 1 << 6;
    /// Strictly ascending RWA issuer lists.
    pub const RWA_STRICT: u64 = 1 << 7;
    // Bit 8 is SHIELDED (ADR-087), defined on its own branch. Not defined here,
    // so KNOWN skips it until that branch merges and makes KNOWN contiguous.
    /// Account authority, wire version 9 and guardian recovery (ADR-086).
    pub const ACCOUNT_AUTHORITY: u64 = 1 << 9;
    /// Smart-account policy: session keys and the daily window (ADR-135
    /// P-5). Bits 10-40 and 41-44 are other ADRs' claims, not defined here.
    /// Schedulable only at or after `ACCOUNT_AUTHORITY` (ADR-135 S-10).
    pub const ACCOUNT_POLICY: u64 = 1 << 45;
    /// Every bit this binary defines. A bit outside it names no feature.
    pub const KNOWN: u64 = ((1 << 8) - 1) | Self::ACCOUNT_AUTHORITY | Self::ACCOUNT_POLICY;

    /// Whether every bit names a defined feature.
    #[must_use]
    pub const fn is_known(self) -> bool {
        self.0 & !Self::KNOWN == 0
    }

    /// Whether `bit` is set.
    #[must_use]
    pub const fn contains(self, bit: u64) -> bool {
        self.0 & bit != 0
    }
}

/// The activation height of each dark feature, as a schedule resolves it.
///
/// `Copy`, so it can sit in every block context. Each field is the
/// compiled-in "never" unless the schedule activates that feature at or below
/// [`SUPPORTED_PROTOCOL_VERSION`].
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
    /// Account authority (ADR-086).
    pub account_authority: u64,
    /// Smart-account policy (bit 45, ADR-135 P-5).
    pub account_policy: u64,
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
            account_authority: ACCOUNT_AUTHORITY_ACTIVATION_HEIGHT,
            account_policy: ACCOUNT_POLICY_ACTIVATION_HEIGHT,
        }
    }
}

/// One scheduled upgrade: a rules epoch, the height it takes effect, and the
/// dark features it switches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolUpgrade {
    /// The version that takes effect.
    pub version: u32,
    /// The first height at which it is in force.
    pub activation_height: u64,
    /// Features this entry activates. Empty for a version that only changes
    /// suite sunsets.
    #[serde(default)]
    pub features: FeatureSet,
}

/// A validated upgrade schedule, ordered by height.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpgradeSchedule {
    upgrades: Vec<ProtocolUpgrade>,
}

impl UpgradeSchedule {
    /// Validates `upgrades`: every version above 1, versions strictly
    /// increasing with strictly increasing heights, no unknown feature bits.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] naming the first entry that breaks a rule.
    pub fn new(mut upgrades: Vec<ProtocolUpgrade>) -> Result<Self> {
        upgrades.sort_by_key(|u| u.activation_height);
        let mut last = ProtocolUpgrade {
            version: 1,
            activation_height: 0,
            features: FeatureSet::default(),
        };
        let mut authority_scheduled = false;
        for u in &upgrades {
            authority_scheduled |= u.features.contains(FeatureSet::ACCOUNT_AUTHORITY);
            // ADR-135 S-10: session keys ride v9 frames, which exist only once
            // account authority is on, at or before this entry's height.
            if u.features.contains(FeatureSet::ACCOUNT_POLICY) && !authority_scheduled {
                return Err(NodeError::Decode(format!(
                    "protocol upgrade to version {} schedules account policy before account authority",
                    u.version
                )));
            }
            if u.version <= last.version || u.activation_height <= last.activation_height {
                return Err(NodeError::Decode(format!(
                    "protocol upgrade to version {} at height {} does not follow version {} at height {}",
                    u.version, u.activation_height, last.version, last.activation_height
                )));
            }
            if !u.features.is_known() {
                return Err(NodeError::Decode(format!(
                    "protocol upgrade to version {} names unknown features {:#x}",
                    u.version, u.features.0
                )));
            }
            // zkML is refused until `zkml::verdict` is a real verifier: a
            // schedule that switched it on would make the chain accept proofs
            // nothing checks (ADR-082, M3).
            if u.features.contains(FeatureSet::ZKML) {
                return Err(NodeError::Decode(format!(
                    "protocol upgrade to version {} schedules zkML, which is refused until its verifier exists",
                    u.version
                )));
            }
            last = *u;
        }
        Ok(Self { upgrades })
    }

    /// The entries, in height order.
    #[must_use]
    pub fn entries(&self) -> &[ProtocolUpgrade] {
        &self.upgrades
    }

    /// The highest version scheduled, or 1 for an empty schedule.
    #[must_use]
    pub fn last_version(&self) -> u32 {
        self.upgrades.last().map_or(1, |u| u.version)
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

    /// The height each dark feature activates at: the earliest entry at or
    /// below [`SUPPORTED_PROTOCOL_VERSION`] that switches it on. An entry above
    /// it contributes nothing, because this binary halts at that entry's
    /// height instead ([`Self::first_unsupported`]).
    #[must_use]
    pub fn features(&self) -> FeatureActivations {
        let at = |bit: u64, default: u64| {
            self.upgrades
                .iter()
                .find(|u| u.version <= SUPPORTED_PROTOCOL_VERSION && u.features.contains(bit))
                .map_or(default, |u| u.activation_height)
        };
        let never = FeatureActivations::default();
        FeatureActivations {
            dvp: at(FeatureSet::DVP, never.dvp),
            iot: at(FeatureSet::IOT, never.iot),
            threat_intel: at(FeatureSet::THREAT_INTEL, never.threat_intel),
            stateless: at(FeatureSet::STATELESS, never.stateless),
            work_claim: at(FeatureSet::WORK_CLAIM, never.work_claim),
            htlc_lattice: at(FeatureSet::HTLC_LATTICE, never.htlc_lattice),
            zkml: at(FeatureSet::ZKML, never.zkml),
            rwa_strict: at(FeatureSet::RWA_STRICT, never.rwa_strict),
            account_authority: at(FeatureSet::ACCOUNT_AUTHORITY, never.account_authority),
            account_policy: at(FeatureSet::ACCOUNT_POLICY, never.account_policy),
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

/// The halt rule on its own, for callers that hold only the first unsupported
/// entry.
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
            features: FeatureSet::default(),
        }
    }

    fn with(version: u32, activation_height: u64, features: u64) -> ProtocolUpgrade {
        ProtocolUpgrade {
            version,
            activation_height,
            features: FeatureSet(features),
        }
    }

    #[test]
    fn an_empty_schedule_is_version_one_forever() {
        let s = UpgradeSchedule::default();
        assert_eq!(s.version_at(u64::MAX), 1);
        assert!(s.check(u64::MAX).is_ok());
        assert_eq!(s.features(), FeatureActivations::default());
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
    fn unknown_feature_bits_are_rejected() {
        let bad = with(2, 10, FeatureSet::KNOWN + 1);
        assert!(UpgradeSchedule::new(vec![bad]).is_err());
        // Bit 8 (SHIELDED) is not defined on this branch.
        assert!(UpgradeSchedule::new(vec![with(2, 10, 1 << 8)]).is_err());
    }

    #[test]
    fn account_authority_resolves_from_its_entry() {
        let s = UpgradeSchedule::new(vec![with(3, 40, FeatureSet::ACCOUNT_AUTHORITY)]).unwrap();
        assert_eq!(s.features().account_authority, 40);
        assert_eq!(
            UpgradeSchedule::default().features().account_authority,
            u64::MAX
        );
    }

    #[test]
    fn account_policy_is_dark_and_needs_account_authority_first() {
        assert_eq!(FeatureActivations::default().account_policy, u64::MAX);
        let alone = UpgradeSchedule::new(vec![with(3, 40, FeatureSet::ACCOUNT_POLICY)]);
        assert!(alone.is_err(), "bit 45 without bit 9");
        let together = UpgradeSchedule::new(vec![with(
            3,
            40,
            FeatureSet::ACCOUNT_AUTHORITY | FeatureSet::ACCOUNT_POLICY,
        )])
        .unwrap();
        assert_eq!(together.features().account_policy, 40);
        let after = UpgradeSchedule::new(vec![
            with(2, 40, FeatureSet::ACCOUNT_AUTHORITY),
            with(3, 90, FeatureSet::ACCOUNT_POLICY),
        ])
        .unwrap();
        assert_eq!(after.features().account_policy, 90);
        let before = UpgradeSchedule::new(vec![
            with(2, 40, FeatureSet::ACCOUNT_POLICY),
            with(3, 90, FeatureSet::ACCOUNT_AUTHORITY),
        ]);
        assert!(before.is_err(), "bit 45 ahead of bit 9");
        // The neighbours of bit 45 are other ADRs' claims.
        assert!(
            UpgradeSchedule::new(vec![with(3, 40, FeatureSet::ACCOUNT_AUTHORITY | 1 << 44)])
                .is_err()
        );
        assert!(
            UpgradeSchedule::new(vec![with(3, 40, FeatureSet::ACCOUNT_AUTHORITY | 1 << 46)])
                .is_err()
        );
    }

    #[test]
    fn features_resolve_from_the_entry_that_switches_them_on() {
        // Two features join one implemented version; a later version above this
        // binary's contributes nothing (the halt covers it).
        let s = UpgradeSchedule::new(vec![
            with(2, 10, FeatureSet::DVP | FeatureSet::IOT),
            with(SUPPORTED_PROTOCOL_VERSION + 1, 50, FeatureSet::RWA_STRICT),
        ])
        .unwrap();
        let f = s.features();
        assert_eq!((f.dvp, f.iot), (10, 10));
        assert_eq!(f.rwa_strict, u64::MAX);
    }

    #[test]
    fn zkml_cannot_be_scheduled() {
        let refused = UpgradeSchedule::new(vec![with(2, 10, FeatureSet::ZKML)]);
        assert!(refused.is_err(), "zkML has no verifier yet (M3)");
    }

    #[test]
    fn an_entry_above_this_binary_contributes_no_features() {
        let future = UpgradeSchedule::new(vec![with(
            SUPPORTED_PROTOCOL_VERSION + 1,
            10,
            FeatureSet::DVP,
        )])
        .unwrap();
        assert_eq!(future.features(), FeatureActivations::default());
    }

    #[test]
    fn an_older_binary_halts_on_a_version_it_does_not_implement() {
        let s = UpgradeSchedule::new(vec![up(SUPPORTED_PROTOCOL_VERSION + 1, 10)]).unwrap();
        assert!(s.first_unsupported_by(SUPPORTED_PROTOCOL_VERSION).is_some());
        assert_eq!(s.first_unsupported_by(SUPPORTED_PROTOCOL_VERSION + 1), None);
    }
}
