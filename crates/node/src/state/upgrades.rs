//! The on-chain upgrade schedule (ADR-082).
//!
//! The schedule a block executes under is the genesis's entries plus every
//! entry governance or the security council has written since. Genesis seeds
//! its entries into these records, so the state root commits to them and every
//! reader — block context, halt check, suite policy, mempool, RPC — reads one
//! source. No entry is ever removed or moved earlier: a record is written once,
//! and a new entry must lie after every existing one.
//!
//! Records, all under [`UPGRADE_PREFIX`]:
//!
//! - `up:v` + version (big-endian `u32`) → a [`ProtocolUpgrade`]: the rules
//!   epoch, its activation height, and the features it switches on;
//! - `up:s` + suite byte → a [`SuiteSunset`]: the suite's announcement and
//!   sunset heights.
//!
//! Writers call [`EffectiveSchedule::admit`] and then [`StateDB::stage_schedule`].
//! Both governance (`ScheduleUpgrade`) and the security council
//! (`SunsetSuites`) go through them, so the rules are one set.

use crate::crypto::suites::{SuiteSunset, SuiteSunsets};
use crate::error::{NodeError, Result};
use crate::state::db::{Overlay, StateDB};
use crate::upgrade::{
    FeatureActivations, FeatureSet, ProtocolUpgrade, UpgradeSchedule, refuse_unsupported,
};
use maya_crypto_pq::suite::SuiteId;

/// Prefix of every upgrade-schedule record. A committed layer
/// (`StateLayer::UpgradeSchedule`), so it is under the state root (invariant 25).
pub const UPGRADE_PREFIX: &[u8] = b"up:";

const VERSION_KIND: u8 = b'v';
const SUNSET_KIND: u8 = b's';

const VERSION_RECORD_LEN: usize = 4 + 8 + 8;
const SUNSET_RECORD_LEN: usize = 8 + 8 + 1;

/// Key of the version entry for `version`.
#[must_use]
pub fn version_key(version: u32) -> Vec<u8> {
    let mut key = UPGRADE_PREFIX.to_vec();
    key.push(VERSION_KIND);
    key.extend_from_slice(&version.to_be_bytes());
    key
}

/// Key of the sunset record for the suite with wire byte `suite`.
#[must_use]
pub fn sunset_key(suite: u8) -> Vec<u8> {
    let mut key = UPGRADE_PREFIX.to_vec();
    key.push(SUNSET_KIND);
    key.push(suite);
    key
}

/// A suite sunset as a governance or council proposal names it. The
/// announcement height is the entry's activation height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SunsetSpec {
    /// Wire byte of the suite being retired.
    pub suite: u8,
    /// First height at which the suite no longer signs.
    pub sunset_height: u64,
    /// Whether the short emergency window is used.
    pub emergency: bool,
}

/// One entry a proposal asks to schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleRequest {
    /// Height the entry takes effect at (the sunsets are announced there).
    pub activation: u64,
    /// The rules epoch, if the entry carries one. `None` for a sunset-only entry.
    pub version: Option<u32>,
    /// Features the epoch switches on. Must be empty when `version` is `None`.
    pub features: FeatureSet,
    /// Suites the entry retires.
    pub sunsets: Vec<SunsetSpec>,
}

/// The schedule a block executes under, read from state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveSchedule {
    /// Every version entry, validated in height order.
    pub upgrades: UpgradeSchedule,
    /// Every suite sunset, validated against the genesis policy.
    pub sunsets: SuiteSunsets,
    /// The highest activation or announcement height; a new entry must lie
    /// strictly after it. Zero when nothing is scheduled.
    pub last_height: u64,
}

impl EffectiveSchedule {
    /// Dark-feature activation heights.
    #[must_use]
    pub fn features(&self) -> FeatureActivations {
        self.upgrades.features()
    }

    /// Refuses `height` if a version this binary does not implement is in
    /// force there (ADR-082: the halt is driven by committed state).
    ///
    /// # Errors
    ///
    /// [`NodeError::UpgradeRequired`] at or past the unsupported entry's height.
    pub fn check_halt(&self, height: u64) -> Result<()> {
        refuse_unsupported(self.upgrades.first_unsupported(), height)
    }

