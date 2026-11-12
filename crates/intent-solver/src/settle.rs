//! Settlement over the hash-lock HTLC the chain already has.
//!
//! The maker holds the preimage, so the maker locks first and with the longer
//! timeout; the solver locks second with a timeout one margin shorter. The
//! maker's claim on the solver's lock publishes the preimage, and the solver
//! then has at least one margin of blocks to claim the maker's lock with it.
//! If anyone stops, each lock refunds to its sender at its own expiry.
//!
//! The claim/refund rule is `maya_htlc_lattice::decide_claim`/`decide_refund`,
//! the same functions the node's `htlc_exec` applies, so the tracker here
//! cannot disagree with the chain about which of the two a height admits.
//!
//! **Limit:** the on-chain `HtlcLock` escrows the native coin only. A leg in any
//! other asset is an HTLC on that asset's chain under the same SHA-256 digest,
//! as `bins/htlc-watcher` runs, and its expiry here is in `Maya2C` heights:
//! translating it to the other chain's heights is the watcher's pairing policy.

use maya_htlc_lattice::{
    ClaimOutcome, HashFunction, LockStatus, Preimage, RefundOutcome, decide_claim, decide_refund,
};
use primitive_types::U256;

use crate::book::Quote;
use crate::error::{IntentError, Result};
use crate::intent::{AssetId, Intent, IntentId};

/// One lock of the swap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leg {
    /// What is escrowed.
    pub asset: AssetId,
    /// Who a claim pays.
    pub recipient: Vec<u8>,
    /// How much is escrowed.
    pub amount: U256,
    /// First height at which a claim is refused and a refund admitted.
    pub expiry_height: u64,
    /// Whether this is a `Maya2C` `HtlcLock` (native coin) rather than a lock on
    /// another chain.
    pub on_maya2c: bool,
}

/// Who acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Party {
    /// The intent's signer, holder of the preimage.
    Maker,
    /// The winning solver.
    Solver,
}

/// What a step does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Open the party's own lock.
    Lock,
    /// Claim the other party's lock with the preimage.
    Claim,
    /// Take back the party's own lock after its expiry.
    Refund,
}

/// One step, with the heights it is valid between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// Who.
    pub party: Party,
    /// What.
    pub action: Action,
    /// Whose lock it acts on.
    pub lock_of: Party,
    /// Earliest height.
    pub from_height: u64,
    /// First height at which the step is too late; `None` for a refund.
    pub before_height: Option<u64>,
}

/// The full plan for one intent and its chosen quote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettlementPlan {
    /// Which intent.
    pub intent_id: IntentId,
    /// The chosen solver.
    pub solver: [u8; 32],
    /// The hash both locks are under.
    pub function: HashFunction,
    /// The digest both locks are under. The maker keeps the preimage.
    pub digest: [u8; 32],
    /// The maker pays the sell asset to the solver.
    pub maker_leg: Leg,
    /// The solver pays the buy asset to the maker.
    pub solver_leg: Leg,
    /// Both locks must be open before this height.
    pub lock_by: u64,
    /// The height the plan was made at.
    pub planned_at: u64,
}

/// Plans the swap for `intent` against `quote` at `height`.
///
/// `margin` is the number of blocks each party is guaranteed to act in: the
/// solver's window to claim after the maker reveals, and the window for both
/// locks to land.
///
/// # Errors
///
/// [`IntentError::UnknownIntent`] for a quote on another intent,
/// [`IntentError::Overflow`], [`IntentError::WorseThanLimit`],
/// [`IntentError::HashNotPortable`], [`IntentError::Expired`], or
/// [`IntentError::NoRoom`] when the deadline cannot fit the margins.
pub fn plan_settlement(
    intent: &Intent,
    quote: &Quote,
    function: HashFunction,
    digest: [u8; 32],
    height: u64,
    margin: u64,
) -> Result<SettlementPlan> {
    if quote.intent_id != intent.id() {
        return Err(IntentError::UnknownIntent);
    }
    let total = quote.total()?;
    if total > intent.max_sell {
        return Err(IntentError::WorseThanLimit);
    }
    let all_native = intent.sell_asset.is_native() && intent.buy_asset.is_native();
    if !all_native && function != HashFunction::Sha256 {
        return Err(IntentError::HashNotPortable);
    }
    let maker_expiry = intent.deadline_height;
    if height >= maker_expiry {
        return Err(IntentError::Expired);
    }
    let (solver_expiry, lock_by) = timelocks(height, maker_expiry, margin)?;
    Ok(SettlementPlan {
        intent_id: quote.intent_id,
        solver: quote.solver,
        function,
        digest,
        maker_leg: Leg {
            asset: intent.sell_asset,
            recipient: quote.pay_to.clone(),
            amount: total,
            expiry_height: maker_expiry,
            on_maya2c: intent.sell_asset.is_native(),
        },
        solver_leg: Leg {
            asset: intent.buy_asset,
            recipient: intent.buy_recipient.clone(),
            amount: intent.buy_amount,
            expiry_height: solver_expiry,
            on_maya2c: intent.buy_asset.is_native(),
        },
        lock_by,
        planned_at: height,
    })
}

