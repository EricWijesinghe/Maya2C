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

use crate::suite::{SuiteFamily, SuiteId};

/// The floor below which the audit flags a suite: NIST category 1.
pub const MIN_PQ_SECURITY_BITS: u16 = 128;

/// The floor for the *default* suite, the one new accounts are created under:
/// NIST category 3 (ADR-082 amendment, G23). Above [`MIN_PQ_SECURITY_BITS`]
/// on purpose: a suite at the category-1 floor may still sign, but governance
/// can never make it what every new account gets.
pub const MIN_DEFAULT_PQ_SECURITY_BITS: u16 = 192;

/// The shortest ordinary migration window: sized as about 30 days at 15 s
/// blocks, which is 2 days at the measured DAG-BFT rate of about one block a
/// second (`docs/block-time.md`, open item 4; ADR-082 H3 counts it as 2 days).
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
    /// Retiring the suite would leave no undeprecated permitted suite of its
    /// family (G13). Only a family with an emergency sunset among its entries
    /// may go, and only while the other family keeps a live suite.
    #[error("{suite:?} is the last live {family:?} suite")]
    LastOfFamily { suite: SuiteId, family: SuiteFamily },
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

    /// Whether `suite` may be the default here: permitted, and off devnet at
    /// least [`MIN_DEFAULT_PQ_SECURITY_BITS`]. With the registry this is the
    /// allow-list (G23): on mainnet today ML-DSA-65, ML-DSA-87 and
    /// SLH-DSA-SHAKE-256f.
    #[must_use]
    pub fn may_be_default(&self, suite: SuiteId) -> bool {
        self.permitted(suite)
            && (self.network == Network::Devnet
                || suite.info().pq_security_bits >= MIN_DEFAULT_PQ_SECURITY_BITS)
    }

    /// Whether some permitted suite of `family` has no deprecation at all,
    /// so it outlives every scheduled sunset.
    fn family_survives(&self, family: SuiteFamily) -> bool {
        SuiteId::ALL.into_iter().any(|s| {
            s.family() == family && self.permitted(s) && !self.deprecations.contains_key(&s)
        })
    }

    /// A policy with `suite` as the default.
    ///
    /// # Errors
    ///
    /// [`PolicyError::DefaultNotPermitted`] or [`PolicyError::DefaultDeprecated`].
    pub fn with_default(&self, suite: SuiteId) -> Result<Self, PolicyError> {
        if !self.may_be_default(suite) {
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
    /// Deprecating the default, a window below the minimum, a suite that is
    /// already deprecated, or the last live lattice or hash-based suite
    /// (allowed only when the family has an emergency sunset and the other
    /// family stays live).
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
        let next = Self {
            deprecations,
            ..self.clone()
        };
        // Counted against the schedule as it stands after every sunset, not at
        // `height`: a floor that held only until the next window closed would
        // not be a floor. Ordinary sunsets alone never retire a whole family;
        // one that has an emergency sunset among its entries may go, because
        // that family is the one broken, but never both: the other family
        // must outlive the schedule too. Both tests read the set of entries,
        // not their order, so a node replaying the schedule in key order
        // reaches the verdict admission did.
        let family = suite.family();
        let other = match family {
            SuiteFamily::Lattice => Some(SuiteFamily::HashBased),
            SuiteFamily::HashBased => Some(SuiteFamily::Lattice),
            _ => None,
        };
        let emergency_in_family = next
            .deprecations
            .iter()
            .any(|(s, d)| s.family() == family && d.emergency);
        if let Some(other) = other
            && !next.family_survives(family)
            && !(emergency_in_family && next.family_survives(other))
        {
            return Err(PolicyError::LastOfFamily { suite, family });
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests;
