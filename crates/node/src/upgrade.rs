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
//! The schedule is not part of any hash: genesis ids and roots are unchanged
//! for a network that declares none. Two nodes with different schedules share
//! a genesis and diverge only at the first height where one of them halts,
//! which is the failure this module exists to make loud.
//!
//! Version 1 is the protocol as specified in `spec/` v0.1.0.

use serde::{Deserialize, Serialize};

use crate::error::{NodeError, Result};

/// The newest protocol version whose rules this binary implements.
pub const SUPPORTED_PROTOCOL_VERSION: u32 = 1;

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

    /// The first scheduled upgrade this binary does not implement, if any.
    #[must_use]
    pub fn first_unsupported(&self) -> Option<ProtocolUpgrade> {
        self.upgrades
            .iter()
            .copied()
            .find(|u| u.version > SUPPORTED_PROTOCOL_VERSION)
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
}
