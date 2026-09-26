//! Executing an attack attestation.
//!
//! ## What is checked, in order
//!
//! 1. The author's gossipsub signature over the rebuilt signed bytes, with
//!    `verify_strict`. Strict, because consensus needs one answer: a signature
//!    libp2p's looser check accepted and this one refuses is evidence nobody can
//!    attest — evasion by the author, never a way to frame someone else.
//! 2. That the frame decodes and then fails the check the kind names. A frame
//!    that verifies is a lie about an honest author and is refused.
//!
//! Both are stateless, so a miner learns whether an attestation is valid
//! before including it.
//!
//! ## Invalid evidence is an error; repeated evidence is a no-op
//!
//! The attestation is its submitter's own transaction with nobody else's
//! outcome riding on it, so evidence that does not verify fails the block —
//! as an HTLC lock that cannot be made does (`crate::state::htlc_exec`). A
//! *second copy* of evidence already on chain is different: two honest
//! observers of the same offence submit it independently as the ordinary case,
//! and if the later one were an error it would void the block the earlier one
//! sits in. So it returns, the nonce advances, and nothing is written —
//! invariant 7's rule applied to a race between reporters.

use ed25519_dalek::{Signature, VerifyingKey};
use maya_threat_intel::{AttackAttestation, OffenceKind, ThreatIndicator};

use crate::core::{Block, Transaction};
use crate::error::{NodeError, Result};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::threat::{derive_evidence_id, evidence_key, indicator_key};

impl StateDB {
    /// Records verified evidence against its author.
    ///
    /// # Errors
    ///
    /// [`NodeError::ThreatIntel`] before activation or for evidence that does
    /// not verify; a read or decode failure otherwise.
    pub(crate) fn attest_attack(
        &self,
        overlay: &mut Overlay,
        attestation: &AttackAttestation,
        context: BlockContext,
    ) -> Result<()> {
        require_active(context)?;
        let id = verify_evidence(attestation, context.height)?;
        let marker = evidence_key(&id);
        if self.record(overlay, &marker)?.is_some() {
            return Ok(());
        }
        let author = attestation.gossip.author;
        let previous = self.threat_indicator(overlay, &author)?;
        let next = ThreatIndicator::observe(previous.as_ref(), attestation.kind, context.height);
        StateDB::put_record(overlay, marker, context.height.to_le_bytes().to_vec());
        StateDB::put_record(overlay, indicator_key(&author), next.encode().to_vec());
        Ok(())
    }
}

/// Checks evidence and returns its id. Stateless: the mempool calls it at
/// admission, and block execution calls it again.
///
/// # Errors
///
/// [`NodeError::ThreatIntel`] naming the first check that failed.
pub fn verify_evidence(attestation: &AttackAttestation, height: u64) -> Result<[u8; 32]> {
    let gossip = &attestation.gossip;
    let key = VerifyingKey::from_bytes(&gossip.author)
        .map_err(|_| refused("the author is not an ed25519 key"))?;
    let signature = Signature::from_bytes(&gossip.signature);
    key.verify_strict(&attestation.signed_bytes(), &signature)
        .map_err(|_| refused("the author's gossip signature does not verify"))?;
    match attestation.kind {
        OffenceKind::InvalidSignature => frame_fails_signature(&gossip.data, height)?,
        OffenceKind::TxRootMismatch => frame_fails_tx_root(&gossip.data)?,
    }
    Ok(derive_evidence_id(attestation))
}

/// The frame is a transaction, and its authorization is what fails — exactly
/// the errors `peer_health::classify_transaction` scores as a bad signature.
fn frame_fails_signature(frame: &[u8], height: u64) -> Result<()> {
    let tx = Transaction::from_bytes(frame)
        .map_err(|_| refused("the evidence frame is not a transaction"))?;
    // Judged at `height` under the verification policy, so a suite-tagged
    // frame is evidence of a failed signature when its *suite* signature fails
    // -- not merely because the height-less `verify` refuses every v7 frame.
    // `SignatureSuite` joins the three hybrid failures for the same reason.
    match tx.verify_at(height, &crate::crypto::suites::verification_policy()) {
        Err(
            NodeError::SignatureVerification
            | NodeError::HashSignatureVerification
            | NodeError::MissingSignature
            | NodeError::SignatureSuite(_),
        ) => Ok(()),
        Ok(()) => Err(refused("the transaction's signature verifies")),
        Err(other) => Err(refused(format!(
            "the transaction fails for a reason other than its signature: {other}"
        ))),
    }
}

/// The frame is a block whose body its header does not commit to.
fn frame_fails_tx_root(frame: &[u8]) -> Result<()> {
    let block =
        Block::from_bytes(frame).map_err(|_| refused("the evidence frame is not a block"))?;
    if block.check_tx_root().is_ok() {
        return Err(refused("the block's body matches its tx_root"));
    }
    Ok(())
}

fn refused(reason: impl Into<String>) -> NodeError {
    NodeError::ThreatIntel(reason.into())
}

/// Refuses every attestation before the activation height.
fn require_active(context: BlockContext) -> Result<()> {
    if context.threat_intel_active() {
        Ok(())
    } else {
        Err(refused(format!(
            "threat intel is not active at height {}",
            context.height
        )))
    }
}
