//! RWA records in the state, and their place in the root.
//!
//! Invariant 25: one prefix in `RECORD_LAYERS`, one
//! [`StateLayer`](crate::state::proof::StateLayer), and the
//! undo journal covers everything written through `put_record`. The layer folds
//! only when non-empty, so a chain with no tokenised assets produces the root
//! it would have produced before the subsystem existed.
//!
//! | Prefix | Holds |
//! |---|---|
//! | `r:tok:<asset>` | the [`RwaToken`] |
//! | `r:cap:<asset><page>` | one [`CapTablePage`] |
//! | `r:leg:<asset><doc>` | a [`LegalAttestation`] |
//! | `r:dist:<asset><round>` | a [`RevenueDistribution`], so a round cannot be replayed |
//! | `r:elig:<asset><holder>` | a cached [`Eligibility`] |
//!
//! Separate records rather than one per asset, because they change at different
//! rates: a cap table page moves on every transfer, a token record never after
//! issuance. Folding them together would journal the token on every trade.

use maya_rwa::cap_table::CapTablePage;
use maya_rwa::token::{Eligibility, LegalAttestation, RevenueDistribution, RwaToken};
use maya_rwa::{Address, Digest};

use crate::error::{NodeError, Result};
use crate::state::db::{Overlay, StateDB};

/// Prefix shared by every record this subsystem owns.
pub const RWA_PREFIX: &[u8] = b"r:";

/// Prefix for a token.
pub(crate) const TOKEN_PREFIX: &[u8] = b"r:tok:";

/// Prefix for a cap table page.
pub(crate) const CAP_TABLE_PREFIX: &[u8] = b"r:cap:";

/// Prefix for a legal attestation.
pub(crate) const LEGAL_PREFIX: &[u8] = b"r:leg:";

/// Prefix for a settled distribution round.
pub(crate) const DISTRIBUTION_PREFIX: &[u8] = b"r:dst:";

/// Prefix for a cached eligibility.
pub(crate) const ELIGIBILITY_PREFIX: &[u8] = b"r:elg:";

// Every sub-prefix must sit under the one the fold knows about, or it is state
// outside the root.
const _: () = {
    assert!(TOKEN_PREFIX[0] == RWA_PREFIX[0] && TOKEN_PREFIX[1] == RWA_PREFIX[1]);
    assert!(CAP_TABLE_PREFIX[0] == RWA_PREFIX[0] && CAP_TABLE_PREFIX[1] == RWA_PREFIX[1]);
    assert!(LEGAL_PREFIX[0] == RWA_PREFIX[0] && LEGAL_PREFIX[1] == RWA_PREFIX[1]);
    assert!(DISTRIBUTION_PREFIX[0] == RWA_PREFIX[0] && DISTRIBUTION_PREFIX[1] == RWA_PREFIX[1]);
    assert!(ELIGIBILITY_PREFIX[0] == RWA_PREFIX[0] && ELIGIBILITY_PREFIX[1] == RWA_PREFIX[1]);
};

/// A key from a prefix and its parts.
fn key(prefix: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + parts.iter().map(|p| p.len()).sum::<usize>());
    out.extend_from_slice(prefix);
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

/// Storage key for a token.
#[must_use]
pub fn token_key(asset: &Digest) -> Vec<u8> {
    key(TOKEN_PREFIX, &[asset])
}

/// Storage key for one cap table page.
#[must_use]
pub fn cap_table_key(asset: &Digest, page: u32) -> Vec<u8> {
    key(CAP_TABLE_PREFIX, &[asset, &page.to_le_bytes()])
}

/// Storage key for a legal attestation.
#[must_use]
pub fn legal_key(asset: &Digest, document: &Digest) -> Vec<u8> {
    key(LEGAL_PREFIX, &[asset, document])
}

/// Storage key for a settled distribution round.
#[must_use]
pub fn distribution_key(asset: &Digest, round: u64) -> Vec<u8> {
    key(DISTRIBUTION_PREFIX, &[asset, &round.to_le_bytes()])
}

