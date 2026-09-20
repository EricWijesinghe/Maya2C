//! Channel updates, HTLCs, routing, and multi-hop payment construction.

use custom_l1_node::crypto::hybrid::{HybridPublicKey, HybridSigningKey, generate_signing_key};

use l2_flash::channel::{Channel, ChannelState, Party, SignedState};
use l2_flash::error::FlashError;
use l2_flash::htlc::{HOP_EXPIRY_DELTA, hash_lock};
use l2_flash::payment::PaymentPlan;
use l2_flash::routing::{ChannelGraph, FeePolicy, MAX_HOPS, NodeId};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// A key and its public encoding.
///
/// A channel needs the *public key*, not the address: under ML-DSA an address
/// is a hash and cannot verify the states its participants sign.
fn keypair() -> (HybridSigningKey, Box<HybridPublicKey>) {
    let key = generate_signing_key().expect("keygen");
    let public = Box::new(key.public_key());
    (key, public)
}

fn commit(
    channel: &mut Channel,
    next: ChannelState,
    key_a: &HybridSigningKey,
    key_b: &HybridSigningKey,
    revoked_secret: [u8; 32],
) -> l2_flash::Result<()> {
    let signed = SignedState {
        sig_a: Channel::sign(&next, key_a).expect("sign a"),
        sig_b: Channel::sign(&next, key_b).expect("sign b"),
        state: next,
    };
    channel.commit(signed, revoked_secret)
}

fn node(byte: u8) -> NodeId {
    [byte; 32]
}

// ---------------------------------------------------------------------------
// channel state
// ---------------------------------------------------------------------------

#[test]
fn a_transfer_moves_balance_and_advances_the_sequence() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let next = channel
        .propose_transfer(Party::A, 300, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [0u8; 32]).expect("commit");

    assert_eq!(channel.current.seq, 1);
    assert_eq!(channel.current.balance_a, 700);
    assert_eq!(channel.current.balance_b, 300);
}

#[test]
fn a_transfer_beyond_the_senders_side_is_refused() {
    let (_, pk_a) = keypair();
    let (_, pk_b) = keypair();
    let channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    // B holds nothing yet, so B cannot send.
    assert_eq!(
        channel.propose_transfer(Party::B, 1, [9u8; 32]),
        Err(FlashError::InsufficientBalance {
            required: 1,
            available: 0
        })
    );
}

#[test]
fn a_state_signed_by_only_one_party_is_not_binding() {
    let (key_a, pk_a) = keypair();
    let (_key_b, pk_b) = keypair();
    let impostor = generate_signing_key().expect("keygen");
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let next = channel
        .propose_transfer(Party::A, 300, [9u8; 32])
        .expect("propose");

    // A signs; the second signature is from someone who is not B.
    let signed = SignedState {
        sig_a: Channel::sign(&next, &key_a).expect("sign a"),
        sig_b: Channel::sign(&next, &impostor).expect("sign impostor"),
        state: next,
    };

    assert_eq!(
        channel.commit(signed, [0u8; 32]),
        Err(FlashError::InvalidSignature { party: "b" })
    );
}

#[test]
fn a_state_that_does_not_advance_the_sequence_is_refused() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let next = channel
        .propose_transfer(Party::A, 100, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next.clone(), &key_a, &key_b, [0u8; 32]).expect("commit");

    // Replaying the same state would overwrite an agreed balance with a stale one.
    assert_eq!(
        commit(&mut channel, next, &key_a, &key_b, [0u8; 32]),
        Err(FlashError::StaleSequence {
            current: 1,
            proposed: 1
        })
    );
}

#[test]
fn a_state_that_does_not_conserve_capacity_is_refused() {
    let (_, pk_a) = keypair();
    let (_, pk_b) = keypair();
    let channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let inflated = ChannelState {
        channel_id: [1u8; 32],
        seq: 1,
        balance_a: 900,
        balance_b: 900,
        htlcs: Vec::new(),
        revocation_commitment: [0u8; 32],
    };

    assert_eq!(
        channel.validate_successor(&inflated),
        Err(FlashError::CapacityMismatch {
            expected: 1_000,
            actual: 1_800
        })
    );
}

#[test]
fn advancing_a_state_records_the_revocation_secret() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let next = channel
        .propose_transfer(Party::A, 100, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [77u8; 32]).expect("commit");

    // The secret for the state just left behind is the evidence that punishes
    // its republication.
    assert_eq!(channel.revocation_secret(0), Some([77u8; 32]));
    assert_eq!(channel.revocation_secret(1), None);
}

