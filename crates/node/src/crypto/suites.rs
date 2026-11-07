//! The node's side of ADR-007: suite ids, addresses, and the activation gate.
//!
//! The registry, the envelope and the migration engine live in
//! `maya_crypto_pq`; this module is the thin layer that decides *when* the
//! chain accepts them. [`SUITE_ENVELOPE_ACTIVATION_HEIGHT`] is genesis
//! (ADR-013), and every verification path judges under
//! [`verification_policy`], a constant, so no vote and no config file can
//! change which signatures a block's validity rests on.
//!
//! [`SUITE_ENVELOPE_ACTIVATION_HEIGHT`]: crate::state::context::SUITE_ENVELOPE_ACTIVATION_HEIGHT

use maya_crypto_pq::agility::{Network, SuitePolicy};
use maya_crypto_pq::suite::SuiteId;
use maya_governance::params::ParameterKey;

use crate::error::{NodeError, Result};
use crate::governance::ParameterTable;
use crate::state::context::SUITE_ENVELOPE_ACTIVATION_HEIGHT;

/// Domain for the address of a suite-tagged key. Distinct from the hybrid
/// address domain, so a v7 key can never collide with a v5 account.
pub use maya_crypto_pq::suite::SUITE_ADDRESS_DOMAIN;

/// The address a suite-tagged key controls: `BLAKE3-derive-key(suite ‖ pk)`.
///
/// The suite byte is inside the hash, so the same bytes read as a key of
/// another suite name a different account.
#[must_use]
pub fn suite_address(suite: SuiteId, public_key: &[u8]) -> [u8; 32] {
    // One implementation, in `crypto-pq`, so apps that do not link the node
    // (chat, wallet) derive the same address.
    maya_crypto_pq::suite::suite_address(suite, public_key)
}

/// The default suite governance has chosen.
///
/// # Errors
///
/// As [`suite_for_default`].
pub fn default_suite(parameters: &ParameterTable) -> Result<SuiteId> {
    suite_for_default(parameters.get(ParameterKey::DefaultSignatureSuite))
}

/// The suite a `DefaultSignatureSuite` value names, if mainnet permits it.
///
/// The governance crate can only range-check the byte (it has no
/// dependencies, invariant 12), and the range `0x10..=0x30` contains bytes
/// that name no suite. This is the registry half of the check, run at both
/// moments invariant 12 checks — proposal and execution — by
/// [`check_parameter`], and again whenever the table is read.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] for a value naming no suite, or a suite
/// mainnet does not permit.
pub fn suite_for_default(raw: maya_dex::Amount) -> Result<SuiteId> {
    let byte = raw
        .to_u64()
        .and_then(|v| u8::try_from(v).ok())
        .ok_or_else(|| NodeError::SignatureSuite(format!("default suite {raw} is not a byte")))?;
    let suite = SuiteId::try_from(byte).map_err(|e| NodeError::SignatureSuite(e.to_string()))?;
    if SuitePolicy::genesis(Network::Mainnet).permitted(suite) {
        Ok(suite)
    } else {
        Err(NodeError::SignatureSuite(format!(
            "{suite:?} may not be the mainnet default"
        )))
    }
}

/// The node-side half of a parameter change's validation: governance's own
/// range check first, then this for the keys whose meaning the governance
/// crate cannot know.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] for a `DefaultSignatureSuite` value that
/// names no permitted suite.
pub fn check_parameter(key: ParameterKey, value: maya_dex::Amount) -> Result<()> {
    match key {
        ParameterKey::DefaultSignatureSuite => suite_for_default(value).map(|_| ()),
        _ => Ok(()),
    }
}

/// The policy consensus verifies under: the genesis schedule, mainnet rules,
/// at every height and on every network.
///
/// Deliberately *not* a function of state or of configuration. Three reasons,
/// and each one is a fork this avoids:
///
/// - [`SuitePolicy::may_sign`] reads only the deprecation schedule and the
///   height, never the default, so the governance table cannot change whether a
///   signature verifies. A verification rule that moved with a vote would make
///   the validity of a *historical* block depend on a later one.
/// - The network is fixed at `Mainnet` rather than read from config, because
///   [`SuitePolicy::permitted`] is laxer on `Devnet` (it admits `0x01`, which
///   has no post-quantum security at all). A node's own config file must never
///   decide which signatures its chain accepts.
/// - It allocates nothing and reads no column family, so the apply path spends
///   no I/O on it per transaction.
///
/// The consequence is worth stating: Ed25519 (`0x01`) can never sign a
/// suite-tagged frame on any network. It stays in the registry for tooling and
/// for the parity tests, and the audit in [`crate::crypto::suites`] flags it at
/// zero post-quantum bits.
///
/// The deprecation schedule is the genesis's own (ADR-082): empty at launch,
/// so the policy is the genesis policy, and a later upgrade may sunset a broken
/// suite at a height every node agrees on because the schedule is in the id.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] if the schedule does not apply to the policy.
pub fn verification_policy(sunsets: &SuiteSunsets) -> Result<SuitePolicy> {
    let mut policy = SuitePolicy::genesis(Network::Mainnet);
    for sunset in sunsets.entries() {
        let window = sunset
            .sunset_height
            .checked_sub(sunset.announced_at)
            .ok_or_else(|| {
                NodeError::SignatureSuite(format!(
                    "{:?} sunsets at {} before its deprecation at {}",
                    sunset.suite, sunset.sunset_height, sunset.announced_at
                ))
            })?;
        policy = policy
            .with_deprecation(sunset.suite, sunset.announced_at, window, sunset.emergency)
            .map_err(|e| NodeError::SignatureSuite(e.to_string()))?;
    }
    Ok(policy)
}

