//! Checks 1 and 1b: nothing was minted, and the mining reserve reconciles to its
//! genesis allocation through every balance change.

use maya_amount::Amount;
use maya_ledger_math::credit;

use crate::genesis::Plan;
use crate::numbers::MINING_RESERVE;
use crate::observe::{Fetch, Observed};
use crate::report::{Finding, Verdict};
use crate::rpc::{Rpc, RpcError};

/// Check 1: the node's total supply at the tip equals the genesis total.
///
/// Total counts every account, the shielded pool and staked funds, so transfers,
/// burns and bonds leave it unchanged. Only a mint, or a balance that vanished,
/// moves it. The sum is the node's own (`get_supply`), so this checks the node's
/// arithmetic against genesis; it does not recount accounts.
///
/// # Errors
///
/// Transport and malformed answers.
pub fn conservation(rpc: &dyn Rpc, plan: &Plan) -> Result<Finding, RpcError> {
    let tip = rpc.chain_info()?.height;
    let supply = rpc.supply()?;
    let mut finding = Finding::new(
        "supply-conservation",
        "total supply equals the genesis total (no mint)",
        Verdict::Pass,
    );
    if supply.total == plan.genesis_total {
        finding.note(format!(
            "tip {tip}: total {} = genesis {} (shielded {}, burned {})",
            supply.total, plan.genesis_total, supply.shielded, supply.burned
        ));
        return Ok(finding);
    }
    finding.verdict = Verdict::Fail;
    let (minted, missing) = difference(supply.total, plan.genesis_total);
    finding.note(format!(
        "tip {tip}: total {} != genesis {} ({})",
        supply.total,
        plan.genesis_total,
        match (minted, missing) {
            (Some(x), _) => format!("+{x} above genesis: coins were printed"),
            (_, Some(x)) => format!("-{x} below genesis: value vanished"),
            _ => "unreachable".to_string(),
        }
    ));
    Ok(finding)
}

/// `(a - b, b - a)` as the side that is positive: `(Some(x), None)` when `a > b`.
fn difference(a: Amount, b: Amount) -> (Option<Amount>, Option<Amount>) {
    (a.checked_sub(b), b.checked_sub(a))
}

/// Check 1b: `reserve now + every reserve decrease, from genesis` equals the
/// genesis reserve allocation. Needs every block from height 1 and the tip, so
/// it runs only over a window that starts at 1 and ends at the tip.
///
/// # Errors
///
/// Transport and malformed answers.
pub fn reserve_reconciles(
    rpc: &dyn Rpc,
    plan: &Plan,
    observed: &Observed,
) -> Result<Finding, RpcError> {
    let mut finding = Finding::new(
        "reserve-genesis",
        "mining reserve = genesis allocation - every payout (from height 1)",
        Verdict::Pass,
    );
    if plan.mining.is_none() {
        finding.verdict = Verdict::Skipped;
        finding.note("no mining in genesis: the reserve is not used");
        return Ok(finding);
    }
    if observed.from != 1 {
        finding.verdict = Verdict::Skipped;
        finding.note(format!(
            "window starts at {}: needs every block from 1",
            observed.from
        ));
        return Ok(finding);
    }
    let Some(paid) = reserve_paid(observed, &mut finding) else {
        return Ok(finding);
    };
    let reserve_now = rpc.balance(&MINING_RESERVE)?;
    // A block landing between the reads would make the sum refer to two heights.
    if rpc.chain_info()?.height != observed.to() {
        finding.verdict = Verdict::Unavailable;
        finding.note("the tip moved during the audit; rerun with --to set to the tip");
        return Ok(finding);
    }
    let Some(reconciled) = credit(reserve_now, paid) else {
        finding.verdict = Verdict::Fail;
        finding.note("reserve now plus payouts overflows U256");
        return Ok(finding);
    };
    if reconciled == plan.reserve_genesis {
        finding.note(format!(
            "reserve {reserve_now} + paid {paid} = genesis {}",
            plan.reserve_genesis
        ));
    } else {
        finding.verdict = Verdict::Fail;
        finding.note(format!(
            "reserve {reserve_now} + paid {paid} = {reconciled} != genesis {}",
            plan.reserve_genesis
        ));
    }
    Ok(finding)
}

/// Sums every reserve decrease over the window. A rise is a FAIL, recorded on
/// the finding. Returns `None` when a height is unavailable, because a partial
/// sum would reconcile to nothing and must not be compared.
fn reserve_paid(observed: &Observed, finding: &mut Finding) -> Option<Amount> {
    let mut paid = Amount::ZERO;
    let mut complete = true;
    for facts in &observed.heights {
        match &facts.changes {
            Fetch::Unavailable(why) => {
                complete = false;
                finding.verdict = finding.verdict.worst(Verdict::Unavailable);
                finding.note(format!(
                    "height {}: balance changes not kept ({why})",
                    facts.height
                ));
            }
            Fetch::Got(changes) => {
                for c in changes.iter().filter(|c| c.address == MINING_RESERVE) {
                    if c.after > c.before {
                        finding.verdict = Verdict::Fail;
                        finding.note(format!(
                            "height {}: reserve rose {} -> {} (emission never adds to the reserve)",
                            facts.height, c.before, c.after
                        ));
                    } else if let Some(drop) = c.before.checked_sub(c.after) {
                        paid = credit(paid, drop)?;
                    }
                }
            }
        }
    }
    complete.then_some(paid)
}
