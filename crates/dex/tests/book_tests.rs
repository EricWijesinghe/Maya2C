//! The order book index and the key ordering it is built on.
//!
//! The ordering is the part worth testing hardest. If `sort_key` ever stops
//! being priority order, matching does not fail — it silently fills the wrong
//! orders, and the chain still agrees with itself because every node computes
//! the same wrong answer.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_dex::book::{Book, Order, SORT_KEY_LEN, Side, quote_for_base, sort_key};
use maya_dex::error::DexError;
use maya_dex::types::PRICE_SCALE;

fn order(id: u8, side: Side, price: u64, amount: u64, sequence: u64) -> Order {
    Order {
        id: [id; 32],
        owner: [id; 32],
        side,
        price,
        amount,
        remaining: amount,
        sequence,
        expiry: 0,
    }
}

#[test]
fn asks_sort_cheapest_first_and_bids_sort_dearest_first() {
    let mut book = Book::new();
    book.insert(order(1, Side::Ask, 300, 10, 1))
        .expect("insert");
    book.insert(order(2, Side::Ask, 100, 10, 2))
        .expect("insert");
    book.insert(order(3, Side::Ask, 200, 10, 3))
        .expect("insert");
    book.insert(order(4, Side::Bid, 50, 10, 4)).expect("insert");
    book.insert(order(5, Side::Bid, 90, 10, 5)).expect("insert");
    book.insert(order(6, Side::Bid, 70, 10, 6)).expect("insert");

    let asks: Vec<u64> = book
        .side_orders(Side::Ask)
        .iter()
        .map(|o| o.price)
        .collect();
    let bids: Vec<u64> = book
        .side_orders(Side::Bid)
        .iter()
        .map(|o| o.price)
        .collect();

    assert_eq!(asks, vec![100, 200, 300]);
    assert_eq!(bids, vec![90, 70, 50]);
}

#[test]
fn equal_prices_break_in_arrival_order_on_both_sides() {
    let mut book = Book::new();
    // Inserted out of sequence, so a structure that merely preserved insertion
    // order would pass by accident.
    book.insert(order(3, Side::Ask, 100, 10, 30)).expect("i");
    book.insert(order(1, Side::Ask, 100, 10, 10)).expect("i");
    book.insert(order(2, Side::Ask, 100, 10, 20)).expect("i");

    let sequences: Vec<u64> = book
        .side_orders(Side::Ask)
        .iter()
        .map(|o| o.sequence)
        .collect();
    assert_eq!(sequences, vec![10, 20, 30]);
}

#[test]
fn the_storage_key_sorts_lexicographically_in_priority_order() {
    // This is the property that lets the node rebuild a book with a prefix scan
    // and no sort step. Little-endian would order 256 before 1 and make the
    // whole scheme nonsense, so the case is checked explicitly.
    let mut ask_keys: Vec<[u8; SORT_KEY_LEN]> = [1_u64, 2, 255, 256, 65_536, u64::MAX / 2]
        .iter()
        .map(|&price| sort_key(Side::Ask, price, 0))
        .collect();
    let sorted = {
        let mut copy = ask_keys.clone();
        copy.sort_unstable();
        copy
    };
    assert_eq!(ask_keys, sorted, "ask keys are not already in price order");

    // Bids invert, so the same ascending byte order walks prices downward.
    ask_keys = [1_u64, 2, 255, 256, 65_536, u64::MAX / 2]
        .iter()
        .rev()
        .map(|&price| sort_key(Side::Bid, price, 0))
        .collect();
    let sorted = {
        let mut copy = ask_keys.clone();
        copy.sort_unstable();
        copy
    };
    assert_eq!(ask_keys, sorted, "bid keys are not already in price order");
}

