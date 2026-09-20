//! Threat-intel records: one indicator per author, one marker per piece of
//! evidence.
//!
//! Every key is under `t:`, one layer of the state root (invariant 25):
//!
//! - `t:in:<author>` — [`ThreatIndicator::encode`];
//! - `t:ev:<evidence id>` — the height that recorded it, little-endian.
//!
//! The marker is what makes a second copy of the same evidence a no-op. Its id
//! covers the offence kind and the signed bytes but not the signature, so an
//! author who re-signs one message has produced the same evidence, not more.

use maya_threat_intel::{AttackAttestation, Author, ThreatIndicator};

use crate::error::{NodeError, Result};
use crate::state::db::{Overlay, StateDB};

/// Prefix of every threat-intel record.
pub const THREAT_PREFIX: &[u8] = b"t:";

/// Prefix of indicators.
pub(crate) const INDICATOR_PREFIX: &[u8] = b"t:in:";

/// Prefix of evidence markers.
pub(crate) const EVIDENCE_PREFIX: &[u8] = b"t:ev:";

const _: () =
    assert!(INDICATOR_PREFIX[0] == THREAT_PREFIX[0] && INDICATOR_PREFIX[1] == THREAT_PREFIX[1]);
const _: () =
    assert!(EVIDENCE_PREFIX[0] == THREAT_PREFIX[0] && EVIDENCE_PREFIX[1] == THREAT_PREFIX[1]);

/// The evidence topics are the node's gossip topics, or no signature captured
/// here would ever verify.
const _: () = assert!(bytes_eq(
    maya_threat_intel::evidence::TXS_TOPIC.as_bytes(),
    crate::network::topics::TXS_TOPIC.as_bytes()
));
const _: () = assert!(bytes_eq(
    maya_threat_intel::evidence::BLOCKS_TOPIC.as_bytes(),
    crate::network::topics::BLOCKS_TOPIC.as_bytes()
));

/// BLAKE3 derive-key domain for evidence ids. Consensus.
const EVIDENCE_ID_DOMAIN: &str = "maya2c threat evidence id v1";

/// Storage key of an author's indicator.
#[must_use]
pub fn indicator_key(author: &Author) -> Vec<u8> {
    [INDICATOR_PREFIX, author.as_slice()].concat()
}

/// Storage key of an evidence marker.
#[must_use]
pub fn evidence_key(id: &[u8; 32]) -> Vec<u8> {
    [EVIDENCE_PREFIX, id.as_slice()].concat()
}

/// The id two copies of the same evidence share.
#[must_use]
pub fn derive_evidence_id(attestation: &AttackAttestation) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(EVIDENCE_ID_DOMAIN);
    hasher.update(&[attestation.kind.tag()]);
    hasher.update(&attestation.signed_bytes());
    *hasher.finalize().as_bytes()
}

impl StateDB {
    /// An author's indicator through the overlay.
    pub(crate) fn threat_indicator(
        &self,
        overlay: &Overlay,
        author: &Author,
    ) -> Result<Option<ThreatIndicator>> {
        self.record(overlay, &indicator_key(author))?
            .map(|bytes| decode(&bytes))
            .transpose()
    }

    /// An author's committed indicator.
    ///
    /// # Errors
    ///
    /// A read failure, or a stored record that does not decode.
    pub fn stored_threat_indicator(&self, author: &Author) -> Result<Option<ThreatIndicator>> {
        self.raw_get(&indicator_key(author))?
            .map(|bytes| decode(&bytes))
            .transpose()
    }

    /// Every committed indicator, in author order. Lifted ones included: the
    /// caller decides at which height to ask whether each is still active.
    ///
    /// # Errors
    ///
    /// As [`StateDB::stored_threat_indicator`].
    pub fn stored_threat_indicators(&self) -> Result<Vec<(Author, ThreatIndicator)>> {
        self.scan_prefix(INDICATOR_PREFIX)?
            .into_iter()
            .map(|(key, value)| {
                let author = key
                    .strip_prefix(INDICATOR_PREFIX)
                    .and_then(|rest| Author::try_from(rest).ok())
                    .ok_or_else(|| NodeError::Decode("threat indicator key".to_string()))?;
                Ok((author, decode(&value)?))
            })
            .collect()
    }

    /// What every committed indicator obliges a node to do at `height`: one
    /// entry per author still quarantined, none for lifted ones.
    ///
    /// # Errors
    ///
    /// As [`StateDB::stored_threat_indicators`].
    pub fn active_mitigations(
        &self,
        height: u64,
    ) -> Result<Vec<maya_threat_intel::AutomatedMitigation>> {
        Ok(self
            .stored_threat_indicators()?
            .iter()
            .filter_map(|(author, indicator)| {
                maya_threat_intel::mitigation(author, indicator, height)
            })
            .collect())
    }
}

fn decode(bytes: &[u8]) -> Result<ThreatIndicator> {
    ThreatIndicator::decode(bytes).map_err(|e| NodeError::Decode(format!("threat indicator: {e}")))
}

const fn bytes_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}
