//! Executing the RWA transitions: issuance, DvP, gated transfers, revenue.
//!
//! ## A DvP that cannot settle is a no-op, never an error
//!
//! Invariant 7, and it is not an analogy — it is the same rule:
//!
//! > A trade that merely *loses* is a no-op, never an `Err`. A failing
//! > transaction fails its whole block here, so making any of these an error
//! > hands every trader a way to void a block.
//!
//! Delivery-versus-payment means both legs or neither. The obvious
//! implementation returns an error when a leg cannot settle — and on this chain
//! that voids the block, so anyone could kill any block by submitting a DvP
//! they know will fail. The counterparty does not even have to be involved.
//!
//! So `settle_dvp` — `pub(crate)`, so named here rather than linked —
//! applies both legs to the overlay or applies
//! nothing. The nonce advances, the fee is spent, and the state is exactly what
//! it was. Atomicity comes from the overlay mutation being all-or-nothing, not
//! from an error unwinding it.
//!
//! ## Eligibility is read, not proved, on the transfer path
//!
//! A Groth16 pairing check is one to two milliseconds. On a block with ten
//! thousand transfers that is ten to twenty seconds of validation every node
//! pays, forever, for a check whose answer changes rarely. So the proof is
//! verified once in its own transaction and cached in `r:elg:`; the transfer
//! reads a record and a height.
//!
//! The cache expires, in **block height** rather than time (invariant 9): a
//! miner may write any `header.timestamp`, so a freshness rule measured in
//! seconds would read as safety and provide none.
//!
//! ## Distribution places exactly what it debits
//!
//! `ledger_math::distribute` is Kani-checked to place exactly the total. That
//! is not a fairness nicety — the invariant guard refuses any block whose value
//! deltas do not balance, so a rounding rule that lost a base unit would be a
//! distribution nobody can mine.

use maya_ledger_math::distribute;
use maya_rwa::cap_table::HOLDERS_PER_PAGE;
use maya_rwa::token::{Eligibility, LegalAttestation, RevenueDistribution, RwaToken, TransferRule};
use maya_rwa::{Address, Digest};

use crate::error::{NodeError, Result};
use crate::state::account::Address as ChainAddress;
use crate::state::db::{Overlay, StateDB};
use crate::state::rwa::{
    cap_table_key, derive_asset, distribution_key, eligibility_key, legal_key, token_key,
};

/// The most cap table pages one distribution may fan out over.
///
/// Forty pages of 256 is 10,240 holders — the scale the brief names, with a
/// little room. A bound rather than none, because the alternative is a single
/// transaction whose cost is whatever an issuer's cap table happens to be, and
/// a block whose validation time nobody can predict.
pub const MAX_DISTRIBUTION_PAGES: u32 = 40;

/// The most holders one distribution may pay.
pub const MAX_DISTRIBUTION_HOLDERS: usize = MAX_DISTRIBUTION_PAGES as usize * HOLDERS_PER_PAGE;

/// Why a DvP did not settle.
///
/// Returned rather than raised. Every variant is a reason the swap is a no-op,
/// and none of them is a reason to fail the block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DvpOutcome {
    /// Both legs moved.
    Settled,
    /// The buyer does not have the native balance.
    BuyerShort,
    /// The seller does not hold the units.
    SellerShort,
    /// One side is not eligible to hold this asset under its rule.
    NotEligible,
    /// The asset does not exist.
    UnknownAsset,
    /// The cap table page could not take another holder.
    PageFull,
}

impl DvpOutcome {
    /// Whether value moved.
    #[must_use]
    pub const fn settled(self) -> bool {
        matches!(self, Self::Settled)
    }
}

impl StateDB {
    /// Issues a token and seats its whole supply with the issuer.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the asset already exists or the token
    /// is outside its bounds. Issuance is one of the few places an error is
    /// right: it is the issuer's own transaction and nobody else's block is at
    /// stake in the way a DvP's counterparty's is.
    pub(crate) fn issue_rwa(
        &self,
        overlay: &mut Overlay,
        issuer: &ChainAddress,
        label: &str,
        total_units: u64,
        rule: Option<TransferRule>,
    ) -> Result<Digest> {
        let asset = derive_asset(issuer, label);
        if self.rwa_token(overlay, &asset)?.is_some() {
            return Err(NodeError::Decode(format!(
                "rwa: {label:?} is already issued by this address"
            )));
        }
        let token = RwaToken::new(asset, *issuer, label, total_units, rule).map_err(shape)?;
        StateDB::put_record(overlay, token_key(&asset), token.encode());

        // The whole supply starts with the issuer. A token whose units exist
        // but are held by nobody would make every distribution's weights sum to
        // less than the supply, and the difference would have no owner.
        let page = maya_rwa::cap_table::CapTablePage::new(
            asset,
            0,
            vec![maya_rwa::cap_table::Holder {
                address: *issuer,
                units: total_units,
            }],
        )
        .map_err(shape)?;
        StateDB::put_record(overlay, cap_table_key(&asset, 0), page.encode());
        Ok(asset)
    }