/// Storage key for a cached eligibility.
#[must_use]
pub fn eligibility_key(asset: &Digest, holder: &Address) -> Vec<u8> {
    key(ELIGIBILITY_PREFIX, &[asset, holder])
}

/// The asset id an issuer and label produce.
///
/// Derived rather than chosen, so an issuer cannot pick an id that collides
/// with somebody else's asset — and so the same issuer cannot register two
/// tokens under one label and leave holders unable to tell which they hold.
#[must_use]
pub fn derive_asset(issuer: &Address, label: &str) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"maya-rwa-asset-v1:");
    hasher.update(issuer);
    hasher.update(&(label.len() as u16).to_le_bytes());
    hasher.update(label.as_bytes());
    *hasher.finalize().as_bytes()
}

impl StateDB {
    /// A token, through the overlay.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub(crate) fn rwa_token(&self, overlay: &Overlay, asset: &Digest) -> Result<Option<RwaToken>> {
        self.record(overlay, &token_key(asset))?
            .map(|bytes| decode(&bytes, RwaToken::decode))
            .transpose()
    }

    /// A token from committed state alone.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub fn stored_rwa_token(&self, asset: &Digest) -> Result<Option<RwaToken>> {
        self.raw_get(&token_key(asset))?
            .map(|bytes| decode(&bytes, RwaToken::decode))
            .transpose()
    }

    /// One cap table page, through the overlay.
    ///
    /// An absent page is an **empty** page: a token whose holders all fit on
    /// page zero has written no page one, and a distribution reading it should
    /// see no holders rather than an error.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub(crate) fn cap_table_page(
        &self,
        overlay: &Overlay,
        asset: &Digest,
        page: u32,
    ) -> Result<CapTablePage> {
        match self.record(overlay, &cap_table_key(asset, page))? {
            Some(bytes) => decode(&bytes, CapTablePage::decode),
            None => Ok(CapTablePage::empty(*asset, page)),
        }
    }

    /// One cap table page from committed state.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub fn stored_cap_table_page(&self, asset: &Digest, page: u32) -> Result<CapTablePage> {
        match self.raw_get(&cap_table_key(asset, page))? {
            Some(bytes) => decode(&bytes, CapTablePage::decode),
            None => Ok(CapTablePage::empty(*asset, page)),
        }
    }

    /// A settled distribution round from committed state.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub fn stored_distribution(
        &self,
        asset: &Digest,
        round: u64,
    ) -> Result<Option<RevenueDistribution>> {
        self.raw_get(&distribution_key(asset, round))?
            .map(|bytes| decode(&bytes, RevenueDistribution::decode))
            .transpose()
    }

    /// A legal attestation from committed state.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub fn stored_legal_attestation(
        &self,
        asset: &Digest,
        document: &Digest,
    ) -> Result<Option<LegalAttestation>> {
        self.raw_get(&legal_key(asset, document))?
            .map(|bytes| decode(&bytes, LegalAttestation::decode))
            .transpose()
    }

    /// A holder's cached eligibility, through the overlay.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub(crate) fn eligibility(
        &self,
        overlay: &Overlay,
        asset: &Digest,
        holder: &Address,
    ) -> Result<Option<Eligibility>> {
        self.record(overlay, &eligibility_key(asset, holder))?
            .map(|bytes| decode(&bytes, Eligibility::decode))
            .transpose()
    }

    /// A holder's cached eligibility from committed state.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure.
    pub fn stored_eligibility(
        &self,
        asset: &Digest,
        holder: &Address,
    ) -> Result<Option<Eligibility>> {
        self.raw_get(&eligibility_key(asset, holder))?
            .map(|bytes| decode(&bytes, Eligibility::decode))
            .transpose()
    }
}

/// Turns an RWA decode failure into a node error.
fn decode<T, F>(bytes: &[u8], parse: F) -> Result<T>
where
    F: Fn(&[u8]) -> maya_rwa::Result<T>,
{
    parse(bytes).map_err(|error| NodeError::Decode(format!("rwa: {error}")))
}
