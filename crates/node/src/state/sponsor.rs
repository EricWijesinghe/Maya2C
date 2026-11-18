//! Sponsored fees (ADR-135 P-4, feature bit 44, wire v13): the consensus rule
//! on who pays and how much. The frame and the sponsor's signature are in
//! [`crate::core::sponsor_tx`].
//!
//! No state prefix and no invariant-guard module: a sponsorship moves the
//! fee from the sponsor's account to the collector, both ordinary accounts,
//! so the undo journal and the accounts root already cover it — a reorg puts
//! a sponsor's balance back exactly as it puts a sender's back.
//!
//! # Why a sponsor can never pay more than it signed for
//!
//! One sponsorship debits `required(outer_len)`, refused above its signed
//! `max_fee`, and is bound to one inner txid, which covers the inner's nonce:
//! the first application consumes that nonce, so the same sponsorship can
//! never apply twice, in one block or across blocks. The debit is one
//! `checked_sub` on the sponsor's balance; a frame it cannot cover is refused
//! whole. So across any set of blocks a sponsor pays at most the sum of the
//! `max_fee`s it signed for transactions that actually applied, and never
//! more than it holds.
//!
//! Mempool limits (64 pending, the 86,400-block window, pending `Σ max_fee`)
//! are node policy in [`crate::network::mempool`], never validity rules here
//! (security-architect S-5).

use primitive_types::U256;

use crate::core::Transaction;
use crate::core::sponsor_tx::{MAX_SPONSOR_WINDOW, Sponsorship};
use crate::error::{NodeError, Result};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::fees::{FEE_COLLECTOR, FeeRecord};

/// Refuses a v13 frame below feature bit 44's activation height. The mempool
/// and the executor both call this, so they agree on every frame (inv 31).
///
/// # Errors
///
/// [`NodeError::SponsoredFeesInactive`] for a sponsored frame before
/// activation.
pub fn require_active(tx: &Transaction, context: BlockContext) -> Result<()> {
    if tx.sponsor.is_some() && !context.sponsored_fees_active() {
        return Err(NodeError::SponsoredFeesInactive {
            height: context.height,
            activation: context.sponsored_fees_activation,
        });
    }
    Ok(())
}

const fn refuse(reason: &'static str) -> NodeError {
    NodeError::SponsorshipRefused { reason }
}

/// What the sponsor of `tx` pays at `height` under `fees`, after the
/// consensus rules (ADR-135 P-4): the inner pays the collector nothing
/// itself, the sponsorship has not expired, and the fee on the whole outer
/// frame is within `max_fee`. Zero where the chain runs no fee market.
///
/// # Errors
///
/// [`NodeError::SponsorshipRefused`] naming the rule that failed.
pub(crate) fn sponsored_fee(
    tx: &Transaction,
    sponsorship: &Sponsorship,
    fees: Option<&FeeRecord>,
    height: u64,
) -> Result<U256> {
    // An inner that pays its own fee would let a relay wrap a stranger's
    // transaction and make the sponsor pay for it twice over (P-5).
    if tx.outputs.iter().any(|o| o.recipient == FEE_COLLECTOR) {
        return Err(refuse("the inner frame pays the fee collector itself"));
    }
    if height > sponsorship.valid_until {
        return Err(refuse(
            "the sponsorship expired: height is past valid_until",
        ));
    }
    // `required` saturates, so a size times a fee past 2^256 is a requirement
    // no max_fee meets, never a wrapped small one.
    let required = fees.map_or(U256::zero(), |f| {
        maya_stateless_core::amount::to_u256(f.required(tx.to_bytes().len()))
    });
    if required > sponsorship.max_fee {
        return Err(refuse("the fee on the outer frame exceeds max_fee"));
    }
    Ok(required)
}

/// Admission only (node default, Eric O-4): a sponsorship may run at most
/// [`MAX_SPONSOR_WINDOW`] blocks past the height it is admitted at.
///
/// # Errors
///
/// [`NodeError::SponsorshipRefused`] for a longer window.
pub(crate) fn check_window(sponsorship: &Sponsorship, height: u64) -> Result<()> {
    if sponsorship.valid_until.saturating_sub(height) > MAX_SPONSOR_WINDOW {
        return Err(refuse(
            "valid_until is more than MAX_SPONSOR_WINDOW blocks past admission",
        ));
    }
    Ok(())
}

impl StateDB {
    /// Charges a v13 frame's fee to its sponsor: the rules above, the
    /// sponsor's vault window (invariant 32), one `checked_sub` on the
    /// sponsor's balance, credited to the collector, and the base fee booked
    /// for the burn exactly as an unsponsored fee is. Runs before the inner
    /// executes; any failure refuses the whole frame, so no nonce is consumed.
    ///
    /// # Errors
    ///
    /// [`NodeError::SponsorshipRefused`], [`NodeError::Vault`],
    /// [`NodeError::InsufficientBalance`] when the sponsor cannot cover the
    /// fee, or [`NodeError::BalanceOverflow`].
    pub(crate) fn charge_sponsor(
        &self,
        overlay: &mut Overlay,
        tx: &Transaction,
        sponsorship: &Sponsorship,
        height: u64,
    ) -> Result<()> {
        let fees = self.fee_record(overlay)?;
        let required = sponsored_fee(tx, sponsorship, fees.as_ref(), height)?;
        if fees.is_none() {
            // No fee market: the rules above still hold, and nothing moves.
            return Ok(());
        }
        if !required.is_zero() {
            self.debit_sponsor(overlay, sponsorship, required, height)?;
        }
        // The same booking `charge_fee` makes: the base fee burns at the end
        // of the block and the tip (none here: the sponsor pays exactly the
        // base fee) stays with the collector (P-11).
        overlay.fee_burn = overlay
            .fee_burn
            .checked_add(required)
            .ok_or(NodeError::BalanceOverflow)?;
        overlay.fee_bytes = overlay
            .fee_bytes
            .saturating_add(u64::try_from(tx.to_bytes().len()).unwrap_or(u64::MAX));
        Ok(())
    }

    /// Moves `required` from the sponsor to the collector. Never called with
    /// zero: staging an untouched sponsor account would create it and change
    /// the root.
    fn debit_sponsor(
        &self,
        overlay: &mut Overlay,
        sponsorship: &Sponsorship,
        required: U256,
        height: u64,
    ) -> Result<()> {
        let payer = sponsorship.address();
        self.charge_vault_window(overlay, &payer, required, height)?;
        let mut account = self.load(overlay, &payer)?;
        let available = account.balance;
        account.balance =
            available
                .checked_sub(required)
                .ok_or_else(|| NodeError::InsufficientBalance {
                    address: hex::encode(payer),
                    required,
                    available,
                })?;
        overlay.accounts.insert(payer, account);
        let mut collector = self.load(overlay, &FEE_COLLECTOR)?;
        collector.balance = collector
            .balance
            .checked_add(required)
            .ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(FEE_COLLECTOR, collector);
        Ok(())
    }
}
