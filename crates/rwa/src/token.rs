//! The token, the rule that gates it, what a lawyer said, and what was paid.

use crate::error::{Error, Result};
use crate::{ADDRESS_BYTES, Address, DIGEST_BYTES, Digest};

/// The longest a human-readable label may be.
pub const MAX_LABEL_CHARS: usize = 64;

/// The longest a legal document reference may be.
pub const MAX_REFERENCE_CHARS: usize = 128;

/// A tokenised real-world asset.
///
/// The supply is fixed at issuance. A mintable RWA is a token whose issuer can
/// dilute every holder silently, and the chain has no way to tell that from an
/// issuance somebody agreed to — so if supply must change, it changes by
/// retiring this token and issuing another, which every holder can see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RwaToken {
    /// The asset identifier, derived by the node from the issuer and label.
    pub asset: Digest,
    /// Who issued it.
    pub issuer: Address,
    /// A human-readable label. Never a claim about a holder.
    pub label: String,
    /// Total units, fixed at issuance.
    pub total_units: u64,
    /// The rule a transfer must satisfy, if the issuer set one.
    pub rule: Option<TransferRule>,
}

/// What a transfer must satisfy.
///
/// An issuer **selects** a rule; it does not supply one. Invariant 13: no
/// governed value is a program, and native code is never fetched from chain
/// state and run. So this names a claim schema, a predicate the binary already
/// knows how to evaluate, and the credential issuers whose word counts — three
/// values, no logic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferRule {
    /// Hash of the claim schema a holder must have a credential for.
    pub schema: Digest,
    /// What must hold of the claim's value.
    pub predicate: RulePredicate,
    /// Credential issuers this asset's issuer trusts, ascending.
    ///
    /// An empty list is a rule nobody can satisfy, and it is refused at
    /// construction: a token that cannot be transferred by anyone is one whose
    /// issuer meant something else.
    pub trusted_issuers: Vec<Address>,
    /// Blocks an eligibility proof stays good for.
    ///
    /// Height, never a timestamp — invariant 9. A miner may write any
    /// `header.timestamp`, so a freshness rule measured in time would read as
    /// safety and provide none.
    pub eligibility_blocks: u64,
}

/// The three shapes a rule may take.
///
/// Exactly the shapes `maya_zk_stark::credential::Predicate` proves, because a
/// rule the disclosure circuit cannot evaluate is a rule no holder can satisfy
/// privately — and one they must therefore satisfy by revealing the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulePredicate {
    /// The claim value is at least this. `age >= 18`.
    AtLeast(u64),
    /// The claim value is at most this.
    AtMost(u64),
    /// The claim value is exactly this. A jurisdiction code, an accreditation.
    EqualTo(u64),
}

impl RulePredicate {
    /// The wire tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::AtLeast(_) => 0,
            Self::AtMost(_) => 1,
            Self::EqualTo(_) => 2,
        }
    }

    /// The bound.
    #[must_use]
    pub const fn bound(self) -> u64 {
        match self {
            Self::AtLeast(bound) | Self::AtMost(bound) | Self::EqualTo(bound) => bound,
        }
    }

    /// The predicate a tag and bound name.
    #[must_use]
    pub const fn from_parts(tag: u8, bound: u64) -> Option<Self> {
        match tag {
            0 => Some(Self::AtLeast(bound)),
            1 => Some(Self::AtMost(bound)),
            2 => Some(Self::EqualTo(bound)),
            _ => None,
        }
    }
}

impl TransferRule {
    /// A rule.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for an empty trusted-issuer list — a rule
    /// nobody can satisfy — or a zero validity window, and
    /// [`Error::Oversized`] for more trusted issuers than [`MAX_TRUSTED_ISSUERS`].
    pub fn new(
        schema: Digest,
        predicate: RulePredicate,
        mut trusted_issuers: Vec<Address>,
        eligibility_blocks: u64,
    ) -> Result<Self> {
        if trusted_issuers.is_empty() {
            return Err(Error::Invalid {
                field: "trustedIssuers",
                reason: "a rule with no trusted issuer can be satisfied by nobody".to_owned(),
            });
        }
        if trusted_issuers.len() > MAX_TRUSTED_ISSUERS {
            return Err(Error::Oversized {
                what: "trusted issuers",
                found: trusted_issuers.len(),
                limit: MAX_TRUSTED_ISSUERS,
            });
        }
        if eligibility_blocks == 0 {
            return Err(Error::Invalid {
                field: "eligibilityBlocks",
                reason: "a proof good for zero blocks is good at no height".to_owned(),
            });
        }
        trusted_issuers.sort_unstable();
        trusted_issuers.dedup();
        Ok(Self {
            schema,
            predicate,
            trusted_issuers,
            eligibility_blocks,
        })
    }

