//! Collision avoidance checked off chain, against plans committed on chain.
//!
//! Each agent commits to its flight plan — timed waypoints in millimetres and
//! milliseconds — before flying. Anyone can check every pair of plans off
//! chain; a conflict is proved on chain by opening the two commitments and
//! naming the instant the agents come within [`MIN_SEPARATION_MM`]. The
//! arithmetic is integer only, so every verifier reaches the same verdict.

use crate::{AgentId, SwarmError};

/// Closest two agents may come, mm.
pub const MIN_SEPARATION_MM: i64 = 2_000;
/// Time step of the check, ms.
pub const STEP_MS: u64 = 100;
/// Most waypoints in one plan.
pub const MAX_WAYPOINTS: usize = 1_024;

/// A timed position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waypoint {
    /// ms from the plan's epoch.
    pub t_ms: u64,
    /// x, y, z in mm.
    pub at: [i64; 3],
}

/// An agent's committed plan: waypoints in increasing time order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Whose.
    pub agent: AgentId,
    /// The waypoints.
    pub waypoints: Vec<Waypoint>,
}

impl Plan {
    /// Checks the shape: at least one waypoint, strictly increasing times.
    ///
    /// # Errors
    ///
    /// [`SwarmError::Refused`] for an empty, too long or unordered plan.
    pub fn validate(&self) -> Result<(), SwarmError> {
        if self.waypoints.is_empty() || self.waypoints.len() > MAX_WAYPOINTS {
            return Err(SwarmError::Refused("a plan has 1 to 1,024 waypoints"));
        }
        if self.waypoints.windows(2).any(|w| w[1].t_ms <= w[0].t_ms) {
            return Err(SwarmError::Refused("waypoint times must increase"));
        }
        Ok(())
    }

    /// The commitment published on chain.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new_derive_key("maya2c swarm flight plan v1");
        h.update(&self.agent.to_le_bytes());
        for w in &self.waypoints {
            h.update(&w.t_ms.to_le_bytes());
            w.at.iter().for_each(|c| {
                h.update(&c.to_le_bytes());
            });
        }
        *h.finalize().as_bytes()
    }

    /// Position at `t_ms`, linearly between waypoints, held before the first
    /// and after the last. Integer arithmetic throughout.
    #[must_use]
    pub fn position(&self, t_ms: u64) -> [i64; 3] {
        let w = &self.waypoints;
        let first = w[0];
        if t_ms <= first.t_ms {
            return first.at;
        }
        for pair in w.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t_ms <= b.t_ms {
                let span = i128::from(b.t_ms - a.t_ms);
                let done = i128::from(t_ms - a.t_ms);
                return core::array::from_fn(|i| {
                    let d = i128::from(b.at[i] - a.at[i]) * done / span;
                    a.at[i] + i64::try_from(d).unwrap_or(0)
                });
            }
        }
        w[w.len() - 1].at
    }

    fn end(&self) -> u64 {
        self.waypoints.last().map_or(0, |w| w.t_ms)
    }
}

fn too_close(a: [i64; 3], b: [i64; 3]) -> bool {
    let d2: i128 = (0..3).map(|i| i128::from(a[i] - b[i]).pow(2)).sum();
    d2 < i128::from(MIN_SEPARATION_MM).pow(2)
}

/// A proven conflict: two plans and the instant they are too close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// One agent.
    pub a: AgentId,
    /// The other.
    pub b: AgentId,
    /// When, ms.
    pub t_ms: u64,
}

/// The first instant (on the [`STEP_MS`] grid) two plans come too close.
#[must_use]
pub fn first_conflict(a: &Plan, b: &Plan) -> Option<u64> {
    let end = a.end().max(b.end());
    (0..=end / STEP_MS)
        .map(|k| k * STEP_MS)
        .find(|t| too_close(a.position(*t), b.position(*t)))
}

/// Every conflicting pair among `plans`.
#[must_use]
pub fn check_all(plans: &[Plan]) -> Vec<Conflict> {
    let mut out = Vec::new();
    for (i, a) in plans.iter().enumerate() {
        for b in &plans[i + 1..] {
            if let Some(t_ms) = first_conflict(a, b) {
                out.push(Conflict {
                    a: a.agent,
                    b: b.agent,
                    t_ms,
                });
            }
        }
    }
    out
}

/// What the chain checks for a conflict claim: both plans open their
/// commitments, are well formed, and are too close at `t_ms`.
#[must_use]
pub fn verify_conflict(
    commit_a: &[u8; 32],
    a: &Plan,
    commit_b: &[u8; 32],
    b: &Plan,
    t_ms: u64,
) -> bool {
    a.commitment() == *commit_a
        && b.commitment() == *commit_b
        && a.validate().is_ok()
        && b.validate().is_ok()
        && t_ms.is_multiple_of(STEP_MS)
        && too_close(a.position(t_ms), b.position(t_ms))
}
