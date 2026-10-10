//! Attestations and revocation: what an issuer puts on chain, and what it
//! never puts there.
//!
//! ## An attestation carries a root, and nothing else
//!
//! A [`CryptographicAttestation`] is an issuer, a schema, and a **Merkle root**
//! over commitments to `(subject, claim, value)`. It carries no claim, no
//! value, no subject list and no cleartext of any kind. There is no field for
//! one.
//!
//! That is the property the whole design rests on. A chain is permanent and
//! public, and a claim written to it once is written forever — no later fix
//! reaches it. So the type is shaped so that putting a claim on chain is not a
//! mistake somebody could make; it is not expressible.
//!
//! ## Why the issuer's signature never enters a circuit
//!
//! The usual construction for selective disclosure verifies the issuer's
//! signature *inside* the zero-knowledge circuit, so the verifier learns the
//! issuer attested something without learning what. `Maya2C`'s signature is a
//! hybrid pair, and verifying ML-DSA-65 in R1CS means an NTT over a 23-bit
//! prime, 256-coefficient polynomials, a 6×5 matrix and SHAKE256 — on the order
//! of 10^7 to 10^8 constraints, against a joinsplit circuit that is a few tens
//! of thousands. No proof system in this tree makes that affordable per
//! presentation.
//!
//! BBS+ is the industry answer and is pairing-based, which is not post-quantum
//! and so defeats the premise.
//!
//! So the two concerns are split. The issuer's attestation is an ordinary
//! transaction: the hybrid signature is verified **natively, by consensus**,
//! and the root becomes consensus state. The holder's proof then only has to
//! show membership under a root the chain already vouches for — no signature in
//! the circuit at all.
//!
//! The holder's proof is a Plonky3 STARK (`maya_zk_stark::credential`,
//! ADR-008): hash-based, no setup, post-quantum like the attestation. It
//! replaced a Groth16 presentation over BLS12-381 that a quantum adversary
//! could have forged.
//!
//! ## Revocation is a bitmap, and the bitmap has a root
//!
//! An issuer publishes a page of bits; bit `i` set means credential `i` is
//! revoked. Reading one bit from committed state is the cheap path, and it is
//! the right one when the index is not sensitive.
//!
//! It is the wrong one when it is. A holder who proves `age >= 18` in zero
//! knowledge and is then asked for credential index 4,721 has just linked the
//! presentation to a credential — and, across two verifiers, to itself. So the
//! page also has a Merkle root, and the private path proves *"the bit at my
//! index is zero"* in the same circuit as the rest. Same state, two ways to
//! read it, and the verifier chooses.

use crate::did::{ADDRESS_BYTES, Did};
use crate::error::{Error, Result};

/// Bytes in a Merkle root or a commitment.
pub const DIGEST_BYTES: usize = 32;

/// A commitment to one claim, as the issuer's tree holds it.
pub type Commitment = [u8; DIGEST_BYTES];

/// The root of an issuer's credential tree.
pub type Root = [u8; DIGEST_BYTES];

/// Credentials one revocation page covers.
///
/// 4,096 bits is 512 bytes — one page fits comfortably in a state record, and
/// an issuer with more credentials publishes more pages. A single unbounded
/// bitmap would be a record that grows without a limit anybody wrote down.
pub const BITS_PER_PAGE: usize = 4_096;

/// Bytes in a revocation page.
pub const PAGE_BYTES: usize = BITS_PER_PAGE / 8;

/// The longest a schema identifier may be.
pub const MAX_SCHEMA_CHARS: usize = 64;

/// What an issuer has attested, as the chain holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CryptographicAttestation {
    /// Who attested. A DID, so the chain can find the key that signed it.
    pub issuer: Did,
    /// What kind of claim this tree holds, e.g. `age-over-18` or `residency`.
    ///
    /// A label for the *schema*, never for a subject. `age-over-18` says what
    /// the tree is about; it says nothing about who is in it.
    pub schema: String,
    /// Root of the Poseidon tree over commitments.
    pub root: Root,
    /// Height at which the issuer published it.
    pub anchored_at: u64,
    /// Height past which presentations against this root should be refused, or
    /// `None` for an attestation that does not expire.
    pub expires_at: Option<u64>,
}

