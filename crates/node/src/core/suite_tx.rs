//! Wire version 7: suite-tagged transactions — ADR-007.
//!
//! ```text
//! 7 ‖ inputs ‖ outputs ‖ suite:u8 ‖ pk_len:u32le ‖ pk ‖ nonce:u64le
//!   ‖ has_sig:u8 [‖ sig_len:u32le ‖ sig] ‖ has_payload:u8 [‖ payload]
//! ```
//!
//! Both lengths must equal the registry's for the suite — checked before the
//! bytes are read, so a hostile length costs nothing — and the signing bytes
//! commit to the suite id as well as the key, so a signature cannot be
//! re-presented under another suite that happens to accept the same bytes.
//!
//! Live from genesis (ADR-013), but only through [`Transaction::verify_at`]:
//! the height-less `verify` refuses every v7 frame, so a path that forgets to
//! pass a height fails closed.

use maya_crypto_pq::agility::SuitePolicy;
use maya_crypto_pq::suite::{self as pq_suite, SignatureSuite, SuiteId};

use crate::core::codec::ByteReader;
use crate::core::payload::TxKind;
use crate::core::transaction::{ChainTag, Transaction};
use crate::crypto::suites;
use crate::error::{NodeError, Result};

/// The wire version byte of a suite-tagged transaction.
pub const WIRE_VERSION_SUITE: u8 = 7;

/// Signing domain for suite-tagged transactions; bumped to v2 with chain-bound
/// signatures (ADR-036). Suite-tagged transactions now commit to the chain's
/// genesis block id to prevent cross-chain replay.
const TX_DOMAIN_SUITE: &[u8] = b"custom-l1-node.tx.suite.v2";

/// One suite's key and signature, as a v7 transaction carries them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuiteAuth {
    /// The suite.
    pub suite: SuiteId,
    /// The encoded public key, exactly the suite's length.
    pub public_key: Vec<u8>,
    /// The signature over [`Transaction::signing_bytes`], once signed.
    pub signature: Option<Vec<u8>>,
}

fn write_len(buf: &mut Vec<u8>, len: usize) {
    buf.extend_from_slice(
        &u32::try_from(len)
            .expect("registry lengths fit in u32")
            .to_le_bytes(),
    );
}

fn read_exact_len(reader: &mut ByteReader<'_>, expected: usize, what: &str) -> Result<()> {
    let got = reader.read_u32()?;
    if usize::try_from(got).ok() == Some(expected) {
        Ok(())
    } else {
        Err(NodeError::Decode(format!(
            "{what} length {got}, suite requires {expected}"
        )))
    }
}

/// The bytes a v7 transaction encodes without domain or chain tag.
fn body_bytes(tx: &Transaction, auth: &SuiteAuth) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32 + auth.public_key.len());
    tx.encode_io_into(&mut buf);
    buf.push(auth.suite.to_byte());
    write_len(&mut buf, auth.public_key.len());
    buf.extend_from_slice(&auth.public_key);
    buf.extend_from_slice(&tx.nonce.to_le_bytes());
    tx.kind.encode_into(&mut buf);
    buf
}

/// The bytes a v7 signature covers.
///
/// Structure: TX_DOMAIN || chain_tag || body_bytes()
/// Includes the chain tag (ADR-036) to bind the signature to a specific chain,
/// preventing cross-chain replay attacks.
pub(crate) fn signing_bytes(tx: &Transaction, auth: &SuiteAuth, chain: &ChainTag) -> Vec<u8> {
    let body = body_bytes(tx, auth);
    let mut buf = Vec::with_capacity(TX_DOMAIN_SUITE.len() + 32 + body.len());
    buf.extend_from_slice(TX_DOMAIN_SUITE);
    buf.extend_from_slice(&chain.0);
    buf.extend_from_slice(&body);
    buf
}

/// The v7 wire encoding.
pub(crate) fn encode(tx: &Transaction, auth: &SuiteAuth) -> Vec<u8> {
    let mut buf = vec![WIRE_VERSION_SUITE];
    tx.encode_io_into(&mut buf);
    buf.push(auth.suite.to_byte());
    write_len(&mut buf, auth.public_key.len());
    buf.extend_from_slice(&auth.public_key);
    buf.extend_from_slice(&tx.nonce.to_le_bytes());
    match &auth.signature {
        Some(signature) => {
            buf.push(1);
            write_len(&mut buf, signature.len());
            buf.extend_from_slice(signature);
        }
        None => buf.push(0),
    }
    buf.push(u8::from(tx.kind.has_payload()));
    tx.kind.encode_into(&mut buf);
    buf
}

