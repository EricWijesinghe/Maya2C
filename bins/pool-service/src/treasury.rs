//! The treasury: the key that signs payouts, and the limits on what it may do.
//!
//! ## Where the money comes from
//!
//! Not from a block reward — this chain has none (`docs/stratum-v2.md` §5).
//! Miners are paid out of an ordinary account the operator funds, by ordinary
//! signed transfers that need no consensus change. Everything the pool computes
//! upstream of here is denominated in share weight; this is where weight
//! becomes coin.
//!
//! ## The key is hot, and the limits exist because of that
//!
//! A daemon that signs payouts is a daemon holding a spending key on a machine
//! with a public mining port. That is a real exposure and it is not resolved by
//! being careful. What is here instead is a bound on the damage:
//!
//! - [`PoolConfig::max_batch_value`] caps what one batch can move;
//! - the balance floor check refuses to sign against an account that cannot
//!   cover the batch, so a drained treasury fails loudly rather than emitting
//!   transactions the mempool will reject;
//! - the password never appears in an argument, matching the wallet's own
//!   reasoning (`bins/l1-wallet/src/main.rs`): a password on a command line is in shell
//!   history and in the process list.
//!
//! Moving the signer out of the daemon entirely is the real fix and is out of
//! scope here. It is named in `docs/pool-service.md` rather than left implied.
//!
//! ## Nonces are the idempotency mechanism
//!
//! `src/state/db.rs:418` requires `tx.nonce == sender.nonce` exactly, so
//! transactions from one account are strictly sequential and a nonce is a slot
//! that exists once. A batch reserves its slot before it is signed and keeps
//! it across restarts, which is what makes "crashed after signing, before
//! broadcasting" recoverable: the resumed daemon rebroadcasts the same bytes
//! rather than signing a second transaction for the same slot.
//!
//! Consequently **one batch is in flight at a time**. Pipelining several would
//! mean predicting nonces for transactions not yet accepted, and a single
//! rejection in the middle would strand every batch behind it.

use std::path::Path;

use custom_l1_node::core::{ChainTag, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::HybridSigningKey;
use custom_l1_node::state::Address;

use crate::config::PoolConfig;
use crate::error::{PoolError, Result};
use crate::model::PayoutEntry;

/// Environment variable holding the treasury keystore password.
///
/// The same imperfect-but-better mechanism the wallet uses: an environment
/// variable is not written to disk by default and is not visible in `ps` on a
/// modern system, where a command-line flag is both.
pub const PASSWORD_ENV: &str = "MAYA_POOL_TREASURY_PASSWORD";

/// The failure a first-time operator hits, and the one place it is explained.
///
/// A function rather than an inline `format!` so the message can be asserted
/// without a test manipulating the process environment to provoke it.
fn missing_password_error(path: &Path) -> PoolError {
    PoolError::Treasury(format!(
        "{PASSWORD_ENV} is unset, so the treasury keystore at {} cannot be \
         opened. There is deliberately no password flag: a password on a \
         command line is in shell history and in the process list",
        path.display()
    ))
}

/// An unlocked treasury.
///
/// Deliberately not `Clone` and not `Debug`-printable in a way that could reach
/// a log: the key inside is the pool's ability to spend.
pub struct Treasury {
    /// The signing key.
    key: HybridSigningKey,
    /// Its address, cached.
    address: Address,
}

impl core::fmt::Debug for Treasury {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Address only. A key that can be printed is a key that ends up in a
        // log aggregator.
        f.debug_struct("Treasury")
            .field("address", &hex::encode(self.address))
            .finish_non_exhaustive()
    }
}

impl Treasury {
    /// Unlocks a keystore with a password from the environment.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Treasury`] if the password is unset, the file is
    /// missing, or the keystore does not decrypt.
    pub fn unlock(path: &Path) -> Result<Self> {
        let password = std::env::var(PASSWORD_ENV).map_err(|_| missing_password_error(path))?;
        Self::unlock_with(path, &password)
    }