/// `(solver_expiry, lock_by)`: each one margin before the next, and the lock
/// window non-empty at `height`.
fn timelocks(height: u64, maker_expiry: u64, margin: u64) -> Result<(u64, u64)> {
    if margin == 0 {
        return Err(IntentError::NoRoom);
    }
    let solver_expiry = maker_expiry
        .checked_sub(margin)
        .ok_or(IntentError::NoRoom)?;
    let earliest_solver_expiry = height.checked_add(margin).ok_or(IntentError::Overflow)?;
    if earliest_solver_expiry >= solver_expiry {
        return Err(IntentError::NoRoom);
    }
    // Cannot underflow: solver_expiry > height + margin.
    Ok((solver_expiry, solver_expiry - margin))
}

impl SettlementPlan {
    /// The happy path in order, then the two refunds.
    #[must_use]
    pub fn steps(&self) -> [Step; 6] {
        let start = self.planned_at;
        let solver_expiry = self.solver_leg.expiry_height;
        let maker_expiry = self.maker_leg.expiry_height;
        let step = |party, action, lock_of, from_height, before_height| Step {
            party,
            action,
            lock_of,
            from_height,
            before_height,
        };
        [
            step(
                Party::Maker,
                Action::Lock,
                Party::Maker,
                start,
                Some(self.lock_by),
            ),
            step(
                Party::Solver,
                Action::Lock,
                Party::Solver,
                start,
                Some(self.lock_by),
            ),
            step(
                Party::Maker,
                Action::Claim,
                Party::Solver,
                start,
                Some(solver_expiry),
            ),
            step(
                Party::Solver,
                Action::Claim,
                Party::Maker,
                start,
                Some(maker_expiry),
            ),
            step(
                Party::Solver,
                Action::Refund,
                Party::Solver,
                solver_expiry,
                None,
            ),
            step(
                Party::Maker,
                Action::Refund,
                Party::Maker,
                maker_expiry,
                None,
            ),
        ]
    }

    fn opens(&self, preimage: &Preimage) -> bool {
        self.function.digest(preimage) == self.digest
    }
}

/// One lock as observed, advanced by the chain's own rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegState {
    /// The lock.
    pub leg: Leg,
    /// Where it stands.
    pub status: LockStatus,
}

impl LegState {
    /// A lock just opened.
    #[must_use]
    pub const fn locked(leg: Leg) -> Self {
        Self {
            leg,
            status: LockStatus::Locked,
        }
    }

    /// A claim at `height` with `preimage`.
    pub fn claim(
        &mut self,
        plan: &SettlementPlan,
        preimage: &Preimage,
        height: u64,
    ) -> ClaimOutcome {
        let outcome = decide_claim(self.status, self.leg.expiry_height, height, || {
            plan.opens(preimage)
        });
        if outcome.settled() {
            self.status = LockStatus::Claimed;
        }
        outcome
    }

    /// A refund at `height`.
    pub fn refund(&mut self, height: u64) -> RefundOutcome {
        let outcome = decide_refund(self.status, self.leg.expiry_height, height);
        if outcome.settled() {
            self.status = LockStatus::Refunded;
        }
        outcome
    }
}