    /// Whether this rule trusts a credential issuer.
    #[must_use]
    pub fn trusts(&self, issuer: &Address) -> bool {
        self.trusted_issuers.binary_search(issuer).is_ok()
    }
}

/// The most credential issuers one rule may trust.
pub const MAX_TRUSTED_ISSUERS: usize = 16;

/// A cached "this holder cleared the rule" record.
///
/// Written when a holder's proof is verified, read on every transfer. The
/// alternative — a STARK disclosure proof inside each transfer, hundreds of
/// kilobytes apiece — would put gigabytes into a ten-thousand-transfer block
/// for every node to download and verify. So the proof is checked once,
/// in its own transaction, and the transfer path reads a record.
///
/// It expires. An eligibility that never went stale would be a jurisdiction
/// check answered by a proof from years ago.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Eligibility {
    /// Who cleared.
    pub holder: Address,
    /// For which asset.
    pub asset: Digest,
    /// The height at which the proof was verified.
    pub verified_at: u64,
    /// First height at which it is no longer good.
    pub expires_at: u64,
}

impl Eligibility {
    /// Whether this is still good at `height`.
    #[must_use]
    pub const fn is_live(&self, height: u64) -> bool {
        height >= self.verified_at && height < self.expires_at
    }
}

/// What a lawyer said about the asset behind the token.
///
/// A hash and a reference, never a document. The chain is permanent and public,
/// and a prospectus on it is a prospectus that cannot be corrected — and one
/// that likely names people. The reference says where the document lives; the
/// hash says which one was meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegalAttestation {
    /// Which asset.
    pub asset: Digest,
    /// Who attested. A DID address.
    pub attestor: Address,
    /// Hash of the document.
    pub document: Digest,
    /// Where to find it. A URI, bounded and printable.
    pub reference: String,
    /// Height at which it was recorded.
    pub recorded_at: u64,
}

/// What one revenue distribution paid.
///
/// Recorded so a holder can reconcile, and so a second distribution cannot
/// replay the first: the round number is part of the key, and a round already
/// written is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevenueDistribution {
    /// Which asset.
    pub asset: Digest,
    /// Which round.
    pub round: u64,
    /// Total paid out, which equals what was debited from the issuer.
    pub total: u64,
    /// How many holders were paid.
    pub holders: u32,
    /// Height at which it settled.
    pub settled_at: u64,
}

// ---------------------------------------------------------------------------
// encoding
// ---------------------------------------------------------------------------

impl RwaToken {
    /// A token.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for an empty or non-printable label or a zero
    /// supply, and [`Error::Oversized`] for a label past [`MAX_LABEL_CHARS`].
    pub fn new(
        asset: Digest,
        issuer: Address,
        label: &str,
        total_units: u64,
        rule: Option<TransferRule>,
    ) -> Result<Self> {
        printable(label, "label", MAX_LABEL_CHARS)?;
        if total_units == 0 {
            return Err(Error::Invalid {
                field: "totalUnits",
                reason: "a token with no units is a token nobody can hold".to_owned(),
            });
        }
        Ok(Self {
            asset,
            issuer,
            label: label.to_owned(),
            total_units,
            rule,
        })
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(&self.asset);
        out.extend_from_slice(&self.issuer);
        out.extend_from_slice(&self.total_units.to_le_bytes());
        out.push(self.label.len() as u8);
        out.extend_from_slice(self.label.as_bytes());
        match &self.rule {
            None => out.push(0),
            Some(rule) => {
                out.push(1);
                out.extend_from_slice(&rule.schema);
                out.push(rule.predicate.tag());
                out.extend_from_slice(&rule.predicate.bound().to_le_bytes());
                out.extend_from_slice(&rule.eligibility_blocks.to_le_bytes());
                out.push(rule.trusted_issuers.len() as u8);
                for issuer in &rule.trusted_issuers {
                    out.extend_from_slice(issuer);
                }
            }
        }
        out
    }

