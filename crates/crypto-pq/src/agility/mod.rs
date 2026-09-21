//! The crypto-agility engine — ADR-007.
//!
//! Three parts, each a pure function of its inputs so the node can run it
//! inside consensus without a clock or a random number:
//!
//! - [`SuitePolicy`] — the governance-chosen default suite and every suite's
//!   deprecation schedule, by block height. Changing it returns a new policy;
//!   nothing is mutated in place, so a rejected proposal leaves no trace.
//! - [`audit`] — flags every live suite whose claimed post-quantum security
//!   is below [`MIN_PQ_SECURITY_BITS`], and every deprecated suite accounts
//!   still need to leave.
//! - [`migration`] — moves accounts off a deprecated suite: a window in which
//!   the old key can register an upgraded one, then a sweep of whatever is
//!   left into a vault only an upgraded key can open.
//!
//! Heights are `u64`; nothing here reads wall-clock time (Standing Order 4).

pub mod audit;
pub mod migration;

use std::collections::BTreeMap;

use crate::suite::SuiteId;

/// The floor below which the audit flags a suite: NIST category 1.
pub const MIN_PQ_SECURITY_BITS: u16 = 128;

/// The shortest ordinary migration window: about 30 days at 15 s blocks.
pub const MIN_MIGRATION_WINDOW: u64 = 172_800;

/// The shortest *emergency* window: about 3 days. For a suite that has been
/// broken in public, a month of exposure is worse than a rushed migration.
pub const MIN_EMERGENCY_WINDOW: u64 = 17_280;

/// Which network a policy governs. Only devnet admits Ed25519.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// Local development; any suite.
    Devnet,
    /// Public test network; mainnet rules.
    Testnet,
    /// Production.
    Mainnet,
}

/// Where a suite stands at a given height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuiteStatus {
    /// May sign.
    Active,
    /// May still sign, and accounts should migrate before `sunset_height`.
    Deprecated {
        /// First height at which the suite no longer signs.
        sunset_height: u64,
    },
    /// Past its sunset: signatures are refused and balances are swept.
    Sunset,
    /// Never allowed on this network.
    Forbidden,
}

/// One suite's retirement schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deprecation {
    /// Height the deprecation took effect.
    pub announced_at: u64,
    /// First height at which the suite no longer signs.
    pub sunset_height: u64,
    /// Whether the short emergency window was used.
    pub emergency: bool,
}

/// Why a policy change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// The suite is below the security floor, or not allowed on this network.
    #[error("{0:?} may not be the default on this network")]
    DefaultNotPermitted(SuiteId),
    /// The default suite cannot be deprecated; change the default first.
    #[error("{0:?} is the default suite")]
    DeprecatingDefault(SuiteId),
    /// The window is shorter than the minimum for its kind.
    #[error("migration window {got} is shorter than the minimum {minimum}")]
    WindowTooShort { got: u64, minimum: u64 },
    /// The suite is already deprecated; a schedule is never quietly moved.
    #[error("{0:?} is already deprecated")]
    AlreadyDeprecated(SuiteId),
    /// A deprecated or sunset suite cannot become the default.
    #[error("{0:?} is deprecated")]
    DefaultDeprecated(SuiteId),
}

/// The default suite and the deprecation schedule for one network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuitePolicy {
    network: Network,
    default: SuiteId,
    deprecations: BTreeMap<SuiteId, Deprecation>,
}

impl SuitePolicy {
    /// The genesis policy: ML-DSA-87 on mainnet and testnet, per the brief.
    #[must_use]
    pub fn genesis(network: Network) -> Self {
        Self {
            network,
            default: SuiteId::MlDsa87,
            deprecations: BTreeMap::new(),
        }
    }

    /// The network.
    #[must_use]
    pub fn network(&self) -> Network {
        self.network
    }

    /// The suite new accounts are created under.
    #[must_use]
    pub fn default_suite(&self) -> SuiteId {
        self.default
    }

    /// The deprecation schedule for `suite`, if any.
    #[must_use]
    pub fn deprecation(&self, suite: SuiteId) -> Option<Deprecation> {
        self.deprecations.get(&suite).copied()
    }

    /// Whether the registry and the floor allow `suite` on this network at all.
    #[must_use]
    pub fn permitted(&self, suite: SuiteId) -> bool {
        let info = suite.info();
        match self.network {
            Network::Devnet => true,
            Network::Testnet | Network::Mainnet => {
                info.mainnet_allowed && info.pq_security_bits >= MIN_PQ_SECURITY_BITS
            }
        }
    }

    /// Where `suite` stands at `height`.
    #[must_use]
    pub fn status(&self, suite: SuiteId, height: u64) -> SuiteStatus {
        if !self.permitted(suite) {
            return SuiteStatus::Forbidden;
        }
        match self.deprecations.get(&suite) {
            Some(d) if height >= d.sunset_height => SuiteStatus::Sunset,
            Some(d) if height >= d.announced_at => SuiteStatus::Deprecated {
                sunset_height: d.sunset_height,
            },
            // Never deprecated, or deprecated from a height not yet reached.
            _ => SuiteStatus::Active,
        }
    }

    /// Whether a signature under `suite` is accepted at `height`.
    #[must_use]
    pub fn may_sign(&self, suite: SuiteId, height: u64) -> bool {
        matches!(
            self.status(suite, height),
            SuiteStatus::Active | SuiteStatus::Deprecated { .. }
        )
    }

    /// A policy with `suite` as the default.
    ///
    /// # Errors
    ///
    /// [`PolicyError::DefaultNotPermitted`] or [`PolicyError::DefaultDeprecated`].
    pub fn with_default(&self, suite: SuiteId) -> Result<Self, PolicyError> {
        if !self.permitted(suite) {
            return Err(PolicyError::DefaultNotPermitted(suite));
        }
        if self.deprecations.contains_key(&suite) {
            return Err(PolicyError::DefaultDeprecated(suite));
        }
        Ok(Self {
            default: suite,
            ..self.clone()
        })
    }

    /// A policy that deprecates `suite` at `height` with `window` blocks until
    /// sunset. `emergency` selects the shorter minimum window.
    ///
    /// # Errors
    ///
    /// Deprecating the default, a window below the minimum, or a suite that is
    /// already deprecated.
    pub fn with_deprecation(
        &self,
        suite: SuiteId,
        height: u64,
        window: u64,
        emergency: bool,
    ) -> Result<Self, PolicyError> {
        if suite == self.default {
            return Err(PolicyError::DeprecatingDefault(suite));
        }
        if self.deprecations.contains_key(&suite) {
            return Err(PolicyError::AlreadyDeprecated(suite));
        }
        let minimum = if emergency {
            MIN_EMERGENCY_WINDOW
        } else {
            MIN_MIGRATION_WINDOW
        };
        if window < minimum {
            return Err(PolicyError::WindowTooShort {
                got: window,
                minimum,
            });
        }
        let mut deprecations = self.deprecations.clone();
        deprecations.insert(
            suite,
            Deprecation {
                announced_at: height,
                sunset_height: height.saturating_add(window),
                emergency,
            },
        );
        Ok(Self {
            deprecations,
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests;
