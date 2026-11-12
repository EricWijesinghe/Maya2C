//! Check 2: the mining reserve fell by the scheduled emission, epoch by epoch.
//!
//! Epoch `e` covers heights `e·E+1 ..= (e+1)·E` (`E` = staking epoch length).
//! Shares are applied in the transactions of those blocks; the committee's part
//! is paid at the boundary `(e+1)·E` (`settle_mining`, `crates/node/src/state/mining_exec.rs`).
//! So over one epoch the reserve must fall by exactly `committee + paid shares`,
//! and the shares may not exceed the miners' budget (MINE-2, MINE-4).
//!
//! What the RPC cannot show: the split between committee and shares inside one
//! epoch. Per-share amounts are not in any block output. The check therefore
//! proves the epoch total lies in `[committee, committee + budget]`, and reports
//! the unspent budget (spec/09 open question: it stays in the reserve).

use maya_amount::Amount;
use maya_spec_ref::mining::{epoch_reward, split};

use crate::genesis::{MiningParams, Plan};
use crate::numbers::{MINING_RESERVE, from_spec, to_spec};
use crate::observe::{Fetch, Observed};
use crate::report::{Finding, Verdict};
use crate::rpc::{Rpc, RpcError};

/// One epoch's heights, inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    epoch: u64,
    lo: u64,
    hi: u64,
}

/// Running totals over the epochs that passed.
#[derive(Default)]
struct Totals {
    scheduled: Amount,
    unspent: Amount,
    paid: Amount,
    passed: Vec<Span>,
    partial: u64,
}

/// Check 2 over every complete epoch inside the window.
///
/// # Errors
///
/// Transport and malformed answers.
pub fn check(rpc: &dyn Rpc, plan: &Plan, observed: &Observed) -> Result<Finding, RpcError> {
    let mut finding = Finding::new(
        "reserve-emission",
        "reserve fell by the scheduled emission, per epoch",
        Verdict::Pass,
    );
    let (Some(params), Some(epoch_blocks)) = (plan.mining, plan.epoch_blocks) else {
        finding.verdict = Verdict::Skipped;
        finding.note("no mining in genesis: no emission schedule");
        return Ok(finding);
    };
    let mut totals = Totals::default();
    for span in epochs_in(observed, epoch_blocks) {
        if span.lo < observed.from {
            totals.partial += 1;
            continue;
        }
        let verdict = judge(rpc, &params, observed, span, &mut totals, &mut finding)?;
        finding.verdict = finding.verdict.worst(verdict);
    }
    summarise(&totals, &mut finding);
    Ok(finding)
}

/// Every epoch whose last height is inside the window, in order.
fn epochs_in(observed: &Observed, epoch_blocks: u64) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut epoch = 0u64;
    while let Some(span) = span_of(epoch, epoch_blocks) {
        if span.hi > observed.to() {
            break;
        }
        spans.push(span);
        epoch += 1;
    }
    spans
}

/// Heights of epoch `e`. `None` when they overflow `u64`.
fn span_of(epoch: u64, epoch_blocks: u64) -> Option<Span> {
    let lo = epoch.checked_mul(epoch_blocks)?.checked_add(1)?;
    let hi = epoch.checked_add(1)?.checked_mul(epoch_blocks)?;
    Some(Span { epoch, lo, hi })
}