    /// Reads a token.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a truncated record, trailing bytes, or
    /// a predicate tag that names nothing, and whatever the constructors return
    /// for a value outside its bounds.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(bytes, "rwa token");
        let asset: Digest = cursor.take(DIGEST_BYTES)?.try_into().expect("sized");
        let issuer: Address = cursor.take(ADDRESS_BYTES)?.try_into().expect("sized");
        let total_units = u64::from_le_bytes(cursor.take(8)?.try_into().expect("sized"));
        let label_len = cursor.take(1)?[0] as usize;
        let label = text(cursor.take(label_len)?)?;

        let rule = match cursor.take(1)?[0] {
            0 => None,
            1 => {
                let schema: Digest = cursor.take(DIGEST_BYTES)?.try_into().expect("sized");
                let tag = cursor.take(1)?[0];
                let bound = u64::from_le_bytes(cursor.take(8)?.try_into().expect("sized"));
                let predicate = RulePredicate::from_parts(tag, bound).ok_or(Error::Malformed {
                    what: "rwa token",
                    reason: format!("predicate tag {tag} names nothing"),
                })?;
                let eligibility_blocks =
                    u64::from_le_bytes(cursor.take(8)?.try_into().expect("sized"));
                let count = cursor.take(1)?[0] as usize;
                let mut trusted = Vec::with_capacity(count.min(MAX_TRUSTED_ISSUERS));
                for _ in 0..count {
                    trusted.push(cursor.take(ADDRESS_BYTES)?.try_into().expect("sized"));
                }
                Some(TransferRule::new(
                    schema,
                    predicate,
                    trusted,
                    eligibility_blocks,
                )?)
            }
            other => {
                return Err(Error::Malformed {
                    what: "rwa token",
                    reason: format!("rule flag is {other}"),
                });
            }
        };
        cursor.finish()?;
        Self::new(asset, issuer, &label, total_units, rule)
    }
}

impl LegalAttestation {
    /// An attestation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] or [`Error::Oversized`] for a reference
    /// outside its bounds.
    pub fn new(
        asset: Digest,
        attestor: Address,
        document: Digest,
        reference: &str,
        recorded_at: u64,
    ) -> Result<Self> {
        printable(reference, "reference", MAX_REFERENCE_CHARS)?;
        Ok(Self {
            asset,
            attestor,
            document,
            reference: reference.to_owned(),
            recorded_at,
        })
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(&self.asset);
        out.extend_from_slice(&self.attestor);
        out.extend_from_slice(&self.document);
        out.extend_from_slice(&self.recorded_at.to_le_bytes());
        out.push(self.reference.len() as u8);
        out.extend_from_slice(self.reference.as_bytes());
        out
    }

    /// Reads an attestation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a truncated record or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(bytes, "legal attestation");
        let asset: Digest = cursor.take(DIGEST_BYTES)?.try_into().expect("sized");
        let attestor: Address = cursor.take(ADDRESS_BYTES)?.try_into().expect("sized");
        let document: Digest = cursor.take(DIGEST_BYTES)?.try_into().expect("sized");
        let recorded_at = u64::from_le_bytes(cursor.take(8)?.try_into().expect("sized"));
        let reference_len = cursor.take(1)?[0] as usize;
        let reference = text(cursor.take(reference_len)?)?;
        cursor.finish()?;
        Self::new(asset, attestor, document, &reference, recorded_at)
    }
}

impl RevenueDistribution {
    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(DIGEST_BYTES + 28);
        out.extend_from_slice(&self.asset);
        out.extend_from_slice(&self.round.to_le_bytes());
        out.extend_from_slice(&self.total.to_le_bytes());
        out.extend_from_slice(&self.holders.to_le_bytes());
        out.extend_from_slice(&self.settled_at.to_le_bytes());
        out
    }

    /// Reads a distribution.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for anything but exactly the fixed size.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const SIZE: usize = DIGEST_BYTES + 8 + 8 + 4 + 8;
        if bytes.len() != SIZE {
            return Err(Error::Malformed {
                what: "revenue distribution",
                reason: format!("{} bytes; the record is exactly {SIZE}", bytes.len()),
            });
        }
        let mut at = DIGEST_BYTES;
        let asset: Digest = bytes[..DIGEST_BYTES].try_into().expect("sized");
        let round = u64::from_le_bytes(bytes[at..at + 8].try_into().expect("sized"));
        at += 8;
        let total = u64::from_le_bytes(bytes[at..at + 8].try_into().expect("sized"));
        at += 8;
        let holders = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("sized"));
        at += 4;
        let settled_at = u64::from_le_bytes(bytes[at..at + 8].try_into().expect("sized"));
        Ok(Self {
            asset,
            round,
            total,
            holders,
            settled_at,
        })
    }
}