impl CryptographicAttestation {
    /// An attestation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Oversized`] for a schema past [`MAX_SCHEMA_CHARS`],
    /// [`Error::Invalid`] for one that is empty or not lowercase ASCII with
    /// hyphens, and [`Error::Inconsistent`] for an expiry at or before the
    /// anchor — an attestation that expired before it existed is a record
    /// nobody can use and everybody has to store.
    pub fn new(
        issuer: Did,
        schema: &str,
        root: Root,
        anchored_at: u64,
        expires_at: Option<u64>,
    ) -> Result<Self> {
        if schema.is_empty() {
            return Err(Error::Invalid {
                field: "schema",
                reason: "is empty".to_owned(),
            });
        }
        if schema.len() > MAX_SCHEMA_CHARS {
            return Err(Error::Oversized {
                what: "schema",
                found: schema.len(),
                limit: MAX_SCHEMA_CHARS,
            });
        }
        // Lowercase, digits and hyphens. Narrow on purpose: a schema is a key
        // two parties look up by, and `Age-Over-18` matching `age-over-18` in
        // one implementation and not another is a credential that verifies on
        // one verifier and fails on the next.
        if !schema
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(Error::Invalid {
                field: "schema",
                reason: format!("{schema:?} is not lowercase ASCII with hyphens"),
            });
        }
        if let Some(expiry) = expires_at
            && expiry <= anchored_at
        {
            return Err(Error::Inconsistent {
                what: "attestation",
                reason: format!("expires at {expiry}, anchored at {anchored_at}"),
            });
        }
        Ok(Self {
            issuer,
            schema: schema.to_owned(),
            root,
            anchored_at,
            expires_at,
        })
    }

    /// Whether presentations against this root are still worth checking at
    /// `height`.
    #[must_use]
    pub fn is_live(&self, height: u64) -> bool {
        height >= self.anchored_at && self.expires_at.is_none_or(|expiry| height < expiry)
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ADDRESS_BYTES + DIGEST_BYTES + 32);
        out.extend_from_slice(&self.issuer.address());
        out.extend_from_slice(&self.root);
        out.extend_from_slice(&self.anchored_at.to_le_bytes());
        out.extend_from_slice(&self.expires_at.unwrap_or(0).to_le_bytes());
        out.push(u8::from(self.expires_at.is_some()));
        out.push(self.schema.len() as u8);
        out.extend_from_slice(self.schema.as_bytes());
        out
    }

    /// Reads an attestation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a truncated record, trailing bytes, a
    /// schema length that disagrees with what arrived, or an expiry flag
    /// outside zero and one.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const FIXED: usize = ADDRESS_BYTES + DIGEST_BYTES + 8 + 8 + 1 + 1;
        if bytes.len() < FIXED {
            return Err(Error::Malformed {
                what: "attestation",
                reason: format!("{} bytes is shorter than the fixed section", bytes.len()),
            });
        }
        let issuer: [u8; ADDRESS_BYTES] = bytes[..ADDRESS_BYTES].try_into().expect("sized");
        let root: Root = bytes[ADDRESS_BYTES..ADDRESS_BYTES + DIGEST_BYTES]
            .try_into()
            .expect("sized");
        let mut at = ADDRESS_BYTES + DIGEST_BYTES;
        let anchored_at = u64::from_le_bytes(bytes[at..at + 8].try_into().expect("sized"));
        at += 8;
        let expiry = u64::from_le_bytes(bytes[at..at + 8].try_into().expect("sized"));
        at += 8;
        let expiry_flag = bytes[at];
        at += 1;
        let schema_len = bytes[at] as usize;
        at += 1;

        if bytes.len() != at + schema_len {
            return Err(Error::Malformed {
                what: "attestation",
                reason: format!(
                    "declares a {schema_len}-byte schema and carries {}",
                    bytes.len() - at
                ),
            });
        }
        let schema = std::str::from_utf8(&bytes[at..]).map_err(|_| Error::Malformed {
            what: "attestation",
            reason: "schema is not UTF-8".to_owned(),
        })?;
        let expires_at = match expiry_flag {
            0 => None,
            1 => Some(expiry),
            other => {
                return Err(Error::Malformed {
                    what: "attestation",
                    reason: format!("expiry flag is {other}"),
                });
            }
        };
        Self::new(
            Did::from_address(issuer),
            schema,
            root,
            anchored_at,
            expires_at,
        )
    }
}

