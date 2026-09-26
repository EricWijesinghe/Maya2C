//! Wire version 8: m-of-n multisig transactions.
//!
//! ```text
//! 8 ‖ inputs ‖ outputs ‖ policy ‖ nonce:u64le
//!   ‖ count:u8 ‖ (index:u8 ‖ sig_len:u32le ‖ sig)* ‖ has_payload:u8 [‖ payload]
//! ```
//!
//! `policy` is [`MultisigPolicy::encode`]; the sender is its digest, so the
//! account *is* the policy and a spend has to present exactly the policy the
//! address commits to. Every approver signs the same bytes — the domain, the
//! inputs and outputs, the whole policy, the nonce and the payload — and
//! approvals are excluded from them, so collecting signatures needs no
//! ordering among signers.
//!
//! Each signature's length is the registry's for the key at its index,
//! checked before the bytes are read. Gated exactly as v7 is: only
//! [`Transaction::verify_at`] can accept one (live from genesis, ADR-013), and
//! every listed key's suite must be admissible at that height.

use maya_crypto_pq::agility::SuitePolicy;
use maya_crypto_pq::multisig::{Approval, MAX_SIGNERS, MultisigPolicy};

use crate::core::codec::ByteReader;
use crate::core::payload::TxKind;
use crate::core::transaction::Transaction;
use crate::crypto::suites;
use crate::error::{NodeError, Result};

/// The wire version byte of a multisig transaction.
pub const WIRE_VERSION_MULTISIG: u8 = 8;

/// Signing domain; shares nothing with v3 hybrid or v7 suite signing.
const TX_DOMAIN_MULTISIG: &[u8] = b"custom-l1-node.tx.multisig.v1";

/// Domain for a multisig account address, distinct from both other address
/// derivations so no key and no policy can name the same account.
const MULTISIG_ADDRESS_DOMAIN: &str = "maya2c 2026-09-21 multisig account address v1";

/// A policy and the approvals collected for one transaction.
///
/// Fields are private so that no instance holds more approvals than the
/// policy has keys, or approvals out of order: the wire count is one byte and
/// the verifier requires strictly increasing indices, and a value that could
/// violate either would encode to a frame no node decodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultisigAuth {
    policy: MultisigPolicy,
    approvals: Vec<Approval>,
}

impl MultisigAuth {
    /// A policy with nothing approved yet — what the signers sign over.
    #[must_use]
    pub fn unsigned(policy: MultisigPolicy) -> Self {
        Self {
            policy,
            approvals: Vec::new(),
        }
    }

    /// A policy with `approvals`, checked for shape (not for validity: that
    /// is [`Transaction::verify_at`]'s job).
    ///
    /// # Errors
    ///
    /// [`NodeError::SignatureSuite`] for more approvals than keys, an index
    /// outside the policy, or indices not strictly increasing.
    pub fn new(policy: MultisigPolicy, approvals: Vec<Approval>) -> Result<Self> {
        let keys = policy.keys().len();
        let ordered = approvals.windows(2).all(|w| w[0].index < w[1].index);
        let inside = approvals.iter().all(|a| usize::from(a.index) < keys);
        if approvals.len() > keys || !ordered || !inside {
            return Err(NodeError::SignatureSuite(
                "multisig approvals must be distinct listed keys in increasing order".into(),
            ));
        }
        Ok(Self { policy, approvals })
    }

    /// The policy the sender address commits to.
    #[must_use]
    pub const fn policy(&self) -> &MultisigPolicy {
        &self.policy
    }

    /// The approvals, in strictly increasing index order.
    #[must_use]
    pub fn approvals(&self) -> &[Approval] {
        &self.approvals
    }
}

/// The account a policy controls.
#[must_use]
pub fn multisig_address(policy: &MultisigPolicy) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(MULTISIG_ADDRESS_DOMAIN);
    hasher.update(&policy.digest());
    *hasher.finalize().as_bytes()
}

