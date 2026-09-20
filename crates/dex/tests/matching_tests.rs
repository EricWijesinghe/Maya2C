//! Crossing the book.

use maya_dex::book::{Book, Order, Side};
use maya_dex::fees::FeeSchedule;
use maya_dex::matching::{MatchLimits, match_book};
use maya_dex::types::PRICE_SCALE;

/// A price of `n` whole quote units per whole base unit.
fn price(n: u64) -> u64 {
    n * PRICE_SCALE as u64
}

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

fn book_of(orders: &[Order]) -> Book {
    let mut book = Book::new();
    for order in orders {
        book.insert(*order).expect("insert");
    }
    book
}

const FREE: FeeSchedule = FeeSchedule {
    lp_bps: 0,
    protocol_bps: 0,
};

#[test]
fn two_orders_that_cross_trade_and_leave_the_book_empty() {
    let book = book_of(&[
        order(1, Side::Ask, price(10), 100, 1),
        order(2, Side::Bid, price(10), 100, 2),
    ]);

    let outcome = match_book(&book, FREE, MatchLimits::at(1)).expect("match");

    assert_eq!(outcome.fills.len(), 1);
    assert_eq!(outcome.fills[0].base, 100);
    assert_eq!(outcome.fills[0].quote, 1_000);
    assert!(outcome.book.is_empty());
    assert!(!outcome.limit_reached);
}

#[test]
fn a_positive_spread_produces_no_trade() {
    let book = book_of(&[
        order(1, Side::Ask, price(11), 100, 1),
        order(2, Side::Bid, price(10), 100, 2),
    ]);

    let outcome = match_book(&book, FREE, MatchLimits::at(1)).expect("match");

    assert!(outcome.fills.is_empty());
    assert_eq!(outcome.book.len(), 2, "both orders still rest");
}

#[test]
fn the_order_that_arrived_first_sets_the_price() {
    // The ask rested at 10 and a buyer arrived willing to pay 12. The buyer
    // accepted a price that was already on the book, so they pay 10 — not 12,
    // and not some midpoint. Without this, resting an order would buy nothing
    // and nobody would do it.
    let book = book_of(&[
        order(1, Side::Ask, price(10), 100, 1),
        order(2, Side::Bid, price(12), 100, 2),
    ]);

    let outcome = match_book(&book, FREE, MatchLimits::at(1)).expect("match");

    assert_eq!(outcome.fills[0].price, price(10));
    assert_eq!(outcome.fills[0].maker_side, Side::Ask);
    assert_eq!(outcome.fills[0].quote, 1_000);
}

#[test]
fn the_rule_is_symmetric_when_the_bid_rested_first() {
    let book = book_of(&[
        order(1, Side::Bid, price(12), 100, 1),
        order(2, Side::Ask, price(10), 100, 2),
    ]);

    let outcome = match_book(&book, FREE, MatchLimits::at(1)).expect("match");

    assert_eq!(outcome.fills[0].price, price(12), "the seller gets 12");
    assert_eq!(outcome.fills[0].maker_side, Side::Bid);
}

#[test]
fn a_partial_fill_leaves_the_remainder_resting() {
    let book = book_of(&[
        order(1, Side::Ask, price(10), 100, 1),
        order(2, Side::Bid, price(10), 30, 2),
    ]);

    let outcome = match_book(&book, FREE, MatchLimits::at(1)).expect("match");

    assert_eq!(outcome.fills.len(), 1);
    assert_eq!(outcome.fills[0].base, 30);
    assert_eq!(outcome.book.len(), 1);
    assert_eq!(outcome.book.best(Side::Ask).expect("ask").remaining, 70);
}

#[test]
fn one_large_order_walks_the_book_in_price_order() {
    let book = book_of(&[
        order(1, Side::Ask, price(12), 10, 1),
        order(2, Side::Ask, price(10), 10, 2),
        order(3, Side::Ask, price(11), 10, 3),
        order(4, Side::Bid, price(12), 30, 4),
    ]);

    let outcome = match_book(&book, FREE, MatchLimits::at(1)).expect("match");

    let prices: Vec<u64> = outcome.fills.iter().map(|f| f.price).collect();
    assert_eq!(prices, vec![price(10), price(11), price(12)]);
    assert!(outcome.book.is_empty());
}

#[test]
fn the_book_is_never_left_crossed_by_a_completed_pass() {
    // The property the whole engine exists to maintain: two parties each
    // willing to trade at a price the other accepts must not both still be
    // sitting there.
    let mut orders = Vec::new();
    for i in 0..20u8 {
        orders.push(order(i, Side::Ask, price(10) + i as u64, 7, i as u64));
        orders.push(order(
            100 + i,
            Side::Bid,
            price(30) - i as u64,
            5,
            100 + i as u64,
        ));
    }

    let outcome = match_book(&book_of(&orders), FREE, MatchLimits::at(1)).expect("match");

    assert!(!outcome.book.is_crossed());
    assert!(!outcome.limit_reached);
}

#[test]
fn the_work_ceiling_stops_the_pass_and_says_so() {
    // A deep book crossed by one order is work every node must redo. The
    // ceiling stops the pass; it must not turn one large order into a rejected
    // block.
    let mut orders = vec![order(0, Side::Bid, price(100), 10_000, 0)];
    for i in 1..50u8 {
        orders.push(order(i, Side::Ask, price(10), 10, i as u64));
    }

    let outcome = match_book(
        &book_of(&orders),
        FREE,
        MatchLimits {
            max_fills: 5,
            height: 1,
        },
    )
    .expect("match");

    assert_eq!(outcome.fills.len(), 5);
    assert!(outcome.limit_reached, "the book is still crossed");
    assert!(outcome.book.is_crossed());
}

