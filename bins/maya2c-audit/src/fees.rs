//! Check 3: fee-sink accounting. Fees are tx outputs to the fee collector; the
//! base-fee part of them is burned to the sink (`settle_fees`,
//! `crates/node/src/state/fees.rs`); the tips stay in the collector until the
//! staking epoch ends.
//!
//! Per block, from the RPC alone:
//! - the sink only grows, and its growth beyond direct sink outputs is the burn;
//! - the burn cannot exceed what the block sent to the collector;
//! - off an epoch boundary, the collector gains exactly `collected - burned`.
//!
//! Not checked: that the burn equals `base_fee x bytes`. The RPC gives the
//! current base fee only (`get_fee_info`), not the one each height used.

use maya_amount::Amount;
use maya_ledger_math::{credit, total_outputs};

use crate::genesis::Plan;
use crate::numbers::{FEE_COLLECTOR, FEE_SINK};
use crate::observe::{Fetch, Observed};
use crate::report::{Finding, Verdict};
use crate::rpc::{Change, Output};

/// What one height's fee accounting came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Flow {
    burned: Amount,
    collected: Amount,
}

/// Runs check 3 over every height in the window.
#[must_use]
pub fn check(plan: &Plan, observed: &Observed) -> Finding {
    let mut finding = Finding::new(
        "fee-sink",
        "fees: burn <= collected, collector gains collected - burn",
        Verdict::Pass,
    );
    let mut total = Flow::default();
    let mut exact_heights = 0u64;
    let mut boundaries = 0u64;
    for facts in &observed.heights {
        let (Fetch::Got(changes), Fetch::Got(outputs)) = (&facts.changes, &facts.outputs) else {
            finding.verdict = finding.verdict.worst(Verdict::Unavailable);
            finding.note(format!(
                "height {}: balance changes or block not kept",
                facts.height
            ));
            continue;
        };
        let boundary = is_boundary(facts.height, plan.epoch_blocks);
        match judge_height(facts.height, changes, outputs, boundary) {
            Ok(flow) => match add_flows(total, flow) {
                Some(sum) => total = sum,
                None => {
                    finding.verdict = Verdict::Fail;
                    finding.note(format!(
                        "height {}: running fee totals overflow U256",
                        facts.height
                    ));
                }
            },
            Err(reason) => {
                finding.verdict = Verdict::Fail;
                finding.note(reason);
            }
        }
        if boundary {
            boundaries += 1;
        } else {
            exact_heights += 1;
        }
    }
    summarise(&total, exact_heights, boundaries, observed, &mut finding);
    finding
}

fn add_flows(a: Flow, b: Flow) -> Option<Flow> {
    Some(Flow {
        burned: credit(a.burned, b.burned)?,
        collected: credit(a.collected, b.collected)?,
    })
}

fn is_boundary(height: u64, epoch_blocks: Option<u64>) -> bool {
    matches!(epoch_blocks, Some(e) if height > 0 && height % e == 0)
}

/// Judges one height. `Err` names the height and the rule it broke.
fn judge_height(
    height: u64,
    changes: &[Change],
    outputs: &[Output],
    boundary: bool,
) -> Result<Flow, String> {
    let sum_to = |who: [u8; 32]| {
        total_outputs(
            outputs
                .iter()
                .filter(|o| o.recipient == who)
                .map(|o| o.amount),
        )
    };
    let collected = sum_to(FEE_COLLECTOR)
        .ok_or_else(|| format!("height {height}: collected fees overflow U256"))?;
    let direct_burn =
        sum_to(FEE_SINK).ok_or_else(|| format!("height {height}: sink outputs overflow U256"))?;

    let (sink_before, sink_after) = balance_pair(changes, FEE_SINK);
    let sink_gain = sink_after.checked_sub(sink_before).ok_or_else(|| {
        format!("height {height}: the burn sink decreased ({sink_before} -> {sink_after})")
    })?;
    let burned = sink_gain.checked_sub(direct_burn).ok_or_else(|| {
        format!("height {height}: sink gained {sink_gain} but received {direct_burn} directly")
    })?;
    if burned > collected {
        return Err(format!(
            "height {height}: burned {burned} from fees, but only {collected} was collected"
        ));
    }

    let (coll_before, coll_after) = balance_pair(changes, FEE_COLLECTOR);
    if !boundary {
        // `burned <= collected` was checked above, so this cannot underflow.
        let expected = collected
            .checked_sub(burned)
            .ok_or_else(|| format!("height {height}: burn exceeds collected"))?;
        let gain = coll_after
            .checked_sub(coll_before)
            .ok_or_else(|| format!("height {height}: collector decreased off an epoch boundary"))?;
        if gain != expected {
            return Err(format!(
                "height {height}: collector gained {gain}, expected collected {collected} - burned {burned}"
            ));
        }
    }
    Ok(Flow { burned, collected })
}

/// The `(before, after)` of `address` in one block; zeros when it did not change.
/// The node writes one entry per changed account, so the first match is the one.
fn balance_pair(changes: &[Change], address: [u8; 32]) -> (Amount, Amount) {
    changes
        .iter()
        .find(|c| c.address == address)
        .map_or((Amount::ZERO, Amount::ZERO), |c| (c.before, c.after))
}

fn summarise(
    total: &Flow,
    exact: u64,
    boundaries: u64,
    observed: &Observed,
    finding: &mut Finding,
) {
    finding.note(format!(
        "heights {}..={}: burned {} from fees, collected {}",
        observed.from,
        observed.to(),
        total.burned,
        total.collected
    ));
    finding.note(format!(
        "{exact} off-boundary height(s) checked for collector accounting; {boundaries} epoch boundary height(s) checked for burn only (staking pays the collector out there)"
    ));
    finding.note("not checked: burn = base fee x bytes (no per-height base fee in RPC)");
}
