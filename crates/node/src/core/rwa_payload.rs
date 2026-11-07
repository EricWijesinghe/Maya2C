//! The wire forms of the RWA transitions.
//!
//! Five actions, and the split between them is about who bears the cost of a
//! failure. Issuance, distribution and attestation are the issuer's own
//! transactions, so a bad one is an error and fails only that transaction's
//! block-worth of intent. A **`DvP` is not**: it names a counterparty, and an
//! error there would let anyone void a block by submitting a swap they know
//! cannot settle. See `crate::state::rwa_exec`.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Bytes in an asset id or a document hash.
pub const DIGEST_LEN: usize = 32;

/// The longest label a token may carry.
pub const MAX_LABEL_CHARS: usize = 64;

/// The longest reference a legal attestation may carry.
pub const MAX_REFERENCE_CHARS: usize = 128;

/// The most credential issuers a rule may trust.
pub const MAX_TRUSTED_ISSUERS: usize = 16;

/// Issue a token, seating its whole supply with the sender.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueRwa {
    /// A human-readable label. Never a claim about a holder.
    pub label: String,
    /// Total units, fixed at issuance.
    pub total_units: u64,
    /// The transfer rule, if the issuer sets one: schema, predicate tag,
    /// bound, validity window, trusted credential issuers.
    pub rule: Option<RulePayload>,
}

/// A transfer rule on the wire.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RulePayload {
    /// Hash of the claim schema.
    pub schema: [u8; DIGEST_LEN],
    /// Which predicate: 0 at-least, 1 at-most, 2 equal-to.
    pub predicate_tag: u8,
    /// The predicate's bound.
    pub bound: u64,
    /// Blocks an eligibility proof stays good for.
    pub eligibility_blocks: u64,
    /// Credential issuers the asset issuer trusts.
    pub trusted_issuers: Vec<[u8; DIGEST_LEN]>,
}

/// Swap native coin for RWA units, atomically or not at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettleDvp {
    /// Which asset.
    pub asset: [u8; DIGEST_LEN],
    /// Who is selling the units. The sender is the buyer.
    pub seller: [u8; DIGEST_LEN],
    /// How many units.
    pub units: u64,
    /// What the buyer pays, in native base units.
    pub price: primitive_types::U256,
    /// Which cap table page the seller's holding is on.
    pub page: u32,
}

/// Record that the sender cleared an asset's rule.
///
/// The disclosure proof rides here and is verified once; the transfer path then
/// reads the cached record rather than pairing again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordEligibility {
    /// Which asset.
    pub asset: [u8; DIGEST_LEN],
}

/// Pay revenue across every holder on the named pages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DistributeRevenue {
    /// Which asset.
    pub asset: [u8; DIGEST_LEN],
    /// Which round. A round already settled is refused, so a distribution
    /// cannot be replayed.
    pub round: u64,
    /// Total to pay, in native base units.
    pub total: primitive_types::U256,
    /// How many cap table pages to fan out over.
    pub pages: u32,
}

/// Record a legal attestation about an asset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttestLegal {
    /// Which asset.
    pub asset: [u8; DIGEST_LEN],
    /// Hash of the document. Never the document.
    pub document: [u8; DIGEST_LEN],
    /// Where to find it.
    pub reference: String,
}

impl IssueRwa {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.total_units.to_le_bytes());
        buf.push(self.label.len() as u8);
        buf.extend_from_slice(self.label.as_bytes());
        match &self.rule {
            None => buf.push(0),
            Some(rule) => {
                buf.push(1);
                buf.extend_from_slice(&rule.schema);
                buf.push(rule.predicate_tag);
                buf.extend_from_slice(&rule.bound.to_le_bytes());
                buf.extend_from_slice(&rule.eligibility_blocks.to_le_bytes());
                buf.push(rule.trusted_issuers.len() as u8);
                for issuer in &rule.trusted_issuers {
                    buf.extend_from_slice(issuer);
                }
            }
        }
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload, a label or issuer
    /// list past its bound, a rule flag outside zero and one, or text that is
    /// not UTF-8.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let total_units = reader.read_u64()?;
        let label_len = reader.read_u8()? as usize;
        if label_len > MAX_LABEL_CHARS {
            return Err(NodeError::Decode(format!(
                "rwa label is {label_len} bytes, limit {MAX_LABEL_CHARS}"
            )));
        }
        let label = text(reader.read_slice(label_len)?)?;

        let rule = match reader.read_u8()? {
            0 => None,
            1 => {
                let schema = reader.read_array::<DIGEST_LEN>()?;
                let predicate_tag = reader.read_u8()?;
                let bound = reader.read_u64()?;
                let eligibility_blocks = reader.read_u64()?;
                let count = reader.read_u8()? as usize;
                if count > MAX_TRUSTED_ISSUERS {
                    return Err(NodeError::Decode(format!(
                        "rwa rule trusts {count} issuers, limit {MAX_TRUSTED_ISSUERS}"
                    )));
                }
                let mut trusted_issuers = Vec::with_capacity(count);
                for _ in 0..count {
                    trusted_issuers.push(reader.read_array::<DIGEST_LEN>()?);
                }
                Some(RulePayload {
                    schema,
                    predicate_tag,
                    bound,
                    eligibility_blocks,
                    trusted_issuers,
                })
            }
            other => {
                return Err(NodeError::Decode(format!("rwa rule flag is {other}")));
            }
        };
        Ok(Self {
            label,
            total_units,
            rule,
        })
    }
}