#[test]
fn a_pass_that_clears_the_last_cross_with_its_last_fill_is_not_limited() {
    let book = book_of(&[
        order(1, Side::Ask, price(10), 100, 1),
        order(2, Side::Bid, price(10), 100, 2),
    ]);

    let outcome = match_book(
        &book,
        FREE,
        MatchLimits {
            max_fills: 1,
            height: 1,
        },
    )
    .expect("match");

    assert_eq!(outcome.fills.len(), 1);
    assert!(
        !outcome.limit_reached,
        "there is no work left, so nothing was deferred"
    );
}

#[test]
fn the_taker_pays_the_protocol_fee_and_the_maker_does_not() {
    let fees = FeeSchedule {
        lp_bps: 30,
        protocol_bps: 100,
    };

    // Ask rested, bid took. The bid's output is base, so the fee is base.
    let outcome = match_book(
        &book_of(&[
            order(1, Side::Ask, price(10), 1_000, 1),
            order(2, Side::Bid, price(10), 1_000, 2),
        ]),
        fees,
        MatchLimits::at(1),
    )
    .expect("match");

    let fill = outcome.fills[0];
    assert_eq!(fill.taker_fee, 10, "100 bps of the taker's 1000 base");
    assert_eq!(fill.base_to_bidder(), 990);
    assert_eq!(fill.quote_to_asker(), fill.quote, "the maker keeps it all");
    assert_eq!(outcome.protocol_fees, 10);
}

#[test]
fn the_lp_rate_is_not_charged_on_a_book_trade() {
    // There are no liquidity providers in a trade between two traders. A pool
    // fee here would be a fee taken on behalf of a party that did not
    // participate.
    let lp_only = FeeSchedule {
        lp_bps: 300,
        protocol_bps: 0,
    };

    let outcome = match_book(
        &book_of(&[
            order(1, Side::Ask, price(10), 1_000, 1),
            order(2, Side::Bid, price(10), 1_000, 2),
        ]),
        lp_only,
        MatchLimits::at(1),
    )
    .expect("match");

    assert_eq!(outcome.protocol_fees, 0);
    assert_eq!(outcome.fills[0].base_to_bidder(), 1_000);
}

#[test]
fn expired_orders_are_reaped_before_matching_and_returned_for_refund() {
    let mut stale = order(1, Side::Ask, price(10), 100, 1);
    stale.expiry = 5;
    let book = book_of(&[stale, order(2, Side::Bid, price(10), 100, 2)]);

    let outcome = match_book(&book, FREE, MatchLimits::at(6)).expect("match");

    assert!(outcome.fills.is_empty(), "an expired order does not trade");
    assert_eq!(outcome.expired.len(), 1);
    assert_eq!(outcome.expired[0].id, [1; 32]);
    // The bid is untouched and still resting, so its escrow stays put.
    assert_eq!(outcome.book.len(), 1);
}

#[test]
fn a_cross_too_small_to_move_a_quote_unit_does_not_trade_base_for_nothing() {
    // At a price below one quote unit per base unit, a one-unit fill rounds the
    // quote to zero. Filling it would hand over base for free.
    let book = book_of(&[
        order(1, Side::Ask, 1, 1, 1),
        order(2, Side::Bid, u64::MAX, 1, 2),
    ]);

    let outcome = match_book(&book, FREE, MatchLimits::at(1)).expect("match");

    assert!(outcome.fills.is_empty());
    assert_eq!(outcome.book.len(), 2);
}

#[test]
fn matching_does_not_disturb_the_book_it_was_given() {
    let book = book_of(&[
        order(1, Side::Ask, price(10), 100, 1),
        order(2, Side::Bid, price(10), 100, 2),
    ]);

    let outcome = match_book(&book, FREE, MatchLimits::at(1)).expect("match");

    assert_eq!(book.len(), 2, "the caller's book is unchanged");
    assert!(outcome.book.is_empty());
}

#[test]
fn every_fill_conserves_value_between_the_two_parties_and_the_sink() {
    let fees = FeeSchedule {
        lp_bps: 0,
        protocol_bps: 250,
    };
    let mut orders = Vec::new();
    for i in 0..10u8 {
        orders.push(order(i, Side::Ask, price(10) + i as u64, 137, i as u64));
        orders.push(order(50 + i, Side::Bid, price(20), 91, 50 + i as u64));
    }

    let outcome = match_book(&book_of(&orders), fees, MatchLimits::at(1)).expect("match");
    assert!(!outcome.fills.is_empty());

    for fill in &outcome.fills {
        // What leaves the seller is `base`; what reaches the buyer plus what
        // reaches the sink is the same number. Same on the quote side.
        let base_accounted = fill.base_to_bidder()
            + if fill.maker_side == Side::Ask {
                fill.taker_fee
            } else {
                0
            };
        let quote_accounted = fill.quote_to_asker()
            + if fill.maker_side == Side::Bid {
                fill.taker_fee
            } else {
                0
            };
        assert_eq!(base_accounted, fill.base);
        assert_eq!(quote_accounted, fill.quote);
    }
}
