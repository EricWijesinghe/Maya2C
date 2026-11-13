//! m-of-n multisig accounts (P-16): set one up, propose a spend, approve it,
//! collect the other members' approvals, finish.
//!
//! The policy, its rules and the wire format belong to `maya-crypto-pq` and
//! the node (ADR-011, wire version 8). Nothing here is new cryptography: this
//! module only moves an unsigned v8 transaction between members and checks
//! every approval it is handed.
//!
//! # Mixed suites
//!
//! Each member names its own suite. This wallet's key is the hybrid `0x30`,
//! a Ledger's is ML-DSA-65 `0x10`, a cold key may be SLH-DSA. A classical
//! suite is refused by [`MultisigPolicy::new`]. The per-family approval floor
//! proposed in ADR-117 is not on `master` and is not enforced here.
//!
//! # What travels between members
//!
//! The v8 transaction itself, hex-encoded, carrying the approvals collected
//! so far. Approvals are not part of the signed bytes, so members can sign in
//! any order and in parallel, and [`Proposal::merge`] combines their copies.
//! A proposal from another member is untrusted input: [`Proposal::from_hex`]
//! checks every approval it carries against the policy key it names before
//! the proposal is returned.

use custom_l1_node::core::Transaction;
use custom_l1_node::core::describe::is_fee_output;
use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
use custom_l1_node::core::payload::TxKind;
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{self, SuiteId};
use serde::{Deserialize, Serialize};

use crate::error::{Result, WalletError};
use crate::payment::SignedTransfer;
use crate::wallet::ChainTag;

/// One member key, as a person pastes or copies it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberKey {
    /// The suite byte (`0x30` hybrid, `0x10` ML-DSA-65, ...).
    pub suite: u8,
    /// The encoded public key, hex.
    pub public_key: String,
}

impl MemberKey {
    /// The policy member this names.
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] for an unknown suite or non-hex key.
    pub fn to_policy_key(&self) -> Result<PolicyKey> {
        let suite = SuiteId::try_from(self.suite)
            .map_err(|e| WalletError::Multisig(format!("member suite: {e}")))?;
        let public_key = hex::decode(self.public_key.trim())
            .map_err(|_| WalletError::Multisig("a member key is not hexadecimal".into()))?;
        Ok(PolicyKey { suite, public_key })
    }

    /// The form [`MemberKey::to_policy_key`] reads back.
    #[must_use]
    pub fn from_policy_key(key: &PolicyKey) -> Self {
        Self {
            suite: key.suite.to_byte(),
            public_key: hex::encode(&key.public_key),
        }
    }
}

/// A multisig account: the policy, which is the account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultisigAccount {
    policy: MultisigPolicy,
}

impl MultisigAccount {
    /// Builds an `threshold`-of-`members.len()` account.
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] for anything [`MultisigPolicy::new`] refuses:
    /// an unmeetable threshold, a repeated key, a classical suite, a wrong
    /// key length.
    pub fn new(threshold: usize, members: &[MemberKey]) -> Result<Self> {
        let keys = members
            .iter()
            .map(MemberKey::to_policy_key)
            .collect::<Result<Vec<_>>>()?;
        let policy = MultisigPolicy::new(threshold, keys)
            .map_err(|e| WalletError::Multisig(e.to_string()))?;
        Ok(Self { policy })
    }

    /// Reads an account another member shared with [`MultisigAccount::to_hex`].
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] for malformed hex, a malformed policy, or
    /// trailing bytes.
    pub fn from_hex(text: &str) -> Result<Self> {
        let bytes = hex::decode(text.trim())
            .map_err(|_| WalletError::Multisig("the account is not hexadecimal".into()))?;
        let (policy, used) =
            MultisigPolicy::decode(&bytes).map_err(|e| WalletError::Multisig(e.to_string()))?;
        if used != bytes.len() {
            return Err(WalletError::Multisig(
                "trailing bytes after the account policy".into(),
            ));
        }
        Ok(Self { policy })
    }

    /// The policy encoding, hex: what members share to set the account up.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.policy.encode())
    }

    /// The account's address: the policy's digest, so a changed key, order or
    /// threshold is a different account.
    #[must_use]
    pub fn address(&self) -> [u8; 32] {
        multisig_address(&self.policy)
    }

    /// `m`.
    #[must_use]
    pub fn threshold(&self) -> usize {
        self.policy.threshold()
    }

    /// The members, in policy order.
    #[must_use]
    pub fn members(&self) -> Vec<MemberKey> {
        self.policy
            .keys()
            .iter()
            .map(MemberKey::from_policy_key)
            .collect()
    }

    /// Where `key` sits in the policy, if it is a member.
    #[must_use]
    pub fn position_of(&self, key: &PolicyKey) -> Option<u8> {
        self.policy
            .keys()
            .iter()
            .position(|k| k == key)
            .and_then(|i| u8::try_from(i).ok())
    }

    pub(crate) fn policy(&self) -> &MultisigPolicy {
        &self.policy
    }
}

