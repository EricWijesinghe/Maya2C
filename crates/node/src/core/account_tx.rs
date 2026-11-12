//! Wire version 9: account-bound transactions (ADR-086 §3.2).
//!
//! ```text
//! 9 ‖ account[32] ‖ inputs ‖ outputs ‖ policy ‖ nonce:u64le
//!   ‖ count:u8 ‖ (index:u8 ‖ sig_len:u32le ‖ sig)* ‖ has_payload:u8 [‖ payload]
//! ```
//!
//! A v8 frame whose sender is the account it names rather than the policy's
//! own address. That is safe only beside a stored authority record: the
//! account must have an `ka:a:` record whose policy digest equals this
//! frame's policy (Rule A, `state::account_authority`), and with no record a
//! v9 frame is refused, because otherwise anyone could name any account.
//!
//! Every approver signs
//! `maya2c/account-bound/v1 ‖ chain_tag ‖ account ‖ policy_digest ‖ v8 body`,
//! so an approval cannot be lifted onto another account, another chain, or
//! another policy — nor read as a v8 approval, whose domain differs.
//! [`Transaction::verify`] refuses every v9 frame (it refuses every multisig
//! frame); only `verify_at` checks one, and staging refuses one below the
//! `ACCOUNT_AUTHORITY` activation height before any signature is checked.

use maya_crypto_pq::multisig::{MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::SuiteId;

use crate::core::codec::ByteReader;
use crate::core::multisig_tx::{self, MultisigAuth};
use crate::core::transaction::{ChainTag, Transaction};
use crate::error::{NodeError, Result};

/// The wire version byte of an account-bound transaction.
pub const WIRE_VERSION_ACCOUNT: u8 = 9;

/// Signing domain (ADR-086).
const ACCOUNT_BOUND_DOMAIN: &[u8] = b"maya2c/account-bound/v1";

/// The bytes a v9 transaction encodes without domain or chain tag.
pub(crate) fn body_bytes(tx: &Transaction, auth: &MultisigAuth, account: &[u8; 32]) -> Vec<u8> {
    let body = multisig_tx::body_bytes(tx, auth);
    let mut buf = Vec::with_capacity(64 + body.len());
    buf.extend_from_slice(account);
    buf.extend_from_slice(&auth.policy().digest());
    buf.extend_from_slice(&body);
    buf
}

/// The bytes every approver of a v9 frame signs.
pub(crate) fn signing_bytes(
    tx: &Transaction,
    auth: &MultisigAuth,
    account: &[u8; 32],
    chain: &ChainTag,
) -> Vec<u8> {
    let body = body_bytes(tx, auth, account);
    let mut buf = Vec::with_capacity(ACCOUNT_BOUND_DOMAIN.len() + 32 + body.len());
    buf.extend_from_slice(ACCOUNT_BOUND_DOMAIN);
    buf.extend_from_slice(&chain.0);
    buf.extend_from_slice(&body);
    buf
}

/// The v9 wire encoding.
pub(crate) fn encode(tx: &Transaction, auth: &MultisigAuth, account: &[u8; 32]) -> Vec<u8> {
    let mut buf = vec![WIRE_VERSION_ACCOUNT];
    buf.extend_from_slice(account);
    multisig_tx::encode_tail(tx, auth, &mut buf);
    buf
}

/// Decodes a v9 frame; the version byte has been read.
pub(crate) fn decode(reader: &mut ByteReader<'_>) -> Result<Transaction> {
    let account: [u8; 32] = reader.read_array()?;
    let mut tx = multisig_tx::decode(reader)?;
    tx.multisig = tx.multisig.map(|auth| Box::new(auth.bound_to(account)));
    Ok(tx)
}

/// The policy that authorized `tx`: a v8/v9 frame's own policy, or the 1-of-1
/// policy over the key of a v5/v6 hybrid or v7 suite frame — "a single key is a
/// 1-of-1 policy" (ADR-086 §3.1). Its digest is what an authority record
/// stores when this transaction opts the account in.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] when the key does not form a policy (a
/// classical suite, or a key of the wrong length).
pub fn signer_policy(tx: &Transaction) -> Result<MultisigPolicy> {
    if let Some(auth) = &tx.multisig {
        return Ok(auth.policy().clone());
    }
    let key = match &tx.suite_auth {
        Some(auth) => PolicyKey {
            suite: auth.suite,
            public_key: auth.public_key.clone(),
        },
        None => {
            let mut public_key = Vec::new();
            tx.public_key.encode_into(&mut public_key);
            PolicyKey {
                suite: SuiteId::HybridMlDsa65SlhDsa128s,
                public_key,
            }
        }
    };
    MultisigPolicy::new(1, vec![key]).map_err(|e| NodeError::SignatureSuite(e.to_string()))
}