    /// Whether a new entry may be written at `now`.
    ///
    /// Refuses an entry that schedules nothing, a version or a height that does
    /// not lie after every existing one, an activation fewer than `min_lead`
    /// blocks after `now`, unknown feature bits, features on a sunset-only entry,
    /// and any sunset the genesis policy would refuse (a duplicate, the default
    /// suite, or a window shorter than the minimum).
    ///
    /// # Errors
    ///
    /// [`NodeError::Governance`] naming the first rule the entry breaks.
    pub fn admit(&self, request: &ScheduleRequest, now: u64, min_lead: u64) -> Result<()> {
        let refuse = |reason: String| Err(NodeError::Governance { reason });
        if request.version.is_none() && request.sunsets.is_empty() {
            return refuse("a schedule entry must schedule a version or a sunset".into());
        }
        if !request.features.is_known() {
            return refuse(format!("unknown feature bits {:#x}", request.features.0));
        }
        if request.version.is_none() && request.features.0 != 0 {
            return refuse("a sunset-only entry cannot switch features on".into());
        }
        if request.activation <= self.last_height {
            return refuse(format!(
                "activation {} must follow every scheduled entry, the last at {}",
                request.activation, self.last_height
            ));
        }
        if now.saturating_add(min_lead) > request.activation {
            return refuse(format!(
                "activation {} is under the {min_lead}-block lead from height {now}",
                request.activation
            ));
        }

        let mut versions: Vec<ProtocolUpgrade> = self.upgrades.entries().to_vec();
        if let Some(version) = request.version {
            versions.push(ProtocolUpgrade {
                version,
                activation_height: request.activation,
                features: request.features,
            });
        }
        UpgradeSchedule::new(versions)?;

        let mut sunsets: Vec<SuiteSunset> = self.sunsets.entries().collect();
        for spec in &request.sunsets {
            let suite = SuiteId::try_from(spec.suite).map_err(|e| NodeError::Governance {
                reason: format!("unknown suite {}: {e}", spec.suite),
            })?;
            if spec.sunset_height < request.activation {
                return refuse(format!(
                    "{suite:?} sunsets at {} before its announcement at {}",
                    spec.sunset_height, request.activation
                ));
            }
            sunsets.push(SuiteSunset {
                suite,
                announced_at: request.activation,
                sunset_height: spec.sunset_height,
                emergency: spec.emergency,
            });
        }
        SuiteSunsets::new(&sunsets).map_err(|e| NodeError::Governance {
            reason: e.to_string(),
        })?;
        Ok(())
    }

    /// The entry a request writes, as records: the version (if any) and the
    /// sunsets, each announced at the request's activation height.
    #[must_use]
    pub fn entry_for(request: &ScheduleRequest) -> (Option<ProtocolUpgrade>, Vec<SuiteSunset>) {
        let upgrade = request.version.map(|version| ProtocolUpgrade {
            version,
            activation_height: request.activation,
            features: request.features,
        });
        // Only suites the spec names are validated by `admit`, so a byte that is
        // not a suite is dropped here rather than written.
        let sunsets = request
            .sunsets
            .iter()
            .filter_map(|spec| {
                SuiteId::try_from(spec.suite).ok().map(|suite| SuiteSunset {
                    suite,
                    announced_at: request.activation,
                    sunset_height: spec.sunset_height,
                    emergency: spec.emergency,
                })
            })
            .collect();
        (upgrade, sunsets)
    }
}

impl StateDB {
    /// The schedule as committed state holds it now.
    ///
    /// # Errors
    ///
    /// Storage failure, or a record the schedule rules refuse.
    pub fn effective_schedule(&self) -> Result<EffectiveSchedule> {
        self.effective_schedule_through(&Overlay::new())
    }

