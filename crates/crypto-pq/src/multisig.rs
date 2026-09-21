//! m-of-n multisignature policies over the suite registry.
//!
//! # What this is
//!
//! A policy is a threshold `m` and an ordered list of `n` suite-tagged public
//! keys. A message is authorized when at least `m` *distinct* listed keys have
//! signed it. There is no aggregation and no interaction: each signer produces
//! an ordinary FIPS 204/205 signature, and the verifier checks them one by
//! one. That is the point — every piece of it is a primitive already verified
//! against NIST vectors (`tests/acvp_tests.rs`), so nothing here is new
//! cryptography.
//!
//! A chain enforces a policy by making it the address: the account is the
//! [`MultisigPolicy::digest`], and a spend must carry the policy whose digest
//! it is. Changing one key, the order, or `m` is a different account.
//!
//! # What this is not
//!
//! A threshold signature. `n` signatures travel and are checked, so the cost
//! grows with the quorum — see `docs/adr/ADR-011-threshold-custody.md` for why
//! true threshold lattice signing stays RESEARCH.
//!
//! # Rules a policy enforces at construction
//!
//! - `1 ≤ m ≤ n ≤` [`MAX_SIGNERS`]: an unmeetable or vacuous policy is refused
//!   rather than creating an account nobody, or anybody, can spend from.
//! - No key listed twice: otherwise one signer would count twice.
//! - No suite without post-quantum security: a multisig whose members could be
//!   forged by a quantum adversary is not a custody control.

use crate::suite::{self, SuiteError, SuiteId};

/// Most keys a policy may list. Sixteen ML-DSA-87 signatures are ~74 KB per
/// spend; a larger committee belongs to a threshold scheme, not this one.
pub const MAX_SIGNERS: usize = 16;

/// Domain for [`MultisigPolicy::digest`].
const POLICY_DOMAIN: &str = "maya2c.crypto-pq.multisig-policy.v1";

/// Errors from building, decoding, or checking a policy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MultisigError {
    /// `m` or `n` outside `1 ≤ m ≤ n ≤ MAX_SIGNERS`.
    #[error("threshold {threshold} of {signers} is not a meetable policy (max {MAX_SIGNERS})")]
    BadThreshold {
        /// The `m` asked for.
        threshold: usize,
        /// The `n` supplied.
        signers: usize,
    },
    /// The same key appears twice.
    #[error("key {second} duplicates key {first}")]
    DuplicateKey {
        /// Earlier position.
        first: usize,
        /// Later position.
        second: usize,
    },
    /// A suite with no post-quantum security was listed.
    #[error("{0:?} has no post-quantum security and cannot join a policy")]
    ClassicalSuite(SuiteId),
    /// Fewer approvals than the threshold.
    #[error("{received} approvals, the policy needs {threshold}")]
    BelowThreshold {
        /// Approvals presented.
        received: usize,
        /// The policy's `m`.
        threshold: usize,
    },
    /// Approvals out of order, repeated, or naming no listed key.
    #[error("approval index {0} is repeated, out of order, or not in the policy")]
    BadIndex(u8),
    /// A key or signature failed its suite's checks.
    #[error("signer {index}: {source}")]
    Suite {
        /// The approval's index.
        index: u8,
        /// What the suite said.
        source: SuiteError,
    },
    /// The encoding is malformed.
    #[error("malformed policy encoding: {0}")]
    Malformed(&'static str),
}

/// One member of a policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyKey {
    /// The member's suite.
    pub suite: SuiteId,
    /// The encoded public key, exactly the suite's length.
    pub public_key: Vec<u8>,
}

/// A validated m-of-n policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultisigPolicy {
    threshold: u8,
    keys: Vec<PolicyKey>,
}

/// One signer's approval: which listed key, and its signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    /// Position of the signing key in the policy.
    pub index: u8,
    /// The signature over the message.
    pub signature: Vec<u8>,
}

impl MultisigPolicy {
    /// Validates and builds a policy.
    ///
    /// # Errors
    ///
    /// [`MultisigError::BadThreshold`], [`MultisigError::DuplicateKey`],
    /// [`MultisigError::ClassicalSuite`], or a key-length error.
    pub fn new(threshold: usize, keys: Vec<PolicyKey>) -> Result<Self, MultisigError> {
        if threshold == 0 || threshold > keys.len() || keys.len() > MAX_SIGNERS {
            return Err(MultisigError::BadThreshold {
                threshold,
                signers: keys.len(),
            });
        }
        for (position, key) in keys.iter().enumerate() {
            check_key(position, key)?;
            if let Some(first) = keys[..position].iter().position(|k| k == key) {
                return Err(MultisigError::DuplicateKey {
                    first,
                    second: position,
                });
            }
        }
        Ok(Self {
            threshold: u8::try_from(threshold).map_err(|_| MultisigError::BadThreshold {
                threshold,
                signers: keys.len(),
            })?,
            keys,
        })
    }

