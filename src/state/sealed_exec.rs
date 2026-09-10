//! Executing the sealed mempool: taking envelopes in, taking shares in, and
//! opening what is due.
//!
//! ## The one inequality everything rests on
//!
//! An envelope is included at some height `h` and opened at `reveal_height > h`.
//! The miner who chose the block at `h` — who chose *whether* to include it and
//! in what company — could not read it, because the shares that open it are not
//! valid until `reveal_height`. That is the property. Every other rule in this
//! file exists to keep that inequality true or to keep its consequences
//! bounded.
//!
//! ## Where the revealed batch executes, and why it matters
//!
//! [`StateDB::settle_sealed`] runs after every plaintext transaction in the
//! block has been staged and *before* [`crate::state::db::StateDB`]'s trading
//! pass. Both halves of that sentence are load-bearing.
//!
//! After the plaintext transactions: a miner who assembles the reveal block
//! learns the plaintexts while building it, and could place a transaction of
//! their own in the same block. Running the batch last means any such
//! transaction executes *before* the revealed one, so the miner can back-run
//! but not front-run. Back-running is ordinary arbitrage; front-running is the
//! attack.
//!
//! Before the trading pass: a revealed swap therefore stages into the same
//! uniform-price batch as every plaintext swap in the block. A miner who saw a
//! revealed swap coming and raced it settles at the identical clearing price —
//! which is what turns the residual advantage above from a profit into a
//! nuisance. See `docs/dex.md`.
//!
//! ## A revealed action that fails is a no-op, never an error
//!
//! This is [`crate::state`]'s trading rule applied to a strictly worse case. A
//! failing transaction fails its whole block here, so if a revealed action
//! could return `Err`, anyone could seal a payload that is guaranteed to fail
//! and thereby void the block that opens it — a censorship weapon costing one
//! transaction fee, aimed at a block chosen days in advance.
//!
//! So a revealed action that cannot execute simply does not: the envelope is
//! consumed, the sender's nonce was spent when they submitted it, and nothing
//! else moves. The sender loses what a failed transaction always loses.

use std::collections::BTreeMap;

use maya_mev::cipher::{DecryptionShare, SealedPayload, ShareProof, combine, verify_share};
use maya_mev::committee::Committee;

use curve25519_dalek::ristretto::CompressedRistretto;

use crate::core::TxKind;
use crate::core::codec::ByteReader;
use crate::core::sealed_payload::{
    MAX_SEALED_CIPHERTEXT, RevealShare, SealedEnvelope, derive_envelope_id, envelope_aad,
};
use crate::error::{NodeError, Result};
use crate::sealed::{
    COMMITTEE_KEY, CommitteeRecord, EnvelopeRecord, MAX_REVEAL_DELAY, MAX_REVEAL_SHARES_PER_BLOCK,
    MAX_SEALED_PER_BLOCK, MIN_REVEAL_DELAY, envelope_height_prefix, envelope_key,
    share_envelope_prefix, share_key,
};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

impl StateDB {
    /// Installs the encryption committee at genesis.
    ///
    /// Written directly rather than through an overlay, because genesis has no
    /// block to stage into. `seed_oracle` does the same thing for the same
    /// reason.
    ///
    /// # Errors
    ///
    /// Propagates storage failures.
    pub fn seed_sealed_committee(&self, committee: &CommitteeRecord) -> Result<()> {
        self.raw_put(COMMITTEE_KEY, &committee.encode())
    }

    /// A pending envelope, if one is awaiting reveal under that identifier.
    ///
    /// Public because a committee member has to find the ciphertext it is
    /// about to produce a share for, and a wallet has to be able to see that
    /// its own envelope is still queued. Neither can read what is inside it.
    ///
    /// # Errors
    ///
    /// Propagates storage failures, or [`NodeError::Decode`] for a damaged
    /// record.
    pub fn pending_envelope(
        &self,
        reveal_height: u64,
        id: &[u8; 32],
    ) -> Result<Option<EnvelopeRecord>> {
        match self.raw_get(&envelope_key(reveal_height, id))? {
            Some(bytes) => EnvelopeRecord::decode(&bytes).map(Some),
            None => Ok(None),
        }
    }

    /// The installed committee, if this chain has one.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the stored record is malformed, which
    /// on a chain that started from a validated genesis means the database has
    /// been damaged rather than that anything was misconfigured.
    pub(crate) fn sealed_committee(&self, overlay: &Overlay) -> Result<Option<CommitteeRecord>> {
        match self.record(overlay, COMMITTEE_KEY)? {
            Some(bytes) => CommitteeRecord::decode(&bytes).map(Some),
            None => Ok(None),
        }
    }