// ---------------------------------------------------------------------------
// HTLCs
// ---------------------------------------------------------------------------

#[test]
fn an_htlc_reserves_value_belonging_to_neither_side() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let lock = hash_lock(&[5u8; 32]);
    let next = channel
        .propose_htlc(Party::A, 250, lock, 100, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [0u8; 32]).expect("commit");

    assert_eq!(channel.current.balance_a, 750);
    assert_eq!(channel.current.balance_b, 0);
    assert_eq!(channel.current.htlcs.len(), 1);
    // Still conserved — the value is in the HTLC, not gone.
    assert_eq!(channel.current.total().expect("total"), 1_000);
}

#[test]
fn a_state_with_pending_htlcs_cannot_be_settled() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let next = channel
        .propose_htlc(Party::A, 250, hash_lock(&[5u8; 32]), 100, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [0u8; 32]).expect("commit");

    assert!(!channel.current.is_settleable());
    // The on-chain format carries only balances, so settling now would silently
    // destroy the in-flight value.
    assert_eq!(
        channel.settlement_closure(&key_a, &key_b).err(),
        Some(FlashError::HtlcsPending { pending: 1 })
    );
}

#[test]
fn the_correct_preimage_pays_the_receiver() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let preimage = [5u8; 32];
    let next = channel
        .propose_htlc(Party::A, 250, hash_lock(&preimage), 100, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [0u8; 32]).expect("commit");

    let fulfilled = channel
        .fulfil_htlc(0, &preimage, 50, [8u8; 32])
        .expect("fulfil");
    commit(&mut channel, fulfilled, &key_a, &key_b, [1u8; 32]).expect("commit");

    assert_eq!(channel.current.balance_b, 250);
    assert!(channel.current.htlcs.is_empty());
    assert!(channel.current.is_settleable());
}

#[test]
fn a_wrong_preimage_claims_nothing() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let next = channel
        .propose_htlc(Party::A, 250, hash_lock(&[5u8; 32]), 100, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [0u8; 32]).expect("commit");

    assert_eq!(
        channel.fulfil_htlc(0, &[6u8; 32], 50, [8u8; 32]),
        Err(FlashError::PreimageMismatch { id: 0 })
    );
}

#[test]
fn an_expired_htlc_cannot_be_claimed_but_can_be_refunded() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let preimage = [5u8; 32];
    let next = channel
        .propose_htlc(Party::A, 250, hash_lock(&preimage), 100, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [0u8; 32]).expect("commit");

    // Expiry is inclusive: at exactly 100 the claim window has shut.
    assert_eq!(
        channel.fulfil_htlc(0, &preimage, 100, [8u8; 32]),
        Err(FlashError::HtlcExpired {
            id: 0,
            expiry: 100,
            height: 100
        })
    );

    let refunded = channel.refund_htlc(0, 100, [8u8; 32]).expect("refund");
    commit(&mut channel, refunded, &key_a, &key_b, [1u8; 32]).expect("commit");
    assert_eq!(channel.current.balance_a, 1_000);
}

#[test]
fn an_unexpired_htlc_cannot_be_refunded_early() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();
    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 1_000);

    let next = channel
        .propose_htlc(Party::A, 250, hash_lock(&[5u8; 32]), 100, [9u8; 32])
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [0u8; 32]).expect("commit");

    // Refunding early would cancel a payment the receiver can still claim.
    assert_eq!(
        channel.refund_htlc(0, 99, [8u8; 32]),
        Err(FlashError::HtlcNotExpired {
            id: 0,
            expiry: 100,
            height: 99
        })
    );
}

// ---------------------------------------------------------------------------
// routing
// ---------------------------------------------------------------------------

/// A → B → C, plus an expensive direct A → C.
fn line_graph() -> ChannelGraph {
    let mut graph = ChannelGraph::new();
    graph.add_channel(
        [1u8; 32],
        node(1),
        node(2),
        10_000,
        10_000,
        FeePolicy::new(1, 1_000),
    );
    graph.add_channel(
        [2u8; 32],
        node(2),
        node(3),
        10_000,
        10_000,
        FeePolicy::new(1, 1_000),
    );
    graph
}

#[test]
fn a_direct_channel_routes_in_one_hop() {
    let graph = line_graph();
    let route = graph
        .find_route(&node(1), &node(2), 500, MAX_HOPS)
        .expect("route");

    assert_eq!(route.hop_count(), 1);
    assert_eq!(route.delivered, 500);
    // The sender pays no forwarding fee to itself.
    assert_eq!(route.total_amount, 500);
    assert_eq!(route.total_fees(), 0);
}