    /// The schedule as it stands with `overlay`'s staged writes applied, so a
    /// block sees the entries it has itself written.
    ///
    /// # Errors
    ///
    /// As [`Self::effective_schedule`].
    pub(crate) fn effective_schedule_through(
        &self,
        overlay: &Overlay,
    ) -> Result<EffectiveSchedule> {
        let mut versions = Vec::new();
        let mut sunsets = Vec::new();
        for (key, value) in self.merged_records(overlay, UPGRADE_PREFIX)? {
            match key.get(UPGRADE_PREFIX.len()) {
                Some(&VERSION_KIND) => versions.push(decode_version(&value)?),
                Some(&SUNSET_KIND) => sunsets.push(decode_sunset(&key, &value)?),
                _ => {
                    return Err(NodeError::Decode(
                        "unknown upgrade-schedule record".to_string(),
                    ));
                }
            }
        }
        let last_height = versions
            .iter()
            .map(|u| u.activation_height)
            .chain(sunsets.iter().map(|s| s.announced_at))
            .max()
            .unwrap_or(0);
        let upgrades = UpgradeSchedule::new(versions)?;
        let sunsets = SuiteSunsets::new(&sunsets)?;
        Ok(EffectiveSchedule {
            upgrades,
            sunsets,
            last_height,
        })
    }

    /// Stages an admitted entry's records. Call only after
    /// [`EffectiveSchedule::admit`] has accepted the request.
    pub(crate) fn stage_schedule(
        overlay: &mut Overlay,
        upgrade: Option<ProtocolUpgrade>,
        sunsets: &[SuiteSunset],
    ) {
        if let Some(upgrade) = upgrade {
            Self::put_record(
                overlay,
                version_key(upgrade.version),
                encode_version(&upgrade),
            );
        }
        for sunset in sunsets {
            Self::put_record(
                overlay,
                sunset_key(sunset.suite.to_byte()),
                encode_sunset(sunset),
            );
        }
    }
}

pub(crate) fn encode_version(u: &ProtocolUpgrade) -> Vec<u8> {
    let mut out = Vec::with_capacity(VERSION_RECORD_LEN);
    out.extend_from_slice(&u.version.to_le_bytes());
    out.extend_from_slice(&u.activation_height.to_le_bytes());
    out.extend_from_slice(&u.features.0.to_le_bytes());
    out
}

fn decode_version(bytes: &[u8]) -> Result<ProtocolUpgrade> {
    let record: &[u8; VERSION_RECORD_LEN] = bytes
        .try_into()
        .map_err(|_| NodeError::Decode("upgrade version record length".to_string()))?;
    Ok(ProtocolUpgrade {
        version: u32::from_le_bytes(record[0..4].try_into().unwrap_or_default()),
        activation_height: u64::from_le_bytes(record[4..12].try_into().unwrap_or_default()),
        features: FeatureSet(u64::from_le_bytes(
            record[12..20].try_into().unwrap_or_default(),
        )),
    })
}

pub(crate) fn encode_sunset(s: &SuiteSunset) -> Vec<u8> {
    let mut out = Vec::with_capacity(SUNSET_RECORD_LEN);
    out.extend_from_slice(&s.announced_at.to_le_bytes());
    out.extend_from_slice(&s.sunset_height.to_le_bytes());
    out.push(u8::from(s.emergency));
    out
}

fn decode_sunset(key: &[u8], bytes: &[u8]) -> Result<SuiteSunset> {
    let suite_byte = *key
        .get(UPGRADE_PREFIX.len() + 1)
        .ok_or_else(|| NodeError::Decode("upgrade sunset key".to_string()))?;
    let suite = SuiteId::try_from(suite_byte)
        .map_err(|e| NodeError::Decode(format!("upgrade sunset suite: {e}")))?;
    let record: &[u8; SUNSET_RECORD_LEN] = bytes
        .try_into()
        .map_err(|_| NodeError::Decode("upgrade sunset record length".to_string()))?;
    Ok(SuiteSunset {
        suite,
        announced_at: u64::from_le_bytes(record[0..8].try_into().unwrap_or_default()),
        sunset_height: u64::from_le_bytes(record[8..16].try_into().unwrap_or_default()),
        emergency: record[16] != 0,
    })
}
