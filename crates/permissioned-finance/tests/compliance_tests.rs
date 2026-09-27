//! 500 generated transactions checked against independent references: the
//! dark pool's clearing against a brute-force auction, and the tax
//! calculators against conservation identities and hand-worked cases.

#![allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::many_single_char_names
)]

use maya_permissioned_finance::darkpool::{self, Batch, Order, Side};
use maya_permissioned_finance::tax::{self, Gains, Jurisdiction, TaxError, Trade};

const TRANSACTIONS: usize = 500;

/// `SplitMix64`: deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn order(rng: &mut Rng, i: usize) -> Order {
    let mut trader = [0u8; 32];
    trader[..8].copy_from_slice(&(i as u64).to_le_bytes());
    Order {
        side: if rng.below(2) == 0 {
            Side::Buy
        } else {
            Side::Sell
        },
        price: 90 + rng.below(21),
        size: 1 + rng.below(50),
        trader,
    }
}

#[test]
fn five_hundred_orders_clear_at_the_brute_force_price_and_conserve() {
    let mut rng = Rng(6);
    let mut batch = Batch::new();
    let mut opened = Vec::new();
    for i in 0..TRANSACTIONS {
        let o = order(&mut rng, i);
        let salt = [(i % 251) as u8; 32];
        let slot = batch.place(darkpool::commit(&o, &salt));
        opened.push((slot, o, salt));
    }
    // The public book is commitments only: nothing about any order.
    assert_eq!(batch.book().len(), TRANSACTIONS);

    // A reveal that does not open its commitment is refused.
    let (slot, o, _) = opened[0];
    assert!(!batch.reveal(
        slot,
        Order {
            size: o.size + 1,
            ..o
        },
        &[0; 32]
    ));
    // Every tenth trader never reveals; their orders must not trade.
    let revealed: Vec<(usize, Order)> = opened
        .iter()
        .filter(|(slot, _, _)| slot % 10 != 9)
        .map(|(slot, o, salt)| {
            assert!(batch.reveal(*slot, *o, salt));
            (*slot, *o)
        })
        .collect();

    let clearing = batch.close();
    let price = clearing.price.unwrap();

    // Brute force: the lowest price maximising min(demand, supply).
    let volume = |p: u64| {
        let side = |s: Side, ok: fn(u64, u64) -> bool| -> u64 {
            revealed
                .iter()
                .filter(|(_, o)| o.side == s && ok(o.price, p))
                .map(|(_, o)| o.size)
                .sum()
        };
        side(Side::Buy, |a, b| a >= b).min(side(Side::Sell, |a, b| a <= b))
    };
    let best = (0..=200).map(volume).max().unwrap();
    let expected = (0..=200).find(|p| volume(*p) == best).unwrap();
    // Candidate prices are order prices; the brute force searches every
    // integer, so they must agree on volume, and on price up to the gap
    // below the lowest order price that achieves it.
    assert_eq!(volume(price), best);
    assert!(revealed.iter().any(|(_, o)| o.price == price));
    assert!(price >= expected);

    let bought: u64 = clearing
        .fills
        .iter()
        .filter(|f| f.side == Side::Buy)
        .map(|f| f.size)
        .sum();
    let sold: u64 = clearing
        .fills
        .iter()
        .filter(|f| f.side == Side::Sell)
        .map(|f| f.size)
        .sum();
    assert_eq!(bought, best);
    assert_eq!(
        sold, best,
        "quantity conserved; value conserved at one price"
    );

    let hidden: Vec<[u8; 32]> = opened
        .iter()
        .filter(|(s, _, _)| s % 10 == 9)
        .map(|(_, o, _)| o.trader)
        .collect();
    assert!(clearing.fills.iter().all(|f| !hidden.contains(&f.trader)));

    // The auditor opening: exactly the disclosed order checks out.
    let (_, o, salt) = opened[3];
    assert!(darkpool::audit(&batch_commitment(&o, &salt), &o, &salt));
    assert!(!darkpool::audit(
        &batch_commitment(&o, &salt),
        &Order {
            price: o.price + 1,
            ..o
        },
        &salt
    ));
}

