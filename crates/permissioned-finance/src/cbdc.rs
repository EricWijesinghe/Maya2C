//! A permissioned CBDC vault with ZK-KYC admission.

use maya_amount::Amount;
use std::collections::{BTreeMap, BTreeSet};

use maya_zk_stark::Proof;
use maya_zk_stark::credential::{self, DisclosurePublic, Predicate};
use maya_zk_stark::hash::Digest;

use crate::Account;

/// Why the vault refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultError {
    /// Only the issuer may do this.
    NotIssuer,
    /// The account has not been admitted.
    NotAdmitted(Account),
    /// The account is frozen.
    Frozen(Account),
    /// Over the sender's per-transfer limit for its tier.
    OverLimit {
        /// The limit.
        limit: Amount,
    },
    /// Balance too low.
    Insufficient,
    /// The KYC proof did not verify.
    BadKycProof(String),
    /// Arithmetic overflow.
    Overflow,
}

/// The vault.
#[derive(Clone, Debug)]
pub struct Vault {
    issuer: Account,
    /// The KYC issuer's credential and revocation roots (anchored on chain).
    kyc_roots: (Digest, Digest),
    /// Per-transfer limit per tier; tier 0 is "not admitted".
    limits: BTreeMap<u32, Amount>,
    tiers: BTreeMap<Account, u32>,
    frozen: BTreeSet<Account>,
    balances: BTreeMap<Account, Amount>,
    supply: Amount,
}

impl Vault {
    /// A vault run by `issuer`, admitting against the given KYC roots.
    #[must_use]
    pub fn new(
        issuer: Account,
        kyc_roots: (Digest, Digest),
        limits: BTreeMap<u32, Amount>,
    ) -> Self {
        Self {
            issuer,
            kyc_roots,
            limits,
            tiers: BTreeMap::new(),
            frozen: BTreeSet::new(),
            balances: BTreeMap::new(),
            supply: Amount::default(),
        }
    }

    /// Admits `account` at `tier` on a STARK proof that some credential in the
    /// issuer's tree, unrevoked, has a KYC tier of at least `tier`.
    ///
    /// # Errors
    ///
    /// [`VaultError::BadKycProof`] if the proof does not verify.
    pub fn admit(&mut self, account: Account, tier: u32, proof: &Proof) -> Result<(), VaultError> {
        let public = DisclosurePublic {
            issuer_root: self.kyc_roots.0,
            revocation_root: self.kyc_roots.1,
            predicate: Predicate::AtLeast(tier),
        };
        credential::verify(proof, &public)
            .map_err(|e| VaultError::BadKycProof(format!("{e:?}")))?;
        self.tiers.insert(account, tier);
        Ok(())
    }

    /// Issues `amount` to an admitted `to`.
    ///
    /// # Errors
    ///
    /// Not the issuer, not admitted, or overflow.
    pub fn issue(&mut self, actor: Account, to: Account, amount: Amount) -> Result<(), VaultError> {
        if actor != self.issuer {
            return Err(VaultError::NotIssuer);
        }
        self.require_admitted(&to)?;
        self.supply = self
            .supply
            .checked_add(amount)
            .ok_or(VaultError::Overflow)?;
        let b = self.balances.entry(to).or_default();
        *b = b.checked_add(amount).ok_or(VaultError::Overflow)?;
        Ok(())
    }

    /// Freezes an account (issuer only). A frozen account can neither send
    /// nor receive until unfrozen.
    ///
    /// # Errors
    ///
    /// Not the issuer.
    pub fn set_frozen(
        &mut self,
        actor: Account,
        who: Account,
        frozen: bool,
    ) -> Result<(), VaultError> {
        if actor != self.issuer {
            return Err(VaultError::NotIssuer);
        }
        if frozen {
            self.frozen.insert(who);
        } else {
            self.frozen.remove(&who);
        }
        Ok(())
    }

    fn require_admitted(&self, who: &Account) -> Result<u32, VaultError> {
        if self.frozen.contains(who) {
            return Err(VaultError::Frozen(*who));
        }
        match self.tiers.get(who) {
            Some(t) if *t > 0 => Ok(*t),
            _ => Err(VaultError::NotAdmitted(*who)),
        }
    }

    /// Settles a transfer between two admitted, unfrozen accounts within the
    /// sender's tier limit.
    ///
    /// # Errors
    ///
    /// Any [`VaultError`]; a refusal changes nothing.
    pub fn settle(&mut self, from: Account, to: Account, amount: Amount) -> Result<(), VaultError> {
        let tier = self.require_admitted(&from)?;
        self.require_admitted(&to)?;
        let limit = self.limits.get(&tier).copied().unwrap_or_default();
        if amount > limit {
            return Err(VaultError::OverLimit { limit });
        }
        let from_balance = self.balances.get(&from).copied().unwrap_or_default();
        let rest = from_balance
            .checked_sub(amount)
            .ok_or(VaultError::Insufficient)?;
        let to_balance = self.balances.get(&to).copied().unwrap_or_default();
        let credited = to_balance.checked_add(amount).ok_or(VaultError::Overflow)?;
        self.balances.insert(from, rest);
        self.balances.insert(to, credited);
        Ok(())
    }

    /// A balance.
    #[must_use]
    pub fn balance(&self, who: &Account) -> Amount {
        self.balances.get(who).copied().unwrap_or_default()
    }

    /// Total issued.
    #[must_use]
    pub fn supply(&self) -> Amount {
        self.supply
    }

    /// Sum of every balance: must always equal [`Vault::supply`].
    #[must_use]
    pub fn held(&self) -> Amount {
        // shortcut: saturates past 2^256 - 1; `issue` refuses any supply that
        // would pass it, and every balance is part of the supply.
        self.balances
            .values()
            .fold(Amount::default(), |sum, b| sum.saturating_add(*b))
    }
}
