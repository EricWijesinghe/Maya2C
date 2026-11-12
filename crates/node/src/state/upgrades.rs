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
use crate::state::vault::MAX_DELAY_BLOCKS;
use crate::upgrade::{
    FeatureActivations, FeatureSet, ProtocolUpgrade, UpgradeSchedule, refuse_unsupported,
};
use maya_crypto_pq::agility::{Network, SuitePolicy};
use maya_crypto_pq::suite::SuiteId;
use maya_governance::limits::MAX_SCHEDULE_HORIZON_BLOCKS;

/// The shortest lead a council sunset may carry, derived rather than chosen
/// (H3): a frozen vault must be able to finish a withdrawal that a sunset
/// announcement starts, and the vault's delay is at most `MAX_DELAY_BLOCKS`.
/// Then the migration window, so users can move funds to another suite before
/// the sunset height. There is no emergency window on this path and no exit-only
/// mode: an exit a broken suite could sign would let an attacker exit a victim's
/// funds too, so users migrate by transfer.
pub const COUNCIL_SUNSET_LEAD_BLOCKS: u64 =
    MAX_DELAY_BLOCKS + maya_crypto_pq::agility::MIN_MIGRATION_WINDOW;

/// First height at which capped-asset mint and burn (`FeatureSet::ASSET_MINT`,
/// bit 41, ADR-135 P-1) execute, for a schedule that never names the bit:
/// never. The v4 entry moves it; a node that never sees that entry refuses
/// tags 55-58 on admission and apply alike, at every height (ADR-002: a
/// height, never a cargo feature).
pub const ASSET_MINT_ACTIVATION_HEIGHT: u64 = u64::MAX;

