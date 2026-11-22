//! The detectors. Each poll they answer one question: which conditions hold
//! right now? Edge-triggering (raise once, clear once) is `incident.rs`'s job.
//!
//! Level-triggered on purpose: a condition that stays true keeps appearing in
//! the findings, so the incident layer sees it as one ongoing incident.

use std::collections::{BTreeMap, HashMap};

use primitive_types::U256;

use crate::liveness::{self, Liveness};
pub use crate::liveness::{DEFAULT_FINALITY_LAG_BLOCKS, DEFAULT_LIVENESS_MS};
use crate::obs::{NodeObs, Sample, Supply};

/// Default halt gap: 30 s with no new block, at a 1 block/s target.
pub const DEFAULT_HALT_MS: u64 = 30_000;
/// Default unreachable window before a node is reported down.
pub const DEFAULT_UNREACHABLE_MS: u64 = 45_000;
/// Default peer floor. Below this a node is isolated, not part of the network.
pub const DEFAULT_MIN_PEERS: usize = 2;
/// Default count of consecutive empty blocks, with transactions waiting, that
/// counts as stranding.
pub const DEFAULT_STRAND_BLOCKS: u64 = 20;
/// Default age of a waiting transaction with no block carrying one, that counts
/// as stranding.
pub const DEFAULT_STRAND_MS: u64 = 120_000;

/// How loud a finding is. Maps to the ntfy priority in `notify.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth a look within the hour.
    Warning,
    /// Wake someone up.
    Critical,
}

/// One condition that holds right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// Stable identity of the incident, e.g. `halt` or `fork:4211`.
    pub key: String,
    /// How loud it is.
    pub severity: Severity,
    /// Short headline, ASCII.
    pub title: String,
    /// One line of detail.
    pub body: String,
}

/// Configured limits. Milliseconds are wall time between polls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    /// No new tip for this long is a halt.
    pub halt_ms: u64,
    /// No answer for this long is an unreachable node.
    pub unreachable_ms: u64,
    /// Fewer peers than this is a collapse.
    pub min_peers: usize,
    /// Consecutive empty blocks with transactions waiting.
    pub strand_blocks: u64,
    /// Transactions waiting this long with no block carrying one.
    pub strand_ms: u64,
    /// Tip this many blocks past the newest checkpoint is a finality lag.
    pub finality_lag_blocks: u64,
    /// A committee member with no new round for this long is not live.
    pub liveness_ms: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            halt_ms: DEFAULT_HALT_MS,
            unreachable_ms: DEFAULT_UNREACHABLE_MS,
            min_peers: DEFAULT_MIN_PEERS,
            strand_blocks: DEFAULT_STRAND_BLOCKS,
            strand_ms: DEFAULT_STRAND_MS,
            finality_lag_blocks: DEFAULT_FINALITY_LAG_BLOCKS,
            liveness_ms: DEFAULT_LIVENESS_MS,
        }
    }
}

/// Carries what a detector must remember between polls.
#[derive(Debug)]
pub struct Detector {
    limits: Thresholds,
    /// Highest tip seen, and the wall time it was first seen.
    tip: Option<(u64, u64)>,
    down_since: HashMap<String, u64>,
    last_total: HashMap<String, (u64, U256)>,
    last_strand_height: Option<u64>,
    strand_streak: u64,
    strand_since: Option<u64>,
    liveness: Liveness,
}

impl Detector {
    /// A detector with no history.
    #[must_use]
    pub fn new(limits: Thresholds) -> Self {
        Self {
            limits,
            tip: None,
            down_since: HashMap::new(),
            last_total: HashMap::new(),
            last_strand_height: None,
            strand_streak: 0,
            strand_since: None,
            liveness: Liveness::default(),
        }
    }

    /// The conditions that hold at wall time `now` (ms), given this poll.
    pub fn evaluate(&mut self, now: u64, samples: &[Sample]) -> Vec<Finding> {
        let mut out = self.reachability(now, samples);
        let ups: Vec<(&str, &NodeObs)> = samples
            .iter()
            .filter_map(|s| s.state.as_ref().ok().map(|o| (s.name.as_str(), o)))
            .collect();
        out.extend(self.halt(now, &ups));
        out.extend(fork(&ups));
        out.extend(self.supply(&ups));
        out.extend(peers(&ups, self.limits.min_peers));
        out.extend(self.stranding(now, &ups));
        out.extend(
            self.liveness
                .blind(now, samples, self.limits.unreachable_ms),
        );
        out.extend(liveness::finality_lag(
            &ups,
            self.limits.finality_lag_blocks,
        ));
        out.extend(self.liveness.validators(now, &ups, self.limits.liveness_ms));
        out
    }

    fn reachability(&mut self, now: u64, samples: &[Sample]) -> Vec<Finding> {
        let mut out = Vec::new();
        for s in samples {
            match &s.state {
                Ok(_) => {
                    self.down_since.remove(&s.name);
                }
                Err(reason) => {
                    let since = *self.down_since.entry(s.name.clone()).or_insert(now);
                    if now.saturating_sub(since) >= self.limits.unreachable_ms {
                        out.push(Finding {
                            key: format!("rpc:{}", s.name),
                            severity: Severity::Warning,
                            title: format!("Maya2C node {} unreachable", s.name),
                            body: format!("no answer for {} s: {reason}", (now - since) / 1000),
                        });
                    }
                }
            }
        }
        out
    }

