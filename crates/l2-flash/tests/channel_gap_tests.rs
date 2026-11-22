//! Channel signature, overflow, HTLC boundary, fee and route-length gaps
//! (test-gaps items 4 and 5), plus conservation properties.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::ChainTag;
use custom_l1_node::crypto::hybrid::{HybridPublicKey, HybridSigningKey, generate_signing_key};
use l2_flash::channel::{Channel, ChannelState, Party, SignedState};
use l2_flash::error::FlashError;
use l2_flash::htlc::{Direction, Htlc, hash_lock};
use l2_flash::routing::{ChannelGraph, Edge, FeePolicy};
use primitive_types::U256;
use proptest::prelude::*;

static PUBKEYS: std::sync::OnceLock<(Box<HybridPublicKey>, Box<HybridPublicKey>)> =
    std::sync::OnceLock::new();

const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);

fn keypair() -> (HybridSigningKey, Box<HybridPublicKey>) {
    let key = generate_signing_key().expect("keygen");
    let public = Box::new(key.public_key());
    (key, public)
}

fn open(capacity: u64) -> (Channel, HybridSigningKey, HybridSigningKey) {
    let (ka, pa) = keypair();
    let (kb, pb) = keypair();
    (
        Channel::open([1u8; 32], pa, pb, U256::from(capacity)),
        ka,
        kb,
    )
}

fn signed(next: ChannelState, ka: &HybridSigningKey, kb: &HybridSigningKey) -> SignedState {
    SignedState {
        sig_a: Channel::sign(&next, ka, &CHAIN).unwrap(),
        sig_b: Channel::sign(&next, kb, &CHAIN).unwrap(),
        state: next,
    }
}

fn node(b: u8) -> [u8; 32] {
    [b; 32]
}

// ---- signatures ----------------------------------------------------------

#[test]
fn a_tampered_lattice_or_hash_based_half_fails_for_that_party() {
    let (mut ch, ka, kb) = open(1_000);
    let next = ch
        .propose_transfer(Party::A, U256::from(10), [9; 32])
        .unwrap();

    let mut s = signed(next.clone(), &ka, &kb);
    s.sig_b.lattice[0] ^= 1;
    assert_eq!(
        ch.commit(s, [0; 32], &CHAIN),
        Err(FlashError::InvalidSignature { party: "b" })
    );

    let mut s = signed(next.clone(), &ka, &kb);
    s.sig_a.hash_based[0] ^= 1;
    assert_eq!(
        ch.commit(s, [0; 32], &CHAIN),
        Err(FlashError::InvalidSignature { party: "a" })
    );

    // Refusal left the channel where it was.
    assert_eq!(ch.current.seq, 0);
    assert!(ch.revocation_secret(0).is_none());
}

#[test]
fn a_state_edited_after_signing_is_refused() {
    let (mut ch, ka, kb) = open(1_000);
    let next = ch
        .propose_transfer(Party::A, U256::from(10), [9; 32])
        .unwrap();
    let mut s = signed(next, &ka, &kb);
    // Shift 5 units from B back to A: still conserves capacity, so only the
    // signatures can catch it.
    s.state.balance_a += U256::from(5);
    s.state.balance_b -= U256::from(5);
    assert!(matches!(
        ch.commit(s, [0; 32], &CHAIN),
        Err(FlashError::InvalidSignature { .. })
    ));
}

#[test]
fn a_signature_for_another_chain_is_refused() {
    let (mut ch, ka, kb) = open(1_000);
    let next = ch
        .propose_transfer(Party::A, U256::from(10), [9; 32])
        .unwrap();
    let other = ChainTag::from_genesis([0x77; 32]);
    let s = SignedState {
        sig_a: Channel::sign(&next, &ka, &other).unwrap(),
        sig_b: Channel::sign(&next, &kb, &other).unwrap(),
        state: next,
    };
    assert!(matches!(
        ch.commit(s, [0; 32], &CHAIN),
        Err(FlashError::InvalidSignature { .. })
    ));
}

#[test]
fn a_state_for_another_channel_is_refused() {
    let (mut ch, ka, kb) = open(1_000);
    let mut next = ch
        .propose_transfer(Party::A, U256::from(10), [9; 32])
        .unwrap();
    next.channel_id = [2u8; 32];
    // Both parties signed it, but it is not this channel's state.
    assert_eq!(
        ch.commit(signed(next, &ka, &kb), [0; 32], &CHAIN),
        Err(FlashError::ChannelIdMismatch)
    );
    assert_eq!(ch.current.seq, 0);
}

// ---- overflow ------------------------------------------------------------