    /// The installed committee, or the error that says this chain has none.
    fn require_committee(&self, overlay: &Overlay) -> Result<CommitteeRecord> {
        self.sealed_committee(overlay)?
            .ok_or(NodeError::SealedCommitteeUnset)
    }

    /// Records an envelope for opening at its reveal height.
    ///
    /// The ciphertext is not decrypted, inspected, or validated beyond its
    /// shape — there is nothing here that could inspect it, which is the point.
    /// What *is* checked is everything visible: that a committee exists, that
    /// the reveal height is in the future and not too far into it, and that the
    /// block has not already accepted more envelopes than it may.
    ///
    /// # Errors
    ///
    /// - [`NodeError::SealedCommitteeUnset`] on a chain with no committee.
    /// - [`NodeError::SealedRevealWindow`] for a reveal height in this block,
    ///   in the past, or beyond [`MAX_REVEAL_DELAY`].
    /// - [`NodeError::SealedBlockLimit`] past [`MAX_SEALED_PER_BLOCK`].
    /// - [`NodeError::SealedEnvelopeExists`] for a repeated identifier.
    pub(crate) fn stage_envelope(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        envelope: &SealedEnvelope,
        context: BlockContext,
    ) -> Result<()> {
        // Refuse before anything else if there is nobody to open it. An
        // envelope accepted onto a committee-less chain would be an envelope
        // that expires by construction, and the sender would have paid to
        // discover that. The committee is not otherwise consulted here — there
        // is nothing about a ciphertext this function could check with it.
        self.require_committee(overlay)?;

        // Saturating rather than checked: a reveal height at or below the
        // current one has a delay of zero, which is already outside the window
        // below. Distinguishing "in this block" from "in the past" would be two
        // errors for one mistake.
        let delay = envelope.reveal_height.saturating_sub(context.height);
        if !(MIN_REVEAL_DELAY..=MAX_REVEAL_DELAY).contains(&delay) {
            return Err(NodeError::SealedRevealWindow {
                reveal_height: envelope.reveal_height,
                height: context.height,
                min: MIN_REVEAL_DELAY,
                max: MAX_REVEAL_DELAY,
            });
        }

        if envelope.ciphertext.len() > MAX_SEALED_CIPHERTEXT {
            return Err(NodeError::SealedBlockLimit {
                reason: format!(
                    "ciphertext of {} bytes exceeds the maximum {MAX_SEALED_CIPHERTEXT}",
                    envelope.ciphertext.len()
                ),
            });
        }

        if overlay.sealed_envelopes >= MAX_SEALED_PER_BLOCK {
            return Err(NodeError::SealedBlockLimit {
                reason: format!("a block may carry at most {MAX_SEALED_PER_BLOCK} envelopes"),
            });
        }

        let id = derive_envelope_id(sender, nonce);
        let key = envelope_key(envelope.reveal_height, &id);
        if self.record(overlay, &key)?.is_some() {
            return Err(NodeError::SealedEnvelopeExists(hex::encode(id)));
        }

        StateDB::put_record(
            overlay,
            key,
            EnvelopeRecord {
                sender: *sender,
                ciphertext: envelope.ciphertext.clone(),
            }
            .encode(),
        );
        overlay.sealed_envelopes += 1;
        Ok(())
    }

