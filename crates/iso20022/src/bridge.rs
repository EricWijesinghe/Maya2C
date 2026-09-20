//! From a bank message to a Maya2C payment intent, and the reason this must not
//! run on a chain that holds value.
//!
//! ## The one idea to read first
//!
//! **A payment sealed to the committee is confidential against a classical
//! adversary and nobody else, and the envelope is on chain forever.**
//!
//! The sealed mempool (`mev`, `src/sealed/`) is a KEM/DEM construction whose
//! KEM half is Ristretto ElGamal. That is a deliberate choice — there is no
//! ML-KEM analogue of threshold ElGamal, and a threshold scheme is the whole
//! point of the sealed mempool — but it means the confidentiality of anything
//! sealed rests on the discrete log problem, on a chain whose every signature
//! and every transport handshake is post-quantum precisely because that
//! assumption is expected to fail.
//!
//! For an ordinary transaction that is a bounded loss: the envelope is opened a
//! block or two later and its contents become public anyway. For a bank payment
//! it is not. A pacs.008 carries a debtor, a creditor, their account numbers
//! and an amount, and those are still sensitive in twenty years. Sealing one is
//! writing it down in a form that is readable later, at a time of the
//! adversary's choosing, and no later fix reaches an envelope already on chain.
//!
//! So [`check_chain`] refuses a value-bearing chain while
//! [`CONFIDENTIALITY_IS_POST_QUANTUM`] is `false`, exactly as
//! `maya_zkml::srs::check_chain` refuses one while its SRS is untrusted. This
//! is not a feature flag to be flipped when the bridge is "ready" — it is
//! flipped when the sealing is post-quantum, and not before.
//!
//! ## The gateway is a trusted party
//!
//! A pacs.008 arrives over a bank rail carrying no Maya2C keypair, so something
//! has to sign on its behalf. That something is the gateway, and it can
//! therefore mint a payment instruction the bank never sent. The chain has one
//! trusted party today — the oracle, which invariant 11 keeps absent by default
//! — and this is the second. It is absent by default for the same reason, and
//! turning it on is a decision somebody writes down.
//!
//! ## What this module does not do
//!
//! It does not build a transaction, hold a key, or seal anything. It produces a
//! [`PaymentIntent`]: who, to whom, how much, under what reference. Turning one
//! into a `TxKind` and sealing it to the committee is the gateway's job, in the
//! crate that already depends on chain types. That boundary is what keeps this
//! crate fuzzable without RocksDB in the graph.

use crate::amount::Amount;
use crate::error::{Error, Result};
use crate::pacs008::CustomerCreditTransfer;
use crate::pacs009::FinancialInstitutionTransfer;
use crate::party::{Currency, Party};

/// Whether what seals a payment resists a quantum adversary.
///
/// `false`, and the whole of this module's caution follows from it. Flipping it
/// is a claim about `mev`'s KEM, not about this crate.
pub const CONFIDENTIALITY_IS_POST_QUANTUM: bool = false;

/// Chain identifiers this bridge refuses while the seal is classical.
///
/// Duplicated rather than imported, like every other guard in this workspace
/// (`crates/zkml/src/srs.rs:54`, `apps/faucet/src/lib.rs:50`, `bins/maya2c-node/src/main.rs:91`): the
/// guard must not depend on the crate it guards against.
const VALUE_BEARING_CHAINS: &[&str] = &["maya-mainnet", "mainnet"];

/// Refuses to let the bridge near a chain that holds value.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] for a value-bearing chain while
/// [`CONFIDENTIALITY_IS_POST_QUANTUM`] is `false`.
pub fn check_chain(chain_id: &str) -> Result<()> {
    if !CONFIDENTIALITY_IS_POST_QUANTUM && VALUE_BEARING_CHAINS.contains(&chain_id) {
        return Err(Error::Unsupported(format!(
            "the ISO 20022 bridge refuses {chain_id}: a sealed payment's confidentiality \
             rests on Ristretto ElGamal, the envelope is on chain permanently, and a \
             debtor's account number is still sensitive when that assumption fails"
        )));
    }
    Ok(())
}