#[test]
fn a_state_whose_total_passes_the_ceiling_overflows() {
    let (ch, _, _) = open(1_000);
    let mut next = ch.current.clone();
    next.seq = 1;
    next.balance_a = U256::MAX;
    next.balance_b = U256::one();
    assert_eq!(
        ch.validate_successor(&next),
        Err(FlashError::BalanceOverflow)
    );
    assert_eq!(next.total(), Err(FlashError::BalanceOverflow));
}

#[test]
fn an_htlc_amount_that_overflows_the_total_is_caught() {
    let (ch, _, _) = open(1_000);
    let mut next = ch.current.clone();
    next.seq = 1;
    next.balance_a = U256::MAX;
    next.balance_b = U256::zero();
    next.htlcs.push(Htlc {
        id: 0,
        amount: U256::one(),
        hash_lock: [0; 32],
        expiry_height: 10,
        direction: Direction::AtoB,
    });
    assert_eq!(
        ch.validate_successor(&next),
        Err(FlashError::BalanceOverflow)
    );
}

#[test]
fn fulfilling_into_a_full_balance_overflows_instead_of_wrapping() {
    let (mut ch, _, _) = open(1_000);
    let pre = [5u8; 32];
    ch.current.htlcs.push(Htlc {
        id: 0,
        amount: U256::from(10),
        hash_lock: hash_lock(&pre),
        expiry_height: 100,
        direction: Direction::AtoB,
    });
    ch.current.balance_b = U256::MAX;
    assert_eq!(
        ch.fulfil_htlc(0, &pre, 1, [8; 32]),
        Err(FlashError::BalanceOverflow)
    );
    assert_eq!(
        ch.refund_htlc(0, 100, [8; 32]).map(|s| s.balance_a),
        Ok(U256::from(1_010))
    );
}

#[test]
fn a_transfer_into_a_full_balance_overflows() {
    let (mut ch, _, _) = open(1_000);
    ch.current.balance_b = U256::MAX;
    assert_eq!(
        ch.propose_transfer(Party::A, U256::one(), [9; 32]),
        Err(FlashError::BalanceOverflow)
    );
}

// ---- htlc ----------------------------------------------------------------

fn with_htlc(expiry: u64) -> (Channel, [u8; 32]) {
    let (mut ch, ka, kb) = open(1_000);
    let pre = [5u8; 32];
    let next = ch
        .propose_htlc(Party::A, U256::from(250), hash_lock(&pre), expiry, [9; 32])
        .unwrap();
    ch.commit(signed(next, &ka, &kb), [0; 32], &CHAIN).unwrap();
    (ch, pre)
}

#[test]
fn claim_window_is_exactly_below_expiry_and_refund_window_exactly_at_or_after() {
    let (ch, pre) = with_htlc(100);
    // expiry-1: claimable, not refundable.
    assert!(ch.fulfil_htlc(0, &pre, 99, [8; 32]).is_ok());
    assert!(matches!(
        ch.refund_htlc(0, 99, [8; 32]),
        Err(FlashError::HtlcNotExpired { .. })
    ));
    // expiry and expiry+1: refundable, not claimable.
    for h in [100, 101] {
        assert!(matches!(
            ch.fulfil_htlc(0, &pre, h, [8; 32]),
            Err(FlashError::HtlcExpired { .. })
        ));
        assert!(ch.refund_htlc(0, h, [8; 32]).is_ok());
    }
}

#[test]
fn no_height_is_both_claimable_and_refundable() {
    let (ch, pre) = with_htlc(100);
    for h in [0, 1, 98, 99, 100, 101, 102, u64::MAX] {
        let claim = ch.fulfil_htlc(0, &pre, h, [8; 32]).is_ok();
        let refund = ch.refund_htlc(0, h, [8; 32]).is_ok();
        assert!(claim ^ refund, "height {h}: claim={claim} refund={refund}");
    }
}

#[test]
fn a_zero_expiry_htlc_is_refundable_from_the_start_and_never_claimable() {
    let (ch, pre) = with_htlc(0);
    assert!(ch.fulfil_htlc(0, &pre, 0, [8; 32]).is_err());
    assert!(ch.refund_htlc(0, 0, [8; 32]).is_ok());
}

