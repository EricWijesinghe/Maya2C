//! Plain-language transaction summaries (Master Prompt 29 §3).
//!
//! The sentence says only what the indexed record proves. Amounts are in base
//! units because the chain has no ratified display symbol or decimals yet
//! (`chain/maya2c-testnet.json` leaves both null); inventing "MAYA" with a
//! decimal point here would put a number on screen nobody has agreed to.
//! Recipients are counted, not named: the index keeps output totals, not
//! output addresses. The technical view stays one click away on every page.

use crate::model::IndexedTx;
use crate::ui::short_hash;

/// Groups digits in threes so large base-unit amounts stay readable.
#[must_use]
pub fn group_digits(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// One sentence describing `tx`.
#[must_use]
pub fn describe(tx: &IndexedTx) -> String {
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
        group_digits(tx.total_out),
        tx.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(outputs: i32, total: i64, signed: bool) -> IndexedTx {
        IndexedTx {
            txid: "ab".repeat(32),
            height: 1_204,
            sender: "5ad1a0".repeat(10) + "beef",
            nonce: 3,
            output_count: outputs,
            total_out: total,
            signed,
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
    fn digits_group_in_threes() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(-1_234_567), "-1,234,567");
    }
}