#[test]
fn a_two_hop_route_charges_the_intermediary() {
    let graph = line_graph();
    let route = graph
        .find_route(&node(1), &node(3), 1_000, MAX_HOPS)
        .expect("route");

    assert_eq!(route.hop_count(), 2);
    assert_eq!(route.delivered, 1_000);
    // Node 2 keeps base 1 + 1000ppm of 1000 = 2.
    assert_eq!(route.total_amount, 1_002);
    assert_eq!(route.total_fees(), 2);
    assert_eq!(route.hops[0].from, node(1));
    assert_eq!(route.hops[1].to, node(3));
}

#[test]
fn forwarded_amounts_never_increase_along_a_route() {
    let graph = line_graph();
    let route = graph
        .find_route(&node(1), &node(3), 1_000, MAX_HOPS)
        .expect("route");

    // An intermediary must never be asked to forward more than it received.
    for pair in route.hops.windows(2) {
        assert!(pair[0].amount_in >= pair[1].amount_in);
    }
}

#[test]
fn a_route_never_revisits_a_node() {
    let mut graph = line_graph();
    // Add cycles: a channel network is not a DAG, so the search itself has to
    // guarantee acyclicity.
    graph.add_channel(
        [3u8; 32],
        node(3),
        node(1),
        10_000,
        10_000,
        FeePolicy::new(1, 1_000),
    );
    graph.add_channel(
        [4u8; 32],
        node(2),
        node(4),
        10_000,
        10_000,
        FeePolicy::new(1, 1_000),
    );
    graph.add_channel(
        [5u8; 32],
        node(4),
        node(3),
        10_000,
        10_000,
        FeePolicy::new(1, 1_000),
    );

    let route = graph
        .find_route(&node(1), &node(3), 500, MAX_HOPS)
        .expect("route");

    let mut visited = vec![route.hops[0].from];
    for hop in &route.hops {
        assert!(!visited.contains(&hop.to), "route revisited a node");
        visited.push(hop.to);
    }
}

#[test]
fn a_channel_without_the_capacity_is_not_a_route() {
    let mut graph = ChannelGraph::new();
    // Only 100 available in the A→B direction.
    graph.add_channel(
        [1u8; 32],
        node(1),
        node(2),
        100,
        10_000,
        FeePolicy::new(0, 0),
    );

    assert!(matches!(
        graph.find_route(&node(1), &node(2), 500, MAX_HOPS),
        Err(FlashError::NoRoute { .. })
    ));
    // The reverse direction has room, which is exactly why capacity is
    // directional rather than a single per-channel number.
    assert!(graph.find_route(&node(2), &node(1), 500, MAX_HOPS).is_ok());
}

#[test]
fn an_unreachable_destination_has_no_route() {
    let graph = line_graph();
    assert!(matches!(
        graph.find_route(&node(1), &node(9), 100, MAX_HOPS),
        Err(FlashError::NoRoute { .. })
    ));
}

#[test]
fn the_sender_pays_no_fee_on_its_own_channel() {
    let mut graph = ChannelGraph::new();
    // A high advertised fee on the sender's own outgoing channel.
    graph.add_channel(
        [1u8; 32],
        node(1),
        node(2),
        10_000,
        10_000,
        FeePolicy::new(5_000, 0),
    );

    let route = graph
        .find_route(&node(1), &node(2), 1_000, MAX_HOPS)
        .expect("route");

    // A node does not pay itself to forward. Fees are what a sender pays
    // *intermediaries*, so a one-hop payment is always fee-free.
    assert_eq!(route.total_fees(), 0);
    assert_eq!(route.total_amount, 1_000);
}

#[test]
fn the_cheaper_path_wins_even_when_it_is_longer() {
    let mut graph = ChannelGraph::new();
    // Two hops through one expensive intermediary: 1 -> 2 -> 4.
    graph.add_channel(
        [1u8; 32],
        node(1),
        node(2),
        10_000,
        10_000,
        FeePolicy::new(0, 0),
    );
    graph.add_channel(
        [2u8; 32],
        node(2),
        node(4),
        10_000,
        10_000,
        FeePolicy::new(5_000, 0),
    );
    // Three hops through two cheap ones: 1 -> 3 -> 5 -> 4.
    graph.add_channel(
        [3u8; 32],
        node(1),
        node(3),
        10_000,
        10_000,
        FeePolicy::new(0, 0),
    );
    graph.add_channel(
        [4u8; 32],
        node(3),
        node(5),
        10_000,
        10_000,
        FeePolicy::new(1, 0),
    );
    graph.add_channel(
        [5u8; 32],
        node(5),
        node(4),
        10_000,
        10_000,
        FeePolicy::new(1, 0),
    );

    let route = graph
        .find_route(&node(1), &node(4), 1_000, MAX_HOPS)
        .expect("route");

    // Fee, not hop count, is the objective.
    assert_eq!(route.hop_count(), 3);
    assert_eq!(route.total_fees(), 2);
}