/// A spend from a multisig account and the approvals collected for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    tx: Transaction,
}

impl Proposal {
    /// `tx` as an unapproved spend from `account`.
    pub(crate) fn unsigned(account: &MultisigAccount, mut tx: Transaction) -> Self {
        // A v8 frame carries no single-signer key; clear the builder's so a
        // proposal reads back from its own hex unchanged.
        *tx.public_key = Default::default();
        tx.multisig = Some(Box::new(MultisigAuth::unsigned(account.policy().clone())));
        Self { tx }
    }

    /// Reads a proposal another member shared, checking every approval it
    /// carries against `chain`.
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] for anything but a v8 plain transfer, or an
    /// approval that does not verify.
    pub fn from_hex(text: &str, chain: &ChainTag) -> Result<Self> {
        let bytes = hex::decode(text.trim())
            .map_err(|_| WalletError::Multisig("the proposal is not hexadecimal".into()))?;
        let tx =
            Transaction::from_bytes(&bytes).map_err(|e| WalletError::Multisig(e.to_string()))?;
        if tx.multisig.is_none() {
            return Err(WalletError::Multisig(
                "not a multisig transaction (wire version 8)".into(),
            ));
        }
        // The confirm screen describes a transfer in full; anything else is
        // refused rather than approved from a summary.
        if !matches!(tx.kind, TxKind::Transfer) {
            return Err(WalletError::Multisig(
                "only plain transfers can be approved from this wallet".into(),
            ));
        }
        // The shape this wallet builds and the confirm screen shows in full:
        // one payment and at most one fee output to the collector. Any other
        // output would be money the screen does not name as a payment.
        let extra_ok = tx
            .outputs
            .get(1..)
            .unwrap_or_default()
            .iter()
            .all(is_fee_output);
        if !tx.inputs.is_empty() || tx.outputs.is_empty() || tx.outputs.len() > 2 || !extra_ok {
            return Err(WalletError::Multisig(
                "a proposal must be one payment and at most one fee output".into(),
            ));
        }
        let proposal = Self { tx };
        let message = proposal.signing_bytes(chain);
        for approval in proposal.auth().approvals() {
            proposal.check(approval, &message)?;
        }
        Ok(proposal)
    }

    /// The v8 frame with the approvals so far, hex: what members pass on.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.tx.to_bytes())
    }

    /// The transaction, for the confirm screen.
    #[must_use]
    pub fn transaction(&self) -> &Transaction {
        &self.tx
    }

    /// The account this spends from.
    #[must_use]
    pub fn account(&self) -> MultisigAccount {
        MultisigAccount {
            policy: self.auth().policy().clone(),
        }
    }

    /// Positions of the members who have approved, in order.
    #[must_use]
    pub fn approved_by(&self) -> Vec<u8> {
        self.auth().approvals().iter().map(|a| a.index).collect()
    }

    /// True once exactly `m` members have approved.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.auth().approvals().len() == self.auth().policy().threshold()
    }

    /// The bytes every member signs.
    #[must_use]
    pub fn signing_bytes(&self, chain: &ChainTag) -> Vec<u8> {
        self.tx.signing_bytes(chain)
    }

    /// This proposal with `approval` added.
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] when the signature does not verify for the
    /// key it names, that member already approved, or the quorum is already
    /// met (a spare approval would give one spend two encodings).
    pub fn with_approval(&self, approval: Approval, chain: &ChainTag) -> Result<Self> {
        self.check(&approval, &self.signing_bytes(chain))?;
        let auth = self.auth();
        if auth.approvals().iter().any(|a| a.index == approval.index) {
            return Err(WalletError::Multisig(format!(
                "member {} has already approved",
                approval.index
            )));
        }
        if self.is_complete() {
            return Err(WalletError::Multisig(
                "the quorum is already met; no further approval is needed".into(),
            ));
        }
        let mut approvals = auth.approvals().to_vec();
        approvals.push(approval);
        approvals.sort_by_key(|a| a.index);
        let auth = MultisigAuth::new(auth.policy().clone(), approvals)
            .map_err(|e| WalletError::Multisig(e.to_string()))?;
        let mut tx = self.tx.clone();
        tx.multisig = Some(Box::new(auth));
        Ok(Self { tx })
    }

    /// Combines another member's copy of the same spend into this one.
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] when `other` is a different spend (different
    /// signed bytes), or as [`Proposal::with_approval`].
    pub fn merge(&self, other: &Self, chain: &ChainTag) -> Result<Self> {
        if other.signing_bytes(chain) != self.signing_bytes(chain) {
            return Err(WalletError::Multisig(
                "the two proposals are different spends".into(),
            ));
        }
        let mut merged = self.clone();
        for approval in other.auth().approvals() {
            if merged.approved_by().contains(&approval.index) || merged.is_complete() {
                continue;
            }
            merged = merged.with_approval(approval.clone(), chain)?;
        }
        Ok(merged)
    }

    /// The broadcastable transaction, once the quorum verifies.
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] below the threshold or if any approval fails.
    pub fn finish(&self, chain: &ChainTag) -> Result<SignedTransfer> {
        let auth = self.auth();
        auth.policy()
            .verify(&self.signing_bytes(chain), auth.approvals())
            .map_err(|e| WalletError::Multisig(e.to_string()))?;
        let first = self.tx.outputs.first();
        let fee = self
            .tx
            .outputs
            .iter()
            .skip(1)
            .fold(primitive_types::U256::zero(), |sum, o| {
                sum.saturating_add(o.amount)
            });
        Ok(SignedTransfer {
            txid: hex::encode(self.tx.txid()),
            sender: hex::encode(self.tx.sender()),
            recipient: first.map(|o| hex::encode(o.recipient)).unwrap_or_default(),
            amount: first.map(|o| o.amount).unwrap_or_default(),
            nonce: self.tx.nonce,
            fee,
            raw_hex: self.to_hex(),
        })
    }

    fn auth(&self) -> &MultisigAuth {
        // Every constructor sets it; `from_hex` refuses a frame without one.
        self.tx
            .multisig
            .as_deref()
            .expect("a proposal is always a multisig transaction")
    }

    /// `approval` verifies under the policy key it names.
    fn check(&self, approval: &Approval, message: &[u8]) -> Result<()> {
        let key = self
            .auth()
            .policy()
            .keys()
            .get(usize::from(approval.index))
            .ok_or_else(|| {
                WalletError::Multisig(format!("member {} is not in the policy", approval.index))
            })?;
        suite::verify(key.suite, &key.public_key, message, &approval.signature).map_err(|e| {
            WalletError::Multisig(format!(
                "member {}'s approval does not verify: {e}",
                approval.index
            ))
        })
    }
}
