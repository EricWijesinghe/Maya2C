//! The MPC dark pool (Master Prompt 6 §4) against the commit-reveal auction:
//! the same price and volume from shares alone, conservation with the
//! residual, and servers that each hold only noise about an order.

#![allow(clippy::unwrap_used)]

use maya_permissioned_finance::darkpool::{self, Order, Side};
use maya_permissioned_finance::mpc_darkpool::{
    MAX_ORDER_SIZE, MpcError, Server, TICKS, clear, my_fill, settle, share_fill, share_order,
};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};

const SERVERS: usize = 3;

fn orders(n: usize, rng: &mut ChaCha20Rng) -> Vec<Order> {
    (0..n)
        .map(|i| Order {
            side: if rng.next_u32().is_multiple_of(2) {
                Side::Buy
            } else {
                Side::Sell
            },
            price: 100 + u64::from(rng.next_u32() % 40),
            size: 1 + u64::from(rng.next_u32() % 500),
            trader: [u8::try_from(i % 251).unwrap(); 32],
        })
        .collect()
}

fn run(
    book: &[Order],
    rng: &mut ChaCha20Rng,
) -> (
    Vec<Server>,
    maya_permissioned_finance::mpc_darkpool::Clearing,
) {
    let mut servers = vec![Server::default(); SERVERS];
    for o in book {
        for (server, share) in servers
            .iter_mut()
            .zip(share_order(o.side, o.price, o.size, SERVERS, rng).unwrap())
        {
            server.receive(&share).unwrap();
        }
    }
    servers.iter_mut().for_each(Server::close);
    let clearing = clear(&servers).unwrap();
    (servers, clearing)
}

#[test]
fn shares_clear_at_the_same_price_as_the_open_auction_and_conserve() {
    let mut rng = ChaCha20Rng::seed_from_u64(6);
    for n in [2, 10, 500, 1_000] {
        let book = orders(n, &mut rng);
        let started = std::time::Instant::now();
        let (mut servers, clearing) = run(&book, &mut rng);
        let open = darkpool::clear(&book.iter().copied().enumerate().collect::<Vec<_>>());
        assert_eq!(clearing.price, open.price, "{n} orders");
        let open_volume: u64 = open
            .fills
            .iter()
            .filter(|f| f.side == Side::Buy)
            .map(|f| f.size)
            .sum();
        assert_eq!(clearing.volume, open_volume);

        // Round two: each trader's own fill, shared the same way.
        let mut crossing = 0u128;
        for o in &book {
            let fill = my_fill(&clearing, o.side, o.price, o.size);
            assert!(fill <= o.size);
            crossing += u128::from(fill > 0);
            let pair = if o.side == Side::Buy {
                [fill, 0]
            } else {
                [0, fill]
            };
            for (server, share) in servers
                .iter_mut()
                .zip(share_fill(pair, SERVERS, &mut rng).unwrap())
            {
                server.receive_fill(share).unwrap();
            }
        }
        let (bought, sold, residual) = settle(&servers);
        assert_eq!(
            i128::from(bought),
            i128::from(sold) + residual,
            "conserved with the residual"
        );
        assert!(
            residual.unsigned_abs() < crossing.max(1),
            "under one lot per crossing order"
        );
        assert!(bought.max(sold) <= clearing.volume);
        println!(
            "mpc darkpool {n} orders, {SERVERS} servers: price {:?}, volume {}, bought {bought}, sold {sold}, residual {residual}, {:?}",
            clearing.price,
            clearing.volume,
            started.elapsed()
        );
    }
}

#[test]
fn fewer_than_all_servers_hold_only_noise() {
    let mut rng = ChaCha20Rng::seed_from_u64(7);
    let shares = share_order(Side::Buy, 120, 77, SERVERS, &mut rng).unwrap();
    let curve: Vec<u64> = (0..TICKS).map(|p| if p <= 120 { 77 } else { 0 }).collect();
    // All shares together are the order's curve...
    let all: Vec<u64> = (0..TICKS)
        .map(|p| shares.iter().fold(0u64, |a, s| a.wrapping_add(s.demand[p])))
        .collect();
    assert_eq!(all, curve);
    // ...and any SERVERS - 1 of them are not: they miss one uniform share.
    for skip in 0..SERVERS {
        let partial: Vec<u64> = (0..TICKS)
            .map(|p| {
                shares
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != skip)
                    .fold(0u64, |a, (_, s)| a.wrapping_add(s.demand[p]))
            })
            .collect();
        assert_ne!(partial, curve);
        assert!(
            partial.iter().all(|v| *v != 77 && *v != 0),
            "no tick shows the size or a gap"
        );
    }
    assert_eq!(
        share_order(Side::Sell, TICKS as u64, 1, SERVERS, &mut rng),
        Err(MpcError::OffGrid(TICKS as u64))
    );
    assert_eq!(
        share_order(Side::Sell, 1, 1, 1, &mut rng),
        Err(MpcError::TooFewServers(1))
    );
    assert_eq!(
        share_fill([5, 0], 1, &mut rng),
        Err(MpcError::TooFewServers(1)),
        "a lone share is the fill itself"
    );
    let mut server = Server::default();
    let mut short = shares[0].clone();
    short.demand.pop();
    assert_eq!(server.receive(&short), Err(MpcError::Malformed));
}

#[test]
fn aggregates_cannot_wrap_or_be_read_mid_batch() {
    let mut rng = ChaCha20Rng::seed_from_u64(8);
    assert_eq!(
        share_order(Side::Buy, 1, MAX_ORDER_SIZE + 1, SERVERS, &mut rng),
        Err(MpcError::TooLarge(MAX_ORDER_SIZE + 1))
    );
    // The largest orders, many of them, still sum exactly.
    let mut servers = vec![Server::default(); SERVERS];
    for side in [Side::Buy, Side::Sell] {
        for _ in 0..4 {
            for (server, share) in servers
                .iter_mut()
                .zip(share_order(side, 7, MAX_ORDER_SIZE, SERVERS, &mut rng).unwrap())
            {
                server.receive(&share).unwrap();
            }
        }
    }
    assert_eq!(
        servers[0].curves().unwrap_err(),
        MpcError::WrongPhase("still open")
    );
    assert!(clear(&servers).is_err(), "no clearing while open");
    servers.iter_mut().for_each(Server::close);
    let clearing = clear(&servers).unwrap();
    assert_eq!(
        (clearing.price, clearing.volume),
        (Some(7), 4 * MAX_ORDER_SIZE)
    );
    let late = share_order(Side::Buy, 7, 1, SERVERS, &mut rng).unwrap();
    assert_eq!(
        servers[0].receive(&late[0]),
        Err(MpcError::WrongPhase("closed"))
    );
    // Servers that saw different batches do not clear.
    let mut short = servers.clone();
    short[1] = Server::default();
    short[1].close();
    assert_eq!(clear(&short), Err(MpcError::Inconsistent));
}
