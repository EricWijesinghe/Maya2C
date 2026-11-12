//! Wire version 13: a sponsored frame — ADR-135 P-4 (ADR-106's design, with
//! the version byte renumbered from 11 to `0x0D`).
//!
//! ```text
//! 0x0D ‖ inner_len:u32le ‖ inner (one v5/v6/v7/v8 frame, never another v13)
//!      ‖ suite:u8 ‖ pk_len:u32le ‖ sponsor_pk ‖ max_fee:32le ‖ valid_until:u64le
//!      ‖ sig_len:u32le ‖ signature
//! sponsor signing bytes =
//!     "maya2c/sponsor/v1" ‖ chain_tag(32) ‖ txid(inner)(32) ‖ max_fee(32 LE) ‖ valid_until(8 LE)
//! ```
//!
//! A sponsor pays the fee of somebody else's transaction. The inner frame is
//! signed by its sender exactly as if it stood alone; the sponsor then signs
//! the inner's txid, a ceiling and an expiry. Why each field is there:
//!
//! - `txid(inner)` covers the inner's nonce and signatures, so one sponsorship
//!   pays for one transaction once: a replay fails on the nonce (threat model
//!   P-1), and changing a byte of the inner changes the txid it signed.
//! - `max_fee` is the most this one sponsorship can ever cost. Apply debits
//!   `required(outer_len)` and refuses the frame if that is above `max_fee`.
//! - `valid_until` and the chain tag stop a stale or another chain's
//!   sponsorship being spent (P-3, P-8).
//!
//! Depth is one: an inner v13 frame is refused at decode, before it is
//! parsed, so a nested frame costs one byte comparison (P-9).
//!
//! The consensus rules on the fee itself are in [`crate::state::sponsor`]; the
//! height-less [`Transaction::verify`] refuses every v13 frame and only
//! [`Transaction::verify_at`] checks the sponsor's signature (invariant 31).

use maya_crypto_pq::agility::SuitePolicy;
use maya_crypto_pq::suite::{self as pq_suite, SignatureSuite, SuiteId};
use primitive_types::U256;

use crate::core::codec::ByteReader;
use crate::core::transaction::{ChainTag, Transaction};
use crate::crypto::keys::ADDRESS_LEN;
use crate::crypto::suites;
use crate::error::{NodeError, Result};

/// The wire version byte of a sponsored frame.
pub const WIRE_VERSION_SPONSORED: u8 = 0x0D;

/// Longest a sponsorship may still have to run when a node admits it to its
/// mempool: 86,400 blocks. Node-local admission policy (Eric, O-4), never a
/// validity rule: a block may carry a frame with a longer window.
pub const MAX_SPONSOR_WINDOW: u64 = 86_400;

/// Most sponsored frames one sponsor may have pending in a node's mempool.
/// Node-local admission policy (Eric, O-4; security-architect S-5): a block
/// holding a 65th frame from one sponsor is valid if every frame applies.
pub const MAX_SPONSORED_PENDING_PER_SPONSOR: usize = 64;

/// Domain of the sponsor's signed bytes. Distinct from every transaction
/// domain, so a sponsorship can never be read as a transaction signature.
const SPONSOR_DOMAIN: &[u8] = b"maya2c/sponsor/v1";

/// The outer half of a v13 frame: who pays, how much at most, until when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sponsorship {
    /// The sponsor's signature suite.
    pub suite: SuiteId,
    /// The sponsor's encoded public key, exactly the suite's length.
    pub public_key: Vec<u8>,
    /// The most this sponsorship pays, in base units.
    pub max_fee: U256,
    /// Last height at which the sponsorship may apply.
    pub valid_until: u64,
    /// The sponsor's signature over [`signing_bytes`].
    pub signature: Vec<u8>,
}

impl Sponsorship {
    /// The account that pays: derived from the suite and key, never a wire
    /// field, so a frame cannot name a payer whose key did not sign.
    #[must_use]
    pub fn address(&self) -> [u8; ADDRESS_LEN] {
        suites::suite_address(self.suite, &self.public_key)
    }
}

/// The bytes a sponsor signs.
#[must_use]
pub fn signing_bytes(
    chain: &ChainTag,
    inner_txid: &[u8; 32],
    max_fee: &U256,
    valid_until: u64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(SPONSOR_DOMAIN.len() + 32 + 32 + 32 + 8);
    buf.extend_from_slice(SPONSOR_DOMAIN);
    buf.extend_from_slice(&chain.0);
    buf.extend_from_slice(inner_txid);
    crate::core::codec::put_amount(&mut buf, max_fee);
    buf.extend_from_slice(&valid_until.to_le_bytes());
    buf
}

/// A length field. Every length written here is a registry length or an
/// inner frame the decoder bounds at `MAX_TX_BYTES`; a value past `u32` is
/// written as `u32::MAX`, which no decoder accepts, rather than panicking.
fn write_len(buf: &mut Vec<u8>, len: usize) {
    buf.extend_from_slice(&u32::try_from(len).unwrap_or(u32::MAX).to_le_bytes());
}

