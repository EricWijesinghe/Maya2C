//! The wire forms of the identity transitions.
//!
//! ## Rotation is authorised by the key it replaces
//!
//! That is the whole security argument, and it needs no new cryptography. A
//! transaction is already signed by a hybrid pair and the sender's address is
//! BLAKE3 over both halves — so a rotation transaction *sent from the subject's
//! own address* is, by construction, signed by the key the document currently
//! names. The new key is data inside that transaction.
//!
//! ML-DSA-65 is a lattice signature. The brief asked for "post-quantum lattice
//! proofs" for rotation, and this is one: the proof that the old key authorised
//! the new is the old key's signature over a payload naming it. A second proof
//! system alongside would be another thing to get wrong for nothing.
//!
//! ## Why the new key rides in the payload rather than being derived
//!
//! The subject signs with the old key and names the new one. The alternative —
//! deriving the next key from the current one — would make the whole chain of
//! keys computable from any single one of them, which is the opposite of what
//! rotation is for.
//!
//! ## What none of these carry
//!
//! A claim. `AnchorAttestation` carries a Merkle root; `SetRevocationBit`
//! carries an index. There is no field in any of them a personal fact could go
//! in, and that is checked by `tests/identity_tests.rs` scanning committed
//! state rather than by reading the types.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Bytes of a hybrid public key. Checked against `crypto::hybrid` by
/// `identity_parity_tests`.
pub const HYBRID_PUBLIC_KEY_LEN: usize = 1_984;

/// Bytes of a Merkle root.
pub const ROOT_LEN: usize = 32;

/// The longest schema label a transaction may name.
pub const MAX_SCHEMA_CHARS: usize = 64;

/// The most service endpoints a registration may declare.
pub const MAX_ENDPOINTS: usize = 8;

/// The longest an endpoint's type may be.
pub const MAX_ENDPOINT_TYPE_CHARS: usize = 32;

/// The longest an endpoint's URI may be.
pub const MAX_ENDPOINT_CHARS: usize = 256;

/// Registers `did:maya2c:<sender>`.
///
/// The subject is the transaction's sender, never a field: a registration that
/// named its subject would let anyone register a document for anybody.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegisterDid {
    /// The hybrid public key the document will name.
    ///
    /// Supplied rather than taken from the transaction's own key so a subject
    /// may register a document for a key it controls but is not spending from
    /// — a cold key, or one held in `custody-mpc`.
    pub public_key: Vec<u8>,
    /// Services the subject runs: `(type, uri)`.
    pub endpoints: Vec<(String, String)>,
}

/// Installs a new key, authorised by the one it replaces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RotateDidKey {
    /// The epoch this installs. Must be exactly one past the current.
    pub epoch: u32,
    /// The key to install.
    pub public_key: Vec<u8>,
}

/// Revokes the sender's own DID, permanently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RevokeDid {
    /// Free-form reason code, for operators. Not interpreted by consensus.
    pub reason: u16,
}

/// Publishes an issuer's credential-tree root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnchorAttestation {
    /// Which schema this tree is about. A label, never a subject.
    pub schema: String,
    /// The Poseidon root over commitments.
    pub root: [u8; ROOT_LEN],
    /// Height past which presentations should be refused, or zero for none.
    pub expires_at: u64,
}

/// Flips one bit of the sender's own revocation bitmap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetRevocationBit {
    /// Which credential index. The page follows from it.
    pub index: u64,
}

impl RegisterDid {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.public_key);
        buf.push(self.endpoints.len() as u8);
        for (service_type, uri) in &self.endpoints {
            buf.push(service_type.len() as u8);
            buf.extend_from_slice(service_type.as_bytes());
            buf.extend_from_slice(&(uri.len() as u16).to_le_bytes());
            buf.extend_from_slice(uri.as_bytes());
        }
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload, an endpoint count
    /// past [`MAX_ENDPOINTS`], a field past its bound, or text that is not
    /// UTF-8.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let public_key = reader.read_slice(HYBRID_PUBLIC_KEY_LEN)?.to_vec();
        let count = reader.read_u8()? as usize;
        if count > MAX_ENDPOINTS {
            return Err(NodeError::Decode(format!(
                "registration declares {count} endpoints, limit {MAX_ENDPOINTS}"
            )));
        }
        let mut endpoints = Vec::with_capacity(count);
        for _ in 0..count {
            let type_len = reader.read_u8()? as usize;
            if type_len > MAX_ENDPOINT_TYPE_CHARS {
                return Err(NodeError::Decode(format!(
                    "endpoint type is {type_len} bytes, limit {MAX_ENDPOINT_TYPE_CHARS}"
                )));
            }
            let service_type = text(reader.read_slice(type_len)?)?;
            let uri_len = usize::from(u16::from_le_bytes(reader.read_array::<2>()?));
            if uri_len > MAX_ENDPOINT_CHARS {
                return Err(NodeError::Decode(format!(
                    "endpoint uri is {uri_len} bytes, limit {MAX_ENDPOINT_CHARS}"
                )));
            }
            let uri = text(reader.read_slice(uri_len)?)?;
            endpoints.push((service_type, uri));
        }
        Ok(Self {
            public_key,
            endpoints,
        })
    }
}

impl RotateDidKey {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.epoch.to_le_bytes());
        buf.extend_from_slice(&self.public_key);
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            epoch: reader.read_u32()?,
            public_key: reader.read_slice(HYBRID_PUBLIC_KEY_LEN)?.to_vec(),
        })
    }
}

impl RevokeDid {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.reason.to_le_bytes());
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            reason: u16::from_le_bytes(reader.read_array::<2>()?),
        })
    }
}

impl AnchorAttestation {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.root);
        buf.extend_from_slice(&self.expires_at.to_le_bytes());
        buf.push(self.schema.len() as u8);
        buf.extend_from_slice(self.schema.as_bytes());
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload, a schema past
    /// [`MAX_SCHEMA_CHARS`], or text that is not UTF-8.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let root: [u8; ROOT_LEN] = reader.read_array::<ROOT_LEN>()?;
        let expires_at = reader.read_u64()?;
        let schema_len = reader.read_u8()? as usize;
        if schema_len > MAX_SCHEMA_CHARS {
            return Err(NodeError::Decode(format!(
                "schema is {schema_len} bytes, limit {MAX_SCHEMA_CHARS}"
            )));
        }
        Ok(Self {
            schema: text(reader.read_slice(schema_len)?)?,
            root,
            expires_at,
        })
    }
}

impl SetRevocationBit {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.index.to_le_bytes());
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            index: reader.read_u64()?,
        })
    }
}

/// UTF-8 text, or a decode failure.
fn text(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| NodeError::Decode("identity payload text is not UTF-8".to_owned()))
}
