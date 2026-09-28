//! Bitcoin lock/mint and burn/release, verified by a header light client
//! (Master Prompt 6 §3; Master Prompt 25).
//!
//! **Lock → mint.** A depositor pays BTC to the bridge's lock script in a
//! transaction that also carries `OP_RETURN <32-byte Maya2C address>`. A
//! relayer submits the transaction, a Merkle proof and the block hash. The
//! bridge mints wrapped BTC only if the block is on the most-work header
//! chain it tracks (`maya-btc-spv`), buried at least `min_confirmations`
//! deep, the proof places the txid under that block's Merkle root, exactly
//! one `OP_RETURN` names the recipient, and the transaction has never been
//! used, as a deposit or as a payout. Nothing about the deposit is taken on
//! a relayer's word: the transaction is parsed and its txid computed here.
//!
//! **Burn → release.** Burning wrapped BTC opens a release request naming a
//! Bitcoin script and amount. The request closes only when a Bitcoin
//! transaction paying at least that amount to that script is proven, just as
//! deposits are, so the bridge's books follow Bitcoin, not an operator's
//! report.
//!
//! **What is trusted, stated:**
//! - The most-work chain is valid. An attacker who out-mines
//!   `min_confirmations` blocks can reverse a credited deposit; the depth is a
//!   policy, not a proof (`docs/crosschain.md`).
//! - **Releasing BTC needs whoever holds the lock key.** Bitcoin's script has
//!   no way to check this chain, so no bridge can release BTC trustlessly. The key
//!   belongs in threshold custody (`custody-mpc`), and a release that is never
//!   paid stays open and visible, but custody can still refuse or steal.
//!
//! RESEARCH: nothing in the node calls this crate.

pub mod tx;

use std::collections::{BTreeMap, BTreeSet};

use maya_btc_spv::{BlockHash, HeaderChain, MerkleProof};

/// An account address on this chain.
pub type Address = [u8; 32];

/// Why the bridge refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// The transaction bytes are not one well-formed transaction.
    #[error("malformed: {0}")]
    Malformed(&'static str),
    /// The block is unknown, off the best chain, or not yet deep enough.
    #[error("block has {have} confirmations, {need} required")]
    TooShallow {
        /// Confirmations on the best chain (0 = not on it).
        have: u32,
        /// The bridge's policy.
        need: u32,
    },
    /// The Merkle proof does not place the transaction in that block.
    #[error("the transaction is not in the block")]
    NotIncluded,
    /// The transaction pays nothing to the lock script.
    #[error("no output pays the lock script")]
    NoDeposit,
    /// No `OP_RETURN <32-byte address>` output names a recipient, or more
    /// than one does: the recipient must not depend on which is read first.
    #[error("exactly one recipient must be named")]
    NoRecipient,
    /// A release may not pay the lock script: that payment would also be a
    /// deposit, so one transaction would mint and close a release at once.
    #[error("a release cannot pay the lock script")]
    ReleaseToLock,
    /// A Bitcoin transaction already used as a deposit or a payout.
    #[error("that transaction was already used")]
    AlreadyUsed,
    /// A burn larger than the balance.
    #[error("insufficient wrapped balance")]
    Insufficient,
    /// No open release with that id.
    #[error("no open release {0}")]
    UnknownRelease(u64),
    /// The proven transaction does not pay the release.
    #[error("the transaction does not pay the release")]
    DoesNotPay,
    /// An amount that overflows.
    #[error("amount overflow")]
    Overflow,
}

/// What a relayer submits: a transaction and where it sits.
#[derive(Clone, Debug)]
pub struct InclusionProof {
    /// The block holding it.
    pub block: BlockHash,
    /// The serialized transaction.
    pub raw_tx: Vec<u8>,
    /// Its Merkle branch under the block's root.
    pub merkle: MerkleProof,
}

/// An open release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Who burned.
    pub burner: Address,
    /// Satoshis owed.
    pub amount: u64,
    /// The Bitcoin script to pay.
    pub script: Vec<u8>,
}

/// The bridge's state.
#[derive(Clone, Debug)]
pub struct Bridge {
    /// The Bitcoin header chain it follows.
    pub headers: HeaderChain,
    lock_script: Vec<u8>,
    min_confirmations: u32,
    balances: BTreeMap<Address, u64>,
    /// Satoshis proven locked minus satoshis proven released.
    locked: u64,
    releases: BTreeMap<u64, Release>,
    /// Bitcoin transactions already used, as a deposit or a payout: one
    /// transaction is one of the two, once.
    used_txs: BTreeSet<[u8; 32]>,
    next_release: u64,
}

impl Bridge {
    /// A bridge following `headers`, locking to `lock_script`, crediting at
    /// `min_confirmations` depth.
    #[must_use]
    pub fn new(headers: HeaderChain, lock_script: Vec<u8>, min_confirmations: u32) -> Self {
        Self {
            headers,
            lock_script,
            min_confirmations,
            balances: BTreeMap::new(),
            locked: 0,
            releases: BTreeMap::new(),
            used_txs: BTreeSet::new(),
            next_release: 0,
        }
    }

