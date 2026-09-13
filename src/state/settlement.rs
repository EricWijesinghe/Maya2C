//! On-chain settlement of Maya Flash channels.
//!
//! Implements the six channel operations against the block-execution overlay,
//! so channel writes are as atomic as account writes: a block that fails at any
//! transaction leaves neither balances nor channel records touched.
//!
//! ## The rule every operation shares
//!
//! A channel may never pay out more or less than it escrowed. Every closure is
//! checked against the recorded capacity before a single unit moves. Without
//! that check a channel would be a mint: two parties could sign any balances
//! they liked and the chain would honour them.
//!
//! ## Why a unilateral close does not pay out
//!
//! [`TxKind::DisputeClose`] only *records* the claimed state and starts a
//! timer. Paying out immediately would make fraud free — publish a stale state
//! showing yourself richer and walk away before anyone can object. The window
//! exists so the counterparty can present the revocation secret and take
//! everything instead.

use maya_ledger_math as ledger_math;

use crate::core::payload::{
    ChannelClosure, ChannelId, ChannelOpen, RevocationProof, derive_channel_id,
    revocation_commitment,
};
use crate::core::{Transaction, TxKind};
use crate::crypto::hybrid::{HybridVerifyingKey, address_of};
use crate::error::{NodeError, Result};
use crate::state::account::{Account, Address};
use crate::state::channel::{ChannelRecord, ChannelStatus};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB, channel_key};
use crate::state::invariant_guard::Module;

