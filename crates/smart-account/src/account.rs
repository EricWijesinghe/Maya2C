//! Accounts, the key registry, and op validation and execution.

use std::collections::{BTreeMap, BTreeSet};

use maya_crypto_pq::suite::{SuiteId, verify};

use crate::fees::{FeeSpec, Paymaster, TokenPrice};
use crate::op::{Action, Op};
use crate::policy::{BLOCKS_PER_DAY, Policy, SessionScope};
use crate::recovery::{GuardianSet, RecoveryRequest};
use crate::{AccountId, KeyHash, key_hash};

/// Most signatures one op may carry.
pub const MAX_SIGNATURES: usize = 8;
/// Validation budget per op, in verification-cost units.
pub const MAX_VALIDATION_COST: u64 = 40;

/// Relative verification cost per suite (ML-DSA-65 = 1). Checked against the
/// budget *before* any signature is verified, so an op cannot make a
/// validator do expensive work and then fail.
pub const fn verify_cost(suite: SuiteId) -> u64 {
    match suite {
        SuiteId::Ed25519 | SuiteId::MlDsa65 => 1,
        SuiteId::MlDsa87 => 2,
        SuiteId::SlhDsaSha2_128s => 10,
        SuiteId::SlhDsaShake256f => 20,
        SuiteId::HybridMlDsa65SlhDsa128s => 11,
    }
}

/// What a key may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyRole {
    /// Full authority, weighted toward the threshold.
    Owner,
    /// A scoped, expiring app key.
    Session(SessionScope),
}

/// One key on an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEntry {
    /// Hash of `(suite, public key)`.
    pub hash: KeyHash,
    /// Weight toward the threshold (owners only).
    pub weight: u8,
    /// Role.
    pub role: KeyRole,
}

/// A transfer waiting out its delay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// Recipient.
    pub to: AccountId,
    /// Amount (already reserved from the balance).
    pub amount: u64,
    /// First height it may be released.
    pub release_at: u64,
}

/// A programmable account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// Stable address.
    pub id: AccountId,
    /// Native balance.
    pub balance: u64,
    /// Next expected nonce.
    pub nonce: u64,
    /// Keys.
    pub keys: Vec<KeyEntry>,
    /// Owner weight needed to authorise.
    pub threshold: u8,
    /// Spending policy.
    pub policy: Policy,
    /// Recovery guardians.
    pub guardians: GuardianSet,
    /// Recovery in progress.
    pub recovery: Option<RecoveryRequest>,
    /// Delayed transfers by id.
    pub pending: BTreeMap<u64, Pending>,
    /// Spent per policy day.
    pub spent: BTreeMap<u64, u64>,
    /// Spent per (recipient, day).
    pub spent_to: BTreeMap<(AccountId, u64), u64>,
}