/// Decodes a v7 frame; the version byte has been read.
pub(crate) fn decode(reader: &mut ByteReader<'_>) -> Result<Transaction> {
    let (inputs, outputs) = Transaction::decode_io(reader)?;
    let suite =
        SuiteId::try_from(reader.read_u8()?).map_err(|e| NodeError::Decode(e.to_string()))?;
    let info = suite.info();
    read_exact_len(reader, info.public_key_len, "public key")?;
    let public_key = reader.read_slice(info.public_key_len)?.to_vec();
    let nonce = reader.read_u64()?;
    let signature = match reader.read_u8()? {
        0 => None,
        1 => {
            read_exact_len(reader, info.signature_len, "signature")?;
            Some(reader.read_slice(info.signature_len)?.to_vec())
        }
        other => {
            return Err(NodeError::Decode(format!(
                "invalid signature presence flag {other}"
            )));
        }
    };
    let kind = match reader.read_u8()? {
        0 => TxKind::Transfer,
        // A set flag over a payload-less kind would be a second encoding of a
        // transfer; one transaction, one encoding.
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
            "{} trailing bytes after a v7 transaction",
            reader.remaining()
        )));
    }
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.inputs = inputs;
    tx.outputs = outputs;
    tx.suite_auth = Some(Box::new(SuiteAuth {
        suite,
        public_key,
        signature,
    }));
    Ok(tx)
}

impl Transaction {
    /// Signs as a suite-tagged (v7) transaction under suite `S`.
    ///
    /// The signature commits to the chain tag, binding it to a specific chain
    /// and preventing cross-chain replay (ADR-036).
    ///
    /// # Errors
    ///
    /// [`NodeError::SignatureSuite`] if the backend refuses to sign.
    pub fn sign_with_suite<S: SignatureSuite>(
        &mut self,
        key: &S::SigningKey,
        chain: &ChainTag,
    ) -> Result<()> {
        // Built and signed on the side, then installed: on failure `self` is
        // exactly as it was, never a half-built unsigned v7 frame.
        let draft = SuiteAuth {
            suite: S::ID,
            public_key: S::public_key(key),
            signature: None,
        };
        let signature = S::sign(key, &signing_bytes(self, &draft, chain))
            .map_err(|e| NodeError::SignatureSuite(e.to_string()))?;
        self.suite_auth = Some(Box::new(SuiteAuth {
            signature: Some(signature),
            ..draft
        }));
        Ok(())
    }

    /// Verifies at `height` under `policy`: the hybrid rule for v5/v6; for v7
    /// the activation gate, the policy, then the suite's signature; for v8 the
    /// gate and policy for every listed key, then the quorum.
    ///
    /// The signature must commit to the provided chain tag (ADR-036).
    ///
    /// # Errors
    ///
    /// As [`Transaction::verify`] for hybrid transactions;
    /// [`NodeError::SignatureSuite`] or [`NodeError::MissingSignature`] for v7.
    pub fn verify_at(&self, height: u64, policy: &SuitePolicy, chain: &ChainTag) -> Result<()> {
        if let Some(multisig) = &self.multisig {
            // One authorization per transaction: a frame carrying two would
            // let the signed bytes and the checked authority disagree.
            if self.suite_auth.is_some() || self.signature.is_some() {
                return Err(NodeError::SignatureSuite(
                    "a multisig transaction carries no other authorization".into(),
                ));
            }
            return crate::core::multisig_tx::verify_at(self, multisig, height, policy, chain);
        }
        let Some(auth) = &self.suite_auth else {
            return self.verify(chain);
        };
        suites::check_admissible(policy, auth.suite, height)?;
        let signature = auth
            .signature
            .as_deref()
            .ok_or(NodeError::MissingSignature)?;
        pq_suite::verify(
            auth.suite,
            &auth.public_key,
            &signing_bytes(self, auth, chain),
            signature,
        )
        .map_err(|e| NodeError::SignatureSuite(e.to_string()))
    }
}
