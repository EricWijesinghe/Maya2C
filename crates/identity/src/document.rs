//! The DID Document: which keys speak for a subject, and where to reach them.
//!
//! ## The key epoch is the whole rotation story
//!
//! A document carries one active verification method and a monotonically
//! increasing `epoch`. Rotation installs a new key and increments it; a
//! signature is checked against the key at the epoch the document currently
//! holds, and nothing else.
//!
//! That is deliberately narrower than DID Core allows. A document with several
//! concurrently valid keys is a document where revoking one leaves the subject
//! reachable through another, and where "which key signed this" becomes a
//! question with several answers. One key, one epoch, and rotation is a state
//! transition somebody can point at.
//!
//! ## Revocation is a state, not a deletion
//!
//! A revoked document stays in state with `revoked_at` set. Deleting it would
//! make a revoked DID indistinguishable from one that never existed, and the
//! two mean opposite things to a verifier: the first is a subject that lost its
//! keys, the second is a string somebody made up.
//!
//! ## What a document may not contain
//!
//! Anything about the person. No name, no jurisdiction, no date of birth, no
//! claim of any kind — those live in commitments under
//! [`crate::attestation`], and the types here have nowhere to put one. That is
//! not a convention; there is no field.
//!
//! A service endpoint is the one free-text field, and it is a URL to a service
//! rather than a fact about a subject. It is bounded, charset-checked, and
//! documented as public — because it is, and an endpoint naming a person's
//! employer is a leak the chain cannot take back.

use crate::did::{ADDRESS_BYTES, Did};
use crate::error::{Error, Result};

/// The most service endpoints one document may carry.
pub const MAX_ENDPOINTS: usize = 8;

/// The longest a service endpoint's URL may be.
pub const MAX_ENDPOINT_CHARS: usize = 256;

/// The longest a service endpoint's type may be.
pub const MAX_ENDPOINT_TYPE_CHARS: usize = 32;

/// Bytes of a hybrid public key: ML-DSA-65 plus SLH-DSA.
///
/// Duplicated from `crate::crypto::hybrid` rather than imported, because this
/// crate links no chain types — and because a guard must not depend on the
/// thing it guards. `identity_parity_tests` in the node's suite is what pins
/// the two together.
pub const HYBRID_PUBLIC_KEY_LEN: usize = 1_984;

/// One way of proving control of a subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationMethod {
    /// Which rotation installed this key. Starts at zero and only rises.
    pub epoch: u32,
    /// The hybrid public key, both halves.
    pub public_key: Vec<u8>,
}

impl VerificationMethod {
    /// A method for a key.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] unless the key is exactly
    /// [`HYBRID_PUBLIC_KEY_LEN`] bytes. A short key is not a weaker key, it is
    /// a different scheme, and accepting one would mean the document promises
    /// something the chain cannot check.
    pub fn new(epoch: u32, public_key: Vec<u8>) -> Result<Self> {
        if public_key.len() != HYBRID_PUBLIC_KEY_LEN {
            return Err(Error::Invalid {
                field: "publicKey",
                reason: format!(
                    "{} bytes; a hybrid key is exactly {HYBRID_PUBLIC_KEY_LEN}",
                    public_key.len()
                ),
            });
        }
        Ok(Self { epoch, public_key })
    }
}

/// Where to reach a service the subject runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceEndpoint {
    /// What kind of service, e.g. `CredentialRepository`.
    pub service_type: String,
    /// Where it is.
    pub uri: String,
}

impl ServiceEndpoint {
    /// An endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Oversized`] past the length bounds and
    /// [`Error::Invalid`] for anything but printable ASCII, or for an empty
    /// field. Control characters are refused because this value reaches logs,
    /// resolver output and operator screens, and a newline in a URL is a way to
    /// forge a log line.
    pub fn new(service_type: &str, uri: &str) -> Result<Self> {
        bounded(service_type, "service type", MAX_ENDPOINT_TYPE_CHARS)?;
        bounded(uri, "service endpoint", MAX_ENDPOINT_CHARS)?;
        Ok(Self {
            service_type: service_type.to_owned(),
            uri: uri.to_owned(),
        })
    }
}