    /// Records one committee member's decryption share.
    ///
    /// Valid only in the block at the envelope's reveal height. Accepting one
    /// earlier would publish a decryption share while blocks were still being
    /// built against the ciphertext, which hands the plaintext to whoever
    /// collects shares fastest — a different party with the same advantage, not
    /// a smaller advantage.
    ///
    /// The proof is verified here rather than at reveal. A share that fails is
    /// then attributable to the member who submitted it, at the moment they
    /// submitted it, instead of surfacing three functions later as a plaintext
    /// that would not decrypt.
    ///
    /// # Errors
    ///
    /// - [`NodeError::SealedCommitteeUnset`] on a chain with no committee.
    /// - [`NodeError::UnknownSealedEnvelope`] if nothing is awaiting reveal
    ///   under that identifier at this height.
    /// - [`NodeError::DuplicateDecryptionShare`] if the member already
    ///   submitted one.
    /// - [`NodeError::InvalidDecryptionShare`] for a malformed point, a
    ///   malformed proof, a non-member, or a proof that does not verify.
    /// - [`NodeError::SealedBlockLimit`] past [`MAX_REVEAL_SHARES_PER_BLOCK`].
    pub(crate) fn stage_reveal_share(
        &self,
        overlay: &mut Overlay,
        share: &RevealShare,
        context: BlockContext,
    ) -> Result<()> {
        let committee = self.require_committee(overlay)?;

        if overlay.reveal_shares >= MAX_REVEAL_SHARES_PER_BLOCK {
            return Err(NodeError::SealedBlockLimit {
                reason: format!(
                    "a block may carry at most {MAX_REVEAL_SHARES_PER_BLOCK} decryption shares"
                ),
            });
        }

        // A share is addressed to `(this height, this envelope)`. There is no
        // field in which a submitter names a different height, so "only in the
        // reveal block" is enforced by the lookup rather than by a comparison
        // somebody could forget.
        let key = envelope_key(context.height, &share.envelope);
        let Some(bytes) = self.record(overlay, &key)? else {
            return Err(NodeError::UnknownSealedEnvelope(hex::encode(
                share.envelope,
            )));
        };
        let record = EnvelopeRecord::decode(&bytes)?;

        let share_key = share_key(context.height, &share.envelope, share.member);
        if self.record(overlay, &share_key)?.is_some() {
            return Err(NodeError::DuplicateDecryptionShare {
                member: share.member,
                envelope: hex::encode(share.envelope),
            });
        }

        let payload = SealedPayload::decode(&record.ciphertext).map_err(|error| {
            NodeError::InvalidDecryptionShare {
                member: share.member,
                reason: format!("the envelope's ciphertext is malformed: {error}"),
            }
        })?;
        let decryption = decode_share(share)?;

        verify_share(&committee.to_committee(), &payload, &decryption).map_err(|error| {
            NodeError::InvalidDecryptionShare {
                member: share.member,
                reason: error.to_string(),
            }
        })?;

        let mut stored = Vec::with_capacity(32 + 64);
        stored.extend_from_slice(&share.share);
        stored.extend_from_slice(&share.proof);
        StateDB::put_record(overlay, share_key, stored);
        overlay.reveal_shares += 1;
        Ok(())
    }

    /// Opens every envelope due at this height, and expires the rest.
    ///
    /// Runs once per block whether or not anything is due, and visits envelopes
    /// in storage-key order — reveal height, then identifier — so every node
    /// executes the same actions in the same sequence. See the module
    /// documentation for why that order is a property of the data rather than a
    /// choice a miner makes.
    ///
    /// # Errors
    ///
    /// Propagates storage failures. A revealed action that *fails* is not one
    /// of them: it is discarded, for the reason the module documentation gives.
    pub(crate) fn settle_sealed(&self, overlay: &mut Overlay, context: BlockContext) -> Result<()> {
        let due = self.merged_records(overlay, &envelope_height_prefix(context.height))?;
        if due.is_empty() {
            return Ok(());
        }

        // Absent only if the committee was never installed, in which case no
        // envelope could have been accepted and `due` would be empty.
        let committee = self.require_committee(overlay)?.to_committee();

        for (key, bytes) in due {
            let Some(id) = envelope_id_from_key(&key) else {
                // A key under this prefix that is not the right length cannot
                // have been written by `stage_envelope`. Skipping it is the
                // conservative reading: it is not an action to perform, and
                // failing the block over it would let corrupt storage on one
                // node fork the chain.
                continue;
            };
            let record = EnvelopeRecord::decode(&bytes)?;

            let shares = self.gather_shares(overlay, context.height, &id)?;
            let opened = open_envelope(&committee, &record, &id, context.height, &shares);

            // Consumed either way, and before the action runs. An envelope that
            // stayed pending after being opened could be opened again in a
            // later block; one that stayed pending after expiring would sit in
            // state forever.
            StateDB::delete_record(overlay, key);
            for share_key in shares.keys() {
                StateDB::delete_record(overlay, share_key.clone());
            }

            if let Some(kind) = opened {
                // The whole reason this is not `?`. A revealed action that
                // fails leaves the overlay as it was — `apply_kind` mutates
                // only on the paths that succeed — and the block continues.
                let _ = self.apply_revealed(overlay, &record.sender, &id, &kind, context);
                overlay.revealed += 1;
            }
        }

        Ok(())
    }

