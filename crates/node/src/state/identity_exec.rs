//! Executing the identity transitions.
//!
//! ## Every one of these acts on the sender's own DID
//!
//! Registration, rotation and revocation take their subject from
//! `Transaction::sender`, never from a field. There is no way to express "do
//! this to somebody else's DID", so there is no authorisation check to get
//! wrong — the signature that made the transaction valid *is* the
//! authorisation, and the address it produces is the subject.
//!
//! That is the same construction the rest of the chain uses and it is worth
//! being explicit about, because the alternative reads as harmless: a `subject`
//! field plus a check that it equals the sender is one refactor away from a
//! `subject` field plus a check somebody removed.
//!
//! ## Rotation is the interesting one
//!
//! The transaction is signed by the key the document currently names — by
//! construction, since the address is BLAKE3 over that key pair. So the *old*
//! key authorises the new one, which is exactly what rotation has to mean. The
//! epoch must advance by exactly one, which is what stops a replayed rotation
//! reinstalling a key.
//!
//! A revoked subject cannot rotate. Otherwise revocation would be a state a
//! subject could leave, and "revoked" would mean "revoked for now".
//!
//! ## Nothing here writes a claim
//!
//! `AnchorAttestation` writes a root. `SetRevocationBit` flips a bit. Neither
//! has a field a personal fact could travel in, and
//! `tests/identity_tests.rs` checks committed state for preimages rather than
//! trusting that reading.

use maya_identity::attestation::{BITS_PER_PAGE, CryptographicAttestation};
use maya_identity::did::Did;
use maya_identity::document::{DidDocument, ServiceEndpoint, VerificationMethod};

use crate::core::identity_payload::{
    AnchorAttestation, RegisterDid, RevokeDid, RotateDidKey, SetRevocationBit,
};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::db::{Overlay, StateDB};
use crate::state::identity::{attestation_key, did_key, revocation_key};

impl StateDB {
    /// Registers `did:maya2c:<sender>`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the subject already has a document, or
    /// if the key or an endpoint is outside its bounds. Re-registration is
    /// refused rather than overwriting: a document that could be replaced
    /// wholesale would make rotation and revocation pointless, since a subject
    /// could simply re-register with any key and epoch zero.
    pub(crate) fn register_did(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        payload: &RegisterDid,
    ) -> Result<()> {
        let did = Did::from_address(*sender);
        if self.did_document(overlay, &did)?.is_some() {
            return Err(NodeError::Decode(format!(
                "identity: {did} already has a document"
            )));
        }

        let endpoints = payload
            .endpoints
            .iter()
            .map(|(service_type, uri)| ServiceEndpoint::new(service_type, uri))
            .collect::<maya_identity::Result<Vec<_>>>()
            .map_err(shape)?;
        let verification = VerificationMethod::new(0, payload.public_key.clone()).map_err(shape)?;
        let document = DidDocument::new(did, verification, endpoints).map_err(shape)?;

        StateDB::put_record(overlay, did_key(&did), document.encode());
        Ok(())
    }

    /// Installs a new key on the sender's own DID.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the subject has no document, is
    /// revoked, the epoch does not advance by exactly one, or the key is the
    /// wrong length.
    pub(crate) fn rotate_did_key(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        payload: &RotateDidKey,
    ) -> Result<()> {
        let did = Did::from_address(*sender);
        let document = self.require_document(overlay, &did)?;
        let next =
            VerificationMethod::new(payload.epoch, payload.public_key.clone()).map_err(shape)?;
        let rotated = document.rotate(next).map_err(shape)?;

        StateDB::put_record(overlay, did_key(&did), rotated.encode());
        Ok(())
    }

    /// Revokes the sender's own DID at this height.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the subject has no document or is
    /// already revoked.
    pub(crate) fn revoke_did(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        height: u64,
        _payload: &RevokeDid,
    ) -> Result<()> {
        let did = Did::from_address(*sender);
        let document = self.require_document(overlay, &did)?;
        let revoked = document.revoke(height).map_err(shape)?;

        StateDB::put_record(overlay, did_key(&did), revoked.encode());
        Ok(())
    }

    /// Publishes an issuer's credential-tree root.
    ///
    /// The issuer must have a live DID. An attestation from a subject nobody
    /// can resolve is one no verifier can check the signature of, so it would
    /// be a record that costs storage and proves nothing.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the issuer has no document, is revoked,
    /// or the schema or expiry is outside its bounds.
    pub(crate) fn anchor_attestation(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        height: u64,
        payload: &AnchorAttestation,
    ) -> Result<()> {
        let issuer = Did::from_address(*sender);
        let document = self.require_document(overlay, &issuer)?;
        if document.is_revoked(height) {
            return Err(NodeError::Decode(format!(
                "identity: {issuer} is revoked and cannot attest"
            )));
        }

        let expires_at = (payload.expires_at != 0).then_some(payload.expires_at);
        let attestation = CryptographicAttestation::new(
            issuer,
            &payload.schema,
            payload.root,
            height,
            expires_at,
        )
        .map_err(shape)?;

        StateDB::put_record(
            overlay,
            attestation_key(&issuer, &payload.schema),
            attestation.encode(),
        );
        Ok(())
    }

    /// Flips one bit of the sender's own revocation bitmap.
    ///
    /// Setting only. There is no clear, and that is the design: a credential
    /// un-revoked is a credential a verifier accepted yesterday, refused today
    /// and accepts again tomorrow, and no holder or verifier can reason about
    /// that. An issuer that made a mistake reissues.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the issuer has no document or the bit
    /// is already set — the second so a no-op transaction cannot be replayed
    /// for its fee side effects alone.
    pub(crate) fn set_revocation_bit(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        payload: &SetRevocationBit,
    ) -> Result<()> {
        let issuer = Did::from_address(*sender);
        self.require_document(overlay, &issuer)?;

        let page_index = (payload.index / BITS_PER_PAGE as u64) as u32;
        let within = (payload.index % BITS_PER_PAGE as u64) as usize;
        let mut page = self.revocation_page(overlay, &issuer, page_index)?;
        if page.is_revoked(within) {
            return Err(NodeError::Decode(format!(
                "identity: credential {} is already revoked",
                payload.index
            )));
        }
        page.revoke(within).map_err(shape)?;

        StateDB::put_record(overlay, revocation_key(&issuer, page_index), page.encode());
        Ok(())
    }

    /// The sender's document, or a refusal.
    fn require_document(&self, overlay: &Overlay, did: &Did) -> Result<DidDocument> {
        self.did_document(overlay, did)?
            .ok_or_else(|| NodeError::Decode(format!("identity: {did} has no document")))
    }
}

/// Turns a shape failure from the identity crate into a node error.
fn shape(error: maya_identity::Error) -> NodeError {
    NodeError::Decode(format!("identity: {error}"))
}