/// A field within its bound and made of printable ASCII.
fn bounded(text: &str, what: &'static str, limit: usize) -> Result<()> {
    if text.is_empty() {
        return Err(Error::Invalid {
            field: "service endpoint",
            reason: format!("{what} is empty"),
        });
    }
    if text.len() > limit {
        return Err(Error::Oversized {
            what,
            found: text.len(),
            limit,
        });
    }
    if !text.bytes().all(|byte| (0x20..0x7f).contains(&byte)) {
        return Err(Error::Invalid {
            field: "service endpoint",
            reason: format!("{what} is not printable ASCII"),
        });
    }
    Ok(())
}

/// Everything the chain holds about a subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DidDocument {
    /// Who this is.
    pub did: Did,
    /// The one key that currently speaks for the subject.
    pub verification: VerificationMethod,
    /// Services the subject runs.
    pub endpoints: Vec<ServiceEndpoint>,
    /// Height at which the subject was revoked, if it was.
    pub revoked_at: Option<u64>,
}

impl DidDocument {
    /// A fresh document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Oversized`] past [`MAX_ENDPOINTS`] and
    /// [`Error::Inconsistent`] for two endpoints of the same type — a resolver
    /// choosing between them would be choosing where a credential request goes.
    pub fn new(
        did: Did,
        verification: VerificationMethod,
        endpoints: Vec<ServiceEndpoint>,
    ) -> Result<Self> {
        if endpoints.len() > MAX_ENDPOINTS {
            return Err(Error::Oversized {
                what: "service endpoints",
                found: endpoints.len(),
                limit: MAX_ENDPOINTS,
            });
        }
        for (index, endpoint) in endpoints.iter().enumerate() {
            if endpoints[..index]
                .iter()
                .any(|earlier| earlier.service_type == endpoint.service_type)
            {
                return Err(Error::Inconsistent {
                    what: "service endpoints",
                    reason: format!("{:?} appears twice", endpoint.service_type),
                });
            }
        }
        Ok(Self {
            did,
            verification,
            endpoints,
            revoked_at: None,
        })
    }

    /// Whether the subject is revoked at `height`.
    #[must_use]
    pub fn is_revoked(&self, height: u64) -> bool {
        self.revoked_at.is_some_and(|at| height >= at)
    }

    /// This document with a new key installed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Inconsistent`] if the subject is already revoked, or if
    /// the epoch does not advance by exactly one. Not "at least one": a gap
    /// would mean a rotation nobody can point at, and an epoch that stood still
    /// would let a replayed rotation reinstall an old key.
    pub fn rotate(&self, next: VerificationMethod) -> Result<Self> {
        if self.revoked_at.is_some() {
            return Err(Error::Inconsistent {
                what: "rotation",
                reason: "the subject is revoked".to_owned(),
            });
        }
        if next.epoch != self.verification.epoch + 1 {
            return Err(Error::Inconsistent {
                what: "rotation",
                reason: format!(
                    "epoch {} does not follow {}",
                    next.epoch, self.verification.epoch
                ),
            });
        }
        Ok(Self {
            verification: next,
            ..self.clone()
        })
    }

    /// This document, revoked at `height`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Inconsistent`] if it is already revoked. Revoking twice
    /// would move the height, and a verifier checking whether a signature
    /// predated revocation would get a different answer than it did yesterday.
    pub fn revoke(&self, height: u64) -> Result<Self> {
        if let Some(at) = self.revoked_at {
            return Err(Error::Inconsistent {
                what: "revocation",
                reason: format!("already revoked at {at}"),
            });
        }
        Ok(Self {
            revoked_at: Some(height),
            ..self.clone()
        })
    }

