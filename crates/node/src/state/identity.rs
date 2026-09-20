//! Identity records in the state: their prefixes, and their place in the root.
//!
//! ## Everything here is under the state root
//!
//! Invariant 25: every persisted consensus record is under the state root, or
//! it is on an explicit local-only list. These are consensus records — a
//! verifier decides whether to accept a credential by reading them — so they
//! get a prefix in [`RECORD_LAYERS`](crate::state::commitments) and a
//! [`StateLayer`](crate::state::proof::StateLayer) of their own, and the undo
//! journal covers them for free
//! because they go through `put_record` into `overlay.records`.
//!
//! The layer folds only when it is non-empty, like every other. So a chain
//! where nobody has registered a DID produces exactly the state root it would
//! have produced before this subsystem existed — the same property invariant 11
//! gives the oracle, and the reason adding identity to an existing chain is not
//! a fork.
//!
//! ## Three prefixes, and why they are separate
//!
//! | Prefix | Holds |
//! |---|---|
//! | `i:did:<address>` | the subject's [`DidDocument`] |
//! | `i:att:<issuer><schema>` | an issuer's [`CryptographicAttestation`] |
//! | `i:rev:<issuer><page>` | one [`RevocationPage`] of that issuer's bitmap |
//!
//! Separate rather than one record per issuer, because they change at different
//! rates and for different reasons. A revocation is a single bit flipped by the
//! issuer, possibly many times a day; an attestation root changes when the
//! issuer reissues, which is rare. Folding them together would mean rewriting
//! the root record — and journalling the whole of it — on every revocation.

use maya_identity::attestation::{CryptographicAttestation, RevocationPage};
use maya_identity::did::Did;
use maya_identity::document::DidDocument;

use crate::error::{NodeError, Result};
use crate::state::db::{Overlay, StateDB};

/// Prefix shared by every record this subsystem owns.
///
/// One prefix for the whole subsystem, so `RECORD_LAYERS` needs one entry and
/// the fold needs one layer. The sub-prefixes below live under it.
pub(crate) const IDENTITY_PREFIX: &[u8] = b"i:";

/// Prefix for a subject's document.
pub(crate) const DID_PREFIX: &[u8] = b"i:did:";

/// Prefix for an issuer's attestation.
pub(crate) const ATTESTATION_PREFIX: &[u8] = b"i:att:";

/// Prefix for one page of an issuer's revocation bitmap.
pub(crate) const REVOCATION_PREFIX: &[u8] = b"i:rev:";

// Every sub-prefix must sit under the one the fold knows about, or it is state
// outside the root. Asserted rather than assumed, because the constants are
// four lines apart today and will not always be.
const _: () = {
    assert!(DID_PREFIX[0] == IDENTITY_PREFIX[0] && DID_PREFIX[1] == IDENTITY_PREFIX[1]);
    assert!(
        ATTESTATION_PREFIX[0] == IDENTITY_PREFIX[0] && ATTESTATION_PREFIX[1] == IDENTITY_PREFIX[1]
    );
    assert!(
        REVOCATION_PREFIX[0] == IDENTITY_PREFIX[0] && REVOCATION_PREFIX[1] == IDENTITY_PREFIX[1]
    );
};

/// Storage key for a subject's document.
#[must_use]
pub fn did_key(did: &Did) -> Vec<u8> {
    let mut key = Vec::with_capacity(DID_PREFIX.len() + 32);
    key.extend_from_slice(DID_PREFIX);
    key.extend_from_slice(&did.address());
    key
}

/// Storage key for an issuer's attestation on one schema.
///
/// The schema is hashed rather than appended raw: it is issuer-supplied text,
/// and a key whose length depends on it would let one issuer's long schema
/// crowd the keyspace of another's.
#[must_use]
pub fn attestation_key(issuer: &Did, schema: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(ATTESTATION_PREFIX.len() + 64);
    key.extend_from_slice(ATTESTATION_PREFIX);
    key.extend_from_slice(&issuer.address());
    key.extend_from_slice(blake3::hash(schema.as_bytes()).as_bytes());
    key
}

/// Storage key for one page of an issuer's bitmap.
#[must_use]
pub fn revocation_key(issuer: &Did, page: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(REVOCATION_PREFIX.len() + 36);
    key.extend_from_slice(REVOCATION_PREFIX);
    key.extend_from_slice(&issuer.address());
    key.extend_from_slice(&page.to_le_bytes());
    key
}

impl StateDB {
    /// A subject's document, through the overlay and then committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure or
    /// [`NodeError::Decode`] for a record that does not parse.
    pub(crate) fn did_document(&self, overlay: &Overlay, did: &Did) -> Result<Option<DidDocument>> {
        self.record(overlay, &did_key(did))?
            .map(|bytes| decode(&bytes, DidDocument::decode))
            .transpose()
    }

    /// A subject's document from committed state alone.
    ///
    /// # Errors
    ///
    /// As `did_document`, which is `pub(crate)` and so named here rather than
    /// linked.
    pub fn stored_did_document(&self, did: &Did) -> Result<Option<DidDocument>> {
        self.raw_get(&did_key(did))?
            .map(|bytes| decode(&bytes, DidDocument::decode))
            .transpose()
    }

    /// An issuer's attestation on a schema, from committed state.
    ///
    /// # Errors
    ///
    /// As `did_document`, which is `pub(crate)` and so named here rather than
    /// linked.
    pub fn stored_attestation(
        &self,
        issuer: &Did,
        schema: &str,
    ) -> Result<Option<CryptographicAttestation>> {
        self.raw_get(&attestation_key(issuer, schema))?
            .map(|bytes| decode(&bytes, CryptographicAttestation::decode))
            .transpose()
    }

    /// One page of an issuer's bitmap, through the overlay.
    ///
    /// An absent page is an **empty** page, not an error: an issuer that has
    /// revoked nothing has written nothing, and a verifier reading a bit from
    /// it should learn "live" rather than "missing".
    ///
    /// # Errors
    ///
    /// As `did_document`, which is `pub(crate)` and so named here rather than
    /// linked.
    pub(crate) fn revocation_page(
        &self,
        overlay: &Overlay,
        issuer: &Did,
        page: u32,
    ) -> Result<RevocationPage> {
        match self.record(overlay, &revocation_key(issuer, page))? {
            Some(bytes) => decode(&bytes, RevocationPage::decode),
            None => Ok(RevocationPage::empty(*issuer, page)),
        }
    }

    /// Whether a credential index is revoked, reading committed state.
    ///
    /// # Errors
    ///
    /// As `did_document`, which is `pub(crate)` and so named here rather than
    /// linked.
    pub fn is_credential_revoked(&self, issuer: &Did, index: u64) -> Result<bool> {
        let page = (index / maya_identity::attestation::BITS_PER_PAGE as u64) as u32;
        let within = (index % maya_identity::attestation::BITS_PER_PAGE as u64) as usize;
        match self.raw_get(&revocation_key(issuer, page))? {
            Some(bytes) => Ok(decode(&bytes, RevocationPage::decode)?.is_revoked(within)),
            None => Ok(false),
        }
    }
}

/// Turns an identity crate decode failure into a node error.
///
/// The crate's errors are about shapes and the node's are about storage, and a
/// record that does not parse is a storage problem from the node's side — this
/// node wrote it, so a failure here means corruption rather than a bad peer.
fn decode<T, F>(bytes: &[u8], parse: F) -> Result<T>
where
    F: Fn(&[u8]) -> maya_identity::Result<T>,
{
    parse(bytes).map_err(|error| NodeError::Decode(format!("identity: {error}")))
}
