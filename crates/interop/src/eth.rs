//! Ethereum execution-layer header hashing: `keccak256(rlp(header))`.
//!
//! Covers the pre-London fifteen-field header and the later optional
//! trailing fields (base fee, withdrawals root, blob gas used, excess blob
//! gas, parent beacon root, requests hash), each appended only when present,
//! as the Yellow Paper and the EIPs that added them specify.

use sha3::{Digest, Keccak256};

/// An execution-layer header.
#[derive(Clone, Debug, Default)]
pub struct Header {
    /// Parent block hash.
    pub parent_hash: [u8; 32],
    /// Ommers hash.
    pub ommers_hash: [u8; 32],
    /// Fee recipient (coinbase).
    pub beneficiary: [u8; 20],
    /// State root.
    pub state_root: [u8; 32],
    /// Transactions root.
    pub transactions_root: [u8; 32],
    /// Receipts root.
    pub receipts_root: [u8; 32],
    /// Logs bloom.
    pub logs_bloom: Vec<u8>,
    /// Difficulty (0 after the Merge).
    pub difficulty: u128,
    /// Block number.
    pub number: u64,
    /// Gas limit.
    pub gas_limit: u64,
    /// Gas used.
    pub gas_used: u64,
    /// Unix timestamp.
    pub timestamp: u64,
    /// Extra data.
    pub extra_data: Vec<u8>,
    /// Mix hash (prevRandao after the Merge).
    pub mix_hash: [u8; 32],
    /// Proof-of-work nonce (8 bytes, zero after the Merge).
    pub nonce: [u8; 8],
    /// EIP-1559 base fee, from London.
    pub base_fee: Option<u128>,
    /// EIP-4895 withdrawals root, from Shanghai.
    pub withdrawals_root: Option<[u8; 32]>,
    /// EIP-4844 blob gas used, from Cancun.
    pub blob_gas_used: Option<u64>,
    /// EIP-4844 excess blob gas, from Cancun.
    pub excess_blob_gas: Option<u64>,
    /// EIP-4788 parent beacon block root, from Cancun.
    pub parent_beacon_root: Option<[u8; 32]>,
    /// EIP-7685 requests hash, from Prague.
    pub requests_hash: Option<[u8; 32]>,
}

fn rlp_bytes(out: &mut Vec<u8>, b: &[u8]) {
    if b.len() == 1 && b[0] < 0x80 {
        out.push(b[0]);
    } else {
        rlp_len(out, b.len(), 0x80);
        out.extend_from_slice(b);
    }
}

fn rlp_len(out: &mut Vec<u8>, len: usize, offset: u8) {
    if len < 56 {
        out.push(offset + u8::try_from(len).unwrap_or(0));
    } else {
        let be = len.to_be_bytes();
        let first = be.iter().position(|x| *x != 0).unwrap_or(be.len() - 1);
        out.push(offset + 55 + u8::try_from(be.len() - first).unwrap_or(0));
        out.extend_from_slice(&be[first..]);
    }
}

/// An integer as RLP: big-endian, no leading zeros, zero as the empty string.
fn rlp_uint(out: &mut Vec<u8>, v: u128) {
    let be = v.to_be_bytes();
    let first = be.iter().position(|x| *x != 0).unwrap_or(be.len());
    rlp_bytes(out, &be[first..]);
}

/// The header's RLP encoding.
#[must_use]
pub fn rlp(h: &Header) -> Vec<u8> {
    let mut body = Vec::new();
    rlp_bytes(&mut body, &h.parent_hash);
    rlp_bytes(&mut body, &h.ommers_hash);
    rlp_bytes(&mut body, &h.beneficiary);
    rlp_bytes(&mut body, &h.state_root);
    rlp_bytes(&mut body, &h.transactions_root);
    rlp_bytes(&mut body, &h.receipts_root);
    rlp_bytes(&mut body, &h.logs_bloom);
    rlp_uint(&mut body, h.difficulty);
    rlp_uint(&mut body, u128::from(h.number));
    rlp_uint(&mut body, u128::from(h.gas_limit));
    rlp_uint(&mut body, u128::from(h.gas_used));
    rlp_uint(&mut body, u128::from(h.timestamp));
    rlp_bytes(&mut body, &h.extra_data);
    rlp_bytes(&mut body, &h.mix_hash);
    rlp_bytes(&mut body, &h.nonce);
    if let Some(v) = h.base_fee {
        rlp_uint(&mut body, v);
    }
    if let Some(v) = h.withdrawals_root {
        rlp_bytes(&mut body, &v);
    }
    if let Some(v) = h.blob_gas_used {
        rlp_uint(&mut body, u128::from(v));
    }
    if let Some(v) = h.excess_blob_gas {
        rlp_uint(&mut body, u128::from(v));
    }
    if let Some(v) = h.parent_beacon_root {
        rlp_bytes(&mut body, &v);
    }
    if let Some(v) = h.requests_hash {
        rlp_bytes(&mut body, &v);
    }
    let mut out = Vec::with_capacity(body.len() + 9);
    rlp_len(&mut out, body.len(), 0xc0);
    out.extend_from_slice(&body);
    out
}

/// The block hash: `keccak256(rlp(header))`.
#[must_use]
pub fn hash(h: &Header) -> [u8; 32] {
    Keccak256::digest(rlp(h)).into()
}

/// Checks a chain of headers: each hash is recomputed, each links to its parent.
///
/// # Errors
///
/// The index of the first header whose parent link does not match.
pub fn check_chain(headers: &[Header]) -> Result<[u8; 32], usize> {
    let mut prev: Option<[u8; 32]> = None;
    for (i, h) in headers.iter().enumerate() {
        if prev.is_some_and(|p| p != h.parent_hash) {
            return Err(i);
        }
        prev = Some(hash(h));
    }
    prev.ok_or(0)
}