    /// The canonical byte encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HYBRID_PUBLIC_KEY_LEN + 128);
        out.extend_from_slice(&self.did.address());
        out.extend_from_slice(&self.verification.epoch.to_le_bytes());
        out.extend_from_slice(&self.verification.public_key);
        out.extend_from_slice(&self.revoked_at.unwrap_or(0).to_le_bytes());
        out.push(u8::from(self.revoked_at.is_some()));
        out.push(self.endpoints.len() as u8);
        for endpoint in &self.endpoints {
            out.push(endpoint.service_type.len() as u8);
            out.extend_from_slice(endpoint.service_type.as_bytes());
            out.extend_from_slice(&(endpoint.uri.len() as u16).to_le_bytes());
            out.extend_from_slice(endpoint.uri.as_bytes());
        }
        out
    }

    /// Reads a document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a truncated record, one with trailing
    /// bytes, or one whose declared lengths disagree with what arrived, and
    /// whatever the constructors return for a value outside its bounds.
    ///
    /// Trailing bytes are refused rather than ignored: a record that decoded
    /// the same with junk appended would have two encodings, and a state root
    /// commits to bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(bytes);
        let address: [u8; ADDRESS_BYTES] = cursor.take(ADDRESS_BYTES)?.try_into().expect("sized");
        let epoch = u32::from_le_bytes(cursor.take(4)?.try_into().expect("sized"));
        let public_key = cursor.take(HYBRID_PUBLIC_KEY_LEN)?.to_vec();
        let revoked_height = u64::from_le_bytes(cursor.take(8)?.try_into().expect("sized"));
        let revoked_flag = cursor.take(1)?[0];
        let count = cursor.take(1)?[0] as usize;

        if count > MAX_ENDPOINTS {
            return Err(Error::Oversized {
                what: "service endpoints",
                found: count,
                limit: MAX_ENDPOINTS,
            });
        }
        let mut endpoints = Vec::with_capacity(count);
        for _ in 0..count {
            let type_len = cursor.take(1)?[0] as usize;
            let service_type = ascii(cursor.take(type_len)?)?;
            let uri_len = u16::from_le_bytes(cursor.take(2)?.try_into().expect("sized")) as usize;
            let uri = ascii(cursor.take(uri_len)?)?;
            endpoints.push(ServiceEndpoint::new(&service_type, &uri)?);
        }
        cursor.finish()?;

        let mut document = Self::new(
            Did::from_address(address),
            VerificationMethod::new(epoch, public_key)?,
            endpoints,
        )?;
        // A flag byte outside 0 and 1 is a record nobody in this tree wrote.
        document.revoked_at = match revoked_flag {
            0 => None,
            1 => Some(revoked_height),
            other => {
                return Err(Error::Malformed {
                    what: "did document",
                    reason: format!("revocation flag is {other}"),
                });
            }
        };
        Ok(document)
    }
}

/// A UTF-8 slice, or a refusal.
fn ascii(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| Error::Malformed {
        what: "did document",
        reason: "a text field is not UTF-8".to_owned(),
    })
}