/// One suite the genesis deprecates (ADR-082).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuiteSunset {
    /// The suite being retired.
    pub suite: SuiteId,
    /// Height the deprecation takes effect.
    pub announced_at: u64,
    /// First height at which the suite no longer signs.
    pub sunset_height: u64,
    /// Whether the short emergency window is used.
    pub emergency: bool,
}

/// The genesis's suite deprecations, fixed-size so that the block context
/// stays `Copy`. At most one entry per registered suite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuiteSunsets {
    entries: [Option<SuiteSunset>; SuiteId::ALL.len()],
}

impl SuiteSunsets {
    /// No deprecations: the genesis policy.
    pub const EMPTY: Self = Self {
        entries: [None; SuiteId::ALL.len()],
    };

    /// Validates `sunsets` against the genesis policy and stores them.
    ///
    /// # Errors
    ///
    /// [`NodeError::SignatureSuite`] for more entries than suites, a suite
    /// named twice, a window shorter than the minimum, or the default suite.
    pub fn new(sunsets: &[SuiteSunset]) -> Result<Self> {
        if sunsets.len() > SuiteId::ALL.len() {
            return Err(NodeError::SignatureSuite(format!(
                "at most {} suite deprecations, got {}",
                SuiteId::ALL.len(),
                sunsets.len()
            )));
        }
        let mut entries = [None; SuiteId::ALL.len()];
        for (slot, sunset) in entries.iter_mut().zip(sunsets) {
            *slot = Some(*sunset);
        }
        let stored = Self { entries };
        verification_policy(&stored)?;
        Ok(stored)
    }

    /// The deprecations, in the order the genesis listed them.
    pub fn entries(&self) -> impl Iterator<Item = SuiteSunset> + '_ {
        self.entries.iter().flatten().copied()
    }
}

/// The policy in force: the genesis schedule with governance's default.
///
/// # Errors
///
/// As [`default_suite`].
pub fn policy(parameters: &ParameterTable, sunsets: &SuiteSunsets) -> Result<SuitePolicy> {
    verification_policy(sunsets)?
        .with_default(default_suite(parameters)?)
        .map_err(|e| NodeError::SignatureSuite(e.to_string()))
}

/// Whether a suite-tagged transaction may be verified at all at `height`.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] before activation, or when `policy` does not
/// let `suite` sign at `height`.
// The activation comparison is always false while the height is 0 (ADR-013),
// and clippy denies a comparison it can prove constant. It stays anyway: the
// constant is the one place activation is decided, and a later ADR that raised
// it — to re-darken the envelope on a fork, say — must take effect without
// anybody remembering to restore a check that had been deleted as dead code.
#[allow(clippy::absurd_extreme_comparisons)]
pub fn check_admissible(policy: &SuitePolicy, suite: SuiteId, height: u64) -> Result<()> {
    if height < SUITE_ENVELOPE_ACTIVATION_HEIGHT {
        return Err(NodeError::SignatureSuite(format!(
            "suite-tagged transactions are not active at height {height}"
        )));
    }
    if !policy.may_sign(suite, height) {
        return Err(NodeError::SignatureSuite(format!(
            "{suite:?} may not sign at height {height}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    const ANNOUNCED: u64 = 100;
    const WINDOW: u64 = 1_000_000;

    fn sunset(suite: SuiteId) -> SuiteSunset {
        SuiteSunset {
            suite,
            announced_at: ANNOUNCED,
            sunset_height: ANNOUNCED + WINDOW,
            emergency: false,
        }
    }

    #[test]
    fn the_empty_schedule_is_the_genesis_policy() {
        let policy = verification_policy(&SuiteSunsets::EMPTY).unwrap();
        assert_eq!(policy, SuitePolicy::genesis(Network::Mainnet));
    }

    #[test]
    fn a_sunset_suite_is_refused_at_its_height_and_accepted_one_block_before() {
        let sunsets = SuiteSunsets::new(&[sunset(SuiteId::MlDsa65)]).unwrap();
        let policy = verification_policy(&sunsets).unwrap();
        let at = ANNOUNCED + WINDOW;
        assert!(check_admissible(&policy, SuiteId::MlDsa65, at - 1).is_ok());
        assert!(check_admissible(&policy, SuiteId::MlDsa65, at).is_err());
        // Other suites are untouched.
        assert!(check_admissible(&policy, SuiteId::MlDsa87, at).is_ok());
    }

    #[test]
    fn a_schedule_cannot_sunset_the_default_or_sunset_before_announcement() {
        assert!(SuiteSunsets::new(&[sunset(SuiteId::MlDsa87)]).is_err());
        let backwards = SuiteSunset {
            sunset_height: ANNOUNCED - 1,
            ..sunset(SuiteId::MlDsa65)
        };
        assert!(SuiteSunsets::new(&[backwards]).is_err());
    }

    #[test]
    fn a_schedule_names_each_suite_at_most_once() {
        let twice = [sunset(SuiteId::MlDsa65), sunset(SuiteId::MlDsa65)];
        assert!(SuiteSunsets::new(&twice).is_err());
    }
}
