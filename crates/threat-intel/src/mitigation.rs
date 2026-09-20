//! What an indicator obliges a node to do.
//!
//! Computed, never stored. A stored mitigation would be a second record that
//! has to agree with the indicator after every block and every reorg; a
//! function of the indicator and the height cannot disagree with it.

use crate::evidence::Author;
use crate::indicator::ThreatIndicator;

/// Refuse `author`, and whatever address this node knows it by, until
/// `until_height`.
///
/// Carries an author, not an address. Resolving an author to an address is
/// each node's own business, from its own authenticated connections — see the
/// crate docs for why an address never reaches the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomatedMitigation {
    /// Who to refuse.
    pub author: Author,
    /// Offences on record, for operators.
    pub offences: u32,
    /// First height at which the refusal lifts.
    pub until_height: u64,
}

/// The mitigation `indicator` requires at `height`, or `None` once it has
/// lifted.
#[must_use]
pub fn mitigation(
    author: &Author,
    indicator: &ThreatIndicator,
    height: u64,
) -> Option<AutomatedMitigation> {
    if !indicator.is_active(height) {
        return None;
    }
    Some(AutomatedMitigation {
        author: *author,
        offences: indicator.offences,
        until_height: indicator.until_height()?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::evidence::OffenceKind;
    use crate::score::HALF_LIFE_BLOCKS;

    #[test]
    fn a_mitigation_lasts_exactly_until_its_height() {
        let indicator = ThreatIndicator::observe(None, OffenceKind::TxRootMismatch, 100);
        let author = [3; 32];
        let active = mitigation(&author, &indicator, 100).expect("active");
        assert_eq!(active.until_height, 100 + HALF_LIFE_BLOCKS);
        assert!(mitigation(&author, &indicator, active.until_height - 1).is_some());
        assert!(mitigation(&author, &indicator, active.until_height).is_none());
    }
}
