//! What to block: a pure function of one observation and what is already
//! enforced.

use std::collections::BTreeSet;
use std::net::IpAddr;

use maya_threat_intel::mitigation;

use crate::source::Observation;

/// The difference between what is enforced and what should be.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Addresses to start dropping, ascending.
    pub block: Vec<IpAddr>,
    /// Addresses to stop dropping, ascending.
    pub unblock: Vec<IpAddr>,
}

/// Addresses the observation says should be dropped: every address an author
/// with an active mitigation connected from.
#[must_use]
pub fn desired(observation: &Observation) -> BTreeSet<IpAddr> {
    let quarantined: BTreeSet<_> = observation
        .indicators
        .iter()
        .filter_map(|(author, indicator)| mitigation(author, indicator, observation.height))
        .map(|active| active.author)
        .collect();
    observation
        .addresses
        .iter()
        .filter(|(author, ip)| quarantined.contains(author) && blockable(*ip))
        .map(|(_, ip)| *ip)
        .collect()
}

/// What to change so that exactly [`desired`] is enforced.
#[must_use]
pub fn decide(observation: &Observation, enforced: &BTreeSet<IpAddr>) -> Plan {
    let wanted = desired(observation);
    Plan {
        block: wanted.difference(enforced).copied().collect(),
        unblock: enforced.difference(&wanted).copied().collect(),
    }
}

/// Never loopback or unspecified. A misattributed local connection — a
/// sidecar, a port-forward — would otherwise cut the host off from itself.
#[must_use]
pub fn blockable(ip: IpAddr) -> bool {
    !ip.is_loopback() && !ip.is_unspecified()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use maya_threat_intel::score::HALF_LIFE_BLOCKS;
    use maya_threat_intel::{OffenceKind, ThreatIndicator};

    use super::*;

    const OFFENDER: [u8; 32] = [1; 32];
    const HONEST: [u8; 32] = [2; 32];

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
    }

    fn observation(height: u64) -> Observation {
        Observation {
            height,
            indicators: vec![(
                OFFENDER,
                ThreatIndicator::observe(None, OffenceKind::InvalidSignature, 10),
            )],
            addresses: vec![
                (OFFENDER, ip(1)),
                (HONEST, ip(2)),
                (OFFENDER, IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ],
        }
    }

    #[test]
    fn an_active_offender_is_blocked_and_nobody_else() {
        let plan = decide(&observation(10), &BTreeSet::new());
        assert_eq!(plan.block, vec![ip(1)]);
        assert!(plan.unblock.is_empty());
    }

    #[test]
    fn nothing_changes_once_enforced() {
        let enforced = BTreeSet::from([ip(1)]);
        assert_eq!(decide(&observation(11), &enforced), Plan::default());
    }

    #[test]
    fn a_lifted_indicator_is_unblocked() {
        let enforced = BTreeSet::from([ip(1)]);
        let plan = decide(&observation(10 + HALF_LIFE_BLOCKS), &enforced);
        assert!(plan.block.is_empty());
        assert_eq!(plan.unblock, vec![ip(1)]);
    }

    #[test]
    fn a_block_nobody_asked_for_is_removed() {
        let enforced = BTreeSet::from([ip(2)]);
        let plan = decide(&observation(10), &enforced);
        assert_eq!(plan.block, vec![ip(1)]);
        assert_eq!(plan.unblock, vec![ip(2)]);
    }
}