/// What kind of message an intent came from.
///
/// Carried rather than discarded because the two settle against different
/// accounts: a customer transfer moves a customer's money, an institution
/// transfer moves the bank's own. A statement that mixed them would reconcile
/// against neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// From a `pacs.008`.
    CustomerCreditTransfer,
    /// From a `pacs.009`.
    InstitutionTransfer,
}

/// One payment, in terms this crate can state without knowing what a block is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaymentIntent {
    /// The message this came from.
    pub origin: Origin,
    /// `GrpHdr/MsgId` of the message that carried it.
    pub message_id: String,
    /// `PmtId/EndToEndId`, the reference that survives the whole chain and the
    /// one a statement reconciles by.
    pub end_to_end_id: String,
    /// Who pays.
    pub debtor: Party,
    /// Who is paid.
    pub creditor: Party,
    /// How much, in base units. See [`crate::amount`].
    pub amount: Amount,
    /// The currency the amount is in.
    pub currency: Currency,
    /// `RmtInf/Ustrd`, when the message carried one. `pacs.009` never does.
    pub remittance: Option<String>,
}

/// Every payment a `pacs.008` instructs.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] if `chain_id` is value-bearing while the seal
/// is classical — checked here rather than at the call site, so a caller cannot
/// reach the intents without passing the chain it means to submit them to.
pub fn intents_from_pacs008(
    message: &CustomerCreditTransfer,
    chain_id: &str,
) -> Result<Vec<PaymentIntent>> {
    check_chain(chain_id)?;
    Ok(message
        .transactions
        .iter()
        .map(|transaction| PaymentIntent {
            origin: Origin::CustomerCreditTransfer,
            message_id: message.message_id.clone(),
            end_to_end_id: transaction.end_to_end_id.clone(),
            debtor: transaction.debtor.clone(),
            creditor: transaction.creditor.clone(),
            amount: transaction.amount,
            currency: transaction.currency,
            remittance: transaction.remittance.clone(),
        })
        .collect())
}

/// Every payment a `pacs.009` instructs.
///
/// # Errors
///
/// As [`intents_from_pacs008`].
pub fn intents_from_pacs009(
    message: &FinancialInstitutionTransfer,
    chain_id: &str,
) -> Result<Vec<PaymentIntent>> {
    check_chain(chain_id)?;
    Ok(message
        .transactions
        .iter()
        .map(|transaction| PaymentIntent {
            origin: Origin::InstitutionTransfer,
            message_id: message.message_id.clone(),
            end_to_end_id: transaction.end_to_end_id.clone(),
            debtor: transaction.debtor.clone(),
            creditor: transaction.creditor.clone(),
            amount: transaction.amount,
            currency: transaction.currency,
            remittance: None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_value_bearing_chain_is_refused_while_the_seal_is_classical() {
        for chain in VALUE_BEARING_CHAINS {
            let error = check_chain(chain).expect_err("mainnet");
            assert!(error.to_string().contains("refuses"), "{error}");
        }
    }

    #[test]
    fn a_test_chain_is_allowed() {
        check_chain("l1-testnet-1").expect("testnet");
        check_chain("maya-devnet").expect("devnet");
    }

    #[test]
    fn the_guard_is_in_the_path_that_produces_intents() {
        // Not merely available to a caller who remembers it. A bridge whose
        // refusal is opt-in is a bridge that runs on mainnet the first time
        // somebody writes a new entry point.
        let message = CustomerCreditTransfer {
            message_id: "MSG-1".into(),
            created_at: "2026-09-13T00:00:00Z".into(),
            transactions: Vec::new(),
        };
        assert!(intents_from_pacs008(&message, "mainnet").is_err());
        assert!(intents_from_pacs008(&message, "l1-testnet-1").is_ok());
    }
}