/// The v13 wire encoding of `tx` sponsored by `sponsorship`.
pub(crate) fn encode(tx: &Transaction, sponsorship: &Sponsorship) -> Vec<u8> {
    let inner = tx.inner_bytes();
    let mut buf = Vec::with_capacity(
        1 + 4
            + inner.len()
            + 1
            + 4
            + sponsorship.public_key.len()
            + 32
            + 8
            + 4
            + sponsorship.signature.len(),
    );
    buf.push(WIRE_VERSION_SPONSORED);
    write_len(&mut buf, inner.len());
    buf.extend_from_slice(&inner);
    buf.push(sponsorship.suite.to_byte());
    write_len(&mut buf, sponsorship.public_key.len());
    buf.extend_from_slice(&sponsorship.public_key);
    crate::core::codec::put_amount(&mut buf, &sponsorship.max_fee);
    buf.extend_from_slice(&sponsorship.valid_until.to_le_bytes());
    write_len(&mut buf, sponsorship.signature.len());
    buf.extend_from_slice(&sponsorship.signature);
    buf
}

/// Decodes a v13 frame; the version byte has been read.
///
/// # Errors
///
/// [`NodeError::Decode`] for an inner v13 frame (depth 1), an inner frame the
/// transaction decoder refuses, a suite the registry does not know, a key or
/// signature of the wrong length, truncation, or trailing bytes.
pub(crate) fn decode(reader: &mut ByteReader<'_>) -> Result<Transaction> {
    let inner_len = usize::try_from(reader.read_u32()?)
        .map_err(|_| NodeError::Decode("inner frame length".into()))?;
    let inner = reader.read_slice(inner_len)?;
    // Before the inner is parsed: a nested frame costs one comparison.
    if inner.first() == Some(&WIRE_VERSION_SPONSORED) {
        return Err(NodeError::Decode(
            "a v13 sponsored frame cannot wrap another v13 frame (depth 1)".into(),
        ));
    }
    let mut tx = Transaction::from_bytes(inner)?;
    let suite =
        SuiteId::try_from(reader.read_u8()?).map_err(|e| NodeError::Decode(e.to_string()))?;
    let info = suite.info();
    crate::core::suite_tx::read_exact_len(reader, info.public_key_len, "sponsor public key")?;
    let public_key = reader.read_slice(info.public_key_len)?.to_vec();
    let max_fee = reader.read_amount()?;
    let valid_until = reader.read_u64()?;
    crate::core::suite_tx::read_exact_len(reader, info.signature_len, "sponsor signature")?;
    let signature = reader.read_slice(info.signature_len)?.to_vec();
    reader.finish()?;
    tx.sponsor = Some(Box::new(Sponsorship {
        suite,
        public_key,
        max_fee,
        valid_until,
        signature,
    }));
    Ok(tx)
}

/// Verifies the sponsor's half of `tx` at `height`: the suite is admissible
/// (invariants 29 and 31), then the signature over the inner's txid.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] for an inadmissible suite or a bad signature.
pub(crate) fn verify(
    tx: &Transaction,
    sponsorship: &Sponsorship,
    height: u64,
    policy: &SuitePolicy,
    chain: &ChainTag,
) -> Result<()> {
    suites::check_admissible(policy, sponsorship.suite, height)?;
    let message = signing_bytes(
        chain,
        &tx.inner_txid(),
        &sponsorship.max_fee,
        sponsorship.valid_until,
    );
    pq_suite::verify(
        sponsorship.suite,
        &sponsorship.public_key,
        &message,
        &sponsorship.signature,
    )
    .map_err(|e| NodeError::SignatureSuite(format!("sponsor: {e}")))
}

impl Transaction {
    /// Sponsors this (already signed) transaction under suite `S`: the
    /// sponsor will pay `required(outer_len)`, at most `max_fee`, at heights
    /// up to `valid_until`.
    ///
    /// Signs on the side and installs only on success, so a failure leaves
    /// `self` exactly as it was.
    ///
    /// # Errors
    ///
    /// [`NodeError::SignatureSuite`] if the backend refuses to sign.
    pub fn sponsor_with<S: SignatureSuite>(
        &mut self,
        key: &S::SigningKey,
        max_fee: U256,
        valid_until: u64,
        chain: &ChainTag,
    ) -> Result<()> {
        let message = signing_bytes(chain, &self.inner_txid(), &max_fee, valid_until);
        let signature =
            S::sign(key, &message).map_err(|e| NodeError::SignatureSuite(e.to_string()))?;
        self.sponsor = Some(Box::new(Sponsorship {
            suite: S::ID,
            public_key: S::public_key(key),
            max_fee,
            valid_until,
            signature,
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_signed_bytes_are_the_documented_layout() {
        let chain = ChainTag::from_genesis([0xC7; 32]);
        let bytes = signing_bytes(&chain, &[0x11; 32], &U256::from(0x0102u64), 0x0A0B);
        assert_eq!(&bytes[..SPONSOR_DOMAIN.len()], b"maya2c/sponsor/v1");
        let rest = &bytes[SPONSOR_DOMAIN.len()..];
        assert_eq!(rest.len(), 32 + 32 + 32 + 8);
        assert_eq!(&rest[..32], &[0xC7; 32]);
        assert_eq!(&rest[32..64], &[0x11; 32]);
        assert_eq!(&rest[64..66], &[0x02, 0x01], "max_fee little-endian");
        assert!(rest[66..96].iter().all(|b| *b == 0));
        assert_eq!(&rest[96..], &0x0A0Bu64.to_le_bytes());
    }

    #[test]
    fn a_nested_v13_frame_is_refused_before_it_is_parsed() {
        // 0x0D, inner_len 1, inner = [0x0D]: refused on the inner's first byte.
        let frame = [WIRE_VERSION_SPONSORED, 1, 0, 0, 0, WIRE_VERSION_SPONSORED];
        let err = Transaction::from_bytes(&frame).unwrap_err();
        assert!(
            matches!(&err, NodeError::Decode(m) if m.contains("depth 1")),
            "{err:?}"
        );
    }
}
