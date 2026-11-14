//! "Explain this block": plain-language explanations built from the
//! clear-signing renderer (`maya-clear-sign`).
//!
//! Each output becomes a clear-sign [`Effect::Transfer`] and is rendered by
//! [`maya_clear_sign::render`], so the explorer prints the same words a wallet
//! and the hardware app show before signing. This module adds only what the
//! renderer has no field for: who sent it, and which signatures it carries.
//!
//! It says nothing the RPC record does not prove. The node's `TransactionInfo`
//! carries no fee, so the sentence says the fee is not reported rather than
//! guessing one. An indexed block is one the node accepted, and the node
//! verifies every signature before accepting a block, so "attached" signatures
//! on an indexed transaction are verified ones.

use custom_l1_node::core::describe::rewrite_addresses;
use custom_l1_node::rpc::TransactionInfo;
use maya_clear_sign::{Effect, Network, render};

use crate::model::{IndexedBlock, IndexedTx};
use crate::ui::short_hash;

/// The all-zero token id: the native coin, in clear-sign's convention.
const NATIVE: [u8; 32] = [0; 32];

/// The clear-sign effects of `tx`, one transfer per output, plus the number of
/// outputs whose recipient is not 32 bytes of hex (skipped, never invented).
#[must_use]
pub fn effects(tx: &TransactionInfo) -> (Vec<Effect>, usize) {
    let mut unreadable = 0;
    let effects = tx
        .outputs
        .iter()
        .filter_map(|o| {
            let to = hex::decode(&o.recipient)
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok());
            if to.is_none() {
                unreadable += 1;
            }
            Some(Effect::Transfer {
                token: NATIVE,
                to: to?,
                // Same little-endian u64 limbs on both sides (ADR-046).
                amount: maya_amount::Amount(o.amount.0),
            })
        })
        .collect();
    (effects, unreadable)
}

/// Which signatures `tx` carries, as one sentence.
fn signatures(tx: &TransactionInfo) -> String {
    if !tx.signed {
        return "No signature is attached.".to_string();
    }
    if let Some(m) = &tx.multisig {
        return format!(
            "{} of {} multisig approvals attached ({} required), verified when the block was accepted.",
            m.approved_by.len(),
            m.keys.len(),
            m.threshold
        );
    }
    if let Some(s) = &tx.suite {
        return format!(
            "Signed with suite 0x{:02x}, verified when the block was accepted.",
            s.id
        );
    }
    "Both signatures (ML-DSA-65 and SLH-DSA) attached, verified when the block was accepted."
        .to_string()
}