    /// Whether a holder may hold this asset at `height`.
    ///
    /// A token with no rule admits everyone. A token with one admits a holder
    /// whose cached eligibility is live — never a proof verified here, for the
    /// reason in the module docs.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub(crate) fn rwa_eligible(
        &self,
        overlay: &Overlay,
        token: &RwaToken,
        holder: &Address,
        height: u64,
    ) -> Result<bool> {
        if token.rule.is_none() {
            return Ok(true);
        }
        Ok(self
            .eligibility(overlay, &token.asset, holder)?
            .is_some_and(|eligibility| eligibility.is_live(height)))
    }

    /// Records that a holder cleared an asset's rule.
    ///
    /// The caller has already verified the disclosure proof against the roots
    /// the chain holds; this writes down that it happened and when it stops
    /// counting.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for an unknown asset or one with no rule —
    /// an eligibility record for an ungated token is a record nothing reads.
    pub(crate) fn record_eligibility(
        &self,
        overlay: &mut Overlay,
        asset: &Digest,
        holder: &Address,
        height: u64,
    ) -> Result<()> {
        let token = self
            .rwa_token(overlay, asset)?
            .ok_or_else(|| NodeError::Decode("rwa: unknown asset".to_owned()))?;
        let rule = token.rule.as_ref().ok_or_else(|| {
            NodeError::Decode("rwa: this asset has no rule to be eligible under".to_owned())
        })?;

        let eligibility = Eligibility {
            holder: *holder,
            asset: *asset,
            verified_at: height,
            // Saturating: a chain that reached the top of u64 has stopped being
            // a chain, and a wrapped expiry would silently make the record dead
            // rather than long-lived.
            expires_at: height.saturating_add(rule.eligibility_blocks),
        };
        StateDB::put_record(
            overlay,
            eligibility_key(asset, holder),
            eligibility.encode(),
        );
        Ok(())
    }

    /// Swaps native coin for RWA units, atomically or not at all.
    ///
    /// Returns why it did not settle rather than raising. See the module docs:
    /// a DvP that errored would hand anyone a way to void a block.
    ///
    /// # Errors
    ///
    /// Only a read or decode failure — a storage problem, not a settlement one.
    // Eight parameters, and grouping them into a struct would move the same
    // values behind a name that adds nothing: every one is a distinct part of
    // one swap, and a `DvpRequest` would be constructed at the only call site
    // and destructured here.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn settle_dvp(
        &self,
        overlay: &mut Overlay,
        buyer: &ChainAddress,
        seller: &ChainAddress,
        asset: &Digest,
        units: u64,
        price: u64,
        page_index: u32,
        height: u64,
    ) -> Result<DvpOutcome> {
        let Some(token) = self.rwa_token(overlay, asset)? else {
            return Ok(DvpOutcome::UnknownAsset);
        };
        if !self.rwa_eligible(overlay, &token, buyer, height)? {
            return Ok(DvpOutcome::NotEligible);
        }

        // Read both sides before writing either. The whole point is that a
        // half-applied swap is not reachable, and the cheapest way to get that
        // is to have nothing written when the second check fails.
        let mut page = self.cap_table_page(overlay, asset, page_index)?;
        if page.units_of(seller) < units || units == 0 {
            return Ok(DvpOutcome::SellerShort);
        }

        let buyer_account = self.load(overlay, buyer)?;
        let Some(buyer_after) = buyer_account.balance.checked_sub(price) else {
            return Ok(DvpOutcome::BuyerShort);
        };
        let seller_account = self.load(overlay, seller)?;
        let Some(seller_after) = seller_account.balance.checked_add(price) else {
            // The seller's balance would overflow. Not a loss and not an error:
            // the swap simply does not happen.
            return Ok(DvpOutcome::BuyerShort);
        };

        if page.transfer(seller, buyer, units).is_err() {
            return Ok(DvpOutcome::PageFull);
        }

        // Both legs, together. Nothing above this line wrote anything.
        let mut buyer_updated = buyer_account;
        buyer_updated.balance = buyer_after;
        let mut seller_updated = seller_account;
        seller_updated.balance = seller_after;
        overlay.accounts.insert(*buyer, buyer_updated);
        overlay.accounts.insert(*seller, seller_updated);
        StateDB::put_record(overlay, cap_table_key(asset, page_index), page.encode());
        Ok(DvpOutcome::Settled)
    }

    /// Pays `total` across every holder on the named pages, in proportion.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for an unknown asset, a round already
    /// settled, more pages than [`MAX_DISTRIBUTION_PAGES`], or a cap table with
    /// no holders — and [`NodeError::InsufficientBalance`] if the issuer cannot
    /// cover the total.
    ///
    /// An error here is right where it is wrong for a DvP: this is the issuer's
    /// own transaction, and there is no counterparty whose block it could void.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn distribute_revenue(
        &self,
        overlay: &mut Overlay,
        issuer: &ChainAddress,
        asset: &Digest,
        round: u64,
        total: u64,
        pages: u32,
        height: u64,
    ) -> Result<u32> {
        if pages > MAX_DISTRIBUTION_PAGES {
            return Err(NodeError::Decode(format!(
                "rwa: {pages} pages exceeds the {MAX_DISTRIBUTION_PAGES} a distribution may span"
            )));
        }
        let token = self
            .rwa_token(overlay, asset)?
            .ok_or_else(|| NodeError::Decode("rwa: unknown asset".to_owned()))?;
        if token.issuer != *issuer {
            return Err(NodeError::Decode(
                "rwa: only the issuer distributes an asset's revenue".to_owned(),
            ));
        }
        if self
            .record(overlay, &distribution_key(asset, round))?
            .is_some()
        {
            return Err(NodeError::Decode(format!(
                "rwa: round {round} has already settled"
            )));
        }

        // Gather every holder across the named pages. Allocated once, up front,
        // rather than per page: this is the hot loop the brief is about.
        let mut holders: Vec<(ChainAddress, u64)> = Vec::with_capacity(MAX_DISTRIBUTION_HOLDERS);
        for page_index in 0..pages {
            let page = self.cap_table_page(overlay, asset, page_index)?;
            for holder in page.holders() {
                holders.push((holder.address, holder.units));
            }
        }
        if holders.is_empty() {
            return Err(NodeError::Decode(
                "rwa: a distribution to no holders would make the value vanish".to_owned(),
            ));
        }

        let weights: Vec<u64> = holders.iter().map(|(_, units)| *units).collect();
        let mut payouts = vec![0u64; holders.len()];
        let mut order = vec![0u32; holders.len()];
        distribute(total, &weights, &mut payouts, &mut order).ok_or_else(|| {
            NodeError::Decode("rwa: the holdings cannot be distributed against".to_owned())
        })?;

        // Debit the issuer first, so an issuer who cannot cover the total does
        // not leave a partially credited cap table behind.
        let mut issuer_account = self.load(overlay, issuer)?;
        issuer_account.balance = issuer_account.balance.checked_sub(total).ok_or_else(|| {
            NodeError::InsufficientBalance {
                address: hex::encode(issuer),
                required: total,
                available: issuer_account.balance,
            }
        })?;
        overlay.accounts.insert(*issuer, issuer_account);

        for ((address, _), payout) in holders.iter().zip(&payouts) {
            if *payout == 0 {
                continue;
            }
            let mut account = self.load(overlay, address)?;
            account.balance = account
                .balance
                .checked_add(*payout)
                .ok_or(NodeError::BalanceOverflow)?;
            overlay.accounts.insert(*address, account);
        }

        let settled = RevenueDistribution {
            asset: *asset,
            round,
            total,
            holders: holders.len() as u32,
            settled_at: height,
        };
        StateDB::put_record(overlay, distribution_key(asset, round), settled.encode());
        Ok(settled.holders)
    }

    /// Records a legal attestation about an asset.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for an unknown asset or a reference
    /// outside its bounds.
    pub(crate) fn attest_legal(
        &self,
        overlay: &mut Overlay,
        attestor: &ChainAddress,
        asset: &Digest,
        document: &Digest,
        reference: &str,
        height: u64,
    ) -> Result<()> {
        if self.rwa_token(overlay, asset)?.is_none() {
            return Err(NodeError::Decode("rwa: unknown asset".to_owned()));
        }
        let attestation = LegalAttestation::new(*asset, *attestor, *document, reference, height)
            .map_err(shape)?;
        StateDB::put_record(overlay, legal_key(asset, document), attestation.encode());
        Ok(())
    }
}

/// Turns a shape failure from the RWA crate into a node error.
fn shape(error: maya_rwa::Error) -> NodeError {
    NodeError::Decode(format!("rwa: {error}"))
}
