//! What the confirm screen shows, computed in Rust from the transaction that
//! will be signed.
//!
//! Anti-blind-sign: the frontend renders these fields and composes no summary
//! of its own. `signing_hash` is BLAKE3 over the exact bytes
//! [`Transaction::sign`] covers, and [`crate::wallet::Wallet::sign`] refuses
//! unless it is handed that hash back.

use custom_l1_node::core::describe::{describe, is_fee_output};
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::vault_payload::VaultAction;
use custom_l1_node::core::{ChainTag, Transaction};
use maya_amount::Amount;
use maya_clear_sign::{Context, Effect, Network, Warning, grouped, review};
use primitive_types::U256;
use serde::Serialize;

use crate::amount::format_display;
use crate::networks;

/// Bit of the BLAKE3 digest that decides a letter's case in a checksummed address.
const UPPER_WHEN_NIBBLE_AT_LEAST: u8 = 8;

/// `describe` detail labels whose value is an account address. Those are
/// shown in full Bech32m; the rest (asset, pool, channel, proposal ids) are
/// identifiers, not something a person pays, and stay hex.
const ADDRESS_LABELS: &[&str] = &[
    "pay to",
    "fee to",
    "counterparty",
    "contract",
    "credited to",
    "seller",
    "recipient",
    "validator",
    "vault owner",
    "guardian",
];

/// Everything a signer reads before signing.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Confirmation {
    /// Lowercase hex BLAKE3 of the signing bytes. Handed back to `sign`.
    pub signing_hash: String,
    /// `describe`: one sentence, produced from the transaction's own payload,
    /// with every address in full Bech32m.
    pub description: String,
    /// `describe`'s detail lines, every value in full, in payload order;
    /// addresses in grouped Bech32m.
    pub details: Vec<(&'static str, String)>,
    /// Genesis id the signature commits to, hex. Shown as `chain_prefix`.
    pub chain_id: String,
    /// The network name, from the genesis id alone (`networks::display_name`).
    pub network: String,
    /// The genesis id's two ends, to compare with the published one.
    pub chain_prefix: String,
    /// The address prefix and what it names, e.g. `tmaya (testnet)`.
    pub address_prefix: String,
    /// Sender address, grouped Bech32m.
    pub sender: String,
    /// Nonce the transaction carries.
    pub nonce: u64,
    /// Transfers only: the first output's recipient, grouped Bech32m.
    pub recipient: Option<String>,
    /// Transfers only: the first output's amount, in display units.
    pub amount: Option<String>,
    /// Transfers only: the outputs after the first that pay a fee address (the
    /// collector or the burn sink), in display units.
    pub fee: Option<String>,
    /// Transfers only: the fee is larger than the amount. Raised as a warning.
    pub fee_exceeds_amount: bool,
    /// `maya_clear_sign::review` of the transaction's effects, most severe first.
    pub warnings: Vec<String>,
}

/// The confirmation for `tx` as it will be signed on `chain`, with no
/// contacts or balances known. Signs nothing.
#[must_use]
pub fn confirmation(tx: &Transaction, chain: &ChainTag) -> Confirmation {
    confirmation_with(tx, chain, &Context::default())
}

/// The confirmation for `tx` on `chain`, reviewed against `ctx` (the address
/// book's contacts and the sender's balances). Signs nothing.
#[must_use]
pub fn confirmation_with(tx: &Transaction, chain: &ChainTag, ctx: &Context) -> Confirmation {
    let chain_id = hex::encode(chain.as_bytes());
    let network = networks::address_network(&chain_id);
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
            Some(grouped(network, &first.recipient)),
            Some(format_display(first.amount)),
            Some(format_display(fee)),
            fee > first.amount,
        ),
        None => (None, None, None, false),
    };
    let mut details = described.details;
    details.extend(extra_details(&tx.kind));
    let (description, details) = bech32m_text(network, described.summary, details);
    let effects = effects_of(tx);
    Confirmation {
        signing_hash: hex::encode(blake3::hash(&tx.signing_bytes(chain)).as_bytes()),
        description,
        details,
        network: networks::display_name(&chain_id).to_string(),
        chain_prefix: networks::fingerprint(&chain_id),
        address_prefix: format!("{} ({})", network.hrp(), networks::prefix_name(network)),
        chain_id,
        sender: grouped(network, &tx.sender()),
        nonce: tx.nonce,
        recipient,
        amount,
        fee,
        fee_exceeds_amount,
        // The wallet built the transaction, so the claim is its own simulation:
        // no app is asking. A dApp's claim enters here once one can.
        warnings: review(&effects, &effects, ctx)
            .into_iter()
            .map(|w| warning_text(w).to_string())
            .collect(),
    }
}