/// One paragraph explaining `tx`, recipients in Bech32m on `network`.
#[must_use]
pub fn explain_tx(tx: &TransactionInfo, network: Network) -> String {
    // A payload kind (tokens, NFTs, names, schedules, ...) moves value through
    // its payload, not its outputs: the node's own sentence and its labelled
    // values say what it does; the outputs alone would read "moved no value".
    if !matches!(tx.kind.as_str(), "" | "transfer") && !tx.summary.is_empty() {
        // Addresses in Bech32m, as the transfer path and the wallet show them.
        let labelled = tx
            .details
            .iter()
            .map(|d| (d.label.clone(), d.value.clone()))
            .collect();
        let (summary, labelled) = rewrite_addresses(tx.summary.clone(), labelled, |a| {
            maya_clear_sign::grouped(network, a)
        });
        let details: Vec<String> = labelled
            .iter()
            .map(|(label, value)| format!("{label} {value}"))
            .collect();
        let details = if details.is_empty() {
            String::new()
        } else {
            format!(" Details: {}.", details.join("; "))
        };
        return format!(
            "{} signed: {summary}{details} {}",
            short_hash(&tx.sender),
            signatures(tx)
        );
    }
    let (effects, unreadable) = effects(tx);
    let moves = if effects.is_empty() {
        "moved no value".to_string()
    } else {
        effects
            .iter()
            .map(|e| render(e, network))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let skipped = match unreadable {
        0 => String::new(),
        n => format!(" {n} output(s) had a recipient the explorer could not read."),
    };
    let fee = match &tx.sponsor {
        Some(s) => format!(
            "Fee: paid by sponsor {}, at most {} base units.",
            short_hash(&s.address),
            crate::plain::group_digits(s.max_fee)
        ),
        None => "Fee: not reported by the node.".to_string(),
    };
    format!(
        "{} signed: {moves}.{skipped} {fee} {}",
        short_hash(&tx.sender),
        signatures(tx)
    )
}

/// One sentence summarising `block` from its indexed transactions.
#[must_use]
pub fn explain_block(block: &IndexedBlock, txs: &[IndexedTx]) -> String {
    if txs.is_empty() {
        return format!("Block {} holds no transactions.", block.height);
    }
    let total = txs.iter().fold(primitive_types::U256::zero(), |sum, t| {
        // An unparsable total is a corrupt row; it adds nothing rather than a guess.
        sum.saturating_add(primitive_types::U256::from_dec_str(&t.total_out).unwrap_or_default())
    });
    let unsigned = txs.iter().filter(|t| !t.signed).count();
    let signed = match unsigned {
        0 => "every one signed".to_string(),
        n => format!("{n} without a signature"),
    };
    format!(
        "Block {} holds {} transaction(s) moving {} base units in total, {signed}.",
        block.height,
        txs.len(),
        crate::plain::group_digits(total)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use custom_l1_node::rpc::{MultisigInfo, OutputInfo, SuiteKeyInfo};
    use primitive_types::U256;

    fn tx(outputs: Vec<(&str, u64)>, signed: bool) -> TransactionInfo {
        TransactionInfo {
            txid: "ab".repeat(32),
            sender: "5ad1a0".repeat(10) + "beef",
            lattice_public_key: "cc".repeat(4),
            hash_public_key: "dd".repeat(4),
            nonce: 0,
            outputs: outputs
                .into_iter()
                .map(|(r, a)| OutputInfo {
                    recipient: r.to_string(),
                    amount: U256::from(a),
                })
                .collect(),
            signed,
            suite: None,
            multisig: None,
            kind: "transfer".into(),
            summary: String::new(),
            careful: true,
            details: Vec::new(),
            sponsor: None,
        }
    }

    #[test]
    fn a_transfer_uses_the_clear_sign_words() {
        let bob = "b0".to_string() + &"00".repeat(30) + "0b";
        let t = tx(vec![(&bob, 5)], true);
        let want = render(
            &Effect::Transfer {
                token: NATIVE,
                to: <[u8; 32]>::try_from(hex::decode(&bob).unwrap()).unwrap(),
                amount: maya_amount::Amount::from_u64(5),
            },
            Network::Testnet,
        );
        assert_eq!(
            explain_tx(&t, Network::Testnet),
            format!(
                "5ad1a05a…a0beef signed: {want}. Fee: not reported by the node. \
                 Both signatures (ML-DSA-65 and SLH-DSA) attached, verified when the block was accepted."
            )
        );
        assert!(want.starts_with("Send 5 base units of the native coin to tmaya1"));
    }

    #[test]
    fn every_output_is_rendered_and_bad_recipients_are_counted_not_invented() {
        let a = "11".repeat(32);
        let t = tx(vec![(&a, 1), ("zz", 2), (&a, 3)], true);
        let (effects, unreadable) = effects(&t);
        assert_eq!(effects.len(), 2);
        assert_eq!(unreadable, 1);
        let text = explain_tx(&t, Network::Devnet);
        assert!(text.contains("Send 1 base units"), "{text}");
        assert!(text.contains("; Send 3 base units"), "{text}");
        assert!(text.contains("1 output(s) had a recipient the explorer could not read."));
    }

    #[test]
    fn a_large_amount_survives_the_u256_to_amount_conversion() {
        let mut t = tx(vec![(&"22".repeat(32), 0)], true);
        t.outputs[0].amount =
            U256::from_dec_str("340282366920938463463374607431768211456").unwrap();
        assert!(
            explain_tx(&t, Network::Devnet)
                .contains("Send 340282366920938463463374607431768211456 base units")
        );
    }

    #[test]
    fn signatures_say_what_is_attached() {
        let mut t = tx(Vec::new(), false);
        assert!(
            explain_tx(&t, Network::Devnet).ends_with(
                "moved no value. Fee: not reported by the node. No signature is attached."
            )
        );
        t.signed = true;
        t.suite = Some(SuiteKeyInfo {
            id: 0x11,
            public_key: String::new(),
        });
        assert!(
            explain_tx(&t, Network::Devnet)
                .ends_with("Signed with suite 0x11, verified when the block was accepted.")
        );
        t.multisig = Some(MultisigInfo {
            threshold: 2,
            keys: vec![
                SuiteKeyInfo {
                    id: 0x11,
                    public_key: String::new()
                };
                3
            ],
            approved_by: vec![0, 2],
        });
        assert!(
            explain_tx(&t, Network::Devnet)
                .contains("2 of 3 multisig approvals attached (2 required)")
        );
    }

    #[test]
    fn a_v4_payload_is_explained_by_the_node_words_not_as_no_value() {
        use custom_l1_node::core::Transaction;
        use custom_l1_node::core::asset_mint_payload::AssetMintAction;
        use custom_l1_node::core::name_payload::{NameAction, NameUpdate};
        use custom_l1_node::core::nft_payload::NftAction;
        use custom_l1_node::core::payload::TxKind;
        use custom_l1_node::core::schedule_payload::ScheduleAction;
        let kinds = [
            TxKind::AssetMint(AssetMintAction::Mint {
                asset: [0xB2; 32],
                to: [0xC3; 32],
                amount: U256::from(250),
            }),
            TxKind::Nft(NftAction::TransferItem {
                collection: [0xA1; 32],
                index: 42,
                to: [0xB2; 32],
            }),
            TxKind::Name(NameAction::Update {
                label: b"alice".to_vec(),
                op: NameUpdate::Transfer([0xB2; 32]),
            }),
            TxKind::Schedule(ScheduleAction::Create {
                payee: [0xB2; 32],
                amount: U256::from(9),
                first_due: 1,
                interval: 2,
                runs: 3,
            }),
        ];
        // The account each form names: the mint pays 0xC3; the rest name 0xB2.
        let accounts = [[0xC3; 32], [0xB2; 32], [0xB2; 32], [0xB2; 32]];
        for (kind, account) in kinds.into_iter().zip(accounts) {
            let info = TransactionInfo::from(&Transaction::with_kind(kind, 0));
            let text = explain_tx(&info, Network::Devnet);
            assert!(!text.contains("moved no value"), "{text}");
            // The account in full Bech32m, as the transfer path renders it:
            // never its hex, never only its short form.
            let shown = maya_clear_sign::grouped(Network::Devnet, &account);
            assert!(text.contains(&shown), "{text}");
            assert!(!text.contains(&hex::encode(account)), "{text}");
            let short = format!("{:02x}{:02x}\u{2026}", account[0], account[1]);
            assert!(!text.contains(&short), "{text}");
        }
    }

    #[test]
    fn a_sponsored_transfer_names_who_pays_the_fee() {
        let mut t = tx(vec![(&"11".repeat(32), 1)], true);
        t.sponsor = Some(custom_l1_node::rpc::SponsorInfo {
            address: "5b".repeat(32),
            max_fee: U256::from(12_000),
            valid_until: 9,
        });
        let text = explain_tx(&t, Network::Devnet);
        assert!(
            text.contains("Fee: paid by sponsor 5b5b5b5b…5b5b5b, at most 12,000 base units."),
            "{text}"
        );
        assert!(!text.contains("not reported"), "{text}");
    }

    fn indexed(total: &str, signed: bool) -> IndexedTx {
        IndexedTx {
            txid: String::new(),
            height: 7,
            sender: String::new(),
            nonce: 0,
            output_count: 1,
            total_out: total.into(),
            signed,
            kind: String::new(),
            summary: String::new(),
            careful: false,
            explanation: String::new(),
        }
    }

    fn block() -> IndexedBlock {
        IndexedBlock {
            height: 7,
            id: String::new(),
            prev_hash: String::new(),
            state_root: String::new(),
            timestamp: 0,
            nonce: 0,
            difficulty_target: String::new(),
            tx_count: 0,
            work: "0".into(),
        }
    }

    #[test]
    fn a_block_sums_its_transactions() {
        assert_eq!(
            explain_block(&block(), &[]),
            "Block 7 holds no transactions."
        );
        assert_eq!(
            explain_block(&block(), &[indexed("1500000", true), indexed("500", false)]),
            "Block 7 holds 2 transaction(s) moving 1,500,500 base units in total, 1 without a signature."
        );
        assert_eq!(
            explain_block(&block(), &[indexed("3", true)]),
            "Block 7 holds 1 transaction(s) moving 3 base units in total, every one signed."
        );
    }
}