/// One page of an issuer's revocation bitmap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationPage {
    /// Which issuer.
    pub issuer: Did,
    /// Which page, so index `n` lives at page `n / BITS_PER_PAGE`.
    pub page: u32,
    /// The bits. Set means revoked.
    pub bits: Vec<u8>,
}

impl RevocationPage {
    /// An empty page: nothing revoked.
    #[must_use]
    pub fn empty(issuer: Did, page: u32) -> Self {
        Self {
            issuer,
            page,
            bits: vec![0u8; PAGE_BYTES],
        }
    }

    /// Whether credential `index` within this page is revoked.
    ///
    /// An index past the page reads as **revoked**, not as live. A verifier
    /// that got the page number wrong should fail closed: the alternative is a
    /// revoked credential accepted because somebody's arithmetic was off.
    #[must_use]
    pub fn is_revoked(&self, index: usize) -> bool {
        let Some(byte) = self.bits.get(index / 8) else {
            return true;
        };
        byte >> (index % 8) & 1 == 1
    }

    /// Marks `index` revoked.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for an index past the page.
    pub fn revoke(&mut self, index: usize) -> Result<()> {
        if index >= BITS_PER_PAGE {
            return Err(Error::Invalid {
                field: "revocation index",
                reason: format!("{index} is past the {BITS_PER_PAGE}-bit page"),
            });
        }
        self.bits[index / 8] |= 1 << (index % 8);
        Ok(())
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ADDRESS_BYTES + 4 + PAGE_BYTES);
        out.extend_from_slice(&self.issuer.address());
        out.extend_from_slice(&self.page.to_le_bytes());
        out.extend_from_slice(&self.bits);
        out
    }

    /// Reads a page.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for anything but exactly the fixed size. A
    /// page is fixed-width on purpose: a length field here would be a number an
    /// issuer chooses, and a short page would read every index past its end as
    /// revoked.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const SIZE: usize = ADDRESS_BYTES + 4 + PAGE_BYTES;
        if bytes.len() != SIZE {
            return Err(Error::Malformed {
                what: "revocation page",
                reason: format!("{} bytes; a page is exactly {SIZE}", bytes.len()),
            });
        }
        let issuer: [u8; ADDRESS_BYTES] = bytes[..ADDRESS_BYTES].try_into().expect("sized");
        let page = u32::from_le_bytes(
            bytes[ADDRESS_BYTES..ADDRESS_BYTES + 4]
                .try_into()
                .expect("sized"),
        );
        Ok(Self {
            issuer: Did::from_address(issuer),
            page,
            bits: bytes[ADDRESS_BYTES + 4..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn issuer() -> Did {
        Did::from_address([3; ADDRESS_BYTES])
    }

    fn attestation() -> CryptographicAttestation {
        CryptographicAttestation::new(issuer(), "age-over-18", [7; DIGEST_BYTES], 100, Some(9_000))
            .expect("attestation")
    }

    #[test]
    fn an_attestation_survives_its_wire_format() {
        let original = attestation();
        assert_eq!(
            CryptographicAttestation::decode(&original.encode()).expect("decode"),
            original
        );
    }

    #[test]
    fn an_attestation_without_an_expiry_survives_too() {
        let forever =
            CryptographicAttestation::new(issuer(), "residency", [1; DIGEST_BYTES], 5, None)
                .expect("attestation");
        assert_eq!(
            CryptographicAttestation::decode(&forever.encode()).expect("decode"),
            forever
        );
        assert!(forever.is_live(1_000_000));
    }

    #[test]
    fn liveness_is_a_window_and_not_a_flag() {
        let live = attestation();
        assert!(!live.is_live(99));
        assert!(live.is_live(100));
        assert!(live.is_live(8_999));
        assert!(!live.is_live(9_000));
    }

    #[test]
    fn an_attestation_that_expires_before_it_exists_is_refused() {
        assert!(
            CryptographicAttestation::new(issuer(), "x", [0; DIGEST_BYTES], 100, Some(100))
                .is_err()
        );
        assert!(
            CryptographicAttestation::new(issuer(), "x", [0; DIGEST_BYTES], 100, Some(99)).is_err()
        );
    }

    #[test]
    fn a_schema_is_lowercase_ascii_with_hyphens() {
        // `Age-Over-18` matching `age-over-18` in one implementation and not
        // another is a credential that verifies on one verifier and fails on
        // the next.
        for bad in [
            "",
            "Age-Over-18",
            "age over 18",
            "age_over_18",
            "age/18",
            "âge",
        ] {
            assert!(
                CryptographicAttestation::new(issuer(), bad, [0; DIGEST_BYTES], 1, None).is_err(),
                "{bad:?} was accepted"
            );
        }
        for good in ["age-over-18", "residency", "accredited-investor-2026"] {
            assert!(
                CryptographicAttestation::new(issuer(), good, [0; DIGEST_BYTES], 1, None).is_ok(),
                "{good:?} was refused"
            );
        }
    }

    #[test]
    fn an_attestation_has_nowhere_to_put_a_claim() {
        // The property the whole design rests on. The encoding is the address,
        // the root, two heights, a flag and the schema label — there is no
        // field a value could go in, so putting one on chain is not a mistake
        // anybody can make here.
        let attestation = attestation();
        assert_eq!(
            attestation.encode().len(),
            ADDRESS_BYTES + DIGEST_BYTES + 8 + 8 + 1 + 1 + attestation.schema.len()
        );
    }

    #[test]
    fn a_truncated_or_padded_attestation_is_refused() {
        let encoded = attestation().encode();
        for len in 0..encoded.len() {
            assert!(
                CryptographicAttestation::decode(&encoded[..len]).is_err(),
                "{len}"
            );
        }
        let mut padded = encoded.clone();
        padded.push(0);
        assert!(CryptographicAttestation::decode(&padded).is_err());
    }

    #[test]
    fn a_fresh_page_revokes_nothing() {
        let page = RevocationPage::empty(issuer(), 0);
        for index in 0..BITS_PER_PAGE {
            assert!(!page.is_revoked(index), "index {index}");
        }
    }

    #[test]
    fn a_revoked_bit_stays_revoked_and_touches_no_neighbour() {
        let mut page = RevocationPage::empty(issuer(), 0);
        page.revoke(1_000).expect("revoke");
        assert!(page.is_revoked(1_000));
        assert!(!page.is_revoked(999));
        assert!(!page.is_revoked(1_001));
    }

    #[test]
    fn an_index_past_the_page_reads_as_revoked() {
        // Fail closed. The alternative is a revoked credential accepted because
        // somebody's page arithmetic was off by one.
        let page = RevocationPage::empty(issuer(), 0);
        assert!(page.is_revoked(BITS_PER_PAGE));
        assert!(page.is_revoked(usize::MAX));
        assert!(page.clone().revoke(BITS_PER_PAGE).is_err());
    }

    #[test]
    fn a_page_survives_its_wire_format() {
        let mut page = RevocationPage::empty(issuer(), 3);
        page.revoke(0).expect("revoke");
        page.revoke(4_095).expect("revoke");
        assert_eq!(
            RevocationPage::decode(&page.encode()).expect("decode"),
            page
        );
    }

    #[test]
    fn a_page_of_the_wrong_size_is_refused() {
        // A short page would read every index past its end as revoked, which is
        // an issuer silently revoking everybody.
        let encoded = RevocationPage::empty(issuer(), 0).encode();
        assert!(RevocationPage::decode(&encoded[..encoded.len() - 1]).is_err());
        let mut padded = encoded.clone();
        padded.push(0);
        assert!(RevocationPage::decode(&padded).is_err());
    }
}