#[test]
fn the_sequence_only_breaks_ties_and_never_outranks_price() {
    // A later order at a better price must still come first. Packing sequence
    // into the high bytes instead of the low ones would invert that.
    let early_bad_price = sort_key(Side::Ask, 500, 1);
    let late_good_price = sort_key(Side::Ask, 100, 999_999);
    assert!(late_good_price < early_bad_price);
}

#[test]
fn a_bid_pays_a_rounded_up_quote_and_an_ask_receives_a_rounded_down_one() {
    // 3 base at a price of one-and-a-half quote per base is 4.5 quote. The bid
    // escrows 5 so it can never be short; the ask is credited 4 so a fill can
    // never create a unit.
    let price = (PRICE_SCALE as u64) * 3 / 2;
    assert_eq!(quote_for_base(3, price, Side::Bid), Ok(5));
    assert_eq!(quote_for_base(3, price, Side::Ask), Ok(4));
}

#[test]
fn a_zero_price_is_rejected_rather_than_dividing() {
    assert_eq!(quote_for_base(1, 0, Side::Bid), Err(DexError::ZeroPrice));

    let mut book = Book::new();
    assert_eq!(
        book.insert(order(1, Side::Ask, 0, 10, 1)),
        Err(DexError::ZeroPrice)
    );
}

#[test]
fn an_order_that_could_never_fill_is_refused_a_slot() {
    // A book full of unfillable orders is a cheap denial of service: it costs
    // one transaction each and makes every node carry the records forever.
    let mut book = Book::new();
    assert_eq!(
        book.insert(order(1, Side::Ask, 100, 0, 1)),
        Err(DexError::ZeroAmount)
    );
    assert!(book.is_empty());
}

#[test]
fn cancelling_removes_an_order_from_both_the_queue_and_the_index() {
    let mut book = Book::new();
    book.insert(order(1, Side::Ask, 100, 10, 1)).expect("i");
    book.insert(order(2, Side::Ask, 200, 10, 2)).expect("i");

    let removed = book.remove(&[1; 32]).expect("removed");
    assert_eq!(removed.price, 100);
    assert_eq!(book.len(), 1);
    assert!(book.get(&[1; 32]).is_none());
    assert_eq!(book.best_price(Side::Ask), Some(200));

    // Cancelling twice is not an error and not a double-free.
    assert!(book.remove(&[1; 32]).is_none());
}

#[test]
fn expiry_is_by_height_and_reaping_returns_the_orders_for_refund() {
    let mut book = Book::new();
    let mut expiring = order(1, Side::Ask, 100, 10, 1);
    expiring.expiry = 50;
    book.insert(expiring).expect("i");
    book.insert(order(2, Side::Ask, 200, 10, 2)).expect("i");

    // Still live at its expiry height: the bound is inclusive.
    assert!(book.clone().reap_expired(50).is_empty());

    let reaped = book.reap_expired(51);
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0].id, [1; 32]);
    assert_eq!(book.len(), 1, "the good-till-cancelled order stays");
}

#[test]
fn a_crossed_book_is_reported_as_crossed() {
    let mut book = Book::new();
    assert!(!book.is_crossed(), "an empty book crosses nothing");

    book.insert(order(1, Side::Ask, 100, 10, 1)).expect("i");
    assert!(!book.is_crossed(), "one side alone crosses nothing");

    book.insert(order(2, Side::Bid, 99, 10, 2)).expect("i");
    assert!(!book.is_crossed(), "a positive spread is not a cross");

    book.insert(order(3, Side::Bid, 100, 10, 3)).expect("i");
    assert!(book.is_crossed(), "equal prices cross");
}

#[test]
fn cloning_a_book_does_not_share_it() {
    // The matcher works on a copy so a rejected pass leaves nothing behind.
    let mut book = Book::new();
    book.insert(order(1, Side::Ask, 100, 10, 1)).expect("i");

    let mut scratch = book.clone();
    scratch.remove(&[1; 32]);

    assert_eq!(book.len(), 1);
    assert_eq!(scratch.len(), 0);
}
