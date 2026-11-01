//! What the confirm screen shows, computed in Rust from the transaction that
//! will be signed.
//!
//! Anti-blind-sign: the frontend renders these fields and composes no summary
//! of its own. `signing_hash` is BLAKE3 over the exact bytes
//! [`Transaction::sign`] covers, and [`crate::wallet::Wallet::sign`] refuses
//! unless it is handed that hash back.

use custom_l1_node::core::describe::{describe, is_fee_output};
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::{ChainTag, Transaction};
use primitive_types::U256;
use serde::Serialize;

use crate::amount::format_display;

/// Bit of the BLAKE3 digest that decides a letter's case in a checksummed address.
const UPPER_WHEN_NIBBLE_AT_LEAST: u8 = 8;

/// Everything a signer reads before signing.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Confirmation {
    /// Lowercase hex BLAKE3 of the signing bytes. Handed back to `sign`.
    pub signing_hash: String,
    /// `describe`: one sentence, produced from the transaction's own payload.
    pub description: String,
    /// `describe`'s detail lines, every value in full, in payload order.
    pub details: Vec<(&'static str, String)>,
    /// Genesis id the signature commits to, hex.
    pub chain_id: String,
    /// Sender address, hex.
    pub sender: String,
    /// Nonce the transaction carries.
    pub nonce: u64,
    /// Transfers only: the first output's recipient, checksummed hex.
    pub recipient: Option<String>,
    /// Transfers only: the first output's amount, in display units.
    pub amount: Option<String>,
    /// Transfers only: the outputs after the first that pay a fee address (the
    /// collector or the burn sink), in display units.
    pub fee: Option<String>,
    /// Transfers only: the fee is larger than the amount. Raised as a warning.
    pub fee_exceeds_amount: bool,
}

/// The confirmation for `tx` as it will be signed on `chain`. Signs nothing.
#[must_use]
pub fn confirmation(tx: &Transaction, chain: &ChainTag) -> Confirmation {
    let described = describe(tx);
    let transfer = match (&tx.kind, tx.outputs.split_first()) {
        (TxKind::Transfer, Some((first, rest))) => {
            let fee = rest
                .iter()
                .filter(|o| is_fee_output(o))
                .fold(U256::zero(), |sum, o| sum.saturating_add(o.amount));
            Some((first, fee))
        }
        _ => None,
    };
    let (recipient, amount, fee, fee_exceeds_amount) = match transfer {
        Some((first, fee)) => (
            Some(checksummed(&first.recipient)),
            Some(format_display(first.amount)),
            Some(format_display(fee)),
            fee > first.amount,
        ),
        None => (None, None, None, false),
    };
    Confirmation {
        signing_hash: hex::encode(blake3::hash(&tx.signing_bytes(chain)).as_bytes()),
        description: described.summary,
        details: described.details,
        chain_id: hex::encode(chain.as_bytes()),
        sender: hex::encode(tx.sender()),
        nonce: tx.nonce,
        recipient,
        amount,
        fee,
        fee_exceeds_amount,
    }
}

/// A 32-byte address as hex with an EIP-55-style case checksum, keyed by BLAKE3.
///
/// Display only: hex parsing ignores case, so a typed address still works and a
/// re-typed one that differs in case from the displayed one is visibly wrong.
#[must_use]
pub fn checksummed(address: &[u8; 32]) -> String {
    let lower = hex::encode(address);
    let digest = blake3::hash(lower.as_bytes());
    lower
        .char_indices()
        .map(|(i, c)| {
            let byte = digest.as_bytes()[i / 2];
            let nibble = if i % 2 == 0 { byte >> 4 } else { byte & 0x0f };
            if c.is_ascii_alphabetic() && nibble >= UPPER_WHEN_NIBBLE_AT_LEAST {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn checksum_only_changes_case() {
        let address = [0xab_u8; 32];
        let shown = checksummed(&address);
        assert_eq!(shown.to_ascii_lowercase(), hex::encode(address));
        assert_eq!(hex::decode(&shown).unwrap(), address);
        assert_ne!(shown, hex::encode(address), "letters must carry case");
    }
}