fn batch_commitment(o: &Order, salt: &[u8; 32]) -> darkpool::Commitment {
    darkpool::commit(o, salt)
}

#[test]
fn a_book_that_does_not_cross_trades_nothing() {
    let t = [1; 32];
    let orders = [
        (
            0,
            Order {
                side: Side::Buy,
                price: 99,
                size: 5,
                trader: t,
            },
        ),
        (
            1,
            Order {
                side: Side::Sell,
                price: 100,
                size: 5,
                trader: t,
            },
        ),
    ];
    assert_eq!(darkpool::clear(&orders).price, None);
}

fn history(rng: &mut Rng) -> Vec<Trade> {
    let mut held = 0u64;
    let mut day = 0u32;
    (0..TRANSACTIONS)
        .map(|_| {
            day += rng.below(5) as u32;
            let buy = held == 0 || rng.below(3) != 0;
            let units = if buy {
                1 + rng.below(20)
            } else {
                1 + rng.below(held)
            };
            held = if buy { held + units } else { held - units };
            Trade {
                day,
                buy,
                units,
                price: 1_000 + rng.below(9_000) as i64,
            }
        })
        .collect()
}

#[test]
fn five_hundred_trades_every_jurisdiction_conserves_the_total_gain() {
    let mut trades = history(&mut Rng(7));
    // Sell everything at the end so the total realised gain is fixed.
    let held: i128 = trades
        .iter()
        .map(|t| {
            if t.buy {
                t.units as i128
            } else {
                -(t.units as i128)
            }
        })
        .sum();
    let last = trades.last().unwrap().day;
    if held > 0 {
        trades.push(Trade {
            day: last + 1,
            buy: false,
            units: held as u64,
            price: 5_000,
        });
    }
    let cash: i64 = trades
        .iter()
        .map(|t| {
            if t.buy {
                -t.price * t.units as i64
            } else {
                t.price * t.units as i64
            }
        })
        .sum();
    for rules in [Jurisdiction::Us, Jurisdiction::De] {
        let g = tax::gains(&trades, rules).unwrap();
        assert_eq!(
            g.taxable_short + g.taxable_long + g.exempt,
            cash,
            "{rules:?}"
        );
    }
    // The average pool rounds allowable cost down at each disposal, so it can
    // overstate the gain by at most one cent per sell.
    let uk = tax::gains(&trades, Jurisdiction::Uk).unwrap();
    let sells = trades.iter().filter(|t| !t.buy).count() as i64;
    assert!(
        (0..=sells).contains(&(uk.taxable_short - cash)),
        "uk {} vs {cash}",
        uk.taxable_short
    );
}

#[test]
fn hand_worked_cases() {
    let t = |day, buy, units, price| Trade {
        day,
        buy,
        units,
        price,
    };
    let h = [
        t(0, true, 10, 100),
        t(100, true, 10, 200),
        t(400, false, 15, 300),
    ];
    // FIFO: 10 @ 100 held 400 days (long), 5 @ 200 held 300 days (short).
    assert_eq!(
        tax::gains(&h, Jurisdiction::Us),
        Ok(Gains {
            taxable_short: 500,
            taxable_long: 2_000,
            exempt: 0
        })
    );
    assert_eq!(
        tax::gains(&h, Jurisdiction::De),
        Ok(Gains {
            taxable_short: 500,
            taxable_long: 0,
            exempt: 2_000
        })
    );
    // Pool: 20 units cost 3,000; 15 sold carry 2,250; proceeds 4,500.
    assert_eq!(
        tax::gains(&h, Jurisdiction::Uk),
        Ok(Gains {
            taxable_short: 2_250,
            taxable_long: 0,
            exempt: 0
        })
    );
    let oversold = [t(0, true, 1, 100), t(1, false, 2, 100)];
    for rules in [Jurisdiction::Us, Jurisdiction::Uk, Jurisdiction::De] {
        assert_eq!(
            tax::gains(&oversold, rules),
            Err(TaxError::Oversold { at: 1 })
        );
    }
}
