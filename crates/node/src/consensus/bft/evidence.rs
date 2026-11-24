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

use std::io::Write as _;
use std::path::{Path, PathBuf};

use maya_dag_bft::{Equivocation, Message};

use super::attest::Attestation;
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

/// File name for an attestation double-sign: who and which epoch. Not the
/// height: one pair per validator and epoch is all a report needs.
#[must_use]
pub fn attestation_file_name(first: &Attestation) -> String {
    format!("attestation-e{}-v{}.hex", first.epoch, first.validator)
}

/// Keeps the pair in `dir` unless this validator already has one on file for
/// this epoch. Returns the path written, or `None` when one was already kept.
///
/// Nothing deters a double-sign yet (CON-05), so a file per height let one
/// Byzantine member grow every node's disk for as long as the chain ran.
/// `create_new` keeps the first pair; a later one never overwrites it.
///
/// # Errors
///
/// The directory or the file could not be created or written.
pub fn save_attestation(
    dir: &Path,
    first: &Attestation,
    second: &Attestation,
) -> std::io::Result<Option<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(attestation_file_name(first));
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path);
    let mut file = match opened {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => return Err(error),
    };
    if let Err(error) = file.write_all(attestation_to_text(first, second).as_bytes()) {
        drop(file);
        // A partial file would keep the whole pair from ever being written.
        // The write error is the one reported; a failed removal adds nothing.
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(Some(path))
}

/// Two lines of hex, one attestation frame each. Not a `ReportEquivocation`:
/// the chain does not accept attestation evidence yet (audit CON-05), so this
/// is kept for when it does, and for the operator.
#[must_use]
pub fn attestation_to_text(first: &Attestation, second: &Attestation) -> String {
    format!(
        "{}\n{}\n",
        hex::encode(first.encode()),
        hex::encode(second.encode())
    )
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