#[test]
fn only_the_exact_preimage_unlocks_and_it_pays_the_receiver_once() {
    let (ch, pre) = with_htlc(100);
    let mut flipped = pre;
    flipped[31] ^= 1;
    assert_eq!(
        ch.fulfil_htlc(0, &flipped, 50, [8; 32]),
        Err(FlashError::PreimageMismatch { id: 0 })
    );
    assert_eq!(
        ch.fulfil_htlc(0, &[0u8; 32], 50, [8; 32]),
        Err(FlashError::PreimageMismatch { id: 0 })
    );
    let next = ch.fulfil_htlc(0, &pre, 50, [8; 32]).unwrap();
    assert_eq!(next.balance_b, U256::from(250));
    assert_eq!(next.balance_a, U256::from(750));
    assert!(next.htlcs.is_empty());
}

#[test]
fn unknown_htlc_ids_are_refused_on_both_paths() {
    let (ch, pre) = with_htlc(100);
    assert_eq!(
        ch.fulfil_htlc(7, &pre, 50, [8; 32]),
        Err(FlashError::UnknownHtlc { id: 7 })
    );
    assert_eq!(
        ch.refund_htlc(7, 500, [8; 32]),
        Err(FlashError::UnknownHtlc { id: 7 })
    );
}

#[test]
fn a_btoa_htlc_pays_a_on_fulfil_and_refunds_b_on_expiry() {
    let (mut ch, ka, kb) = open(1_000);
    let t = ch
        .propose_transfer(Party::A, U256::from(400), [1; 32])
        .unwrap();
    ch.commit(signed(t, &ka, &kb), [0; 32], &CHAIN).unwrap();
    let pre = [3u8; 32];
    let n = ch
        .propose_htlc(Party::B, U256::from(100), hash_lock(&pre), 50, [2; 32])
        .unwrap();
    ch.commit(signed(n, &ka, &kb), [1; 32], &CHAIN).unwrap();
    assert_eq!(
        ch.fulfil_htlc(0, &pre, 49, [3; 32]).unwrap().balance_a,
        U256::from(700)
    );
    assert_eq!(
        ch.refund_htlc(0, 50, [3; 32]).unwrap().balance_b,
        U256::from(400)
    );
}

// ---- fees and routes -----------------------------------------------------

fn edge(base: u64, ppm: u64) -> Edge {
    Edge {
        channel_id: [1; 32],
        from: node(1),
        to: node(2),
        capacity: U256::MAX,
        base_fee: U256::from(base),
        fee_ppm: ppm,
    }
}

#[test]
fn fee_is_base_plus_floor_of_the_proportional_part_and_overflow_is_an_error() {
    let e = edge(3, 1_000);
    assert_eq!(e.fee_for(U256::zero()), Ok(U256::from(3)));
    assert_eq!(e.fee_for(U256::from(999)), Ok(U256::from(3)));
    assert_eq!(e.fee_for(U256::from(1_000)), Ok(U256::from(4)));
    assert_eq!(e.fee_for(U256::from(1_999)), Ok(U256::from(4)));
    // Full-range amount at 100%: proportional part is the amount itself plus base overflows.
    assert_eq!(
        edge(1, 1_000_000).fee_for(U256::MAX),
        Err(FlashError::BalanceOverflow)
    );
    // 256-bit amount times ppm does not wrap in the intermediate product.
    assert_eq!(edge(0, 1_000_000).fee_for(U256::MAX), Ok(U256::MAX));
    assert_eq!(edge(0, 500_000).fee_for(U256::MAX), Ok(U256::MAX / 2));
}

#[test]
fn every_hop_of_a_route_is_paid_at_least_its_edge_fee_on_what_it_forwards() {
    let mut g = ChannelGraph::new();
    let policy = FeePolicy::new(U256::from(2), 2_500);
    for i in 1..5u8 {
        g.add_channel(
            [i; 32],
            node(i),
            node(i + 1),
            U256::from(10_000_000),
            U256::from(10_000_000),
            policy,
        );
    }
    let amount = U256::from(1_234_567);
    let route = g.find_route(&node(1), &node(5), amount, 8).unwrap();
    assert_eq!(route.hop_count(), 4);
    for (i, hop) in route.hops.iter().enumerate() {
        let forwarded = hop.amount_in - hop.fee;
        let e = g
            .edges_from(&hop.from)
            .iter()
            .find(|e| e.channel_id == hop.channel_id && e.to == hop.to)
            .unwrap();
        if i == 0 {
            assert_eq!(hop.fee, U256::zero());
        } else {
            assert!(
                hop.fee >= e.fee_for(forwarded).unwrap(),
                "hop {i} underpaid"
            );
        }
    }
    assert_eq!(route.total_amount - route.delivered, route.total_fees());
    assert_eq!(
        route.hops.last().unwrap().amount_in - route.hops.last().unwrap().fee,
        amount
    );
}