#[test]
fn graph_capacity_updates_change_routing() {
    let mut graph = line_graph();
    assert!(
        graph
            .find_route(&node(1), &node(3), 5_000, MAX_HOPS)
            .is_ok()
    );

    // Drain the middle hop.
    assert!(graph.set_capacity(&[2u8; 32], &node(2), 10));

    assert!(matches!(
        graph.find_route(&node(1), &node(3), 5_000, MAX_HOPS),
        Err(FlashError::NoRoute { .. })
    ));
}

#[test]
fn the_graph_reports_its_shape() {
    let graph = line_graph();
    // Two channels, each contributing two directed edges.
    assert_eq!(graph.edge_count(), 4);
    assert_eq!(graph.node_count(), 3);
    assert_eq!(graph.edges_from(&node(1)).len(), 1);
    assert_eq!(graph.edges_from(&node(2)).len(), 2);
}

// ---------------------------------------------------------------------------
// multi-hop payments
// ---------------------------------------------------------------------------

#[test]
fn a_payment_plan_ladders_expiries_downward() {
    let graph = line_graph();
    let route = graph
        .find_route(&node(1), &node(3), 1_000, MAX_HOPS)
        .expect("route");

    let plan = PaymentPlan::build(&route, &[7u8; 32], 500, 40).expect("plan");

    assert_eq!(plan.hops.len(), 2);
    assert!(
        plan.expiries_are_laddered(),
        "an intermediary whose incoming HTLC expires first can be squeezed"
    );
    assert!(plan.amounts_are_monotonic());

    // Last hop gets the tightest window; each earlier hop gains one delta.
    assert_eq!(plan.hops[1].expiry_height, 540);
    assert_eq!(plan.hops[0].expiry_height, 540 + HOP_EXPIRY_DELTA);
    assert_eq!(plan.max_expiry, plan.hops[0].expiry_height);
}

#[test]
fn every_hop_shares_one_hash_lock() {
    let graph = line_graph();
    let route = graph
        .find_route(&node(1), &node(3), 1_000, MAX_HOPS)
        .expect("route");

    let preimage = [7u8; 32];
    let plan = PaymentPlan::build(&route, &preimage, 500, 40).expect("plan");

    // The shared lock is what makes the payment atomic: claiming the last hop
    // reveals the preimage every upstream hop needs.
    assert_eq!(plan.hash_lock, hash_lock(&preimage));
    for hop in &plan.hops {
        assert_eq!(hop.hash_lock, plan.hash_lock);
    }
}

#[test]
fn a_plan_carries_the_routes_fees() {
    let graph = line_graph();
    let route = graph
        .find_route(&node(1), &node(3), 1_000, MAX_HOPS)
        .expect("route");
    let plan = PaymentPlan::build(&route, &[7u8; 32], 500, 40).expect("plan");

    assert_eq!(plan.delivered, 1_000);
    assert_eq!(plan.total_amount, route.total_amount);
    // The first hop carries the full amount including every downstream fee.
    assert_eq!(plan.hops[0].amount, route.total_amount);
}

#[test]
fn an_htlc_plan_installs_into_real_channels() {
    let (key_a, pk_a) = keypair();
    let (key_b, pk_b) = keypair();

    let graph = line_graph();
    let route = graph
        .find_route(&node(1), &node(3), 1_000, MAX_HOPS)
        .expect("route");
    let plan = PaymentPlan::build(&route, &[7u8; 32], 500, 40).expect("plan");

    let mut channel = Channel::open([1u8; 32], pk_a.clone(), pk_b.clone(), 10_000);
    let first = &plan.hops[0];

    let next = channel
        .propose_htlc(
            Party::A,
            first.amount,
            first.hash_lock,
            first.expiry_height,
            [9u8; 32],
        )
        .expect("propose");
    commit(&mut channel, next, &key_a, &key_b, [0u8; 32]).expect("commit");

    assert_eq!(channel.current.htlcs.len(), 1);
    assert_eq!(channel.current.htlcs[0].amount, first.amount);
    assert_eq!(channel.current.total().expect("total"), 10_000);
}