impl Eligibility {
    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ADDRESS_BYTES + DIGEST_BYTES + 16);
        out.extend_from_slice(&self.holder);
        out.extend_from_slice(&self.asset);
        out.extend_from_slice(&self.verified_at.to_le_bytes());
        out.extend_from_slice(&self.expires_at.to_le_bytes());
        out
    }

    /// Reads an eligibility record.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for anything but exactly the fixed size.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const SIZE: usize = ADDRESS_BYTES + DIGEST_BYTES + 16;
        if bytes.len() != SIZE {
            return Err(Error::Malformed {
                what: "eligibility",
                reason: format!("{} bytes; the record is exactly {SIZE}", bytes.len()),
            });
        }
        Ok(Self {
            holder: bytes[..ADDRESS_BYTES].try_into().expect("sized"),
            asset: bytes[ADDRESS_BYTES..ADDRESS_BYTES + DIGEST_BYTES]
                .try_into()
                .expect("sized"),
            verified_at: u64::from_le_bytes(
                bytes[ADDRESS_BYTES + DIGEST_BYTES..ADDRESS_BYTES + DIGEST_BYTES + 8]
                    .try_into()
                    .expect("sized"),
            ),
            expires_at: u64::from_le_bytes(
                bytes[ADDRESS_BYTES + DIGEST_BYTES + 8..]
                    .try_into()
                    .expect("sized"),
            ),
        })
    }
}

/// A non-empty, bounded, printable-ASCII field.
fn printable(text: &str, field: &'static str, limit: usize) -> Result<()> {
    if text.is_empty() {
        return Err(Error::Invalid {
            field,
            reason: "is empty".to_owned(),
        });
    }
    if text.len() > limit {
        return Err(Error::Oversized {
            what: field,
            found: text.len(),
            limit,
        });
    }
    if !text.bytes().all(|byte| (0x20..0x7f).contains(&byte)) {
        return Err(Error::Invalid {
            field,
            reason: "is not printable ASCII".to_owned(),
        });
    }
    Ok(())
}

/// UTF-8 text, or a refusal.
fn text(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| Error::Malformed {
        what: "rwa record",
        reason: "a text field is not UTF-8".to_owned(),
    })
}

