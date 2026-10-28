//! Equivocation evidence on disk, and back into a `ReportEquivocation`.
//!
//! A node that sees a validator sign two proposals for one slot cannot file
//! the report itself: that is a transaction, signed by an account that pays
//! the fee, and a validator process holds no account key. So it writes the pair
//! here, and an operator files it with `l1-wallet report-equivocation`.
//! Evidence is admissible for `unbonding_epochs + 1` epochs after the offence,
//! while the chain still holds that epoch's committee record (`staking_exec`
//! prunes older ones), whether or not the offender is still bonded. That is
//! the usual evidence age limit (Cosmos ties it to the unbonding period
//! too), so file promptly. The file must outlive a restart.

use maya_dag_bft::{Equivocation, Message};

use super::wire::{BROADCAST, Envelope};
use crate::core::staking_payload::StakingAction;
use crate::error::{NodeError, Result};

fn frame(vertex: &maya_dag_bft::Vertex, signature: &[u8]) -> Vec<u8> {
    Envelope {
        epoch: vertex.epoch,
        from: vertex.author,
        to: BROADCAST,
        message: Message::Propose {
            vertex: vertex.clone(),
            signature: signature.to_vec(),
        },
    }
    .encode()
}

/// The two conflicting proposals as the staking module expects them.
#[must_use]
pub fn report(evidence: &Equivocation) -> StakingAction {
    StakingAction::ReportEquivocation {
        first: frame(&evidence.first, &evidence.first_signature),
        second: frame(&evidence.second, &evidence.second_signature),
    }
}

/// File name for `evidence`: who, which epoch, which round. Re-detecting the
/// same pair overwrites rather than piling up.
#[must_use]
pub fn file_name(evidence: &Equivocation) -> String {
    format!(
        "equivocation-e{}-r{}-v{}.hex",
        evidence.first.epoch, evidence.first.round, evidence.first.author
    )
}

/// Two lines of hex, one frame each.
#[must_use]
pub fn to_text(evidence: &Equivocation) -> String {
    match report(evidence) {
        StakingAction::ReportEquivocation { first, second } => {
            format!("{}\n{}\n", hex::encode(first), hex::encode(second))
        }
        _ => unreachable!("report builds a ReportEquivocation"),
    }
}

/// Reads what [`to_text`] wrote.
///
/// # Errors
///
/// Not exactly two lines of hex.
pub fn from_text(text: &str) -> Result<StakingAction> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let mut next = || -> Result<Vec<u8>> {
        let line = lines
            .next()
            .ok_or_else(|| NodeError::Decode("evidence file needs two frames".to_string()))?;
        hex::decode(line).map_err(|e| NodeError::Decode(format!("evidence is not hex: {e}")))
    };
    let (first, second) = (next()?, next()?);
    if lines.next().is_some() {
        return Err(NodeError::Decode(
            "evidence file has extra lines".to_string(),
        ));
    }
    Ok(StakingAction::ReportEquivocation { first, second })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use maya_dag_bft::Vertex;

    fn vertex(round: u64, timestamp_ms: u64) -> Vertex {
        Vertex {
            epoch: 2,
            round,
            author: 3,
            timestamp_ms,
            parents: vec![],
            batch: vec![],
        }
    }

    fn pair() -> Equivocation {
        Equivocation {
            first: vertex(7, 1),
            first_signature: vec![9; 4],
            second: vertex(7, 2),
            second_signature: vec![8; 4],
        }
    }

    #[test]
    fn evidence_survives_the_file_round_trip_and_names_its_proposals() {
        let e = pair();
        let action = from_text(&to_text(&e)).unwrap();
        assert_eq!(action, report(&e));
        let StakingAction::ReportEquivocation { first, second } = action else {
            panic!("wrong action");
        };
        for (bytes, want) in [(first, &e.first), (second, &e.second)] {
            match Envelope::decode(&bytes).unwrap().message {
                Message::Propose { vertex, .. } => assert_eq!(&vertex, want),
                _ => panic!("not a proposal"),
            }
        }
        assert_eq!(file_name(&e), "equivocation-e2-r7-v3.hex");
    }

    #[test]
    fn a_damaged_file_is_refused() {
        for bad in ["", "zz\nzz\n", "00\n", "00\n11\n22\n"] {
            assert!(from_text(bad).is_err(), "accepted {bad:?}");
        }
    }
}
