//! The checks a public endpoint allows, on top of the sentinel's detectors.
//!
//! The revert, byte-cap and unreadable-block checks are level-triggered like
//! the sentinel's: a violation that persists keeps appearing, and
//! `maya_sentinel::incident` turns it into one incident. The two regress
//! checks fire on the poll the value falls (a restart or a lagging backend
//! behind a load balancer also trips them, so they are warnings).
//!
//! Trust: the first attestation heard for a height is the reference, and
//! signatures are not verified (committee keys are not public). One lying
//! source heard first makes every honest source look like a revert; either
//! way the alert says the sources disagree about a finalized block.

use std::collections::BTreeMap;

use maya_sentinel::detect::{Finding, Severity};
use maya_sentinel::obs::Sample;

use crate::public::PublicObs;

/// Finalized heights remembered for the revert check.
/// shortcut: a bounded window of the newest heights; older ones are not
/// re-checked. Upgrade to a persisted index if a long-range revert must be seen.
pub const FINAL_KEEP: usize = 4_096;

/// What the checks remember between polls.
#[derive(Debug, Default)]
pub struct PublicChecks {
    heights: BTreeMap<String, u64>,
    newest: BTreeMap<String, (u64, u64)>,
    /// Height -> (first attested block id, source that attested it).
    finalized: BTreeMap<u64, (String, String)>,
}

impl PublicChecks {
    /// Checks with no history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Violations in this poll.
    pub fn evaluate(
        &mut self,
        samples: &[Sample],
        publics: &[(String, Result<PublicObs, String>)],
    ) -> Vec<Finding> {
        let mut out = Vec::new();
        for s in samples {
            let Ok(o) = &s.state else { continue };
            if let Some(prev) = self.heights.insert(s.name.clone(), o.height)
                && o.height < prev
            {
                out.push(finding(
                    format!("height-regress:{}", s.name),
                    Severity::Warning,
                    "Maya2C reported height went backwards",
                    format!("{}: height {prev} then {}", s.name, o.height),
                ));
            }
        }
        for (name, p) in publics {
            let Ok(p) = p else { continue };
            out.extend(self.public_one(name, p));
        }
        while self.finalized.len() > FINAL_KEEP {
            self.finalized.pop_first();
        }
        out
    }

    fn public_one(&mut self, name: &str, p: &PublicObs) -> Vec<Finding> {
        let mut out = Vec::new();
        let Some(cp) = &p.checkpoint else { return out };
        let at = (cp.epoch, cp.height);
        if let Some(prev) = self.newest.insert(name.to_owned(), at)
            && at < prev
        {
            out.push(finding(
                format!("checkpoint-regress:{name}"),
                Severity::Warning,
                "Maya2C checkpoint went backwards",
                format!(
                    "{name}: newest checkpoint epoch {} height {} after epoch {} height {}",
                    at.0, at.1, prev.0, prev.1
                ),
            ));
        }
        // First attestation seen for a height wins; any other id there is a revert.
        let (first, by) = self
            .finalized
            .entry(cp.height)
            .or_insert_with(|| (cp.block.clone(), name.to_owned()))
            .clone();
        let served = p.block.as_ref().map(|b| b.id.as_str());
        let conflict = if cp.block == first {
            served.filter(|id| *id != first).map(|id| {
                format!(
                    "{name} serves {} at finalized height {}; the checkpoint is {}",
                    short(id),
                    cp.height,
                    short(&first)
                )
            })
        } else {
            Some(format!(
                "{name} attests {} at height {}; {by} attested {}",
                short(&cp.block),
                cp.height,
                short(&first)
            ))
        };
        if let Some(body) = conflict {
            out.push(finding(
                format!("finality-revert:{}", cp.height),
                Severity::Critical,
                "Maya2C finalized block changed",
                body,
            ));
        }
        if let Some(e) = &p.block_error {
            out.push(finding(
                format!("finalized-block-unreadable:{name}"),
                Severity::Warning,
                "Maya2C finalized block not served",
                format!("{name}: block at checkpoint height {}: {e}", cp.height),
            ));
        }
        if let Some(b) = &p.block
            && b.bytes > maya_fee_market::limits::MAX_BLOCK_BYTES
        {
            out.push(finding(
                format!("block-cap:{}", cp.height),
                Severity::Critical,
                "Maya2C finalized block over the byte cap",
                format!(
                    "{name}: block at height {} is {} bytes, cap {} (invariant 33)",
                    cp.height,
                    b.bytes,
                    maya_fee_market::limits::MAX_BLOCK_BYTES
                ),
            ));
        }
        out
    }

    /// Finalized heights currently remembered.
    #[must_use]
    pub fn finalized_len(&self) -> usize {
        self.finalized.len()
    }
}

fn finding(key: String, severity: Severity, title: &str, body: String) -> Finding {
    Finding {
        key,
        severity,
        title: title.to_owned(),
        body,
    }
}

fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}