/// Why an op was refused. A refused op changes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountError {
    /// No such account.
    UnknownAccount,
    /// Nonce is not the next one.
    BadNonce,
    /// More signatures, or more verification cost, than the budget allows.
    ValidationBudget,
    /// A signature names a key not on the account, or repeats one.
    UnknownKey,
    /// A signature does not verify.
    BadSignature,
    /// Owner weight below threshold.
    BelowThreshold,
    /// A session key used outside its scope or after expiry.
    OutOfScope,
    /// Blocked by the account's policy.
    Policy(&'static str),
    /// Not enough balance.
    Insufficient,
    /// The fee due exceeds the op's quoted maximum.
    FeeAboveMaximum,
    /// The sponsor refused.
    SponsorRefused,
    /// No price for the fee token.
    NoPrice,
    /// Recovery rule violated.
    Recovery(&'static str),
    /// Pending transfer rule violated.
    Pending(&'static str),
}

/// Who authorised an op.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Authority {
    Owner,
    Session(SessionScope),
}

/// All accounts and registered keys.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    accounts: BTreeMap<AccountId, Account>,
    keys: BTreeMap<KeyHash, (SuiteId, Vec<u8>)>,
    /// Sponsors by account.
    pub paymasters: BTreeMap<AccountId, Paymaster>,
    /// Oracle prices of approved fee tokens.
    pub prices: BTreeMap<u32, TokenPrice>,
    /// Token balances used for fees.
    pub token_balances: BTreeMap<(AccountId, u32), u64>,
    /// Fees collected (native).
    pub fees_collected: u64,
    next_pending: u64,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an account owned by one key. Its id is derived from the first
    /// key and a salt and never changes, whatever keys it has later.
    pub fn create(
        &mut self,
        suite: SuiteId,
        public_key: &[u8],
        salt: u64,
        balance: u64,
    ) -> AccountId {
        let hash = key_hash(suite, public_key);
        let mut h = blake3::Hasher::new_derive_key("maya2c/smart-account/id/v1");
        h.update(&hash);
        h.update(&salt.to_le_bytes());
        let id = *h.finalize().as_bytes();
        self.keys.insert(hash, (suite, public_key.to_vec()));
        self.accounts.insert(
            id,
            Account {
                id,
                balance,
                nonce: 0,
                keys: vec![KeyEntry {
                    hash,
                    weight: 1,
                    role: KeyRole::Owner,
                }],
                threshold: 1,
                policy: Policy::default(),
                guardians: GuardianSet::default(),
                recovery: None,
                pending: BTreeMap::new(),
                spent: BTreeMap::new(),
                spent_to: BTreeMap::new(),
            },
        );
        id
    }

    /// An account.
    pub fn get(&self, id: &AccountId) -> Option<&Account> {
        self.accounts.get(id)
    }

    /// Sets an account's owner threshold (test and genesis helper).
    pub fn set_threshold(&mut self, id: &AccountId, threshold: u8) {
        if let Some(a) = self.accounts.get_mut(id) {
            a.threshold = threshold;
        }
    }

    fn authorise(&self, op: &Op, height: u64) -> Result<Authority, AccountError> {
        let account = self
            .accounts
            .get(&op.account)
            .ok_or(AccountError::UnknownAccount)?;
        if op.nonce != account.nonce {
            return Err(AccountError::BadNonce);
        }
        if op.signatures.is_empty() || op.signatures.len() > MAX_SIGNATURES {
            return Err(AccountError::ValidationBudget);
        }
        // Resolve every key and price the whole validation before verifying
        // anything.
        let mut resolved = Vec::with_capacity(op.signatures.len());
        let mut seen = BTreeSet::new();
        let mut cost = 0u64;
        for (hash, sig) in &op.signatures {
            let entry = account
                .keys
                .iter()
                .find(|k| k.hash == *hash)
                .ok_or(AccountError::UnknownKey)?;
            if !seen.insert(*hash) {
                return Err(AccountError::UnknownKey);
            }
            let (suite, pk) = self.keys.get(hash).ok_or(AccountError::UnknownKey)?;
            cost += verify_cost(*suite);
            resolved.push((entry, *suite, pk, sig));
        }
        if cost > MAX_VALIDATION_COST {
            return Err(AccountError::ValidationBudget);
        }
        let message = op.signing_bytes();
        let mut weight = 0u32;
        let mut session = None;
        for (entry, suite, pk, sig) in resolved {
            verify(suite, pk, &message, sig).map_err(|_| AccountError::BadSignature)?;
            match &entry.role {
                KeyRole::Owner => weight += u32::from(entry.weight),
                KeyRole::Session(scope) => session = Some(scope.clone()),
            }
        }
        if let Some(scope) = session {
            // A session key acts alone, and only for in-scope transfers.
            let Action::Transfer { to, amount } = &op.action else {
                return Err(AccountError::OutOfScope);
            };
            if op.signatures.len() != 1 || !scope.allows(to, *amount, height) {
                return Err(AccountError::OutOfScope);
            }
            return Ok(Authority::Session(scope));
        }
        if weight < u32::from(account.threshold) {
            return Err(AccountError::BelowThreshold);
        }
        Ok(Authority::Owner)
    }

    fn charge_fee(&mut self, op: &Op, fee_due: u64, height: u64) -> Result<(), AccountError> {
        let day = height / BLOCKS_PER_DAY;
        match &op.fee {
            FeeSpec::Native { max_fee } => {
                if fee_due > *max_fee {
                    return Err(AccountError::FeeAboveMaximum);
                }
                let a = self
                    .accounts
                    .get_mut(&op.account)
                    .ok_or(AccountError::UnknownAccount)?;
                a.balance = a
                    .balance
                    .checked_sub(fee_due)
                    .ok_or(AccountError::Insufficient)?;
            }
            FeeSpec::Token { token, max_amount } => {
                let price = self.prices.get(token).ok_or(AccountError::NoPrice)?;
                let cost = price.token_cost(fee_due).ok_or(AccountError::NoPrice)?;
                if cost > *max_amount {
                    return Err(AccountError::FeeAboveMaximum);
                }
                let bal = self.token_balances.entry((op.account, *token)).or_insert(0);
                *bal = bal.checked_sub(cost).ok_or(AccountError::Insufficient)?;
            }
            FeeSpec::Sponsored { paymaster } => {
                let pm = self
                    .paymasters
                    .get_mut(paymaster)
                    .ok_or(AccountError::SponsorRefused)?;
                if !pm.charge(&op.account, day, fee_due) {
                    return Err(AccountError::SponsorRefused);
                }
            }
        }
        self.fees_collected += fee_due;
        Ok(())
    }

    /// Validates and applies `op` at `height`, charging `fee_due` (from the
    /// fee market) against the op's quoted maximum. All-or-nothing: on any
    /// error the registry is unchanged.
    pub fn apply(&mut self, op: &Op, height: u64, fee_due: u64) -> Result<(), AccountError> {
        let authority = self.authorise(op, height)?;
        let snapshot = self.clone();
        let result = self
            .charge_fee(op, fee_due, height)
            .and_then(|()| self.execute(op, &authority, height));
        match result {
            Ok(()) => {
                if let Some(a) = self.accounts.get_mut(&op.account) {
                    a.nonce += 1;
                }
                Ok(())
            }
            Err(e) => {
                *self = snapshot;
                Err(e)
            }
        }
    }

    fn execute(&mut self, op: &Op, authority: &Authority, height: u64) -> Result<(), AccountError> {
        let owner = matches!(authority, Authority::Owner);
        let need_owner = |ok: bool| {
            if ok {
                Ok(())
            } else {
                Err(AccountError::OutOfScope)
            }
        };
        match &op.action {
            Action::Transfer { to, amount } => self.transfer(&op.account, to, *amount, height),
            Action::AddKey {
                suite,
                public_key,
                weight,
            } => {
                need_owner(owner)?;
                let hash = key_hash(*suite, public_key);
                self.keys.insert(hash, (*suite, public_key.clone()));
                let a = self.account_mut(&op.account)?;
                a.keys.push(KeyEntry {
                    hash,
                    weight: *weight,
                    role: KeyRole::Owner,
                });
                Ok(())
            }
            Action::RotateKey {
                old,
                suite,
                public_key,
            } => {
                need_owner(owner)?;
                let hash = key_hash(*suite, public_key);
                self.keys.insert(hash, (*suite, public_key.clone()));
                let a = self.account_mut(&op.account)?;
                let entry = a
                    .keys
                    .iter_mut()
                    .find(|k| k.hash == *old)
                    .ok_or(AccountError::UnknownKey)?;
                entry.hash = hash;
                Ok(())
            }
            Action::SetPolicy(p) => {
                need_owner(owner)?;
                self.account_mut(&op.account)?.policy = p.clone();
                Ok(())
            }
            Action::AddSessionKey {
                suite,
                public_key,
                scope,
            } => {
                need_owner(owner)?;
                let hash = key_hash(*suite, public_key);
                self.keys.insert(hash, (*suite, public_key.clone()));
                let a = self.account_mut(&op.account)?;
                a.keys.push(KeyEntry {
                    hash,
                    weight: 0,
                    role: KeyRole::Session(scope.clone()),
                });
                Ok(())
            }
            Action::CancelPending { id } => {
                need_owner(owner)?;
                self.cancel_pending(&op.account, *id)
            }
            Action::ReleasePending { id } => self.release_pending(&op.account, *id, height),
            Action::SetGuardians(g) => {
                need_owner(owner)?;
                self.account_mut(&op.account)?.guardians = g.clone();
                Ok(())
            }
            Action::ApproveRecovery {
                target,
                suite,
                public_key,
            } => {
                need_owner(owner)?;
                self.approve_recovery(&op.account, target, *suite, public_key, height)
            }
            Action::CancelRecovery => {
                need_owner(owner)?;
                self.account_mut(&op.account)?.recovery = None;
                Ok(())
            }
            Action::FinalizeRecovery { target } => self.finalize_recovery(target, height),
        }
    }

    fn account_mut(&mut self, id: &AccountId) -> Result<&mut Account, AccountError> {
        self.accounts
            .get_mut(id)
            .ok_or(AccountError::UnknownAccount)
    }

    fn transfer(
        &mut self,
        from: &AccountId,
        to: &AccountId,
        amount: u64,
        height: u64,
    ) -> Result<(), AccountError> {
        if !self.accounts.contains_key(to) {
            return Err(AccountError::UnknownAccount);
        }
        let day = height / BLOCKS_PER_DAY;
        let a = self.account_mut(from)?;
        let p = &a.policy;
        if !p.allow_list.is_empty() && !p.allow_list.contains(to) {
            return Err(AccountError::Policy("recipient not on the allow-list"));
        }
        let spent = a
            .spent
            .get(&day)
            .copied()
            .unwrap_or(0)
            .saturating_add(amount);
        if p.daily_limit > 0 && spent > p.daily_limit {
            return Err(AccountError::Policy("daily limit"));
        }
        let spent_to = a
            .spent_to
            .get(&(*to, day))
            .copied()
            .unwrap_or(0)
            .saturating_add(amount);
        if p.per_recipient_daily > 0 && spent_to > p.per_recipient_daily {
            return Err(AccountError::Policy("per-recipient daily limit"));
        }
        a.balance = a
            .balance
            .checked_sub(amount)
            .ok_or(AccountError::Insufficient)?;
        a.spent.insert(day, spent);
        a.spent_to.insert((*to, day), spent_to);
        if p.delay_above > 0 && amount > p.delay_above {
            let release_at = height + p.delay_blocks;
            let id = self.next_pending;
            self.next_pending += 1;
            self.account_mut(from)?.pending.insert(
                id,
                Pending {
                    to: *to,
                    amount,
                    release_at,
                },
            );
            return Ok(());
        }
        let b = self.account_mut(to)?;
        b.balance = b
            .balance
            .checked_add(amount)
            .ok_or(AccountError::Insufficient)?;
        Ok(())
    }

    fn cancel_pending(&mut self, account: &AccountId, id: u64) -> Result<(), AccountError> {
        let a = self.account_mut(account)?;
        let p = a
            .pending
            .remove(&id)
            .ok_or(AccountError::Pending("no such pending transfer"))?;
        a.balance += p.amount;
        Ok(())
    }

    /// A guardian of `account` cancels one of its pending transfers. Needs
    /// `threshold` distinct guardians; each approval is that guardian's own
    /// op nonce-checked and signed — here the caller passes guardians whose
    /// ops were already applied as `ApproveRecovery`-style authorisations.
    pub fn guardian_cancel(
        &mut self,
        account: &AccountId,
        id: u64,
        guardians: &[AccountId],
    ) -> Result<(), AccountError> {
        let a = self
            .accounts
            .get(account)
            .ok_or(AccountError::UnknownAccount)?;
        let distinct: BTreeSet<_> = guardians
            .iter()
            .filter(|g| a.guardians.guardians.contains(g))
            .collect();
        if distinct.len() < a.guardians.threshold.max(1) {
            return Err(AccountError::Recovery("not enough guardians"));
        }
        self.cancel_pending(account, id)
    }

    fn release_pending(
        &mut self,
        account: &AccountId,
        id: u64,
        height: u64,
    ) -> Result<(), AccountError> {
        let a = self.account_mut(account)?;
        let p = a
            .pending
            .get(&id)
            .ok_or(AccountError::Pending("no such pending transfer"))?
            .clone();
        if height < p.release_at {
            return Err(AccountError::Pending("delay not over"));
        }
        a.pending.remove(&id);
        let b = self.account_mut(&p.to)?;
        b.balance = b
            .balance
            .checked_add(p.amount)
            .ok_or(AccountError::Insufficient)?;
        Ok(())
    }

    fn approve_recovery(
        &mut self,
        guardian: &AccountId,
        target: &AccountId,
        suite: SuiteId,
        public_key: &[u8],
        height: u64,
    ) -> Result<(), AccountError> {
        let t = self.account_mut(target)?;
        if !t.guardians.guardians.contains(guardian) {
            return Err(AccountError::Recovery("not a guardian of this account"));
        }
        let threshold = t.guardians.threshold.max(1);
        let req = t.recovery.get_or_insert_with(|| RecoveryRequest {
            suite,
            public_key: public_key.to_vec(),
            approvals: BTreeSet::new(),
            quorum_at: None,
        });
        if req.suite != suite || req.public_key != public_key {
            return Err(AccountError::Recovery(
                "a different key is already proposed",
            ));
        }
        req.approvals.insert(*guardian);
        if req.quorum_at.is_none() && req.approvals.len() >= threshold {
            req.quorum_at = Some(height);
        }
        Ok(())
    }

    fn finalize_recovery(&mut self, target: &AccountId, height: u64) -> Result<(), AccountError> {
        let t = self
            .accounts
            .get(target)
            .ok_or(AccountError::UnknownAccount)?;
        let req = t
            .recovery
            .clone()
            .ok_or(AccountError::Recovery("no recovery in progress"))?;
        let at = req
            .quorum_at
            .ok_or(AccountError::Recovery("no guardian quorum yet"))?;
        if height < at + t.guardians.delay_blocks {
            return Err(AccountError::Recovery("cancel window still open"));
        }
        let hash = req.new_key_hash();
        self.keys.insert(hash, (req.suite, req.public_key.clone()));
        let t = self.account_mut(target)?;
        t.keys.retain(|k| !matches!(k.role, KeyRole::Owner));
        t.keys.push(KeyEntry {
            hash,
            weight: t.threshold.max(1),
            role: KeyRole::Owner,
        });
        t.recovery = None;
        Ok(())
    }
}