    /// Unlocks a keystore with a password the caller already has.
    ///
    /// Split out from [`Treasury::unlock`] so the file and decryption paths can
    /// be exercised without a test writing to the process environment. In the
    /// 2024 edition `std::env::set_var` is `unsafe` precisely because it races
    /// every other thread reading the environment, and `cargo test` runs tests
    /// on threads — so a test that reached for it would be introducing a data
    /// race to check an error message.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Treasury`] if the file is missing or does not
    /// decrypt.
    pub fn unlock_with(path: &Path, password: &str) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| PoolError::Treasury(format!("reading {}: {e}", path.display())))?;

        let key = l1_wallet::keystore::decrypt(&bytes, password)
            .map_err(|e| PoolError::Treasury(format!("unlocking {}: {e}", path.display())))?;

        Ok(Self::from_key(key))
    }

    /// Builds a treasury around an already-unlocked key.
    ///
    /// The path tests take, and the path a future external signer would take.
    #[must_use]
    pub fn from_key(key: HybridSigningKey) -> Self {
        let address = key.address();
        Self { key, address }
    }

    /// The account payouts are spent from.
    #[must_use]
    pub fn address(&self) -> Address {
        self.address
    }

    /// Checks a batch against policy, then signs it.
    ///
    /// `nonce` is the slot reserved for this batch and `balance` is what the
    /// account holds. Both checks happen before the signature, because a signed
    /// transaction is a liability even if it is never broadcast: it sits on
    /// disk holding a nonce that nothing else may use.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Treasury`] for an empty batch, a batch over the
    /// per-batch cap, a batch the account cannot cover, or a batch whose
    /// outputs overflow. Returns [`PoolError::Chain`] if signing itself fails —
    /// ML-DSA is permitted to give up if its rejection loop does not terminate.
    pub fn sign_payout(
        &self,
        entries: &[PayoutEntry],
        nonce: u64,
        balance: u64,
        config: &PoolConfig,
        chain_tag: &ChainTag,
    ) -> Result<Transaction> {
        if entries.is_empty() {
            return Err(PoolError::Treasury(
                "refusing to sign a payout with no recipients".to_string(),
            ));
        }

        if entries.len() > config.max_outputs_per_batch {
            return Err(PoolError::Treasury(format!(
                "batch of {} outputs exceeds the {} cap",
                entries.len(),
                config.max_outputs_per_batch
            )));
        }

        // Through the model-checked crate, not a fold with `+`. This is the sum
        // that the balance check and the spend cap are both compared against,
        // so an overflow here would defeat both at once.
        let total = maya_ledger_math::total_outputs(entries.iter().map(|entry| entry.amount))
            .ok_or_else(|| {
                PoolError::Treasury("payout outputs overflow a u64 in total".to_string())
            })?;

        if total > config.max_batch_value {
            return Err(PoolError::Treasury(format!(
                "batch value {total} exceeds the per-batch cap of {}",
                config.max_batch_value
            )));
        }

        if total > balance {
            return Err(PoolError::Treasury(format!(
                "treasury holds {balance} and the batch needs {total}. The pool \
                 pays from an operator-funded account because this chain mints \
                 no block reward; fund it or lower reward_per_block"
            )));
        }

        let outputs: Vec<TxOutput> = entries
            .iter()
            .map(|entry| TxOutput {
                amount: entry.amount,
                recipient: entry.miner,
            })
            .collect();

        let mut transaction = Transaction::new(Vec::new(), outputs, nonce);
        transaction.sign(&self.key, chain_tag)?;
        Ok(transaction)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use custom_l1_node::core::ChainTag;
    use custom_l1_node::crypto::hybrid::generate_signing_key;

    fn config() -> PoolConfig {
        PoolConfig {
            reward_per_block: 1_000,
            ..PoolConfig::default()
        }
    }

    fn treasury() -> Treasury {
        Treasury::from_key(generate_signing_key().expect("key generation"))
    }

    fn test_tag() -> ChainTag {
        ChainTag::from_genesis([0; 32])
    }

    fn entries(amounts: &[u64]) -> Vec<PayoutEntry> {
        amounts
            .iter()
            .enumerate()
            .map(|(index, amount)| PayoutEntry {
                miner: [index as u8; 32],
                amount: *amount,
            })
            .collect()
    }

    #[test]
    fn a_signed_payout_carries_every_recipient_and_verifies() {
        let treasury = treasury();
        let tag = test_tag();
        let transaction = treasury
            .sign_payout(&entries(&[10, 20, 30]), 7, 1_000, &config(), &tag)
            .unwrap();

        assert_eq!(transaction.outputs.len(), 3);
        assert_eq!(transaction.nonce, 7);
        assert_eq!(transaction.sender(), treasury.address());
        assert!(transaction.verify(&tag).is_ok());
    }

    #[test]
    fn a_batch_the_treasury_cannot_cover_is_refused_before_signing() {
        // Signing it would produce a transaction the mempool rejects while
        // still holding the nonce slot on disk.
        let treasury = treasury();
        assert!(matches!(
            treasury.sign_payout(&entries(&[500, 600]), 0, 1_000, &config(), &test_tag()),
            Err(PoolError::Treasury(_))
        ));
    }

    #[test]
    fn a_batch_over_the_custody_cap_is_refused() {
        let config = PoolConfig {
            max_batch_value: 100,
            ..config()
        };
        let treasury = treasury();

        assert!(matches!(
            treasury.sign_payout(&entries(&[60, 60]), 0, u64::MAX, &config, &test_tag()),
            Err(PoolError::Treasury(_))
        ));
    }

    #[test]
    fn an_empty_batch_is_refused() {
        let treasury = treasury();
        assert!(matches!(
            treasury.sign_payout(&[], 0, 1_000, &config(), &test_tag()),
            Err(PoolError::Treasury(_))
        ));
    }

    #[test]
    fn outputs_that_overflow_in_total_are_refused() {
        // The sum feeds both the balance check and the spend cap, so an
        // overflow here would defeat both at once.
        let treasury = treasury();
        assert!(matches!(
            treasury.sign_payout(
                &entries(&[u64::MAX, 1]),
                0,
                u64::MAX,
                &config(),
                &test_tag()
            ),
            Err(PoolError::Treasury(_))
        ));
    }

    #[test]
    fn a_batch_over_the_output_cap_is_refused() {
        let config = PoolConfig {
            max_outputs_per_batch: 2,
            ..config()
        };
        let treasury = treasury();

        assert!(matches!(
            treasury.sign_payout(&entries(&[1, 1, 1]), 0, 1_000, &config, &test_tag()),
            Err(PoolError::Treasury(_))
        ));
    }

    #[test]
    fn the_debug_rendering_does_not_carry_the_key() {
        let treasury = treasury();
        let rendered = format!("{treasury:?}");
        assert!(rendered.contains(&hex::encode(treasury.address())));
        assert!(!rendered.contains("SigningKey"));
    }

    #[test]
    fn the_missing_password_error_names_the_variable_and_says_why() {
        // The failure a first-time operator hits. It has to name the variable —
        // an operator who cannot find it will reach for a flag that does not
        // exist — and say why the flag does not exist, or the next person adds
        // one.
        let error = missing_password_error(Path::new("treasury.key")).to_string();
        assert!(error.contains(PASSWORD_ENV));
        assert!(error.contains("treasury.key"));
        assert!(error.contains("process list"));
    }

    #[test]
    fn a_missing_keystore_file_is_a_treasury_error() {
        assert!(matches!(
            Treasury::unlock_with(Path::new("no-such-keystore.key"), "hunter2"),
            Err(PoolError::Treasury(_))
        ));
    }

    #[test]
    fn a_keystore_round_trips_through_the_wallets_own_format() {
        // Reusing the wallet's keystore rather than reimplementing it is the
        // whole reason `l1-wallet` gained a library target. This is the test
        // that would fail if the two ever drifted.
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("treasury.key");
        let key = generate_signing_key().expect("key generation");
        let address = key.address();

        let blob = l1_wallet::keystore::encrypt(&key, "correct horse").expect("encrypt");
        std::fs::write(&path, blob).expect("write");

        let treasury = Treasury::unlock_with(&path, "correct horse").expect("unlock");
        assert_eq!(treasury.address(), address);

        assert!(Treasury::unlock_with(&path, "wrong password").is_err());
    }
}