/// Fields ADR-086 §6 requires that `describe` gives only as a count.
fn extra_details(kind: &TxKind) -> Vec<(&'static str, String)> {
    match kind {
        TxKind::Vault(action) => match action.as_ref() {
            VaultAction::Configure(c) => c
                .guardians
                .iter()
                .map(|g| ("guardian", hex::encode(g)))
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Rewrites every hex address in `describe`'s output as grouped Bech32m: the
/// detail values named by [`ADDRESS_LABELS`], and the abbreviated forms of
/// those same addresses in the summary sentence.
fn bech32m_text(
    network: Network,
    mut summary: String,
    details: Vec<(&'static str, String)>,
) -> (String, Vec<(&'static str, String)>) {
    let details = details
        .into_iter()
        .map(|(label, value)| {
            let address = ADDRESS_LABELS
                .contains(&label)
                .then(|| hex::decode(&value).ok())
                .flatten()
                .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok());
            match address {
                Some(a) => {
                    let full = grouped(network, &a);
                    let short =
                        format!("{:02x}{:02x}\u{2026}{:02x}{:02x}", a[0], a[1], a[30], a[31]);
                    summary = summary.replace(&short, &full);
                    (label, full)
                }
                None => (label, value),
            }
        })
        .collect();
    (summary, details)
}

/// The value effects `review` checks, read from the transaction's own fields.
fn effects_of(tx: &Transaction) -> Vec<Effect> {
    let native = |to: [u8; 32], amount: U256| Effect::Transfer {
        token: [0; 32],
        to,
        amount: Amount(amount.0),
    };
    match &tx.kind {
        TxKind::Transfer => tx
            .outputs
            .iter()
            .enumerate()
            // The first output is always the payment; later fee outputs are not.
            .filter(|(i, o)| *i == 0 || !is_fee_output(o))
            .map(|(_, o)| native(o.recipient, o.amount))
            .collect(),
        TxKind::TransferAsset(t) => vec![Effect::Transfer {
            token: t.asset,
            to: t.recipient,
            amount: Amount(t.amount.0),
        }],
        TxKind::CallContract(c) => vec![Effect::Call {
            contract: c.contract,
            method: String::new(),
        }],
        TxKind::Vault(action) => match action.as_ref() {
            VaultAction::Request { to, amount } => vec![native(*to, *amount)],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// One line per warning, in the words the confirm screen shows.
#[must_use]
pub fn warning_text(w: Warning) -> &'static str {
    match w {
        Warning::ClaimMismatch => "What the app claims differs from what this transaction does.",
        Warning::ControlChange => "This changes who controls an account or a contract.",
        Warning::UnlimitedApproval => "This allows spending with no practical limit.",
        Warning::ApproveAll => "This hands every item of a collection to someone else.",
        Warning::PermitSignature => "This signature can be used to spend later without asking you.",
        Warning::LookAlikeRecipient => {
            "This recipient looks like one of your contacts but is a different address. Compare every group."
        }
        Warning::DrainsBalance => "This sends 90% or more of your balance.",
        Warning::UnverifiedSpender => "The spender is not a verified contract.",
        Warning::UnverifiedContract => "This calls a contract that is not verified.",
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

/// The text a native approval dialog shows, built from the confirmation the
/// core computed. The webview never supplies these words, so it cannot change
/// what the signer reads. Warnings come first, most severe first.
#[must_use]
pub fn approval_message(shown: &Confirmation) -> String {
    let mut lines: Vec<String> = shown
        .warnings
        .iter()
        .map(|w| format!("Warning: {w}"))
        .collect();
    if shown.fee_exceeds_amount {
        lines.push("Warning: the fee is larger than the amount.".to_string());
    }
    lines.push(shown.description.clone());
    if let Some(to) = &shown.recipient {
        lines.push(format!("To: {to}"));
    }
    if let Some(amount) = &shown.amount {
        lines.push(format!("Amount: {amount} MAYA"));
    }
    if let Some(fee) = &shown.fee {
        lines.push(format!("Fee: {fee} MAYA"));
    }
    lines.push(format!("From: {}", shown.sender));
    lines.push(format!("Network: {}", shown.network));
    lines.push(format!("Address prefix: {}", shown.address_prefix));
    lines.push(format!("Chain: {}", shown.chain_prefix));
    lines.push(format!("Nonce: {}", shown.nonce));
    lines.join("\n")
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