    fn halt(&mut self, now: u64, ups: &[(&str, &NodeObs)]) -> Option<Finding> {
        let (name, top) = ups.iter().max_by_key(|(_, o)| o.height)?;
        match self.tip {
            Some((h, _)) if top.height <= h => {}
            _ => self.tip = Some((top.height, now)),
        }
        let (h, since) = self.tip?;
        let stalled = now.saturating_sub(since);
        (stalled >= self.limits.halt_ms).then(|| Finding {
            key: "halt".into(),
            severity: Severity::Critical,
            title: "Maya2C chain halted".into(),
            body: format!(
                "no new block for {} s at height {h} (seen on {name})",
                stalled / 1000
            ),
        })
    }

    fn supply(&mut self, ups: &[(&str, &NodeObs)]) -> Vec<Finding> {
        let mut out = Vec::new();
        for (name, o) in ups {
            let Some(s) = o.supply else { continue };
            if s.circulating.saturating_add(s.burned) != s.total {
                out.push(mismatch(name, s));
            }
            if let Some((h, prev)) = self.last_total.get(*name)
                && o.height >= *h
                && s.total < *prev
            {
                out.push(Finding {
                    key: format!("supply-decrease:{name}"),
                    severity: Severity::Warning,
                    title: "Maya2C total supply fell".into(),
                    body: format!(
                        "{name}: total fell at height {}; slashing or a bug",
                        o.height
                    ),
                });
            }
            self.last_total
                .insert((*name).to_owned(), (o.height, s.total));
        }
        out.extend(supply_split(ups));
        out
    }

    fn stranding(&mut self, now: u64, ups: &[(&str, &NodeObs)]) -> Option<Finding> {
        let (_, top) = ups.iter().max_by_key(|(_, o)| o.height)?;
        let advanced = self.last_strand_height.is_none_or(|h| top.height > h);
        if advanced {
            self.last_strand_height = Some(top.height);
            self.update_strand(now, top);
        }
        let by_blocks = self.strand_streak >= self.limits.strand_blocks;
        let by_time = self
            .strand_since
            .is_some_and(|t| now.saturating_sub(t) >= self.limits.strand_ms);
        (by_blocks || by_time).then(|| Finding {
            key: "stranded".into(),
            severity: Severity::Critical,
            title: "Maya2C transactions stranded".into(),
            body: format!(
                "{} transactions waiting; {} empty blocks since one carried any; check consensus and the mempool",
                top.pending.unwrap_or(0),
                self.strand_streak
            ),
        })
    }

    /// Counts a new tip against the stranding pattern: transactions waiting,
    /// and the chain finalizing blocks that carry none of them.
    fn update_strand(&mut self, now: u64, top: &NodeObs) {
        match top.pending {
            Some(p) if p > 0 && top.tip_txs == 0 => {
                self.strand_streak += 1;
                self.strand_since.get_or_insert(now);
            }
            _ => {
                self.strand_streak = 0;
                self.strand_since = None;
            }
        }
    }
}

fn mismatch(name: &str, s: Supply) -> Finding {
    Finding {
        key: format!("supply-mismatch:{name}"),
        severity: Severity::Critical,
        title: "Maya2C supply does not add up".into(),
        body: format!(
            "{name}: circulating {} + burned {} != total {}",
            s.circulating, s.burned, s.total
        ),
    }
}

/// Two nodes at one height that disagree on the tip are a fork.
fn fork(ups: &[(&str, &NodeObs)]) -> Vec<Finding> {
    by_height(ups)
        .into_iter()
        .filter(|(_, group)| group.iter().any(|(_, o)| o.tip_id != group[0].1.tip_id))
        .map(|(h, group)| Finding {
            key: format!("fork:{h}"),
            severity: Severity::Critical,
            title: "Maya2C fork detected".into(),
            body: format!(
                "{} nodes disagree on the block at height {h}: {}",
                group.len(),
                group
                    .iter()
                    .map(|(n, o)| format!("{n}={}", short(&o.tip_id)))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        })
        .collect()
}

/// Two nodes at one height that report different totals.
fn supply_split(ups: &[(&str, &NodeObs)]) -> Vec<Finding> {
    let with_supply: Vec<(&str, &NodeObs)> = ups
        .iter()
        .copied()
        .filter(|(_, o)| o.supply.is_some())
        .collect();
    by_height(&with_supply)
        .into_iter()
        .filter(|(_, group)| {
            group
                .iter()
                .any(|(_, o)| o.supply.map(|s| s.total) != group[0].1.supply.map(|s| s.total))
        })
        .map(|(h, _)| Finding {
            key: format!("supply-split:{h}"),
            severity: Severity::Critical,
            title: "Maya2C nodes disagree on supply".into(),
            body: format!("nodes at height {h} report different total supply"),
        })
        .collect()
}

fn peers(ups: &[(&str, &NodeObs)], min: usize) -> Vec<Finding> {
    ups.iter()
        .filter_map(|(name, o)| {
            let n = o.peers?;
            (n < min).then(|| Finding {
                key: format!("peers:{name}"),
                severity: Severity::Warning,
                title: "Maya2C peer count collapsed".into(),
                body: format!("{name} has {n} peers, floor is {min}"),
            })
        })
        .collect()
}

/// Groups observations by height, in height order.
fn by_height<'a>(ups: &[(&'a str, &'a NodeObs)]) -> BTreeMap<u64, Vec<(&'a str, &'a NodeObs)>> {
    let mut map: BTreeMap<u64, Vec<(&str, &NodeObs)>> = BTreeMap::new();
    for &(name, o) in ups {
        map.entry(o.height).or_default().push((name, o));
    }
    map.retain(|_, group| group.len() > 1);
    map
}

fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}
