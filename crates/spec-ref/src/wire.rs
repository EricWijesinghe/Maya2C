//! The v5 transfer frame, its signing bytes, its id and the sender address —
//! `spec/01-encoding.md` ENC-1 .. ENC-8 and `spec/02-transactions.md` TX-2, TX-3.

use crate::Address;

/// ML-DSA-65 public key (FIPS 204, Table 2).
pub const ML_DSA_65_PK: usize = 1952;
/// ML-DSA-65 signature.
pub const ML_DSA_65_SIG: usize = 3309;
/// SLH-DSA-SHA2-128s public key (FIPS 205, Table 2).
pub const SLH_DSA_128S_PK: usize = 32;
/// SLH-DSA-SHA2-128s signature.
pub const SLH_DSA_128S_SIG: usize = 7856;
/// Hybrid public key: lattice half then hash-based half.
pub const HYBRID_PK: usize = ML_DSA_65_PK + SLH_DSA_128S_PK;
/// Hybrid signature: lattice half then hash-based half.
pub const HYBRID_SIG: usize = ML_DSA_65_SIG + SLH_DSA_128S_SIG;
/// ENC-1: the version byte of a plain transfer.
pub const WIRE_VERSION_TRANSFER: u8 = 5;
/// ENC-6: the largest count a collection prefix may carry.
pub const MAX_COLLECTION_LEN: u64 = 65_536;

const TX_DOMAIN: &[u8] = b"custom-l1-node.tx.v3";
const ADDRESS_DOMAIN: &[u8] = b"custom-l1-node.address.v3";

/// A transfer's fields, keys and signature as opaque bytes.
#[derive(Clone, Debug)]
pub struct Frame {
    /// `(recipient, amount)`.
    pub outputs: Vec<(Address, u64)>,
    /// `HYBRID_PK` bytes.
    pub public_key: Vec<u8>,
    /// Replay counter.
    pub nonce: u64,
    /// `HYBRID_SIG` bytes, or none for an unsigned frame.
    pub signature: Option<Vec<u8>>,
}

/// ENC-3: `u64` count, inputs; `u64` count, then per output `amount_le64 ‖ recipient`.
/// Transfers carry no inputs in the account model.
fn io_section(outputs: &[(Address, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&(outputs.len() as u64).to_le_bytes());
    for (recipient, amount) in outputs {
        out.extend_from_slice(&amount.to_le_bytes());
        out.extend_from_slice(recipient);
    }
    out
}

/// ENC-2: `version ‖ io ‖ public_key ‖ nonce_le64 ‖ flag ‖ [signature]`.
#[must_use]
pub fn encode(frame: &Frame) -> Vec<u8> {
    let mut out = vec![WIRE_VERSION_TRANSFER];
    out.extend(io_section(&frame.outputs));
    out.extend_from_slice(&frame.public_key);
    out.extend_from_slice(&frame.nonce.to_le_bytes());
    match &frame.signature {
        Some(sig) => {
            out.push(1);
            out.extend_from_slice(sig);
        }
        None => out.push(0),
    }
    out
}

/// TX-3: what both signatures sign — `domain ‖ io ‖ public_key ‖ nonce_le64`.
#[must_use]
pub fn signing_bytes(frame: &Frame) -> Vec<u8> {
    let mut out = TX_DOMAIN.to_vec();
    out.extend(io_section(&frame.outputs));
    out.extend_from_slice(&frame.public_key);
    out.extend_from_slice(&frame.nonce.to_le_bytes());
    out
}

/// ENC-8: a transfer's id is `blake3(signing_bytes ‖ signature)`, the
/// signature omitted when absent. Hashing it commits the id to one
/// authorization; that is sound only because both schemes sign
/// deterministically, so one payload under one key pair has one id.
#[must_use]
pub fn txid(frame: &Frame) -> [u8; 32] {
    let mut bytes = signing_bytes(frame);
    if let Some(sig) = &frame.signature {
        bytes.extend_from_slice(sig);
    }
    *blake3::hash(&bytes).as_bytes()
}

/// TX-2: `blake3(address_domain ‖ ml_dsa_pk ‖ slh_dsa_pk)`.
#[must_use]
pub fn address_of(public_key: &[u8]) -> Address {
    let mut bytes = ADDRESS_DOMAIN.to_vec();
    bytes.extend_from_slice(public_key);
    *blake3::hash(&bytes).as_bytes()
}
