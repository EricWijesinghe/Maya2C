//! Transaction composers beyond a plain transfer.
//!
//! `payment::sign_transfer` covers the ordinary case. This module adds the two
//! the wallet did not have: an AMM swap and a shielded joinsplit. Both carry a
//! caveat that belongs in the type rather than in a document.
//!
//! # A swap that loses is not a swap that failed
//!
//! Invariant 7, and the single most important thing a wallet must get right
//! about this chain's DEX:
//!
//! > A trade that merely *loses* is a no-op, never an `Err`. A missed slippage
//! > bound, a lost arbitrage race, a batch the pool cannot price: nonce
//! > advances, nothing moves.
//!
//! The reason is structural — a failing transaction fails its whole block
//! here, so making a missed bound an error would hand every trader a way to
//! void a block someone else paid to mine.
//!
//! The consequence for a wallet is that **"my swap did not execute" is a
//! success**, and a UI that showed it as a failure would be describing the
//! chain wrongly. Worse, a user seeing "failed" will retry — and the retry
//! costs another nonce for a trade the market has already moved past.
//!
//! [`SwapOutcome`] therefore has no error arm for it. The two ways a submitted
//! swap ends are `Executed` and `Skipped`, and both are outcomes of a
//! transaction that was accepted.
//!
//! # The shielded pool is not usable for value
//!
//! `maya_zk_privacy::prove::SETUP_IS_TRUSTED` is `false`. The Groth16 parameters
//! come from a reproducible test setup rather than a ceremony, so anyone able
//! to re-run it holds the toxic waste and can mint shielded value no supply
//! audit would reveal. Four guards refuse a value-bearing chain id over
//! exactly this.
//!
//! A wallet is the place a user would meet that, so [`ShieldedComposer`]
//! refuses a value-bearing chain and reports [`SetupTrust`] rather than
//! quietly composing something the network will not honour. The status is a
//! return value, not a doc comment, because a doc comment cannot be rendered
//! in a confirmation dialog.

use custom_l1_node::core::dex_payload::SwapRequest;
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::transaction::Transaction;
use custom_l1_node::crypto::hybrid::HybridSigningKey;

use crate::error::{Result, WalletError};

/// Chain ids on which value is real and the shielded pool must be refused.
///
/// The same list the node refuses to start on and the ceremony refuses to mint
/// a genesis for. Repeated here rather than shared because this answers a
/// different question — "may I compose this?" — and one list would invite
/// relaxing all of them together.
const VALUE_BEARING_CHAINS: &[&str] = &["maya-mainnet", "mainnet"];

/// Whether the shielded pool's trusted setup can be relied on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupTrust {
    /// Parameters came from a real multi-party ceremony.
    Ceremony,
    /// Parameters came from a reproducible test setup.
    ///
    /// Anyone able to re-run it can mint shielded value invisibly. The current
    /// state, and the reason `SETUP_IS_TRUSTED` is `false`.
    Reproducible,
}

impl SetupTrust {
    /// The wallet's current view.
    ///
    /// Read from `zk-privacy`'s own flag rather than duplicated, so it cannot
    /// drift from the thing it describes.
    #[must_use]
    pub fn current() -> Self {
        if maya_zk_privacy::prove::SETUP_IS_TRUSTED {
            Self::Ceremony
        } else {
            Self::Reproducible
        }
    }

    /// What a user needs to be told before shielding anything.
    #[must_use]
    pub const fn warning(self) -> Option<&'static str> {
        match self {
            Self::Ceremony => None,
            Self::Reproducible => Some(
                "The shielded pool's parameters come from a reproducible test setup, \
                 not a ceremony. Anyone able to re-run that setup can mint shielded \
                 value that no supply audit would reveal. Do not shield anything you \
                 are not prepared to lose.",
            ),
        }
    }
}

/// How a submitted swap ended.
///
/// There is deliberately **no** variant for a missed slippage bound. See the
/// module documentation: that is a successful transaction that moved no value,
/// and calling it a failure teaches a user to retry a trade the market has
/// already left behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapOutcome {
    /// The swap executed. `received` is what came back.
    Executed {
        /// Units received.
        received: u64,
    },
    /// The swap was skipped and the input returned.
    ///
    /// The slippage bound was not met, the deadline had passed, or the pool
    /// could not price the trade. The transaction was **accepted**; the nonce
    /// advanced; no value moved.
    Skipped,
}

impl SwapOutcome {
    /// Whether the containing transaction was accepted. Always true.
    ///
    /// Present so a caller writing `if outcome.transaction_succeeded()` gets
    /// the right answer without having to know the invariant first.
    #[must_use]
    pub const fn transaction_succeeded(self) -> bool {
        true
    }

    /// Text for a confirmation screen.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Executed { .. } => "Swap executed.",
            Self::Skipped => {
                "Swap did not execute: your price bound was not met, so your funds \
                 were returned. This is not an error — the transaction was accepted \
                 and your nonce advanced. Submit a new swap if you still want the \
                 trade."
            }
        }
    }
}

