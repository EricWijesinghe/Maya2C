//! The dark pool's aggregation under SPDZ-style MACs (Master Prompt 6 §4):
//! honest servers clear at the open auction's price, a server that alters
//! anything is caught, and the opening protocol refuses to run out of order.

#![allow(clippy::unwrap_used)]

use maya_permissioned_finance::darkpool::{self, Order, Side};
use maya_permissioned_finance::mpc_darkpool::TICKS;
use maya_permissioned_finance::mpc_spdz::{
    AuthShare, Dealer, P, SpdzError, SpdzServer, check, clear_opened, masked_input, open,
    order_vector,
};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};

const SERVERS: usize = 3;
const LEN: usize = 2 * TICKS;

fn book(n: usize, rng: &mut ChaCha20Rng) -> Vec<Order> {
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

/// Dealer, servers, and every order input through a mask.
fn aggregate(orders: &[Order], rng: &mut ChaCha20Rng) -> Vec<SpdzServer> {
    let (dealer, alphas) = Dealer::new(SERVERS, rng).unwrap();
    let mut servers: Vec<SpdzServer> = alphas
        .iter()
        .enumerate()
        .map(|(i, a)| SpdzServer::new(i, *a, LEN))
        .collect();
    for o in orders {
        let (mask, shares) = dealer.mask(LEN, rng);
        let epsilon = masked_input(
            &order_vector(o.side, usize::try_from(o.price).unwrap(), o.size),
            &mask,
        );
        for (server, share) in servers.iter_mut().zip(&shares) {
            server.receive(share, &epsilon).unwrap();
        }
    }
    servers
}

/// Open, commit every residue, then reveal and check.
fn open_and_check(
    servers: &mut [SpdzServer],
    rng: &mut ChaCha20Rng,
) -> Result<Vec<u64>, SpdzError> {
    let opened = open(
        &servers
            .iter()
            .map(SpdzServer::value_share)
            .collect::<Vec<_>>(),
        SERVERS,
    )?;
    let commitments: Vec<[u8; 32]> = servers
        .iter_mut()
        .map(|s| s.commit_residue(&opened, rng))
        .collect::<Result<_, _>>()?;
    let reveals: Vec<_> = servers
        .iter()
        .map(|s| s.reveal(&commitments, SERVERS))
        .collect::<Result<_, _>>()?;
    check(opened.len(), &commitments, &reveals, SERVERS)?;
    Ok(opened)
}

#[test]
fn honest_servers_clear_at_the_open_auctions_price() {
    let mut rng = ChaCha20Rng::seed_from_u64(21);
    for n in [1, 10, 200] {
        let orders = book(n, &mut rng);
        let started = std::time::Instant::now();
        let mut servers = aggregate(&orders, &mut rng);
        let opened = open_and_check(&mut servers, &mut rng).unwrap();
        let clearing = clear_opened(&opened).unwrap();
        let open_book = darkpool::clear(&orders.iter().copied().enumerate().collect::<Vec<_>>());
        assert_eq!(clearing.price, open_book.price, "{n} orders");
        println!(
            "spdz {n} orders, {SERVERS} servers: price {:?}, volume {}, {:?}",
            clearing.price,
            clearing.volume,
            started.elapsed()
        );
    }
}

#[test]
fn a_server_that_alters_its_share_is_caught() {
    let mut rng = ChaCha20Rng::seed_from_u64(22);
    let orders = book(20, &mut rng);

    // Shifting demand at one tick, to move the price.
    let mut servers = aggregate(&orders, &mut rng);
    servers[1].tamper(120, 1_000, 0);
    assert_eq!(
        open_and_check(&mut servers, &mut rng),
        Err(SpdzError::MacCheck)
    );

    // Shifting the MAC share too, by a guess at the key: still caught.
    let mut servers = aggregate(&orders, &mut rng);
    let guess = rng.next_u64() % P;
    servers[2].tamper(120, 1_000, guess);
    assert_eq!(
        open_and_check(&mut servers, &mut rng),
        Err(SpdzError::MacCheck)
    );
}

#[test]
fn a_server_cannot_change_its_answer_or_be_asked_twice() {
    let mut rng = ChaCha20Rng::seed_from_u64(23);
    let mut servers = aggregate(&book(5, &mut rng), &mut rng);
    let opened = open(
        &servers
            .iter()
            .map(SpdzServer::value_share)
            .collect::<Vec<_>>(),
        SERVERS,
    )
    .unwrap();

    // No reveal before its commitment, nor before every commitment is in.
    assert!(matches!(
        servers[0].reveal(&[[0; 32]; SERVERS], SERVERS),
        Err(SpdzError::Order(_))
    ));
    let commitments: Vec<[u8; 32]> = servers
        .iter_mut()
        .map(|s| s.commit_residue(&opened, &mut rng).unwrap())
        .collect();
    assert!(matches!(
        servers[0].reveal(&commitments[..2], SERVERS),
        Err(SpdzError::Servers { .. })
    ));

    // A second residue on the same sum would reveal the key share.
    let other: Vec<u64> = opened.iter().map(|v| (v + 1) % P).collect();
    assert!(matches!(
        servers[1].commit_residue(&other, &mut rng),
        Err(SpdzError::Order(_))
    ));

    // A reveal that differs from the commitment names the server.
    let mut reveals: Vec<_> = servers
        .iter()
        .map(|s| s.reveal(&commitments, SERVERS).unwrap())
        .collect();
    check(opened.len(), &commitments, &reveals, SERVERS).unwrap();
    reveals[0].0[3] ^= 1;
    assert_eq!(
        check(opened.len(), &commitments, &reveals, SERVERS),
        Err(SpdzError::BadOpening(0))
    );

    // A short residue is refused, not indexed out of bounds.
    reveals[0].0.truncate(4);
    assert!(matches!(
        check(opened.len(), &commitments, &reveals, SERVERS),
        Err(SpdzError::Shape(_))
    ));

    // A missing server is an abort, and nothing is added mid-opening.
    assert!(matches!(
        check(opened.len(), &[], &[], SERVERS),
        Err(SpdzError::Servers { .. })
    ));
    let (dealer, _) = Dealer::new(SERVERS, &mut rng).unwrap();
    let (mask, shares) = dealer.mask(LEN, &mut rng);
    let late = masked_input(&vec![0; LEN], &mask);
    assert!(matches!(
        servers[0].receive(&shares[0], &late),
        Err(SpdzError::Order(_))
    ));
}

#[test]
fn shapes_and_server_counts_are_checked() {
    let mut rng = ChaCha20Rng::seed_from_u64(24);
    assert!(matches!(
        Dealer::new(1, &mut rng),
        Err(SpdzError::Servers { .. })
    ));
    let mut server = SpdzServer::new(0, 1, 4);
    let wrong = AuthShare {
        value: vec![0; 3],
        mac: vec![0; 3],
    };
    assert!(matches!(
        server.receive(&wrong, &[0; 3]),
        Err(SpdzError::Shape(_))
    ));
    assert!(matches!(clear_opened(&[0; 5]), Err(SpdzError::Shape(_))));
    assert!(matches!(
        open(&[&[1, 2][..], &[3][..]], 2),
        Err(SpdzError::Shape(_))
    ));
}