/// Judges one epoch. Returns the verdict it adds to the finding; evidence goes
/// on the finding itself.
fn judge(
    rpc: &dyn Rpc,
    params: &MiningParams,
    observed: &Observed,
    span: Span,
    totals: &mut Totals,
    finding: &mut Finding,
) -> Result<Verdict, RpcError> {
    let reward = from_spec(epoch_reward(
        to_spec(params.initial_epoch_reward),
        to_spec(params.tail_epoch_reward),
        params.halving_epochs,
        span.epoch,
    ));
    let (miners, committee) = split_reward(reward, params.miner_share_bps);
    let Some((drop, end)) = reserve_span(rpc, observed, span, finding)? else {
        return Ok(Verdict::Unavailable);
    };
    let at = format!("epoch {} (heights {}..={})", span.epoch, span.lo, span.hi);
    if end == Amount::ZERO {
        finding.note(format!(
            "{at}: reserve empty at the end; emission ended, not verified"
        ));
        return Ok(Verdict::Unavailable);
    }
    let Some(shares) = drop.checked_sub(committee) else {
        finding.note(format!(
            "{at}: reserve fell {drop}, below the committee part {committee}"
        ));
        return Ok(Verdict::Fail);
    };
    if shares > miners {
        finding.note(format!(
            "{at}: miners paid {shares}, over the budget {miners}"
        ));
        return Ok(Verdict::Fail);
    }
    let unspent = miners.checked_sub(shares).unwrap_or(Amount::ZERO);
    totals.scheduled = totals.scheduled.saturating_add(reward);
    totals.unspent = totals.unspent.saturating_add(unspent);
    totals.paid = totals.paid.saturating_add(drop);
    totals.passed.push(span);
    Ok(Verdict::Pass)
}

/// The committee's and miners' parts of `reward` (MINE-2, through the spec).
fn split_reward(reward: Amount, miner_bps: u64) -> (Amount, Amount) {
    let (miners, committee) = split(to_spec(reward), miner_bps);
    (from_spec(miners), from_spec(committee))
}

/// Sums the reserve's decrease over one epoch and reads its end balance. `None`
/// when the data is missing, with NOT VERIFIED already on the finding.
fn reserve_span(
    rpc: &dyn Rpc,
    observed: &Observed,
    span: Span,
    finding: &mut Finding,
) -> Result<Option<(Amount, Amount)>, RpcError> {
    let mut drop = Amount::ZERO;
    let mut end = None;
    for height in span.lo..=span.hi {
        let Some(facts) = observed.get(height) else {
            return Ok(None);
        };
        let changes = match &facts.changes {
            Fetch::Got(changes) => changes,
            Fetch::Unavailable(why) => {
                finding.note(format!(
                    "epoch {}: height {height} balance changes not kept ({why})",
                    span.epoch
                ));
                return Ok(None);
            }
        };
        for c in changes.iter().filter(|c| c.address == MINING_RESERVE) {
            end = Some(c.after);
            match c.before.checked_sub(c.after) {
                Some(d) => drop = drop.saturating_add(d),
                None => {
                    finding.verdict = Verdict::Fail;
                    finding.note(format!(
                        "height {height}: reserve rose {} -> {} (emission never adds to the reserve)",
                        c.before, c.after
                    ));
                }
            }
        }
    }
    let end = match end {
        Some(value) => value,
        None => match rpc.balance_at(&MINING_RESERVE, span.hi) {
            Ok(value) => value,
            Err(RpcError::Rejected(why)) => {
                finding.note(format!(
                    "epoch {}: reserve balance at {} not readable ({why})",
                    span.epoch, span.hi
                ));
                return Ok(None);
            }
            Err(other) => return Err(other),
        },
    };
    Ok(Some((drop, end)))
}

fn summarise(totals: &Totals, finding: &mut Finding) {
    let (Some(first), Some(last)) = (totals.passed.first(), totals.passed.last()) else {
        finding.verdict = finding.verdict.worst(Verdict::Unavailable);
        finding.note("no complete epoch inside the window that could be judged; widen --from/--to");
        return;
    };
    finding.note(format!(
        "epochs {}..={} judged: heights {}..={}, {} epoch(s)",
        first.epoch,
        last.epoch,
        first.lo,
        last.hi,
        totals.passed.len()
    ));
    finding.note(format!(
        "scheduled emission {} = paid {} + unspent miner budget {} (stays in reserve; spec/09 open question)",
        totals.scheduled, totals.paid, totals.unspent
    ));
    let reconciles = totals.unspent.checked_add(totals.paid) == Some(totals.scheduled);
    if !reconciles {
        finding.verdict = Verdict::Fail;
        finding.note("internal: paid + unspent does not equal the scheduled emission");
    }
    if totals.partial > 0 {
        finding.note(format!(
            "{} epoch(s) only partly inside the window, not judged",
            totals.partial
        ));
    }
}