fn chain_graph(hops: u8) -> ChannelGraph {
    let mut g = ChannelGraph::new();
    for i in 1..=hops {
        g.add_channel(
            [i; 32],
            node(i),
            node(i + 1),
            U256::from(1_000_000),
            U256::from(1_000_000),
            FeePolicy::FREE,
        );
    }
    g
}

#[test]
fn a_route_at_the_hop_limit_is_allowed_and_one_over_is_refused() {
    let g = chain_graph(4);
    let amt = U256::from(100);
    assert_eq!(
        g.find_route(&node(1), &node(5), amt, 4)
            .unwrap()
            .hop_count(),
        4
    );
    let err = g.find_route(&node(1), &node(5), amt, 3).unwrap_err();
    assert!(
        matches!(
            err,
            FlashError::NoRoute { .. } | FlashError::RouteTooLong { .. }
        ),
        "{err:?}"
    );
    assert!(g.find_route(&node(1), &node(5), amt, 0).is_err());
}

#[test]
fn the_default_limit_refuses_a_nine_hop_chain() {
    let g = chain_graph(9);
    let amt = U256::from(100);
    assert!(
        g.find_route(&node(1), &node(10), amt, l2_flash::routing::MAX_HOPS)
            .is_err()
    );
    assert!(
        g.find_route(&node(1), &node(9), amt, l2_flash::routing::MAX_HOPS)
            .is_ok()
    );
}

// ---- conservation properties --------------------------------------------

#[derive(Clone, Debug)]
enum Op {
    Transfer(bool, u64),
    Htlc(bool, u64, u64),
    Fulfil(usize, bool),
    Refund(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (any::<bool>(), 0u64..2_000).prop_map(|(a, v)| Op::Transfer(a, v)),
        (any::<bool>(), 0u64..2_000, 0u64..200).prop_map(|(a, v, e)| Op::Htlc(a, v, e)),
        (0usize..4, any::<bool>()).prop_map(|(i, good)| Op::Fulfil(i, good)),
        (0usize..4).prop_map(Op::Refund),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Every accepted update accounts for exactly the funded capacity and
    /// advances the sequence by one; refused ones change nothing.
    #[test]
    fn channel_funds_are_conserved_across_random_accepted_updates(
        capacity in prop_oneof![1u64..5_000, Just(u64::MAX)],
        height in 0u64..250,
        ops in proptest::collection::vec(op(), 1..30),
    ) {
        // Key generation is slow; the properties do not depend on which keys.
        let (pa, pb) = PUBKEYS.get_or_init(|| (keypair().1, keypair().1)).clone();
        let mut ch = Channel::open([1u8; 32], pa, pb, U256::from(capacity));
        let pre = [5u8; 32];
        for o in ops {
            let party = |a: bool| if a { Party::A } else { Party::B };
            let before = ch.current.clone();
            let id_of = |ch: &Channel, i: usize| ch.current.htlcs.get(i % ch.current.htlcs.len().max(1)).map_or(99, |h| h.id);
            let res = match o {
                Op::Transfer(a, v) => ch.propose_transfer(party(a), U256::from(v), [9; 32]),
                Op::Htlc(a, v, e) => ch.propose_htlc(party(a), U256::from(v), hash_lock(&pre), e, [9; 32]),
                Op::Fulfil(i, good) => {
                    let p = if good { pre } else { [6u8; 32] };
                    ch.fulfil_htlc(id_of(&ch, i), &p, height, [9; 32])
                }
                Op::Refund(i) => ch.refund_htlc(id_of(&ch, i), height, [9; 32]),
            };
            if let Ok(next) = res {
                prop_assert_eq!(next.total().unwrap(), U256::from(capacity));
                prop_assert_eq!(next.seq, before.seq + 1);
                prop_assert!(ch.validate_successor(&next).is_ok());
                ch.current = next; // what commit() installs once signed
            } else {
                prop_assert_eq!(&ch.current, &before);
            }
            prop_assert_eq!(ch.current.total().unwrap(), U256::from(capacity));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    /// The fee is exactly base plus the floored proportional part: it never
    /// undercharges by a whole unit and never falls below the base.
    #[test]
    fn fee_is_base_plus_floored_proportion(
        base in 0u64..1_000, ppm in 0u64..=1_000_000, amount in any::<u64>()
    ) {
        let fee = edge(base, ppm).fee_for(U256::from(amount)).unwrap();
        let exact_floor = (u128::from(amount) * u128::from(ppm)) / 1_000_000 + u128::from(base);
        prop_assert_eq!(fee, U256::from(exact_floor));
    }
}
