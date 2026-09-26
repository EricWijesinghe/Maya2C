//! Quantum-safe vault.
//!
//! Four protections, each from an existing crate:
//!
//! - **Post-quantum keys.** The account signs with ML-DSA-65.
//! - **Time to react.** Withdrawals above a threshold wait `delay_blocks`, and
//!   guardians can cancel a pending one: a thief with the key still has to
//!   outwait the owner.
//! - **Recovery without a seed phrase.** A guardian quorum installs a new key
//!   after a cancel window the owner can use to veto.
//! - **An auditor who sees the vault and nothing else.** Each withdrawal's
//!   memo is sealed to the auditor's ML-KEM viewing key.
//!
//! Plus the onboarding check: before moving Bitcoin in, it says whether the
//! source output already exposes its public key to a quantum attacker.

use maya_crypto_pq::kem::EncapsulationKey;
use maya_privacy::{Envelope, seal};
use maya_quantum_harbor::{Exposure, bitcoin_exposure, explain};
use maya_smart_account::account::AccountError;
use maya_smart_account::op::Action;
use maya_smart_account::policy::Policy;
use maya_smart_account::recovery::GuardianSet;
use maya_smart_account::{AccountId, Registry};

use crate::wallet::Wallet;

/// The vault's settings.
#[derive(Clone, Debug)]
pub struct VaultRules {
    /// Guardian accounts.
    pub guardians: Vec<AccountId>,
    /// Guardians needed to recover or cancel.
    pub threshold: usize,
    /// Withdrawals above this wait.
    pub delay_above: u64,
    /// How long they wait, and how long a recovery's veto window lasts.
    pub delay_blocks: u64,
}

/// Turns an account into a vault.
///
/// # Errors
///
/// [`AccountError`] if the registry refuses either op.
pub fn configure(
    registry: &mut Registry,
    owner: &mut Wallet,
    rules: &VaultRules,
    height: u64,
) -> Result<(), AccountError> {
    let guardians = owner.signed(Action::SetGuardians(GuardianSet {
        guardians: rules.guardians.clone(),
        threshold: rules.threshold,
        delay_blocks: rules.delay_blocks,
    }));
    registry.apply(&guardians, height, 0)?;
    let policy = owner.signed(Action::SetPolicy(Policy {
        delay_above: rules.delay_above,
        delay_blocks: rules.delay_blocks,
        ..Policy::default()
    }));
    registry.apply(&policy, height, 0)
}

/// Withdraws, sealing `memo` for the auditor. A large withdrawal is held
/// pending; the returned envelope is what the chain would store beside it.
///
/// # Errors
///
/// [`AccountError`] if the registry refuses the transfer.
pub fn withdraw(
    registry: &mut Registry,
    owner: &mut Wallet,
    to: AccountId,
    amount: u64,
    memo: &[u8],
    auditor: &EncapsulationKey,
    height: u64,
) -> Result<Envelope, AccountError> {
    let op = owner.signed(Action::Transfer { to, amount });
    if let Err(e) = registry.apply(&op, height, 0) {
        owner.refused();
        return Err(e);
    }
    Ok(seal(memo, auditor))
}

/// The onboarding check for a Bitcoin output about to be moved in: its
/// exposure and the sentence the app shows.
#[must_use]
pub fn bitcoin_source_check(output_script: &[u8]) -> (Exposure, &'static str) {
    let e = bitcoin_exposure(output_script);
    (e, explain(e))
}