    /// `m`.
    #[must_use]
    pub fn threshold(&self) -> usize {
        usize::from(self.threshold)
    }

    /// The members, in policy order.
    #[must_use]
    pub fn keys(&self) -> &[PolicyKey] {
        &self.keys
    }

    /// The canonical encoding: `m:u8 ‖ n:u8 ‖ (suite:u8 ‖ key)*`. Key lengths
    /// are the registry's, so none is written and none can be chosen.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = vec![
            self.threshold,
            u8::try_from(self.keys.len()).unwrap_or(u8::MAX),
        ];
        for key in &self.keys {
            buf.push(key.suite.to_byte());
            buf.extend_from_slice(&key.public_key);
        }
        buf
    }

    /// Decodes [`MultisigPolicy::encode`], re-running every construction rule,
    /// and returns the bytes consumed.
    ///
    /// # Errors
    ///
    /// [`MultisigError::Malformed`] for a short or unknown encoding, and any
    /// error [`MultisigPolicy::new`] returns.
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), MultisigError> {
        let [threshold, signers, rest @ ..] = bytes else {
            return Err(MultisigError::Malformed("header"));
        };
        // Checked before any key is read, so a hostile count costs nothing.
        if usize::from(*signers) > MAX_SIGNERS {
            return Err(MultisigError::BadThreshold {
                threshold: usize::from(*threshold),
                signers: usize::from(*signers),
            });
        }
        let mut keys = Vec::with_capacity(usize::from(*signers));
        let mut offset = 0;
        for _ in 0..*signers {
            let byte = *rest.get(offset).ok_or(MultisigError::Malformed("suite"))?;
            let suite = SuiteId::try_from(byte).map_err(|_| MultisigError::Malformed("suite"))?;
            let len = suite.info().public_key_len;
            let public_key = rest
                .get(offset + 1..offset + 1 + len)
                .ok_or(MultisigError::Malformed("key"))?
                .to_vec();
            keys.push(PolicyKey { suite, public_key });
            offset += 1 + len;
        }
        Ok((Self::new(usize::from(*threshold), keys)?, 2 + offset))
    }

    /// The account this policy controls: BLAKE3 in derive-key mode over the
    /// canonical encoding.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key(POLICY_DOMAIN);
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }

    /// Checks that `approvals` authorize `message`.
    ///
    /// Approvals must be in strictly increasing index order — which makes a
    /// repeated signer unrepresentable rather than merely detected — and
    /// *every* one presented must verify. An invalid signature is refused even
    /// when the valid ones would have met the threshold: a spend carrying
    /// garbage is not one a relayer should be able to pad.
    ///
    /// # Errors
    ///
    /// [`MultisigError::BelowThreshold`], [`MultisigError::BadIndex`], or
    /// [`MultisigError::Suite`] naming the first signer that failed.
    pub fn verify(&self, message: &[u8], approvals: &[Approval]) -> Result<(), MultisigError> {
        if approvals.len() < self.threshold() {
            return Err(MultisigError::BelowThreshold {
                received: approvals.len(),
                threshold: self.threshold(),
            });
        }
        let mut previous: Option<u8> = None;
        for approval in approvals {
            let in_order = previous.is_none_or(|p| approval.index > p);
            let key = self
                .keys
                .get(usize::from(approval.index))
                .filter(|_| in_order)
                .ok_or(MultisigError::BadIndex(approval.index))?;
            suite::verify(key.suite, &key.public_key, message, &approval.signature).map_err(
                |source| MultisigError::Suite {
                    index: approval.index,
                    source,
                },
            )?;
            previous = Some(approval.index);
        }
        Ok(())
    }
}

/// A listed key's suite must be post-quantum and its length the registry's.
fn check_key(position: usize, key: &PolicyKey) -> Result<(), MultisigError> {
    let info = key.suite.info();
    if info.pq_security_bits == 0 {
        return Err(MultisigError::ClassicalSuite(key.suite));
    }
    if key.public_key.len() != info.public_key_len {
        return Err(MultisigError::Suite {
            index: u8::try_from(position).unwrap_or(u8::MAX),
            source: SuiteError::PublicKeyLength {
                suite: key.suite,
                expected: info.public_key_len,
                got: key.public_key.len(),
            },
        });
    }
    Ok(())
}