impl SettleDvp {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.asset);
        buf.extend_from_slice(&self.seller);
        buf.extend_from_slice(&self.units.to_le_bytes());
        crate::core::codec::put_amount(buf, &self.price);
        buf.extend_from_slice(&self.page.to_le_bytes());
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            asset: reader.read_array::<DIGEST_LEN>()?,
            seller: reader.read_array::<DIGEST_LEN>()?,
            units: reader.read_u64()?,
            price: reader.read_amount()?,
            page: reader.read_u32()?,
        })
    }
}

impl RecordEligibility {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.asset);
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            asset: reader.read_array::<DIGEST_LEN>()?,
        })
    }
}

impl DistributeRevenue {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.asset);
        buf.extend_from_slice(&self.round.to_le_bytes());
        crate::core::codec::put_amount(buf, &self.total);
        buf.extend_from_slice(&self.pages.to_le_bytes());
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            asset: reader.read_array::<DIGEST_LEN>()?,
            round: reader.read_u64()?,
            total: reader.read_amount()?,
            pages: reader.read_u32()?,
        })
    }
}

impl AttestLegal {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.asset);
        buf.extend_from_slice(&self.document);
        buf.push(self.reference.len() as u8);
        buf.extend_from_slice(self.reference.as_bytes());
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload, a reference past
    /// [`MAX_REFERENCE_CHARS`], or text that is not UTF-8.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let asset = reader.read_array::<DIGEST_LEN>()?;
        let document = reader.read_array::<DIGEST_LEN>()?;
        let length = reader.read_u8()? as usize;
        if length > MAX_REFERENCE_CHARS {
            return Err(NodeError::Decode(format!(
                "rwa reference is {length} bytes, limit {MAX_REFERENCE_CHARS}"
            )));
        }
        Ok(Self {
            asset,
            document,
            reference: text(reader.read_slice(length)?)?,
        })
    }
}

/// UTF-8 text, or a decode failure.
fn text(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| NodeError::Decode("rwa payload text is not UTF-8".to_owned()))
}

/// First height at which an issuance must list its credential issuers
/// strictly ascending. `u64::MAX` until a height is written down: until then
/// the executed behaviour is the frozen binary's, which sorts and dedups.
pub const RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT: u64 = u64::MAX;

/// Refuses an unsorted or duplicated issuer list at `height` once it reaches
/// [`RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT`]. Before it, the list is accepted
/// and normalised as it always was.
///
/// # Errors
///
/// Returns [`NodeError::Decode`] at or above the activation height when the
/// list is not strictly ascending.
pub fn check_issuer_order(trusted_issuers: &[[u8; DIGEST_LEN]], height: u64) -> Result<()> {
    check_issuer_order_from(
        trusted_issuers,
        height,
        RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT,
    )
}

/// [`check_issuer_order`] with the activation height as an argument: the
/// consensus path passes the height the upgrade schedule gave the block
/// context (ADR-081).
///
/// # Errors
///
/// As [`check_issuer_order`], against `activation`.
pub fn check_issuer_order_from(
    trusted_issuers: &[[u8; DIGEST_LEN]],
    height: u64,
    activation: u64,
) -> Result<()> {
    if height < activation || trusted_issuers.windows(2).all(|pair| pair[0] < pair[1]) {
        return Ok(());
    }
    Err(NodeError::Decode(
        "rwa rule issuers are not strictly ascending".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const UNSORTED: [[u8; DIGEST_LEN]; 2] = [[2; DIGEST_LEN], [1; DIGEST_LEN]];
    const SORTED: [[u8; DIGEST_LEN]; 2] = [[1; DIGEST_LEN], [2; DIGEST_LEN]];
    const DUPLICATE: [[u8; DIGEST_LEN]; 2] = [[1; DIGEST_LEN], [1; DIGEST_LEN]];

    #[test]
    fn with_the_height_unwritten_unsorted_issuers_are_accepted_at_every_height() {
        assert_eq!(RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT, u64::MAX);
        assert!(check_issuer_order(&UNSORTED, 0).is_ok());
        assert!(check_issuer_order(&DUPLICATE, u64::MAX - 1).is_ok());
    }

    #[test]
    fn one_block_below_the_activation_height_the_old_rule_holds() {
        assert!(check_issuer_order_from(&UNSORTED, 99, 100).is_ok());
        assert!(check_issuer_order_from(&DUPLICATE, 99, 100).is_ok());
    }

    #[test]
    fn at_the_activation_height_unsorted_or_duplicate_issuers_are_refused() {
        assert!(check_issuer_order_from(&UNSORTED, 100, 100).is_err());
        assert!(check_issuer_order_from(&DUPLICATE, 101, 100).is_err());
        assert!(check_issuer_order_from(&SORTED, 100, 100).is_ok());
    }
}
