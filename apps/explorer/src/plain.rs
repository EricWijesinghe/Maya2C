//! Plain-language transaction summaries (Master Prompt 29 §3).
//!
//! The sentence says only what the indexed record proves. Amounts are in base
//! units. The wallet owns the display (MAYA, 24 decimals, ADR-068); this page
//! states the indexed count and never applies a decimal point of its own.
//! Recipients are counted, not named: the index keeps output totals, not
//! output addresses. The technical view stays one click away on every page.

use crate::model::IndexedTx;
use crate::ui::short_hash;

/// Groups digits in threes so large base-unit amounts stay readable.
#[must_use]
pub fn group_digits(n: impl std::fmt::Display) -> String {
    let text = n.to_string();
    let digits = text.trim_start_matches('-');
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if text.starts_with('-') {
        format!("-{out}")
    } else {
        out
    }
}

/// One sentence describing `tx`.
#[must_use]
pub fn describe(tx: &IndexedTx) -> String {
    // The node's own sentence is exactly what a wallet showed before signing.
    if !tx.summary.is_empty() {
        return tx.summary.clone();
    }
    let recipients = match tx.output_count {
        1 => "1 recipient".to_string(),
        n => format!("{n} recipients"),
    };
    let signature = if tx.signed {
        ""
    } else {
        " No signature is attached."
    };
    format!(
        "{} sent {} base units to {recipients} in block {}.{signature}",
        short_hash(&tx.sender),
        group_digits(&tx.total_out),
        tx.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(outputs: i32, total: i64, signed: bool) -> IndexedTx {
        let total = total.to_string();
        IndexedTx {
            txid: "ab".repeat(32),
            height: 1_204,
            sender: "5ad1a0".repeat(10) + "beef",
            nonce: 3,
            output_count: outputs,
            total_out: total,
            signed,
            kind: String::new(),
            summary: String::new(),
            careful: false,
        }
    }

    #[test]
    fn a_transfer_reads_as_a_sentence() {
        assert_eq!(
            describe(&tx(2, 1_500_000, true)),
            "5ad1a05a…a0beef sent 1,500,000 base units to 2 recipients in block 1204."
        );
        assert_eq!(
            describe(&tx(1, 7, false)),
            "5ad1a05a…a0beef sent 7 base units to 1 recipient in block 1204. No signature is attached."
        );
    }

    #[test]
    fn the_nodes_own_sentence_wins_over_the_generic_one() {
        let mut with = tx(1, 5, true);
        with.summary = "Vote against proposal 7".to_string();
        assert_eq!(describe(&with), "Vote against proposal 7");
    }

    #[test]
    fn digits_group_in_threes() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(-1_234_567), "-1,234,567");
    }
}
