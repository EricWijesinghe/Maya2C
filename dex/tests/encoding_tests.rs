//! The small closed sets: directions, sides, and what a refusal says.
//!
//! Tag decoding is not incidental. `Direction::from_tag` and `Side::from_tag`
//! sit directly behind the wire format, so a tag the chain accepts that this
//! crate maps to the wrong variant is a swap executed the wrong way round — and
//! every node would make the same mistake, so nothing would flag it.

use maya_dex::book::{Book, Order, Side, quote_for_base};
use maya_dex::error::DexError;
use maya_dex::types::{Direction, NATIVE_ASSET, PRICE_SCALE};

#[test]
fn direction_tags_round_trip_and_reject_everything_else() {
    for direction in [Direction::BaseToQuote, Direction::QuoteToBase] {
        assert_eq!(Direction::from_tag(direction.tag()), Some(direction));
    }
    // The two live tags are 0 and 1. Everything above must be refused rather
    // than folded onto one of them.
    for tag in 2u8..=255 {
        assert_eq!(Direction::from_tag(tag), None, "tag {tag} was accepted");
    }
}

#[test]
fn side_tags_round_trip_and_reject_everything_else() {
    for side in [Side::Bid, Side::Ask] {
        assert_eq!(Side::from_tag(side.tag()), Some(side));
    }
    for tag in 2u8..=255 {
        assert_eq!(Side::from_tag(tag), None, "tag {tag} was accepted");
    }
}

#[test]
fn flipping_twice_returns_where_it_started() {
    assert_eq!(Direction::BaseToQuote.flip().flip(), Direction::BaseToQuote);
    assert_eq!(Direction::QuoteToBase.flip(), Direction::BaseToQuote);
    assert_eq!(Side::Bid.opposite().opposite(), Side::Bid);
    assert_eq!(Side::Ask.opposite(), Side::Bid);
}

#[test]
fn the_native_asset_is_the_one_value_nothing_derived_can_produce() {
    assert_eq!(NATIVE_ASSET, [0u8; 32]);
}

#[test]
fn every_refusal_says_something_specific() {
    // A refusal that renders as an empty string, or as the same string as
    // another refusal, is a log line that cannot be acted on.
    let all = [
        DexError::Overflow,
        DexError::ZeroAmount,
        DexError::EmptyPool,
        DexError::InsufficientLiquidity,
        DexError::FeeTooHigh,
        DexError::InsufficientInitialLiquidity,
        DexError::InsufficientShares,
        DexError::ZeroOutput,
        DexError::InvariantViolation,
        DexError::ZeroPrice,
        DexError::WorkLimitReached,
    ];

    let mut seen = std::collections::HashSet::new();
    for error in all {
        let text = error.to_string();
        assert!(!text.is_empty(), "{error:?} renders as nothing");
        assert!(seen.insert(text), "{error:?} shares a message with another");
    }
}

#[test]
fn an_order_is_live_until_its_expiry_passes_or_it_fills_out() {
    let mut order = Order {
        id: [1u8; 32],
        owner: [1u8; 32],
        side: Side::Ask,
        price: PRICE_SCALE as u64,
        amount: 100,
        remaining: 100,
        sequence: 1,
        expiry: 10,
    };

    assert!(order.is_live(1));
    assert!(order.is_live(10), "the expiry height itself is inclusive");
    assert!(!order.is_live(11));

    order.remaining = 0;
    assert!(
        !order.is_live(1),
        "a filled order is not live at any height"
    );

    order.remaining = 100;
    order.expiry = 0;
    assert!(order.is_live(u64::MAX), "zero is good-till-cancelled");
}

#[test]
fn an_orders_open_quote_follows_its_side() {
    // Three units at one-and-a-half quote each. A bid must escrow enough to
    // cover the whole thing; an ask must not be credited a unit that does not
    // exist.
    let price = PRICE_SCALE as u64 * 3 / 2;
    let bid = Order {
        id: [1u8; 32],
        owner: [1u8; 32],
        side: Side::Bid,
        price,
        amount: 3,
        remaining: 3,
        sequence: 1,
        expiry: 0,
    };
    let ask = Order {
        side: Side::Ask,
        ..bid
    };

    assert_eq!(bid.open_quote(), Ok(5));
    assert_eq!(ask.open_quote(), Ok(4));

    // An order whose quote leaves `u64` is refused rather than truncated. The
    // truncation would be an escrow smaller than the order it is supposed to
    // cover, which is the pool paying for somebody's overflow.
    assert_eq!(
        quote_for_base(u64::MAX, u64::MAX, Side::Bid),
        Err(DexError::Overflow)
    );
    assert_eq!(quote_for_base(1, 0, Side::Ask), Err(DexError::ZeroPrice));
}

#[test]
fn a_books_accessors_agree_with_each_other() {
    let mut book = Book::new();
    assert!(book.is_empty());
    assert!(book.best(Side::Bid).is_none());
    assert!(book.best_price(Side::Ask).is_none());
    assert!(book.get(&[1u8; 32]).is_none());

    let order = Order {
        id: [1u8; 32],
        owner: [2u8; 32],
        side: Side::Ask,
        price: PRICE_SCALE as u64,
        amount: 100,
        remaining: 100,
        sequence: 1,
        expiry: 0,
    };
    book.insert(order).expect("insert");

    assert_eq!(book.len(), 1);
    assert!(!book.is_empty());
    assert_eq!(book.get(&order.id), Some(&order));
    assert_eq!(book.best(Side::Ask), Some(&order));
    assert_eq!(book.best_price(Side::Ask), Some(order.price));
    assert_eq!(book.orders(), vec![order]);
    assert_eq!(book.side_orders(Side::Bid), Vec::new());
}
