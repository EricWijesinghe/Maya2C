//! Moving accounts off a deprecated suite.
//!
//! ## The schedule
//!
//! 1. **Deprecation** (`SuitePolicy::with_deprecation`). The suite still
//!    signs. An account can [`rotate`](MigrationState::rotate) to a key in an
//!    active suite, authorized by its current key, or
//!    [`commit_recovery`](MigrationState::commit_recovery) — register the hash
//!    of an upgraded public key without switching yet.
//! 2. **Sunset.** The suite stops signing. [`sweep_step`](MigrationState::sweep_step)
//!    walks the accounts a bounded number per block and moves every balance
//!    still held under a sunset key into the vault.
//! 3. **Vault.** A vault entry opens only to the key whose commitment the
//!    account registered while its old key was still trusted, and only with a
//!    signature by that new key in a suite that is active. The old key cannot
//!    open it — after sunset, it is assumed broken.
//!
//! An account that registered nothing stays locked. Proving ownership after
//! the fact needs a proof of knowledge of the wallet seed, which is a STARK
//! over the key derivation: PLANNED, not built, and said so here rather than
//! papered over with a path that trusts the broken key.
//!
//! ## No downtime
//!
//! The sweep is incremental: each call examines at most `budget` accounts,
//! resuming from a cursor, so a block's migration work is bounded and every
//! other account keeps transacting while it runs. `tests/agility_migration_tests.rs`
//! runs that with 1,000,000 accounts.

use std::collections::BTreeMap;
use std::ops::Bound;

use super::{SuitePolicy, SuiteStatus};
use crate::envelope::SignedEnvelope;
use crate::suite::SuiteId;

/// An account address.
pub type Address = [u8; 32];

/// `BLAKE3-derive-key(suite ‖ public_key)`: what an account stores instead of
/// a key, so a 49,856-byte-signature suite does not cost state per account.
pub type KeyCommitment = [u8; 32];

const COMMITMENT_DOMAIN: &str = "maya2c 2026-09-21 agility key commitment v1";
const ROTATE_DOMAIN: &[u8] = b"maya2c.agility.rotate.v1";
const RECOVERY_DOMAIN: &[u8] = b"maya2c.agility.commit-recovery.v1";
const CLAIM_DOMAIN: &[u8] = b"maya2c.agility.vault-claim.v1";
const TRANSFER_DOMAIN: &[u8] = b"maya2c.agility.transfer.v1";

/// Commits to a suite and public key.
#[must_use]
pub fn key_commitment(suite: SuiteId, public_key: &[u8]) -> KeyCommitment {
    let mut hasher = blake3::Hasher::new_derive_key(COMMITMENT_DOMAIN);
    hasher.update(&[suite.to_byte()]);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

/// Checks that `authorization` proves control of the key behind a commitment
/// for `message`.
pub trait Authorizer {
    /// `true` if the authorization is valid.
    fn check(&self, commitment: &KeyCommitment, message: &[u8], authorization: &[u8]) -> bool;
}

/// The production authorizer: `authorization` is an encoded
/// [`SignedEnvelope`] whose key matches the commitment and whose signature
/// verifies over the message.
pub struct EnvelopeAuthorizer;

impl Authorizer for EnvelopeAuthorizer {
    fn check(&self, commitment: &KeyCommitment, message: &[u8], authorization: &[u8]) -> bool {
        let Ok(envelope) = SignedEnvelope::decode(authorization) else {
            return false;
        };
        key_commitment(envelope.suite(), envelope.public_key()) == *commitment
            && envelope.verify(message).is_ok()
    }
}

/// One account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Account {
    /// The suite its key belongs to.
    pub suite: SuiteId,
    /// Commitment to its current key.
    pub key: KeyCommitment,
    /// Its balance.
    pub balance: u128,
    /// A registered upgraded key, `(suite, commitment)`.
    pub recovery: Option<(SuiteId, KeyCommitment)>,
}

/// A balance swept out of an account whose suite reached sunset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaultEntry {
    /// The swept balance.
    pub balance: u128,
    /// The suite it was held under.
    pub from_suite: SuiteId,
    /// The only key that can open it; `None` means locked.
    pub recovery: Option<(SuiteId, KeyCommitment)>,
}

/// What one bounded sweep call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SweepReport {
    /// Accounts looked at.
    pub examined: usize,
    /// Accounts moved into the vault.
    pub swept: usize,
    /// Whether the walk reached the end of the account set.
    pub pass_complete: bool,
}