impl StateDB {
    /// Reads a channel record from committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn get_channel(&self, channel_id: &ChannelId) -> Result<Option<ChannelRecord>> {
        match self.raw_get(&channel_key(channel_id))? {
            Some(bytes) => Ok(Some(ChannelRecord::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Reads a channel through the overlay, falling back to committed state.
    fn load_channel(
        &self,
        overlay: &Overlay,
        channel_id: &ChannelId,
    ) -> Result<Option<ChannelRecord>> {
        match overlay.channels.get(channel_id) {
            Some(record) => Ok(Some(record.clone())),
            None => self.get_channel(channel_id),
        }
    }

    /// Requires an existing channel.
    fn require_channel(&self, overlay: &Overlay, channel_id: &ChannelId) -> Result<ChannelRecord> {
        self.load_channel(overlay, channel_id)?
            .ok_or_else(|| NodeError::UnknownChannel(hex::encode(channel_id)))
    }

    /// Credits an account through the overlay.
    fn credit(&self, overlay: &mut Overlay, address: &Address, amount: u64) -> Result<()> {
        let mut account = match overlay.accounts.get(address) {
            Some(existing) => *existing,
            None => self.get_account(address)?,
        };
        account.balance =
            ledger_math::credit(account.balance, amount).ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(*address, account);
        Ok(())
    }

    /// Dispatches a transaction's typed payload.
    pub(crate) fn apply_kind(
        &self,
        overlay: &mut Overlay,
        tx: &Transaction,
        context: BlockContext,
    ) -> Result<()> {
        // Value flow must come from exactly one mechanism. A channel operation
        // that also carried transfer outputs would have two, and reasoning
        // about conservation would stop being local.
        if tx.kind.has_payload() && !tx.outputs.is_empty() {
            return Err(NodeError::MixedTransactionKind(tx.kind.label()));
        }

        // Derived once. Every arm below that needs the sender wants its
        // *address*, and under ML-DSA the address is a hash of the key rather
        // than the key itself, so this is a computation and not a field read.
        let sender = tx.sender();

        self.apply_kind_for(overlay, &sender, tx.nonce, &tx.kind, context)
    }

    /// Dispatches a payload on behalf of a sender that may not be a
    /// transaction's.
    ///
    /// Split out for the sealed mempool. An envelope opened at its reveal
    /// height carries an action but no transaction — the transaction that
    /// submitted it committed blocks ago — so the dispatch has to be reachable
    /// from a `(sender, nonce, kind)` triple rather than only from a
    /// `Transaction`. Everything below reads exactly those three things, which
    /// is why the split is a signature change and not a second code path.
    pub(crate) fn apply_kind_for(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        kind: &TxKind,
        context: BlockContext,
    ) -> Result<()> {
        let sender = *sender;

        // The circuit breaker, at the one place every typed payload passes
        // through. `TxKind::Transfer` maps to no module, so a plain transfer
        // is ungated by construction rather than by an exception somebody has
        // to remember — and the transfer itself has already happened in
        // `stage_transaction`, before this dispatch is reached at all.
        if let Some(module) = Module::of(kind) {
            self.require_module(overlay, module, context.height)?;
        }

        match kind {
            TxKind::Transfer => Ok(()),
            TxKind::OpenChannel(open) => self.open_channel(overlay, &sender, nonce, open),
            TxKind::CooperativeClose(closure) => self.cooperative_close(overlay, closure),
            TxKind::DisputeClose(closure) => self.dispute_close(overlay, &sender, closure, context),
            TxKind::PenaltyClaim(proof) => self.penalty_claim(overlay, &sender, proof),
            TxKind::SettleBatch(closures) => {
                // Sequential, sharing the overlay: two closures naming the same
                // channel cannot both succeed, because the first marks it
                // closed and the second sees that.
                for closure in closures {
                    self.cooperative_close(overlay, closure)?;
                }
                Ok(())
            }
            TxKind::FinalizeDispute(channel_id) => {
                self.finalize_dispute(overlay, channel_id, context)
            }
            TxKind::RegisterAsset(registration) => {
                self.register_asset(overlay, &sender, nonce, registration)
            }
            TxKind::TransferAsset(transfer) => self.transfer_asset(overlay, &sender, transfer),
            TxKind::CreatePool(creation) => self.create_pool(overlay, &sender, creation),
            TxKind::AddLiquidity(deposit) => self.add_liquidity(overlay, &sender, deposit),
            TxKind::RemoveLiquidity(withdrawal) => {
                self.remove_liquidity(overlay, &sender, withdrawal)
            }
            // Staged, not executed. It settles with the rest of its pool's
            // block in `settle_trading` — see `crate::state::dex_exec`.
            TxKind::Swap(request) => self.stage_swap(overlay, &sender, nonce, request, context),
            TxKind::SwapRoute(route) => self.execute_route(overlay, &sender, route, context),
            TxKind::PlaceOrder(placement) => {
                self.place_order(overlay, &sender, nonce, placement, context)
            }
            TxKind::CancelOrder(order_id) => self.cancel_order(overlay, &sender, order_id),
            TxKind::CreateFeed(creation) => self.create_feed(overlay, &sender, creation),
            TxKind::SubmitFeed(submission) => self.submit_feed(overlay, submission, context),
            TxKind::RotateAuthorities(rotation) => self.rotate_authorities(overlay, rotation),
            // Staged, not folded. The accumulator advances exactly once per
            // block in `settle_oracle`, whether or not a proof arrived.
            TxKind::SubmitBeacon(beacon) => self.stage_beacon(overlay, beacon, context),
            TxKind::ClaimWork(claim) => self.claim_work(overlay, claim, context),
            TxKind::LockStake(lock) => self.lock_stake(overlay, &sender, lock, context),
            TxKind::UnlockStake(unlock) => self.unlock_stake(overlay, &sender, unlock, context),
            TxKind::Propose(submission) => {
                self.propose(overlay, &sender, nonce, submission, context)
            }
            TxKind::CastVote(ballot) => self.cast_vote(overlay, &sender, ballot, context),
            TxKind::CancelProposal(id) => self.cancel_proposal(overlay, &sender, id),
            TxKind::DeployContract(deploy) => self
                .deploy_contract(overlay, &sender, nonce, deploy)
                .map(|_| ()),
            TxKind::CallContract(call) => self
                .call_contract(overlay, &sender, call, context)
                .map(|_| ()),
            TxKind::Shielded(joinsplit) => self.apply_joinsplit(overlay, &sender, joinsplit),
            // Recorded, not executed. It opens in `settle_sealed` at its
            // reveal height — see `crate::state::sealed_exec`.
            TxKind::Seal(envelope) => {
                self.stage_envelope(overlay, &sender, nonce, envelope, context)
            }
            TxKind::RevealShare(share) => self.stage_reveal_share(overlay, share, context),

            // Every one of these acts on the sender's own DID. The subject is
            // never a field, so there is no authorisation check to get wrong:
            // the signature that made this transaction valid is the
            // authorisation, and the address it produces is the subject.
            TxKind::RegisterDid(payload) => self.register_did(overlay, &sender, payload),
            TxKind::RotateDidKey(payload) => self.rotate_did_key(overlay, &sender, payload),
            TxKind::RevokeDid(payload) => {
                self.revoke_did(overlay, &sender, context.height, payload)
            }
            TxKind::AnchorAttestation(payload) => {
                self.anchor_attestation(overlay, &sender, context.height, payload)
            }
            TxKind::SetRevocationBit(payload) => self.set_revocation_bit(overlay, &sender, payload),

            // Issuance, distribution and attestation are the issuer's own
            // transactions, so a bad one is an error. A DvP is not: it names a
            // counterparty, and an error there would let anyone void a block by
            // submitting a swap they know cannot settle — invariant 7.
            TxKind::IssueRwa(payload) => {
                let rule = payload
                    .rule
                    .as_ref()
                    .map(|rule| {
                        let predicate = maya_rwa::token::RulePredicate::from_parts(
                            rule.predicate_tag,
                            rule.bound,
                        )
                        .ok_or_else(|| {
                            NodeError::Decode(format!(
                                "rwa: predicate tag {} names nothing",
                                rule.predicate_tag
                            ))
                        })?;
                        maya_rwa::token::TransferRule::new(
                            rule.schema,
                            predicate,
                            rule.trusted_issuers.clone(),
                            rule.eligibility_blocks,
                        )
                        .map_err(|error| NodeError::Decode(format!("rwa: {error}")))
                    })
                    .transpose()?;
                self.issue_rwa(overlay, &sender, &payload.label, payload.total_units, rule)
                    .map(|_| ())
            }
            TxKind::SettleDvp(payload) => self
                .settle_dvp(
                    overlay,
                    &sender,
                    &payload.seller,
                    &payload.asset,
                    payload.units,
                    payload.price,
                    payload.page,
                    context.height,
                )
                .map(|_| ()),
            TxKind::RecordEligibility(payload) => {
                self.record_eligibility(overlay, &payload.asset, &sender, context.height)
            }
            TxKind::DistributeRevenue(payload) => self
                .distribute_revenue(
                    overlay,
                    &sender,
                    &payload.asset,
                    payload.round,
                    payload.total,
                    payload.pages,
                    context.height,
                )
                .map(|_| ()),
            // A lock is the sender's own transaction, so a bad one is an error.
            // A claim or refund that loses its race is a no-op — invariant 7.
            TxKind::HtlcLock(payload) => self
                .htlc_lock(overlay, &sender, nonce, payload, context)
                .map(|_| ()),
            TxKind::HtlcClaim(payload) => self.htlc_claim(overlay, payload, context).map(|_| ()),
            TxKind::HtlcRefund(payload) => self.htlc_refund(overlay, payload, context).map(|_| ()),
            TxKind::AttestLegal(payload) => self.attest_legal(
                overlay,
                &sender,
                &payload.asset,
                &payload.document,
                &payload.reference,
                context.height,
            ),
        }
    }

    /// Escrows the opener's funding into a new channel.
    fn open_channel(
        &self,
        overlay: &mut Overlay,
        opener: &Address,
        nonce: u64,
        open: &ChannelOpen,
    ) -> Result<()> {
        let opener = *opener;
        let channel_id = derive_channel_id(&opener, &open.counterparty, open.funding, nonce);

        if self.load_channel(overlay, &channel_id)?.is_some() {
            return Err(NodeError::ChannelExists(hex::encode(channel_id)));
        }

        // The nonce has already been advanced by the caller, so re-derive the
        // opener's staged account rather than re-reading committed state.
        let mut account = match overlay.accounts.get(&opener) {
            Some(existing) => *existing,
            None => self.get_account(&opener)?,
        };

        if account.balance < open.funding {
            return Err(NodeError::InsufficientBalance {
                address: hex::encode(opener),
                required: open.funding,
                available: account.balance,
            });
        }

        // Funds leave the opener's account and live in the channel record until
        // it closes. They are escrowed, not spent.
        account.balance -= open.funding;
        overlay.accounts.insert(opener, account);

        overlay.channels.insert(
            channel_id,
            ChannelRecord::new(opener, open.counterparty, open.funding, open.dispute_window),
        );

        Ok(())
    }

    /// Verifies a closure against a channel's registered participants.
    ///
    /// This is the check that makes third-party batch submission safe: the
    /// submitter's own signature is irrelevant to whether value may move.
    /// ## Binding a supplied key to a recorded participant
    ///
    /// Under ed25519 the recorded address *was* the verifying key, so this
    /// function could simply decode it. An ML-DSA address is a hash, so the key
    /// has to come from the closure — and a key the submitter chose is worth
    /// nothing until it is pinned to the participant the channel actually
    /// records.
    ///
    /// That pinning is the `address_of(pubkey) != address` check below, and it
    /// runs *before* the signatures. Reversing the order would still be correct
    /// but would let anyone spend two full verifications, the expensive part, by
    /// submitting an unrelated key; hashing first rejects that for the price of
    /// one BLAKE3. The argument only got stronger with hybrid signing — the
    /// work being skipped is now a lattice verification *and* a hash-based one.
    ///
    /// Note also what `address_of` covers: both keys. A closure that named the
    /// victim's ML-DSA key beside an attacker's SLH-DSA key hashes to neither
    /// party's address and dies at this check.
    fn verify_closure(closure: &ChannelClosure, record: &ChannelRecord) -> Result<()> {
        let message = closure.signing_bytes();
        let channel = hex::encode(closure.channel_id);

        for (address, public_key, signature, party) in [
            (&record.party_a, &closure.pubkey_a, &closure.sig_a, "a"),
            (&record.party_b, &closure.pubkey_b, &closure.sig_b, "b"),
        ] {
            let mismatch = || NodeError::ClosureSignature {
                channel: channel.clone(),
                party,
            };

            if &address_of(public_key) != address {
                return Err(mismatch());
            }

            let key = HybridVerifyingKey::from_public_key(public_key).map_err(|_| mismatch())?;
            // Both halves, or the closure does not settle.
            key.verify(&message, signature).map_err(|_| mismatch())?;
        }

        Ok(())
    }

    /// Checks that a closure distributes exactly the escrowed capacity.
    fn check_conservation(closure: &ChannelClosure, record: &ChannelRecord) -> Result<()> {
        let total = closure.total()?;
        if total != record.capacity {
            return Err(NodeError::ChannelCapacityMismatch {
                channel: hex::encode(closure.channel_id),
                expected: record.capacity,
                actual: total,
            });
        }
        Ok(())
    }

    /// Closes a channel on a state both parties signed, paying out immediately.
    ///
    /// No dispute window: a mutually signed final state has no defrauded party
    /// to protect.
    fn cooperative_close(&self, overlay: &mut Overlay, closure: &ChannelClosure) -> Result<()> {
        let mut record = self.require_channel(overlay, &closure.channel_id)?;

        if record.status == ChannelStatus::Closed {
            return Err(NodeError::ChannelState {
                channel: hex::encode(closure.channel_id),
                actual: "closed",
                expected: "open or disputed",
            });
        }

        // A cooperative close may pre-empt a running dispute, but only with a
        // state that actually supersedes the disputed one.
        if record.status == ChannelStatus::Disputed && closure.seq <= record.dispute_seq {
            return Err(NodeError::StaleChannelState {
                channel: hex::encode(closure.channel_id),
                current: record.dispute_seq,
                proposed: closure.seq,
            });
        }

        Self::verify_closure(closure, &record)?;
        Self::check_conservation(closure, &record)?;

        self.credit(overlay, &record.party_a, closure.balance_a)?;
        self.credit(overlay, &record.party_b, closure.balance_b)?;

        record.status = ChannelStatus::Closed;
        overlay.channels.insert(closure.channel_id, record);
        Ok(())
    }

    /// Records a unilaterally submitted state and starts the dispute window.
    fn dispute_close(
        &self,
        overlay: &mut Overlay,
        submitter: &Address,
        closure: &ChannelClosure,
        context: BlockContext,
    ) -> Result<()> {
        let mut record = self.require_channel(overlay, &closure.channel_id)?;
        let channel = hex::encode(closure.channel_id);

        if record.status == ChannelStatus::Closed {
            return Err(NodeError::ChannelState {
                channel,
                actual: "closed",
                expected: "open or disputed",
            });
        }

        if !record.is_participant(submitter) {
            return Err(NodeError::NotAParticipant {
                channel,
                address: hex::encode(submitter),
            });
        }

        // A second dispute is allowed only with a newer state — that is how an
        // honest party overrides a stale submission without needing a penalty.
        if record.status == ChannelStatus::Disputed && closure.seq <= record.dispute_seq {
            return Err(NodeError::StaleChannelState {
                channel,
                current: record.dispute_seq,
                proposed: closure.seq,
            });
        }

        Self::verify_closure(closure, &record)?;
        Self::check_conservation(closure, &record)?;

        record.status = ChannelStatus::Disputed;
        record.dispute_seq = closure.seq;
        record.dispute_balance_a = closure.balance_a;
        record.dispute_balance_b = closure.balance_b;
        record.dispute_closer = *submitter;
        record.dispute_commitment = closure.revocation_commitment;
        record.dispute_deadline = context
            .height
            .checked_add(record.dispute_window)
            .ok_or(NodeError::BalanceOverflow)?;

        overlay.channels.insert(closure.channel_id, record);
        Ok(())
    }

    /// Punishes a party that submitted a revoked state.
    ///
    /// The claimant takes the entire channel capacity. A partial penalty would
    /// leave fraud with positive expected value whenever the attempt is cheap.
    fn penalty_claim(
        &self,
        overlay: &mut Overlay,
        claimant: &Address,
        proof: &RevocationProof,
    ) -> Result<()> {
        let mut record = self.require_channel(overlay, &proof.channel_id)?;
        let channel = hex::encode(proof.channel_id);

        if record.status != ChannelStatus::Disputed {
            return Err(NodeError::ChannelState {
                channel,
                actual: match record.status {
                    ChannelStatus::Open => "open",
                    ChannelStatus::Closed => "closed",
                    ChannelStatus::Disputed => "disputed",
                },
                expected: "disputed",
            });
        }

        if !record.is_participant(claimant) {
            return Err(NodeError::NotAParticipant {
                channel,
                address: hex::encode(claimant),
            });
        }

        // The closer cannot punish itself; otherwise submitting a revoked state
        // and immediately "catching" it would drain the counterparty.
        if &record.dispute_closer == claimant {
            return Err(NodeError::PenaltyByCloser { channel });
        }

        if proof.revoked_seq != record.dispute_seq
            || revocation_commitment(&proof.secret) != record.dispute_commitment
        {
            return Err(NodeError::InvalidRevocationProof { channel });
        }

        // Everything to the victim.
        self.credit(overlay, claimant, record.capacity)?;
        record.status = ChannelStatus::Closed;
        overlay.channels.insert(proof.channel_id, record);
        Ok(())
    }

    /// Pays out a dispute whose window elapsed without a penalty claim.
    fn finalize_dispute(
        &self,
        overlay: &mut Overlay,
        channel_id: &ChannelId,
        context: BlockContext,
    ) -> Result<()> {
        let mut record = self.require_channel(overlay, channel_id)?;
        let channel = hex::encode(channel_id);

        if record.status != ChannelStatus::Disputed {
            return Err(NodeError::ChannelState {
                channel,
                actual: match record.status {
                    ChannelStatus::Open => "open",
                    ChannelStatus::Closed => "closed",
                    ChannelStatus::Disputed => "disputed",
                },
                expected: "disputed",
            });
        }

        // Finalizing early would close the very window the penalty depends on.
        if context.height < record.dispute_deadline {
            return Err(NodeError::DisputeWindowOpen {
                channel,
                deadline: record.dispute_deadline,
                height: context.height,
            });
        }

        self.credit(overlay, &record.party_a, record.dispute_balance_a)?;
        self.credit(overlay, &record.party_b, record.dispute_balance_b)?;

        record.status = ChannelStatus::Closed;
        overlay.channels.insert(*channel_id, record);
        Ok(())
    }

    /// Writes a channel record directly. For genesis fixtures and tests.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a write failure.
    pub fn put_channel(&self, channel_id: &ChannelId, record: &ChannelRecord) -> Result<()> {
        self.raw_put(&channel_key(channel_id), &record.encode())
    }
}

/// Convenience for tests and callers assembling settlement transactions.
#[must_use]
pub fn account_for(balance: u64, nonce: u64) -> Account {
    Account { balance, nonce }
}