/// First height at which unique-asset forms (`FeatureSet::NFT`, bit 42,
/// ADR-135 P-2, tags 59-62) execute. Never, unless a schedule entry names the
/// bit; a height, never a cargo feature (ADR-002).
pub const NFT_ACTIVATION_HEIGHT: u64 = u64::MAX;

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
    /// The highest activation height of any version. A new version must lie
    /// strictly after it. Zero when no version is scheduled.
    pub last_version_height: u64,
    /// The highest announcement height of any sunset. A new sunset must lie
    /// strictly after it. Zero when no sunset is scheduled.
    pub last_sunset_height: u64,
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
    pub fn admit(
        &self,
        request: &ScheduleRequest,
        now: u64,
        min_lead: u64,
        default_suite: SuiteId,
    ) -> Result<()> {
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
        // Heights are bounded above as well as below. Without a horizon one far
        // entry would fix the floor for every later entry of its kind.
        let horizon = now.saturating_add(MAX_SCHEDULE_HORIZON_BLOCKS);
        if request.activation > horizon {
            return refuse(format!(
                "activation {} is past the {MAX_SCHEDULE_HORIZON_BLOCKS}-block horizon from height {now}",
                request.activation
            ));
        }
        if now.saturating_add(min_lead) > request.activation {
            return refuse(format!(
                "activation {} is under the {min_lead}-block lead from height {now}",
                request.activation
            ));
        }

        // Versions order among versions only, and sunsets among sunsets only, so
        // a sunset can never fix the height a version must follow (H1).
        if let Some(version) = request.version {
            if request.activation <= self.last_version_height {
                return refuse(format!(
                    "version {version} at {} must follow the last version at {}",
                    request.activation, self.last_version_height
                ));
            }
            let mut versions: Vec<ProtocolUpgrade> = self.upgrades.entries().to_vec();
            versions.push(ProtocolUpgrade {
                version,
                activation_height: request.activation,
                features: request.features,
            });
            UpgradeSchedule::new(versions)?;
        }

        if !request.sunsets.is_empty() && request.activation <= self.last_sunset_height {
            return refuse(format!(
                "sunsets announced at {} must follow the last at {}",
                request.activation, self.last_sunset_height
            ));
        }
        let mut sunsets: Vec<SuiteSunset> = self.sunsets.entries().collect();
        for spec in &request.sunsets {
            let suite = SuiteId::try_from(spec.suite).map_err(|e| NodeError::Governance {
                reason: format!("unknown suite {}: {e}", spec.suite),
            })?;
            if suite == default_suite {
                return refuse(format!(
                    "{suite:?} is the governance default suite; a sunset would empty every block"
                ));
            }
            if !SuitePolicy::genesis(Network::Mainnet).permitted(suite) {
                return refuse(format!("{suite:?} is not permitted on mainnet"));
            }
            if spec.sunset_height < request.activation {
                return refuse(format!(
                    "{suite:?} sunsets at {} before its announcement at {}",
                    spec.sunset_height, request.activation
                ));
            }
            if spec.sunset_height > horizon {
                return refuse(format!(
                    "{suite:?} sunsets at {}, past the horizon at {horizon}",
                    spec.sunset_height
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
        let last_version_height = versions
            .iter()
            .map(|u| u.activation_height)
            .max()
            .unwrap_or(0);
        let last_sunset_height = sunsets.iter().map(|s| s.announced_at).max().unwrap_or(0);
        let upgrades = UpgradeSchedule::new(versions)?;
        let sunsets = SuiteSunsets::new(&sunsets)?;
        Ok(EffectiveSchedule {
            upgrades,
            sunsets,
            last_version_height,
            last_sunset_height,
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::crypto::suites::check_parameter;
    use maya_governance::limits::{MAX_SCHEDULE_HORIZON_BLOCKS, MIN_UPGRADE_LEAD_BLOCKS};
    use maya_governance::params::ParameterKey;

    const NOW: u64 = 1_000;
    const DEFAULT: SuiteId = SuiteId::MlDsa87;
    const WINDOW: u64 = maya_crypto_pq::agility::MIN_MIGRATION_WINDOW;

    fn empty() -> EffectiveSchedule {
        EffectiveSchedule {
            upgrades: UpgradeSchedule::default(),
            sunsets: SuiteSunsets::EMPTY,
            last_version_height: 0,
            last_sunset_height: 0,
        }
    }

    /// A schedule that already holds one sunset of `suite`, announced at `announced`.
    fn with_sunset(suite: SuiteId, announced: u64) -> EffectiveSchedule {
        let sunsets = SuiteSunsets::new(&[SuiteSunset {
            suite,
            announced_at: announced,
            sunset_height: announced + WINDOW,
            emergency: false,
        }])
        .unwrap();
        EffectiveSchedule {
            sunsets,
            last_sunset_height: announced,
            ..empty()
        }
    }

    fn version_at(activation: u64) -> ScheduleRequest {
        ScheduleRequest {
            activation,
            version: Some(SUPPORTED_VERSION_FOR_TESTS),
            features: FeatureSet(FeatureSet::DVP),
            sunsets: Vec::new(),
        }
    }

    const SUPPORTED_VERSION_FOR_TESTS: u32 = crate::upgrade::SUPPORTED_PROTOCOL_VERSION;

    fn council_sunset(announced: u64, suite: SuiteId) -> ScheduleRequest {
        ScheduleRequest {
            activation: announced,
            version: None,
            features: FeatureSet::default(),
            sunsets: vec![SunsetSpec {
                suite: suite.to_byte(),
                sunset_height: announced + WINDOW,
                emergency: false,
            }],
        }
    }

    #[test]
    fn a_far_council_sunset_does_not_block_a_later_governance_upgrade() {
        // H1: the sunset is announced near the horizon; a governance upgrade at
        // its first legal height is before that announcement. The old shared
        // floor refused it. Sunsets and versions order only among themselves.
        let far = NOW + MAX_SCHEDULE_HORIZON_BLOCKS - WINDOW;
        let schedule = with_sunset(SuiteId::MlDsa65, far);
        let first_legal = NOW + MIN_UPGRADE_LEAD_BLOCKS;
        schedule
            .admit(
                &version_at(first_legal),
                NOW,
                MIN_UPGRADE_LEAD_BLOCKS,
                DEFAULT,
            )
            .expect("a version after a far sunset is admitted");
    }

    #[test]
    fn a_council_sunset_after_a_pending_upgrade_does_not_veto_a_later_upgrade() {
        // H1: a pending version at `pending` and a sunset announced after it.
        // A version between them is still admitted.
        let pending = NOW + MIN_UPGRADE_LEAD_BLOCKS;
        let sunset_at = pending + 500_000;
        let mut schedule = with_sunset(SuiteId::MlDsa65, sunset_at);
        schedule.upgrades = UpgradeSchedule::new(vec![ProtocolUpgrade {
            version: SUPPORTED_VERSION_FOR_TESTS,
            activation_height: pending,
            features: FeatureSet(FeatureSet::DVP),
        }])
        .unwrap();
        schedule.last_version_height = pending;
        let between = pending + 1;
        let request = ScheduleRequest {
            version: Some(SUPPORTED_VERSION_FOR_TESTS + 1),
            ..version_at(between)
        };
        schedule
            .admit(&request, NOW, MIN_UPGRADE_LEAD_BLOCKS, DEFAULT)
            .expect("a version between a version and a later sunset is admitted");
    }

    #[test]
    fn a_height_past_the_horizon_is_refused_for_versions_and_sunsets() {
        let beyond = NOW + MAX_SCHEDULE_HORIZON_BLOCKS + 1;
        assert!(
            empty()
                .admit(&version_at(beyond), NOW, MIN_UPGRADE_LEAD_BLOCKS, DEFAULT)
                .is_err()
        );
        // A sunset whose sunset height passes the horizon is refused too.
        let late_sunset = NOW + MAX_SCHEDULE_HORIZON_BLOCKS - WINDOW + 1;
        assert!(
            empty()
                .admit(
                    &council_sunset(late_sunset, SuiteId::MlDsa65),
                    NOW,
                    COUNCIL_SUNSET_LEAD_BLOCKS,
                    DEFAULT
                )
                .is_err()
        );
    }

    #[test]
    fn the_governance_default_suite_cannot_be_sunset() {
        // H2: sunsetting the default would make every block refuse its frames.
        // ML-DSA-65 sunset while it is the governance default: refused.
        let sunset = council_sunset(NOW + COUNCIL_SUNSET_LEAD_BLOCKS, SuiteId::MlDsa65);
        assert!(
            empty()
                .admit(&sunset, NOW, COUNCIL_SUNSET_LEAD_BLOCKS, SuiteId::MlDsa65)
                .is_err()
        );
        // Once governance has moved the default to ML-DSA-87, it is admitted.
        assert!(
            empty()
                .admit(&sunset, NOW, COUNCIL_SUNSET_LEAD_BLOCKS, DEFAULT)
                .is_ok()
        );
    }

    #[test]
    fn a_default_suite_with_a_schedule_entry_cannot_be_chosen_by_governance() {
        // H2: the governance default may not name a suite already scheduled.
        let value = maya_dex::Amount::from_u64(u64::from(SuiteId::MlDsa65.to_byte()));
        let scheduled = with_sunset(SuiteId::MlDsa65, NOW + COUNCIL_SUNSET_LEAD_BLOCKS);
        assert!(
            check_parameter(
                ParameterKey::DefaultSignatureSuite,
                value,
                &scheduled.sunsets
            )
            .is_err()
        );
        assert!(
            check_parameter(
                ParameterKey::DefaultSignatureSuite,
                value,
                &SuiteSunsets::EMPTY
            )
            .is_ok()
        );
    }

    #[test]
    fn a_suite_the_network_does_not_permit_cannot_be_sunset() {
        // Ed25519 has no post-quantum security and is not permitted on mainnet.
        let request = council_sunset(NOW + COUNCIL_SUNSET_LEAD_BLOCKS, SuiteId::Ed25519);
        assert!(
            empty()
                .admit(&request, NOW, COUNCIL_SUNSET_LEAD_BLOCKS, DEFAULT)
                .is_err()
        );
    }

    #[test]
    fn zkml_cannot_be_scheduled_through_admission() {
        // M3: a version that switches zkML on is refused until its verifier is real.
        let request = ScheduleRequest {
            features: FeatureSet(FeatureSet::ZKML),
            ..version_at(NOW + MIN_UPGRADE_LEAD_BLOCKS)
        };
        assert!(
            empty()
                .admit(&request, NOW, MIN_UPGRADE_LEAD_BLOCKS, DEFAULT)
                .is_err()
        );
    }
}