/// A bounds-checked reader over a record.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// The next `count` bytes.
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(count).ok_or(Error::Malformed {
            what: "did document",
            reason: "a length field overflows".to_owned(),
        })?;
        let slice = self.bytes.get(self.at..end).ok_or(Error::Malformed {
            what: "did document",
            reason: format!("wanted {count} bytes at {} and the record ends", self.at),
        })?;
        self.at = end;
        Ok(slice)
    }

    /// Refuses trailing bytes.
    fn finish(&self) -> Result<()> {
        if self.at == self.bytes.len() {
            return Ok(());
        }
        Err(Error::Malformed {
            what: "did document",
            reason: format!("{} trailing bytes", self.bytes.len() - self.at),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(fill: u8) -> Vec<u8> {
        vec![fill; HYBRID_PUBLIC_KEY_LEN]
    }

    fn document() -> DidDocument {
        DidDocument::new(
            Did::from_address([9; ADDRESS_BYTES]),
            VerificationMethod::new(0, key(1)).expect("key"),
            vec![
                ServiceEndpoint::new("CredentialRepository", "https://example.test/creds")
                    .expect("endpoint"),
            ],
        )
        .expect("document")
    }

    #[test]
    fn a_document_survives_its_wire_format() {
        let original = document();
        assert_eq!(
            DidDocument::decode(&original.encode()).expect("decode"),
            original
        );
    }

    #[test]
    fn a_revoked_document_survives_its_wire_format() {
        let revoked = document().revoke(4_000).expect("revoke");
        assert_eq!(
            DidDocument::decode(&revoked.encode()).expect("decode"),
            revoked
        );
        assert!(revoked.is_revoked(4_000));
        assert!(revoked.is_revoked(9_999));
        assert!(!revoked.is_revoked(3_999));
    }

    #[test]
    fn a_document_with_no_endpoints_is_a_document() {
        let bare = DidDocument::new(
            Did::from_address([1; ADDRESS_BYTES]),
            VerificationMethod::new(0, key(2)).expect("key"),
            Vec::new(),
        )
        .expect("document");
        assert_eq!(DidDocument::decode(&bare.encode()).expect("decode"), bare);
    }

    #[test]
    fn a_key_of_the_wrong_length_is_refused() {
        // A short key is not a weaker key, it is a different scheme.
        assert!(VerificationMethod::new(0, vec![0; HYBRID_PUBLIC_KEY_LEN - 1]).is_err());
        assert!(VerificationMethod::new(0, vec![0; HYBRID_PUBLIC_KEY_LEN + 1]).is_err());
        assert!(VerificationMethod::new(0, Vec::new()).is_err());
    }

    #[test]
    fn rotation_advances_the_epoch_by_exactly_one() {
        let first = document();
        let second = first
            .rotate(VerificationMethod::new(1, key(2)).expect("key"))
            .expect("rotate");
        assert_eq!(second.verification.epoch, 1);
        assert_eq!(second.verification.public_key, key(2));

        // A gap is a rotation nobody can point at; standing still would let a
        // replayed rotation reinstall an old key.
        assert!(
            second
                .rotate(VerificationMethod::new(3, key(3)).expect("key"))
                .is_err()
        );
        assert!(
            second
                .rotate(VerificationMethod::new(1, key(3)).expect("key"))
                .is_err()
        );
        assert!(
            second
                .rotate(VerificationMethod::new(0, key(3)).expect("key"))
                .is_err()
        );
    }

    #[test]
    fn a_revoked_subject_cannot_rotate() {
        let revoked = document().revoke(100).expect("revoke");
        assert!(
            revoked
                .rotate(VerificationMethod::new(1, key(5)).expect("key"))
                .is_err()
        );
    }

    #[test]
    fn revoking_twice_is_refused_rather_than_moving_the_height() {
        // Otherwise a verifier asking whether a signature predated revocation
        // gets a different answer than it did yesterday.
        let revoked = document().revoke(100).expect("revoke");
        assert!(revoked.revoke(200).is_err());
    }

    #[test]
    fn two_endpoints_of_one_type_are_refused() {
        let repeated = vec![
            ServiceEndpoint::new("Repo", "https://a.test").expect("endpoint"),
            ServiceEndpoint::new("Repo", "https://b.test").expect("endpoint"),
        ];
        assert!(
            DidDocument::new(
                Did::from_address([1; ADDRESS_BYTES]),
                VerificationMethod::new(0, key(1)).expect("key"),
                repeated,
            )
            .is_err()
        );
    }

    #[test]
    fn an_endpoint_refuses_control_characters_and_overlong_values() {
        // This value reaches logs and resolver output; a newline in a URL is a
        // way to forge a log line.
        assert!(ServiceEndpoint::new("Repo", "https://a.test\nGET /").is_err());
        assert!(ServiceEndpoint::new("Repo\u{0}", "https://a.test").is_err());
        assert!(ServiceEndpoint::new("", "https://a.test").is_err());
        assert!(ServiceEndpoint::new("Repo", "").is_err());
        assert!(ServiceEndpoint::new("Repo", &"x".repeat(MAX_ENDPOINT_CHARS + 1)).is_err());
    }

    #[test]
    fn a_truncated_record_is_refused_rather_than_read_short() {
        let encoded = document().encode();
        for len in 0..encoded.len() {
            assert!(
                DidDocument::decode(&encoded[..len]).is_err(),
                "{len} bytes decoded as a whole document"
            );
        }
    }

    #[test]
    fn trailing_bytes_are_refused() {
        // A record that decoded the same with junk appended would have two
        // encodings, and a state root commits to bytes.
        let mut encoded = document().encode();
        encoded.push(0);
        assert!(DidDocument::decode(&encoded).is_err());
    }

    #[test]
    fn a_revocation_flag_outside_zero_and_one_is_refused() {
        let mut encoded = document().encode();
        let flag = ADDRESS_BYTES + 4 + HYBRID_PUBLIC_KEY_LEN + 8;
        encoded[flag] = 2;
        assert!(DidDocument::decode(&encoded).is_err());
    }

    #[test]
    fn a_document_has_nowhere_to_put_a_claim() {
        // Not a convention — there is no field. The only free text is a service
        // endpoint, which is a URL to a service rather than a fact about a
        // person, and it is bounded and charset-checked.
        let document = document();
        let encoded = document.encode();
        assert_eq!(
            encoded.len(),
            ADDRESS_BYTES
                + 4
                + HYBRID_PUBLIC_KEY_LEN
                + 8
                + 1
                + 1
                + 1
                + document.endpoints[0].service_type.len()
                + 2
                + document.endpoints[0].uri.len(),
            "the encoding carries exactly the fields the type has"
        );
    }
}