/// A bounds-checked reader.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, at: 0, what }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(count).ok_or(Error::Malformed {
            what: self.what,
            reason: "a length field overflows".to_owned(),
        })?;
        let slice = self.bytes.get(self.at..end).ok_or(Error::Malformed {
            what: self.what,
            reason: format!("wanted {count} bytes at {} and the record ends", self.at),
        })?;
        self.at = end;
        Ok(slice)
    }

    fn finish(&self) -> Result<()> {
        if self.at == self.bytes.len() {
            return Ok(());
        }
        Err(Error::Malformed {
            what: self.what,
            reason: format!("{} trailing bytes", self.bytes.len() - self.at),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn rule() -> TransferRule {
        TransferRule::new(
            [4; DIGEST_BYTES],
            RulePredicate::EqualTo(826),
            vec![[2; ADDRESS_BYTES], [1; ADDRESS_BYTES]],
            1_000,
        )
        .expect("rule")
    }

    fn token(rule: Option<TransferRule>) -> RwaToken {
        RwaToken::new(
            [9; DIGEST_BYTES],
            [7; ADDRESS_BYTES],
            "Building A",
            1_000_000,
            rule,
        )
        .expect("token")
    }

    #[test]
    fn a_token_survives_its_wire_format_with_and_without_a_rule() {
        for candidate in [token(None), token(Some(rule()))] {
            assert_eq!(
                RwaToken::decode(&candidate.encode()).expect("decode"),
                candidate
            );
        }
    }

    #[test]
    fn a_rule_with_no_trusted_issuer_is_refused() {
        // It could be satisfied by nobody, so the token could be transferred by
        // nobody — which is not what an issuer setting a rule meant.
        assert!(
            TransferRule::new([0; DIGEST_BYTES], RulePredicate::AtLeast(1), Vec::new(), 10)
                .is_err()
        );
    }

    #[test]
    fn a_zero_validity_window_is_refused() {
        assert!(
            TransferRule::new(
                [0; DIGEST_BYTES],
                RulePredicate::AtLeast(1),
                vec![[1; ADDRESS_BYTES]],
                0
            )
            .is_err()
        );
    }

    #[test]
    fn trusted_issuers_are_sorted_and_deduplicated() {
        // The encoding has to be canonical: two rules naming the same issuers
        // in different orders would be two records under one state root.
        let first = TransferRule::new(
            [0; DIGEST_BYTES],
            RulePredicate::AtLeast(1),
            vec![[3; ADDRESS_BYTES], [1; ADDRESS_BYTES], [3; ADDRESS_BYTES]],
            10,
        )
        .expect("rule");
        assert_eq!(
            first.trusted_issuers,
            vec![[1; ADDRESS_BYTES], [3; ADDRESS_BYTES]]
        );
        assert!(first.trusts(&[1; ADDRESS_BYTES]));
        assert!(!first.trusts(&[2; ADDRESS_BYTES]));
    }

    #[test]
    fn every_predicate_round_trips_through_its_parts() {
        for predicate in [
            RulePredicate::AtLeast(18),
            RulePredicate::AtMost(65),
            RulePredicate::EqualTo(826),
        ] {
            assert_eq!(
                RulePredicate::from_parts(predicate.tag(), predicate.bound()),
                Some(predicate)
            );
        }
        assert_eq!(RulePredicate::from_parts(3, 0), None);
    }

    #[test]
    fn a_token_with_no_units_is_refused() {
        assert!(RwaToken::new([0; DIGEST_BYTES], [0; ADDRESS_BYTES], "x", 0, None).is_err());
    }

    #[test]
    fn a_label_is_bounded_and_printable() {
        for label in ["", "a\nb", &"x".repeat(MAX_LABEL_CHARS + 1)] {
            assert!(
                RwaToken::new([0; DIGEST_BYTES], [0; ADDRESS_BYTES], label, 1, None).is_err(),
                "{label:?} accepted"
            );
        }
    }

    #[test]
    fn a_truncated_or_padded_token_is_refused() {
        let encoded = token(Some(rule())).encode();
        for len in 0..encoded.len() {
            assert!(RwaToken::decode(&encoded[..len]).is_err(), "{len}");
        }
        let mut padded = encoded.clone();
        padded.push(0);
        assert!(RwaToken::decode(&padded).is_err());
    }

    #[test]
    fn eligibility_is_a_window_and_not_a_flag() {
        let eligible = Eligibility {
            holder: [1; ADDRESS_BYTES],
            asset: [2; DIGEST_BYTES],
            verified_at: 100,
            expires_at: 200,
        };
        assert!(!eligible.is_live(99));
        assert!(eligible.is_live(100));
        assert!(eligible.is_live(199));
        assert!(!eligible.is_live(200));
        assert_eq!(
            Eligibility::decode(&eligible.encode()).expect("decode"),
            eligible
        );
    }

    #[test]
    fn the_other_records_survive_their_wire_formats() {
        let attestation = LegalAttestation::new(
            [1; DIGEST_BYTES],
            [2; ADDRESS_BYTES],
            [3; DIGEST_BYTES],
            "https://registry.test/deed/4711",
            88,
        )
        .expect("attestation");
        assert_eq!(
            LegalAttestation::decode(&attestation.encode()).expect("decode"),
            attestation
        );

        let distribution = RevenueDistribution {
            asset: [1; DIGEST_BYTES],
            round: 3,
            total: 1_000_000,
            holders: 10_000,
            settled_at: 500,
        };
        assert_eq!(
            RevenueDistribution::decode(&distribution.encode()).expect("decode"),
            distribution
        );
    }

    #[test]
    fn a_legal_attestation_carries_a_hash_and_never_a_document() {
        // The chain is permanent and public. A prospectus on it cannot be
        // corrected, and it likely names people.
        let attestation = LegalAttestation::new(
            [1; DIGEST_BYTES],
            [2; ADDRESS_BYTES],
            [3; DIGEST_BYTES],
            "ipfs://bafy",
            1,
        )
        .expect("attestation");
        assert_eq!(
            attestation.encode().len(),
            DIGEST_BYTES + ADDRESS_BYTES + DIGEST_BYTES + 8 + 1 + attestation.reference.len()
        );
    }
}