    /// Checks depth and inclusion, and parses the transaction.
    fn proven(&self, proof: &InclusionProof) -> Result<tx::BtcTx, BridgeError> {
        let have = self.headers.confirmations(&proof.block);
        if have < self.min_confirmations {
            return Err(BridgeError::TooShallow {
                have,
                need: self.min_confirmations,
            });
        }
        let header = self
            .headers
            .header(&proof.block)
            .ok_or(BridgeError::NotIncluded)?;
        let parsed = tx::parse(&proof.raw_tx)?;
        if !proof.merkle.verify_txid(&header.merkle_root, &parsed.txid) {
            return Err(BridgeError::NotIncluded);
        }
        Ok(parsed)
    }

    /// Mints for a proven deposit. Returns the recipient and the amount.
    ///
    /// # Errors
    ///
    /// Any failed check above; the state is unchanged.
    pub fn mint(&mut self, proof: &InclusionProof) -> Result<(Address, u64), BridgeError> {
        let parsed = self.proven(proof)?;
        if self.used_txs.contains(&parsed.txid) {
            return Err(BridgeError::AlreadyUsed);
        }
        let named: Vec<Address> = parsed
            .outputs
            .iter()
            .filter_map(|o| tx::op_return_32(&o.script))
            .collect();
        let [recipient] = named[..] else {
            return Err(BridgeError::NoRecipient);
        };
        let deposits: Vec<u64> = parsed
            .outputs
            .iter()
            .filter(|o| o.script == self.lock_script)
            .map(|o| o.value)
            .collect();
        if deposits.is_empty() {
            return Err(BridgeError::NoDeposit);
        }
        let amount = deposits
            .iter()
            .try_fold(0u64, |a, v| a.checked_add(*v))
            .ok_or(BridgeError::Overflow)?;
        let locked = self
            .locked
            .checked_add(amount)
            .ok_or(BridgeError::Overflow)?;
        let balance = self
            .balance(&recipient)
            .checked_add(amount)
            .ok_or(BridgeError::Overflow)?;
        self.locked = locked;
        self.balances.insert(recipient, balance);
        self.used_txs.insert(parsed.txid);
        Ok((recipient, amount))
    }

    /// Burns `amount` of `burner`'s wrapped BTC and opens a release to
    /// `script`. Returns the release id.
    ///
    /// # Errors
    ///
    /// [`BridgeError::ReleaseToLock`] or [`BridgeError::Insufficient`].
    pub fn burn(
        &mut self,
        burner: Address,
        amount: u64,
        script: Vec<u8>,
    ) -> Result<u64, BridgeError> {
        if script == self.lock_script {
            return Err(BridgeError::ReleaseToLock);
        }
        let balance = self
            .balance(&burner)
            .checked_sub(amount)
            .ok_or(BridgeError::Insufficient)?;
        self.balances.insert(burner, balance);
        let id = self.next_release;
        self.next_release += 1;
        self.releases.insert(
            id,
            Release {
                burner,
                amount,
                script,
            },
        );
        Ok(id)
    }

    /// Closes release `id` with a proven Bitcoin payment of at least its
    /// amount to its script. One Bitcoin transaction closes one release.
    ///
    /// # Errors
    ///
    /// An unknown release, a failed proof, a payment short of the amount, or
    /// a transaction already used as a deposit or for another release.
    pub fn confirm_release(&mut self, id: u64, proof: &InclusionProof) -> Result<(), BridgeError> {
        let release = self
            .releases
            .get(&id)
            .ok_or(BridgeError::UnknownRelease(id))?;
        let parsed = self.proven(proof)?;
        if self.used_txs.contains(&parsed.txid) {
            return Err(BridgeError::AlreadyUsed);
        }
        let paid = parsed
            .outputs
            .iter()
            .filter(|o| o.script == release.script)
            .try_fold(0u64, |a, o| a.checked_add(o.value))
            .ok_or(BridgeError::Overflow)?;
        if paid < release.amount {
            return Err(BridgeError::DoesNotPay);
        }
        self.locked = self
            .locked
            .checked_sub(release.amount)
            .ok_or(BridgeError::Overflow)?;
        self.used_txs.insert(parsed.txid);
        self.releases.remove(&id);
        Ok(())
    }

    /// Wrapped BTC held by `who`.
    #[must_use]
    pub fn balance(&self, who: &Address) -> u64 {
        self.balances.get(who).copied().unwrap_or(0)
    }

    /// Wrapped supply in circulation.
    #[must_use]
    pub fn supply(&self) -> u64 {
        self.balances.values().sum()
    }

    /// Satoshis the bridge believes locked (proven deposits minus proven
    /// releases).
    #[must_use]
    pub fn locked(&self) -> u64 {
        self.locked
    }

    /// Wrapped BTC burned but not yet proven paid out.
    #[must_use]
    pub fn owed(&self) -> u64 {
        self.releases.values().map(|r| r.amount).sum()
    }

    /// Open releases.
    #[must_use]
    pub fn open_releases(&self) -> &BTreeMap<u64, Release> {
        &self.releases
    }
}
