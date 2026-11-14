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
//! Each member names its own suite. This wallet's key is the hybrid `0x30`;
//! a policy may also list an ML-DSA-65 `0x10` (Ledger) or SLH-DSA key. A
//! classical suite is refused by [`MultisigPolicy::new`]. Only a `0x30` key
//! can approve today: the Ledger app refuses v8 bytes and nothing imports an
//! approval signed elsewhere. So [`MultisigAccount::new`] refuses a threshold
//! above the number of `0x30` members, which would lock the funds; other
//! suites can only be spare members. The per-family approval floor proposed
//! in ADR-117 is not built (ADR-117 is not written on `master`).
//!
//! # Fees
//!
//! A finished v8 spend carries the policy and `m` approvals, so it is larger
//! than a single-signature transfer. [`MultisigAccount::spend_size`] measures
//! it, and [`Proposal::check_fee`] refuses an underpaid fee before anyone
//! signs: the fee is inside the signed bytes.
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
use custom_l1_node::core::transaction::TxOutput;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{self, SuiteId};
use primitive_types::U256;
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
    /// [`WalletError::Multisig`] for anything [`MultisigPolicy::new`] refuses
    /// (an unmeetable threshold, a repeated key, a classical suite, a wrong
    /// key length), and for a threshold above the number of hybrid `0x30`
    /// members.
    pub fn new(threshold: usize, members: &[MemberKey]) -> Result<Self> {
        let keys = members
            .iter()
            .map(MemberKey::to_policy_key)
            .collect::<Result<Vec<_>>>()?;
        let policy = MultisigPolicy::new(threshold, keys)
            .map_err(|e| WalletError::Multisig(e.to_string()))?;
        // Only a hybrid 0x30 key can approve today: the Ledger app refuses v8
        // bytes and there is no way to import an approval signed elsewhere.
        // An account needing any other approval would take funds it can never
        // spend. Relax this when a Ledger v8 review and an "add external
        // approval" command exist (the `multisig-v8-spend` Ledger fixture
        // case flips with the first).
        let approvers = policy
            .keys()
            .iter()
            .filter(|k| k.suite == SuiteId::HybridMlDsa65SlhDsa128s)
            .count();
        if threshold > approvers {
            return Err(WalletError::Multisig(format!(
                "{threshold} approvals needed but only {approvers} members use this wallet's \
                 hybrid suite; a Ledger or other suite cannot approve yet, so the account \
                 could never spend"
            )));
        }
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

    /// Bytes a finished spend from this account occupies: one payment, a fee
    /// output to the collector, the policy, and `m` approvals from the
    /// members with the longest signatures. What a fee-market chain charges
    /// `base_fee` per byte for (ADR-029); a single-signature transfer's size
    /// would underprice it.
    ///
    /// # Errors
    ///
    /// As [`Proposal::finished_size`].
    pub fn spend_size(&self) -> Result<u64> {
        // Amounts are fixed-width on the wire, so placeholder values measure
        // the real spend exactly.
        let output = |recipient| TxOutput {
            amount: U256::one(),
            recipient,
        };
        let tx = Transaction::new(Vec::new(), vec![output([0; 32]), output(FEE_COLLECTOR)], 0);
        Proposal::unsigned(self, tx).finished_size()
    }
}

/// A proposal and how far its approval has got, as the multisig screen shows
/// it. Computed from the proposal text itself, so it never describes another.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProposalStatus {
    /// The v8 transaction with the approvals so far, hex: what members pass on.
    pub proposal_hex: String,
    /// Positions of the members who have approved.
    pub approved_by: Vec<u8>,
    /// `m`.
    pub threshold: usize,
    /// True once the quorum is met and the spend can be finished.
    pub complete: bool,
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
        // The node decodes up to n approvals. A copy carrying more than m
        // could never finish, yet would keep accepting approvals: refused.
        if proposal.auth().approvals().len() > proposal.auth().policy().threshold() {
            return Err(WalletError::Multisig(
                "the proposal carries more approvals than the account needs".into(),
            ));
        }
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

    /// What the multisig screen shows for this proposal.
    #[must_use]
    pub fn status(&self) -> ProposalStatus {
        ProposalStatus {
            proposal_hex: self.to_hex(),
            approved_by: self.approved_by(),
            threshold: self.auth().policy().threshold(),
            complete: self.is_complete(),
        }
    }

    /// Everything after the payment: the fee outputs.
    #[must_use]
    pub fn fee(&self) -> U256 {
        self.tx
            .outputs
            .iter()
            .skip(1)
            .fold(U256::zero(), |sum, o| sum.saturating_add(o.amount))
    }

    /// Bytes this spend occupies once `m` members approve, counting the
    /// longest signatures the policy allows.
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] if the placeholder approvals do not fit the
    /// policy (they always do for a policy [`MultisigPolicy`] accepted).
    pub fn finished_size(&self) -> Result<u64> {
        let policy = self.auth().policy();
        let mut longest = policy
            .keys()
            .iter()
            .enumerate()
            .map(|(i, k)| Ok((k.suite.info().signature_len, u8::try_from(i)?)))
            .collect::<std::result::Result<Vec<_>, std::num::TryFromIntError>>()
            .map_err(|_| WalletError::Multisig("a policy index above 255".into()))?;
        longest.sort_by(|a, b| b.0.cmp(&a.0));
        longest.truncate(policy.threshold());
        longest.sort_by_key(|&(_, index)| index);
        let approvals = longest
            .into_iter()
            .map(|(len, index)| Approval {
                index,
                signature: vec![0; len],
            })
            .collect();
        let auth = MultisigAuth::new(policy.clone(), approvals)
            .map_err(|e| WalletError::Multisig(e.to_string()))?;
        let mut tx = self.tx.clone();
        tx.multisig = Some(Box::new(auth));
        u64::try_from(tx.to_bytes().len()).map_err(|_| WalletError::AmountOverflow)
    }

    /// Refuses a fee to the collector under `base_fee` x
    /// [`Proposal::finished_size`]: the node would refuse the finished spend
    /// as underpaid (ADR-029), and the fee is inside the signed bytes, so
    /// every member would have to approve a new proposal. A zero `base_fee`
    /// (fees off) checks nothing.
    ///
    /// # Errors
    ///
    /// [`WalletError::Multisig`] for an underpaid fee;
    /// [`WalletError::AmountOverflow`] if the required fee passes 2^256 - 1.
    pub fn check_fee(&self, base_fee: U256) -> Result<()> {
        if base_fee.is_zero() {
            return Ok(());
        }
        let needed = base_fee
            .checked_mul(U256::from(self.finished_size()?))
            .ok_or(WalletError::AmountOverflow)?;
        let paid = self
            .tx
            .outputs
            .iter()
            .skip(1)
            .filter(|o| o.recipient == FEE_COLLECTOR)
            .fold(U256::zero(), |sum, o| sum.saturating_add(o.amount));
        if paid < needed {
            return Err(WalletError::Multisig(format!(
                "the fee to the collector ({paid} base units) is below the {needed} this spend \
                 needs once approved; nothing was signed"
            )));
        }
        Ok(())
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
        let fee = self.fee();
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