/// Why an operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    /// No such account, or no such vault entry.
    #[error("unknown account")]
    UnknownAccount,
    /// The account's suite may not sign at this height.
    #[error("{0:?} may not sign at this height")]
    SuiteCannotSign(SuiteId),
    /// The target suite is not active at this height.
    #[error("{0:?} is not an active suite")]
    TargetNotActive(SuiteId),
    /// The authorization did not verify.
    #[error("authorization failed")]
    Unauthorized,
    /// The balance is too small.
    #[error("insufficient balance")]
    InsufficientBalance,
    /// The vault entry has no registered recovery key.
    #[error("vault entry is locked: no recovery key was registered")]
    Locked,
    /// The address already holds an account.
    #[error("account already exists")]
    AccountExists,
}

/// Accounts, the vault, and the sweep cursor.
#[derive(Debug, Clone, Default)]
pub struct MigrationState {
    accounts: BTreeMap<Address, Account>,
    vault: BTreeMap<Address, VaultEntry>,
    cursor: Option<Address>,
}

fn message(domain: &[u8], address: &Address, parts: &[&[u8]]) -> Vec<u8> {
    let mut out = domain.to_vec();
    out.extend_from_slice(address);
    for part in parts {
        out.extend_from_slice(&(part.len() as u64).to_le_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// The message an account's current key signs to rotate.
#[must_use]
pub fn rotate_message(address: &Address, suite: SuiteId, commitment: &KeyCommitment) -> Vec<u8> {
    message(ROTATE_DOMAIN, address, &[&[suite.to_byte()], commitment])
}

/// The message an account's current key signs to register a recovery key.
#[must_use]
pub fn recovery_message(address: &Address, suite: SuiteId, commitment: &KeyCommitment) -> Vec<u8> {
    message(RECOVERY_DOMAIN, address, &[&[suite.to_byte()], commitment])
}

/// The message the recovery key signs to open a vault entry.
#[must_use]
pub fn claim_message(address: &Address) -> Vec<u8> {
    message(CLAIM_DOMAIN, address, &[])
}

/// The message a sender signs to transfer.
#[must_use]
pub fn transfer_message(from: &Address, to: &Address, amount: u128) -> Vec<u8> {
    message(TRANSFER_DOMAIN, from, &[to, &amount.to_le_bytes()])
}

impl MigrationState {
    /// Creates an account.
    ///
    /// # Errors
    ///
    /// [`MigrationError::AccountExists`].
    pub fn open(&mut self, address: Address, account: Account) -> Result<(), MigrationError> {
        if self.accounts.contains_key(&address) || self.vault.contains_key(&address) {
            return Err(MigrationError::AccountExists);
        }
        self.accounts.insert(address, account);
        Ok(())
    }

    /// The account at `address`.
    #[must_use]
    pub fn account(&self, address: &Address) -> Option<&Account> {
        self.accounts.get(address)
    }

    /// The vault entry at `address`.
    #[must_use]
    pub fn vault_entry(&self, address: &Address) -> Option<&VaultEntry> {
        self.vault.get(address)
    }

    /// Number of live accounts and of vault entries.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        (self.accounts.len(), self.vault.len())
    }

    /// Accounts whose key is in `suite`.
    #[must_use]
    pub fn accounts_on(&self, suite: SuiteId) -> usize {
        self.accounts.values().filter(|a| a.suite == suite).count()
    }

    /// Every balance, live and vaulted. Conserved by every operation.
    #[must_use]
    pub fn total_supply(&self) -> u128 {
        let live: u128 = self.accounts.values().map(|a| a.balance).sum();
        let vaulted: u128 = self.vault.values().map(|v| v.balance).sum();
        live + vaulted
    }

    fn authorized_signer(
        &self,
        policy: &SuitePolicy,
        height: u64,
        address: &Address,
        msg: &[u8],
        authorization: &[u8],
        auth: &impl Authorizer,
    ) -> Result<Account, MigrationError> {
        let account = *self
            .accounts
            .get(address)
            .ok_or(MigrationError::UnknownAccount)?;
        if !policy.may_sign(account.suite, height) {
            return Err(MigrationError::SuiteCannotSign(account.suite));
        }
        if !auth.check(&account.key, msg, authorization) {
            return Err(MigrationError::Unauthorized);
        }
        Ok(account)
    }

    /// Switches an account to a key in an active suite, authorized by the
    /// current key while it may still sign.
    ///
    /// # Errors
    ///
    /// The target is not active, the current suite may not sign, or the
    /// authorization fails.
    pub fn rotate(
        &mut self,
        policy: &SuitePolicy,
        height: u64,
        address: &Address,
        target: (SuiteId, KeyCommitment),
        authorization: &[u8],
        auth: &impl Authorizer,
    ) -> Result<(), MigrationError> {
        require_active(policy, target.0, height)?;
        let msg = rotate_message(address, target.0, &target.1);
        let account = self.authorized_signer(policy, height, address, &msg, authorization, auth)?;
        self.accounts.insert(
            *address,
            Account {
                suite: target.0,
                key: target.1,
                recovery: None,
                ..account
            },
        );
        Ok(())
    }

    /// Registers the upgraded key that may later open a vault entry.
    ///
    /// # Errors
    ///
    /// As [`Self::rotate`].
    pub fn commit_recovery(
        &mut self,
        policy: &SuitePolicy,
        height: u64,
        address: &Address,
        target: (SuiteId, KeyCommitment),
        authorization: &[u8],
        auth: &impl Authorizer,
    ) -> Result<(), MigrationError> {
        require_active(policy, target.0, height)?;
        let msg = recovery_message(address, target.0, &target.1);
        let account = self.authorized_signer(policy, height, address, &msg, authorization, auth)?;
        self.accounts.insert(
            *address,
            Account {
                recovery: Some(target),
                ..account
            },
        );
        Ok(())
    }

    /// Moves `amount` from one account to another.
    ///
    /// # Errors
    ///
    /// Unknown account, a sender whose suite may not sign, a bad
    /// authorization, or insufficient balance.
    pub fn transfer(
        &mut self,
        policy: &SuitePolicy,
        height: u64,
        (from, to): (&Address, &Address),
        amount: u128,
        authorization: &[u8],
        auth: &impl Authorizer,
    ) -> Result<(), MigrationError> {
        let msg = transfer_message(from, to, amount);
        let sender = self.authorized_signer(policy, height, from, &msg, authorization, auth)?;
        let receiver = *self
            .accounts
            .get(to)
            .ok_or(MigrationError::UnknownAccount)?;
        let debited = sender
            .balance
            .checked_sub(amount)
            .ok_or(MigrationError::InsufficientBalance)?;
        if from == to {
            return Ok(());
        }
        self.accounts.insert(
            *from,
            Account {
                balance: debited,
                ..sender
            },
        );
        self.accounts.insert(
            *to,
            Account {
                balance: receiver.balance.saturating_add(amount),
                ..receiver
            },
        );
        Ok(())
    }

    /// Examines at most `budget` accounts from the cursor, vaulting every one
    /// whose suite is sunset at `height`. Wraps to the start after the end.
    pub fn sweep_step(&mut self, policy: &SuitePolicy, height: u64, budget: usize) -> SweepReport {
        let start = self.cursor.map_or(Bound::Unbounded, Bound::Excluded);
        let batch: Vec<(Address, Account)> = self
            .accounts
            .range((start, Bound::Unbounded))
            .take(budget)
            .map(|(a, acc)| (*a, *acc))
            .collect();

        let mut report = SweepReport {
            examined: batch.len(),
            ..SweepReport::default()
        };
        for (address, account) in &batch {
            if policy.status(account.suite, height) == SuiteStatus::Sunset {
                self.accounts.remove(address);
                self.vault.insert(
                    *address,
                    VaultEntry {
                        balance: account.balance,
                        from_suite: account.suite,
                        recovery: account.recovery,
                    },
                );
                report.swept += 1;
            }
        }
        report.pass_complete = batch.len() < budget;
        self.cursor = if report.pass_complete {
            None
        } else {
            batch.last().map(|(a, _)| *a)
        };
        report
    }

    /// Opens a vault entry with the registered recovery key.
    ///
    /// # Errors
    ///
    /// No entry, a locked entry, a recovery suite that is no longer active, or
    /// an authorization that does not come from the registered key.
    pub fn claim_vault(
        &mut self,
        policy: &SuitePolicy,
        height: u64,
        address: &Address,
        authorization: &[u8],
        auth: &impl Authorizer,
    ) -> Result<(), MigrationError> {
        let entry = *self
            .vault
            .get(address)
            .ok_or(MigrationError::UnknownAccount)?;
        let (suite, key) = entry.recovery.ok_or(MigrationError::Locked)?;
        require_active(policy, suite, height)?;
        if !auth.check(&key, &claim_message(address), authorization) {
            return Err(MigrationError::Unauthorized);
        }
        self.vault.remove(address);
        self.accounts.insert(
            *address,
            Account {
                suite,
                key,
                balance: entry.balance,
                recovery: None,
            },
        );
        Ok(())
    }
}

fn require_active(policy: &SuitePolicy, suite: SuiteId, height: u64) -> Result<(), MigrationError> {
    if policy.status(suite, height) == SuiteStatus::Active {
        Ok(())
    } else {
        Err(MigrationError::TargetNotActive(suite))
    }
}