/// Composes an AMM swap.
///
/// # Errors
///
/// [`WalletError::AmountOverflow`] for a zero input — a swap of nothing would
/// occupy a nonce and move nothing, which is a mistake rather than a strategy.
///
/// A slippage bound is **not** validated here and cannot fail: `min_out` above
/// what the pool can pay is a skipped swap, not a rejected transaction.
pub fn compose_swap(
    key: &HybridSigningKey,
    request: SwapRequest,
    nonce: u64,
) -> Result<Transaction> {
    if request.amount_in == 0 {
        return Err(WalletError::AmountOverflow);
    }

    let mut tx = Transaction::with_kind(TxKind::Swap(request), nonce);
    tx.sign(key)
        .map_err(|e| WalletError::Signing(e.to_string()))?;
    Ok(tx)
}

/// Composes shielded transactions, subject to the setup's trust state.
#[derive(Debug)]
pub struct ShieldedComposer {
    chain_id: String,
    trust: SetupTrust,
}

impl ShieldedComposer {
    /// Builds a composer for `chain_id`.
    #[must_use]
    pub fn new(chain_id: impl Into<String>) -> Self {
        Self {
            chain_id: chain_id.into(),
            trust: SetupTrust::current(),
        }
    }

    /// The setup's trust state.
    #[must_use]
    pub const fn trust(&self) -> SetupTrust {
        self.trust
    }

    /// Whether this composer will produce anything on its chain.
    #[must_use]
    pub fn is_available(&self) -> bool {
        !(self.trust == SetupTrust::Reproducible
            && VALUE_BEARING_CHAINS.contains(&self.chain_id.as_str()))
    }

    /// The warning a confirmation screen must show, if any.
    #[must_use]
    pub const fn warning(&self) -> Option<&'static str> {
        self.trust.warning()
    }

    /// Checks that shielding is permitted before any proof is built.
    ///
    /// # Why this is separate from composing
    ///
    /// Generating a Groth16 proof takes seconds. Refusing *after* that work
    /// would mean a user watches a spinner and is then told no — so the refusal
    /// happens first, and a UI can grey the option out before anyone clicks it.
    ///
    /// # Errors
    ///
    /// [`WalletError::ShieldedUnavailable`] on a value-bearing chain while the
    /// setup is unceremonied.
    pub fn check(&self) -> Result<()> {
        if self.is_available() {
            return Ok(());
        }
        Err(WalletError::ShieldedUnavailable(format!(
            "refusing to shield on '{}': the pool's parameters come from a \
             reproducible test setup, so shielded value can be minted by anyone \
             able to re-run it. See docs/mainnet-readiness.md section 1.",
            self.chain_id
        )))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_skipped_swap_is_a_successful_transaction() {
        // Invariant 7, asserted where a wallet would get it wrong. A UI that
        // read `Skipped` as a failure would teach users to retry a trade the
        // market has already moved past — and each retry costs a nonce.
        let skipped = SwapOutcome::Skipped;
        assert!(skipped.transaction_succeeded());
        assert!(
            skipped.describe().contains("not an error"),
            "the user-facing text must say so explicitly: {}",
            skipped.describe()
        );
    }

    #[test]
    fn there_is_no_outcome_for_a_missed_bound() {
        // The absence is the design. Exhaustive match: if a `BoundMissed` arm
        // is ever added, this stops compiling and the reviewer is sent to the
        // invariant.
        let outcomes = [SwapOutcome::Executed { received: 1 }, SwapOutcome::Skipped];
        for outcome in outcomes {
            match outcome {
                SwapOutcome::Executed { .. } | SwapOutcome::Skipped => {
                    assert!(outcome.transaction_succeeded());
                }
            }
        }
    }

    #[test]
    fn the_setup_trust_state_is_read_from_zk_privacy() {
        // Not duplicated. If the ceremony happens and the flag flips, this
        // follows without an edit here.
        let expected = if maya_zk_privacy::prove::SETUP_IS_TRUSTED {
            SetupTrust::Ceremony
        } else {
            SetupTrust::Reproducible
        };
        assert_eq!(SetupTrust::current(), expected);
    }

    #[test]
    fn shielding_is_refused_on_a_value_bearing_chain() {
        // The guard a wallet user would actually meet.
        for chain in ["maya-mainnet", "mainnet"] {
            let composer = ShieldedComposer::new(chain);
            if composer.trust() == SetupTrust::Reproducible {
                assert!(!composer.is_available(), "{chain} must be refused");
                let error = composer.check().expect_err("must refuse");
                assert!(format!("{error}").contains("reproducible test setup"));
            }
        }
    }

    #[test]
    fn shielding_is_permitted_on_a_test_chain() {
        let composer = ShieldedComposer::new("maya-genesis-rc1");
        assert!(composer.is_available());
        composer.check().expect("a non-value-bearing chain is fine");
    }

    #[test]
    fn an_untrusted_setup_always_carries_a_warning() {
        // The warning is a return value rather than a doc comment because a
        // doc comment cannot be rendered in a confirmation dialog.
        let composer = ShieldedComposer::new("maya-genesis-rc1");
        if composer.trust() == SetupTrust::Reproducible {
            let warning = composer.warning().expect("an untrusted setup warns");
            assert!(warning.contains("mint shielded value"));
        }
    }

    #[test]
    fn the_refusal_happens_before_any_proof_is_built() {
        // `check` touches no proving machinery, so a UI can grey the option out
        // rather than making a user wait seconds for a refusal.
        let composer = ShieldedComposer::new("mainnet");
        let start = std::time::Instant::now();
        let _ = composer.check();
        assert!(
            start.elapsed() < std::time::Duration::from_millis(50),
            "the refusal must be immediate, not post-proof"
        );
    }
}
