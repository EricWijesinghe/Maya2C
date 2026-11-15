//! 256-bit amounts: one whole coin (10^24 base units) does not fit in `u64`,
//! so the watcher must credit it exactly, journal it exactly, and refuse a
//! balance that would leave 256 bits instead of wrapping.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use maya_amount::{Amount, ONE_COIN};
use maya_deposit_watcher::{BlockSource, Transfer, WatchError, Watcher};

const TO: &str = "addr";

fn watched() -> BTreeSet<String> {
    BTreeSet::from([TO.to_string()])
}

fn transfer(txid: &str, amount: Amount) -> Transfer {
    Transfer {
        txid: txid.to_string(),
        to: TO.to_string(),
        amount,
    }
}

/// Block `h` carries the transfers at `h` in `blocks`, one list per height.
struct Blocks(Vec<Vec<Transfer>>);

impl BlockSource for Blocks {
    fn finalized_height(&self) -> u64 {
        u64::try_from(self.0.len()).unwrap()
    }
    fn transfers(&self, h: u64) -> Result<Vec<Transfer>, String> {
        Ok(self.0[usize::try_from(h).unwrap() - 1].clone())
    }
}

#[test]
fn one_whole_coin_is_credited_exactly_and_survives_a_restart() {
    assert!(u64::try_from(ONE_COIN).is_err(), "the test must exceed u64");
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("j.jsonl");
    let coin = Amount::from_u128(ONE_COIN);
    let src = Blocks(vec![vec![transfer("coin", coin)]]);

    let mut w = Watcher::open(&path, watched()).unwrap();
    w.poll(&src).unwrap();
    assert_eq!(w.balances().get(TO), Some(&coin));
    assert_eq!(coin.to_string(), "1000000000000000000000000");
    drop(w);

    let journal = std::fs::read_to_string(&path).unwrap();
    assert!(
        journal.contains(r#""amount":"1000000000000000000000000""#),
        "amount is journaled as a base-10 string: {journal}"
    );
    let w = Watcher::open(&path, watched()).unwrap();
    assert_eq!(w.balances().get(TO), Some(&coin), "replay reads it back");
}

#[test]
fn a_credit_past_256_bits_is_refused_and_nothing_is_journaled() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("j.jsonl");
    let src = Blocks(vec![
        vec![transfer("a", Amount::MAX)],
        vec![transfer("b", Amount::ONE)],
    ]);

    let mut w = Watcher::open(&path, watched()).unwrap();
    let Err(err) = w.poll(&src) else {
        panic!("a balance above 2^256 - 1 was credited");
    };
    assert!(
        matches!(err, WatchError::Overflow(ref a) if a == TO),
        "{err}"
    );
    assert_eq!(w.cursor(), 1, "block 1 is fine, block 2 is not");
    assert_eq!(w.balances().get(TO), Some(&Amount::MAX));
    drop(w);

    let journal = std::fs::read_to_string(&path).unwrap();
    assert_eq!(journal.lines().count(), 1, "block 2 must not be journaled");
}

#[test]
fn amount_text_accepts_the_256_bit_maximum_and_refuses_one_more_digit() {
    let max = Amount::MAX.to_string();
    assert_eq!(
        max,
        "115792089237316195423570985008687907853269984665640564039457584007913129639935"
    );
    let back: Transfer =
        serde_json::from_str(&format!(r#"{{"txid":"t","to":"{TO}","amount":"{max}"}}"#)).unwrap();
    assert_eq!(back.amount, Amount::MAX);

    let too_big = format!("1{}", "0".repeat(78));
    let refused = serde_json::from_str::<Transfer>(&format!(
        r#"{{"txid":"t","to":"{TO}","amount":"{too_big}"}}"#
    ));
    assert!(
        refused.is_err(),
        "10^78 is above 2^256 - 1 and must not wrap"
    );
}
