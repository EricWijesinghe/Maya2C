//! Reads every block in the audit window once, so the checks share one pass.
//!
//! A `Rejected` answer means the node does not keep that data (pruned, or no
//! balance changes kept for genesis): the audit reports NOT VERIFIED for it.
//! Transport and malformed answers stop the run, because nothing read after
//! them can be trusted.

use crate::rpc::{Change, Output, Rpc, RpcError};

/// One call's outcome: the data, or the node's reason it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch<T> {
    Got(T),
    Unavailable(String),
}

/// Everything read for one height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeightFacts {
    pub height: u64,
    pub changes: Fetch<Vec<Change>>,
    pub outputs: Fetch<Vec<Output>>,
}

/// The window `from ..= to`, one entry per height in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    pub from: u64,
    pub heights: Vec<HeightFacts>,
}

impl Observed {
    /// The last height read.
    #[must_use]
    pub fn to(&self) -> u64 {
        self.from + self.heights.len() as u64 - 1
    }

    /// Facts for `height`, when it lies in the window.
    #[must_use]
    pub fn get(&self, height: u64) -> Option<&HeightFacts> {
        let index = usize::try_from(height.checked_sub(self.from)?).ok()?;
        self.heights.get(index)
    }
}

/// Reads `from ..= to`.
///
/// # Errors
///
/// Any RPC error other than a node's refusal to answer for a height.
pub fn observe(rpc: &dyn Rpc, from: u64, to: u64) -> Result<Observed, RpcError> {
    let mut heights = Vec::new();
    for height in from..=to {
        heights.push(HeightFacts {
            height,
            changes: refusable(rpc.balance_changes(height))?,
            outputs: refusable(rpc.block_outputs(height))?,
        });
    }
    Ok(Observed { from, heights })
}

fn refusable<T>(result: Result<T, RpcError>) -> Result<Fetch<T>, RpcError> {
    match result {
        Ok(value) => Ok(Fetch::Got(value)),
        Err(RpcError::Rejected(reason)) => Ok(Fetch::Unavailable(reason)),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::fixture::Fake;

    #[test]
    fn a_refused_height_is_unavailable_not_an_error() {
        let mut fake = Fake::default();
        fake.refuse_changes.push(2);
        let observed = observe(&fake, 1, 3).unwrap();
        assert_eq!(observed.to(), 3);
        assert!(matches!(
            observed.get(2).unwrap().changes,
            Fetch::Unavailable(_)
        ));
        assert!(matches!(observed.get(3).unwrap().changes, Fetch::Got(_)));
        assert!(observed.get(0).is_none());
    }

    #[test]
    fn a_transport_failure_stops_the_read() {
        let mut fake = Fake::default();
        fake.broken = true;
        assert!(matches!(observe(&fake, 1, 2), Err(RpcError::Transport(_))));
    }
}
