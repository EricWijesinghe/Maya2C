//! The consensus mode a node runs, chosen in config and validated once.
//!
//! ADR-015 resolves the brief's mix of `PoW`, `PoUW`, staking and DAG-BFT into
//! one design: **ordering and finality come from DAG-BFT run by staked
//! validators; work never orders anything.** The two work modes exist for
//! devnets and research, and a production build refuses them at config
//! validation rather than at the first block.

use core::fmt;
use core::str::FromStr;

/// How blocks are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConsensusMode {
    /// Single chain, `ArgonBlake` proof of work, most cumulative work wins.
    /// What `custom-l1-node` runs today. Devnet only.
    ArgonBlakePow,
    /// Single chain whose work is a short-vector lattice solution
    /// (`crates/lattice-pow`). RESEARCH: its "usefulness" is unproven.
    PouwLattice,
    /// Narwhal-style certified DAG with a Bullshark commit rule, run by a
    /// staked validator set. The mainnet mode.
    DagBft,
}

impl ConsensusMode {
    /// Every mode, in config-name order.
    pub const ALL: [Self; 3] = [Self::ArgonBlakePow, Self::PouwLattice, Self::DagBft];

    /// The name used in `config.toml` and on the command line.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ArgonBlakePow => "argonblake-pow",
            Self::PouwLattice => "pouw-lattice",
            Self::DagBft => "dag-bft",
        }
    }

    /// Whether this mode gives deterministic (BFT) finality rather than
    /// probabilistic, confirmation-depth finality.
    pub const fn has_deterministic_finality(self) -> bool {
        matches!(self, Self::DagBft)
    }

    /// Checks the mode against the build profile. A production build accepts
    /// only [`ConsensusMode::DagBft`] (Master Prompt 11 §3).
    ///
    /// # Errors
    ///
    /// [`ModeError::NotAllowedInProduction`] for a work mode when
    /// `production` is true.
    pub fn validate(self, production: bool) -> Result<Self, ModeError> {
        if production && self != Self::DagBft {
            return Err(ModeError::NotAllowedInProduction(self));
        }
        Ok(self)
    }
}

impl fmt::Display for ConsensusMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for ConsensusMode {
    type Err = ModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|m| m.name() == s)
            .ok_or_else(|| ModeError::Unknown(s.to_owned()))
    }
}

/// Why a configured mode was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModeError {
    /// Not one of `argonblake-pow`, `pouw-lattice`, `dag-bft`.
    Unknown(String),
    /// A work mode in a production build.
    NotAllowedInProduction(ConsensusMode),
}

impl fmt::Display for ModeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(s) => write!(
                f,
                "unknown consensus mode `{s}`; expected argonblake-pow, pouw-lattice or dag-bft"
            ),
            Self::NotAllowedInProduction(m) => write!(
                f,
                "consensus mode `{m}` is devnet/research only; a production build runs dag-bft (ADR-015)"
            ),
        }
    }
}

impl std::error::Error for ModeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_names_are_refused() {
        for m in ConsensusMode::ALL {
            assert_eq!(m.name().parse::<ConsensusMode>(), Ok(m));
        }
        assert!(matches!("pos".parse::<ConsensusMode>(), Err(ModeError::Unknown(_))));
    }

    #[test]
    fn production_accepts_only_dag_bft() {
        assert_eq!(ConsensusMode::DagBft.validate(true), Ok(ConsensusMode::DagBft));
        for m in [ConsensusMode::ArgonBlakePow, ConsensusMode::PouwLattice] {
            assert_eq!(m.validate(true), Err(ModeError::NotAllowedInProduction(m)));
            assert_eq!(m.validate(false), Ok(m));
        }
    }
}
