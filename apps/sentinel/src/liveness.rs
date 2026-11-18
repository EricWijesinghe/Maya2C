//! Consensus-side detectors (ADR-101): finality lag, per-validator liveness,
//! and the sentinel's own blindness. Level-triggered like `detect.rs`; the
//! incident layer turns them into one push each.

use std::collections::HashMap;

use crate::detect::{Finding, Severity};
use crate::obs::{NodeObs, Sample};

/// Default finality lag: tip minus newest quorum-attested block. 30 blocks is
/// the halt window at the 1 block/s target, so a lag pages on the same clock.
pub const DEFAULT_FINALITY_LAG_BLOCKS: u64 = 30;
/// Default time a committee member may go without a new proposal round while
/// the network commits. Twice the halt window, so a chain-wide stall pages as
/// a halt first and not as one alert per validator.
pub const DEFAULT_LIVENESS_MS: u64 = 60_000;

/// What the consensus detectors remember between polls.
#[derive(Debug, Default)]
pub struct Liveness {
    /// Wall time of the first poll, and of the last poll any node answered.
    first_poll: Option<u64>,
    last_answer: Option<u64>,
    /// Per node: last proposal round seen, when it was first seen, and the
    /// network's highest committed round at that moment.
    rounds: HashMap<String, (u64, u64, u64)>,
}

impl Liveness {
    /// No node answered for `window_ms`: the sentinel sees nothing, so every
    /// other detector is silent for a reason that is not a healthy chain.
    pub fn blind(&mut self, now: u64, samples: &[Sample], window_ms: u64) -> Option<Finding> {
        let first = *self.first_poll.get_or_insert(now);
        if samples.iter().any(|s| s.state.is_ok()) {
            self.last_answer = Some(now);
            return None;
        }
        let since = self.last_answer.unwrap_or(first);
        let blind = now.saturating_sub(since);
        (!samples.is_empty() && blind >= window_ms).then(|| Finding {
            key: "blind".into(),
            severity: Severity::Critical,
            title: "Maya2C sentinel is blind".into(),
            body: format!(
                "none of {} nodes answered for {} s; the sentinel cannot see the chain",
                samples.len(),
                blind / 1000
            ),
        })
    }

    /// Committee members whose proposal round has not moved for `window_ms`
    /// while the network's committed round has.
    pub fn validators(
        &mut self,
        now: u64,
        ups: &[(&str, &NodeObs)],
        window_ms: u64,
    ) -> Vec<Finding> {
        let net = ups
            .iter()
            .filter_map(|(_, o)| o.bft.map(|b| b.committed_round))
            .max()
            .unwrap_or(0);
        let mut out = Vec::new();
        for (name, o) in ups {
            let Some(b) = o.bft.filter(|b| b.validator.is_some()) else {
                self.rounds.remove(*name);
                continue;
            };
            let entry = self
                .rounds
                .entry((*name).to_owned())
                .or_insert((b.round, now, net));
            // Any change counts as progress: an epoch switch restarts rounds.
            if entry.0 != b.round {
                *entry = (b.round, now, net);
            }
            let (round, since, net_then) = *entry;
            let stalled = now.saturating_sub(since);
            if stalled >= window_ms && net > net_then {
                let idx = b.validator.unwrap_or_default();
                let mode = if b.follower {
                    ", following attested blocks"
                } else {
                    ""
                };
                out.push(Finding {
                    key: format!("liveness:{name}"),
                    severity: Severity::Warning,
                    title: format!("Maya2C validator {name} not proposing"),
                    body: format!(
                        "{name} (committee index {idx}) stuck at round {round} for {} s{mode} while the network committed to round {net}",
                        stalled / 1000
                    ),
                });
            }
        }
        out
    }
}

/// The tip is more than `limit` blocks past the newest quorum-attested block:
/// attestations are not reaching `2f + 1`, so a node that falls behind has no
/// checkpoint to catch up to (ADR-038).
#[must_use]
pub fn finality_lag(ups: &[(&str, &NodeObs)], limit: u64) -> Option<Finding> {
    let finalized = ups.iter().filter_map(|(_, o)| o.finalized).max()?;
    let tip = ups.iter().map(|(_, o)| o.height).max()?;
    let lag = tip.saturating_sub(finalized);
    (lag >= limit).then(|| Finding {
        key: "finality-lag".into(),
        severity: Severity::Warning,
        title: "Maya2C finality lagging".into(),
        body: format!(
            "tip {tip} is {lag} blocks past the newest checkpoint {finalized}; limit is {limit}"
        ),
    })
}