/// The bytes every approver signs.
pub(crate) fn signing_bytes(tx: &Transaction, auth: &MultisigAuth) -> Vec<u8> {
    let policy = auth.policy.encode();
    let mut buf = Vec::with_capacity(TX_DOMAIN_MULTISIG.len() + 32 + policy.len());
    buf.extend_from_slice(TX_DOMAIN_MULTISIG);
    tx.encode_io_into(&mut buf);
    buf.extend_from_slice(&policy);
    buf.extend_from_slice(&tx.nonce.to_le_bytes());
    tx.kind.encode_into(&mut buf);
    buf
}

/// The v8 wire encoding.
pub(crate) fn encode(tx: &Transaction, auth: &MultisigAuth) -> Vec<u8> {
    let mut buf = vec![WIRE_VERSION_MULTISIG];
    tx.encode_io_into(&mut buf);
    buf.extend_from_slice(&auth.policy.encode());
    buf.extend_from_slice(&tx.nonce.to_le_bytes());
    // `MultisigAuth::new` holds this to at most the policy's key count (≤ 16).
    buf.push(u8::try_from(auth.approvals.len()).unwrap_or(u8::MAX));
    for approval in &auth.approvals {
        buf.push(approval.index);
        buf.extend_from_slice(
            &u32::try_from(approval.signature.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        buf.extend_from_slice(&approval.signature);
    }
    buf.push(u8::from(tx.kind.has_payload()));
    tx.kind.encode_into(&mut buf);
    buf
}

/// Reads one approval, its length checked against the key it names.
fn decode_approval(reader: &mut ByteReader<'_>, policy: &MultisigPolicy) -> Result<Approval> {
    let index = reader.read_u8()?;
    let key = policy
        .keys()
        .get(usize::from(index))
        .ok_or_else(|| NodeError::Decode(format!("approval index {index} is not in the policy")))?;
    let expected = key.suite.info().signature_len;
    let got = reader.read_u32()?;
    if usize::try_from(got).ok() != Some(expected) {
        return Err(NodeError::Decode(format!(
            "approval {index}: signature length {got}, {:?} requires {expected}",
            key.suite
        )));
    }
    Ok(Approval {
        index,
        signature: reader.read_slice(expected)?.to_vec(),
    })
}

/// Decodes a v8 frame; the version byte has been read.
pub(crate) fn decode(reader: &mut ByteReader<'_>) -> Result<Transaction> {
    let (inputs, outputs) = Transaction::decode_io(reader)?;
    let (policy, used) = MultisigPolicy::decode(reader.peek_remaining())
        .map_err(|e| NodeError::Decode(e.to_string()))?;
    reader.read_slice(used)?;
    let nonce = reader.read_u64()?;
    let count = usize::from(reader.read_u8()?);
    if count > policy.keys().len().min(MAX_SIGNERS) {
        return Err(NodeError::Decode(format!(
            "{count} approvals for a policy of {}",
            policy.keys().len()
        )));
    }
    let approvals = (0..count)
        .map(|_| decode_approval(reader, &policy))
        .collect::<Result<Vec<_>>>()?;
    let kind = match reader.read_u8()? {
        0 => TxKind::Transfer,
        1 => match TxKind::decode(reader)? {
            kind if kind.has_payload() => kind,
            _ => {
                return Err(NodeError::Decode(
                    "payload flag set without a payload".into(),
                ));
            }
        },
        other => return Err(NodeError::Decode(format!("invalid payload flag {other}"))),
    };
    if reader.remaining() != 0 {
        return Err(NodeError::Decode(format!(
            "{} trailing bytes after a v8 transaction",
            reader.remaining()
        )));
    }
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.inputs = inputs;
    tx.outputs = outputs;
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy, approvals).map_err(|e| NodeError::Decode(e.to_string()))?,
    ));
    Ok(tx)
}

/// v8 verification: activation and every member's suite, then the quorum.
pub(crate) fn verify_at(
    tx: &Transaction,
    auth: &MultisigAuth,
    height: u64,
    policy: &SuitePolicy,
) -> Result<()> {
    // Every listed key, not only the approvers: a policy naming a suite the
    // chain has retired is an account that must migrate, and letting the
    // current quorum happen to avoid that key would hide it.
    for key in auth.policy.keys() {
        suites::check_admissible(policy, key.suite, height)?;
    }
    auth.policy
        .verify(&signing_bytes(tx, auth), &auth.approvals)
        .map_err(|e| NodeError::SignatureSuite(e.to_string()))
}