    /// Every share submitted for one envelope, by storage key.
    fn gather_shares(
        &self,
        overlay: &Overlay,
        height: u64,
        id: &[u8; 32],
    ) -> Result<BTreeMap<Vec<u8>, DecryptionShare>> {
        let prefix = share_envelope_prefix(height, id);
        let mut shares = BTreeMap::new();

        for (key, bytes) in self.merged_records(overlay, &prefix)? {
            // The member index is the key's tail. Reading it from the key
            // rather than storing it again in the value means the two cannot
            // disagree about who submitted what.
            let Some(member) = member_from_share_key(&key) else {
                continue;
            };
            let Some(decryption) = stored_share(member, &bytes) else {
                continue;
            };
            shares.insert(key, decryption);
        }

        Ok(shares)
    }

    /// Executes a revealed action under the envelope sender's authority.
    ///
    /// The nonce handed to the action is the envelope identifier's low eight
    /// bytes rather than the sender's transaction nonce, which is long since
    /// spent. Identifiers are unique per `(sender, nonce)`, so anything derived
    /// from it — a channel, an asset, a proposal — is unique too, and derived
    /// from a value the sender fixed when they sealed the envelope.
    fn apply_revealed(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        id: &[u8; 32],
        kind: &TxKind,
        context: BlockContext,
    ) -> Result<()> {
        let nonce = u64::from_le_bytes(
            id[..8]
                .try_into()
                .expect("a 32-byte identifier has eight leading bytes"),
        );
        self.apply_kind_for(overlay, sender, nonce, kind, context)
    }
}

/// Decrypts an envelope, or reports that it could not be.
///
/// Returns `None` for every failure — too few shares, a share that no longer
/// verifies, a ciphertext that does not open — because at this point all of
/// them mean the same thing: the envelope expires and nothing happens. There is
/// no failure here that should stop the block, so there is no error to
/// propagate.
fn open_envelope(
    committee: &Committee,
    record: &EnvelopeRecord,
    id: &[u8; 32],
    reveal_height: u64,
    shares: &BTreeMap<Vec<u8>, DecryptionShare>,
) -> Option<TxKind> {
    if shares.len() < usize::from(committee.threshold) {
        return None;
    }

    let payload = SealedPayload::decode(&record.ciphertext).ok()?;
    let collected: Vec<DecryptionShare> = shares.values().cloned().collect();
    let plaintext = combine(
        committee,
        &payload,
        &collected,
        &envelope_aad(id, reveal_height),
    )
    .ok()?;

    let mut reader = ByteReader::new(&plaintext);
    let kind = TxKind::decode(&mut reader).ok()?;
    reader.finish().ok()?;

    // Nesting is refused rather than recursed into. A sealed envelope inside a
    // sealed envelope would be an envelope whose reveal height is decided by
    // the outer one's, and a chain of them is a queue that grows every time it
    // is drained.
    if matches!(kind, TxKind::Seal(_) | TxKind::RevealShare(_)) {
        return None;
    }

    Some(kind)
}

/// The envelope identifier a storage key ends with.
fn envelope_id_from_key(key: &[u8]) -> Option<[u8; 32]> {
    let tail = key.len().checked_sub(32)?;
    key[tail..].try_into().ok()
}

/// The member index a share's storage key ends with.
fn member_from_share_key(key: &[u8]) -> Option<u16> {
    let tail = key.len().checked_sub(2)?;
    Some(u16::from_be_bytes(key[tail..].try_into().ok()?))
}

/// Rebuilds a stored share.
fn stored_share(member: u16, bytes: &[u8]) -> Option<DecryptionShare> {
    if bytes.len() != 32 + 64 {
        return None;
    }
    let point: [u8; 32] = bytes[..32].try_into().ok()?;
    let proof: [u8; 64] = bytes[32..].try_into().ok()?;
    Some(DecryptionShare {
        index: member,
        share: CompressedRistretto(point),
        proof: ShareProof::decode(&proof).ok()?,
    })
}

/// Converts a wire share into the form [`maya_mev`] verifies.
fn decode_share(share: &RevealShare) -> Result<DecryptionShare> {
    Ok(DecryptionShare {
        index: share.member,
        share: CompressedRistretto(share.share),
        proof: ShareProof::decode(&share.proof).map_err(|error| {
            NodeError::InvalidDecryptionShare {
                member: share.member,
                reason: error.to_string(),
            }
        })?,
    })
}
