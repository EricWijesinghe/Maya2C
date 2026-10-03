//! Byzantine peers against the peer guard.
//!
//! A proof-of-work chain has no validator set, so "40% malicious" counts
//! identities, and identities are free: no number of them can make a node take
//! a chain with less work (`attack_simulation_tests.rs`,
//! `a_sybil_flood_cannot_forge_a_heavier_chain`). What they can do is waste an
//! honest node's bandwidth, push garbage at its mempool, and — the dangerous
//! one — crowd out its honest peers. So these tests verify the network layer:
//!
//! - a coordinated 40% of hostile nodes is quarantined by every honest node,
//!   cut off, and heard from no more, while the honest 60% stay connected and
//!   keep propagating;
//! - nothing hostile reaches an honest mempool, and no honest state moves;
//! - slow honest peers carrying valid traffic are never quarantined;
//! - a node can fetch a block it lacks from a peer that holds it, within the
//!   request bounds.
//!
//! Everything runs on the in-process memory transport.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use custom_l1_node::consensus::uint::U256;
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::sync::MAX_BLOCKS_PER_REQUEST;
use custom_l1_node::network::{GuardConfig, LatencyDial, Node, NodeEvent, NodeHandle};
use custom_l1_node::state::account::{Account, Address};
use custom_l1_node::state::db::StateDB;
use libp2p::{Multiaddr, PeerId};
use tempfile::TempDir;

mod common;

const NETWORK_SIZE: usize = 10;
const MALICIOUS: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);

/// Quarantines that outlast every test, so a release cannot reconnect a
/// hostile peer halfway through an isolation check.
fn guard() -> GuardConfig {
    GuardConfig {
        first_quarantine: Duration::from_secs(600),
        ..GuardConfig::DEFAULT
    }
}

struct SimNode {
    handle: NodeHandle,
    address: Multiaddr,
    state: Arc<StateDB>,
    _dir: TempDir,
}

fn next_memory_address() -> Multiaddr {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let port = NEXT.fetch_add(1, Ordering::Relaxed) + u64::from(std::process::id()) * 10_000;
    format!("/memory/{port}").parse().expect("valid multiaddr")
}

fn spawn(funded: &[(Address, u64)], latency: Option<Duration>) -> SimNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");
    common::bind(&state);
    for (address, balance) in funded {
        let account = Account {
            balance: *balance,
            nonce: 0,
        };
        state.put_account(address, &account).expect("fund");
    }
    let state = Arc::new(state);
    let mut node = match latency {
        Some(delay) => Node::new_memory_with_dial(Arc::clone(&state), LatencyDial::new(delay)),
        None => Node::new_memory_with_guard(Arc::clone(&state), guard()),
    }
    .expect("build node");
    let address = next_memory_address();
    node.listen_on(address.clone()).expect("listen");
    SimNode {
        handle: node.spawn(),
        address,
        state,
        _dir: dir,
    }
}

/// Polls an async condition until it holds or [`TIMEOUT`] passes.
async fn eventually<F, Fut>(mut condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if condition().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn peers_of(handle: &NodeHandle) -> Vec<PeerId> {
    handle.connected_peers().await.unwrap_or_default()
}

/// Dials every pair once and waits for a complete mesh.
async fn connect_all(nodes: &[SimNode]) {
    for (index, node) in nodes.iter().enumerate() {
        for other in nodes.iter().skip(index + 1) {
            node.handle.dial(other.address.clone()).await.expect("dial");
            node.handle
                .add_peer_address(other.handle.peer_id(), other.address.clone())
                .await
                .expect("seed kademlia");
        }
    }
    for node in nodes {
        let wanted = nodes.len() - 1;
        assert!(
            eventually(|| async { peers_of(&node.handle).await.len() >= wanted }).await,
            "mesh never formed"
        );
    }
}

fn transfer(key: &HybridSigningKey, recipient: Address, amount: u64, nonce: u64) -> Transaction {
    let outputs = vec![TxOutput { amount, recipient }];
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

/// A transaction signed by `forger` that claims to come from `victim`: it
/// decodes, and its signature does not verify.
fn forged(forger: &HybridSigningKey, victim: &HybridSigningKey, nonce: u64) -> Transaction {
    let mut tx = transfer(forger, [0x0F; 32], 1, nonce);
    tx.public_key = Box::new(victim.public_key());
    tx
}

fn empty_block(nonce: u64, timestamp: u64) -> Block {
    let header = BlockHeader {
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp,
        nonce,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    Block::new(header, vec![])
}

/// A block whose body is not the one its header commits to: invariant 24's
/// relay substitution.
fn substituted_body(nonce: u64, filler: Transaction) -> Block {
    let honest = empty_block(nonce, 1);
    Block {
        header: honest.header,
        transactions: vec![filler],
    }
}

async fn publish_until_accepted(node: &NodeHandle, tx: &Transaction) {
    node.mempool().insert(tx.clone()).expect("local insert");
    assert!(
        eventually(|| async { node.publish_transaction(tx).await.is_ok() }).await,
        "publish never accepted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_honest_node_quarantines_a_coordinated_forty_percent_and_stays_whole() {
    let funded_key = generate_signing_key().expect("keygen");
    let forger = generate_signing_key().expect("keygen");
    let funded = [(funded_key.address(), 1_000_000u64)];

    let nodes: Vec<SimNode> = (0..NETWORK_SIZE).map(|_| spawn(&funded, None)).collect();
    connect_all(&nodes).await;
    let (malicious, honest) = nodes.split_at(MALICIOUS);
    let hostile: Vec<PeerId> = malicious.iter().map(|n| n.handle.peer_id()).collect();
    let roots_before: Vec<[u8; 32]> = honest
        .iter()
        .map(|n| n.state.state_root().expect("root"))
        .collect();

    // The coordinated attack: every hostile node sends a forged transaction and
    // a block with a substituted body, and keeps sending until it is cut off.
    // An attacker does not stop after one message, and a single publish can
    // land before gossipsub has exchanged topic subscriptions and reach nobody.
    // Signed once, because signing is slow; every publish is a fresh gossip
    // message whatever its payload.
    let payloads: Vec<(Transaction, Block)> = (0..MALICIOUS as u64)
        .map(|index| {
            let filler = transfer(&forger, [0x0E; 32], 1, index);
            (
                forged(&forger, &funded_key, index),
                substituted_body(index, filler),
            )
        })
        .collect();

    // Every honest node quarantines every hostile peer.
    for (position, node) in honest.iter().enumerate() {
        let all_quarantined = eventually(|| async {
            for (attacker, (tx, block)) in malicious.iter().zip(&payloads) {
                let _ = attacker.handle.publish_transaction(tx).await;
                let _ = attacker.handle.publish_block(block).await;
            }
            for peer in &hostile {
                let report = node.handle.peer_report(*peer).await.ok().flatten();
                if !report.is_some_and(|r| r.quarantined) {
                    return false;
                }
            }
            true
        })
        .await;
        assert!(
            all_quarantined,
            "honest node {position} did not quarantine every hostile peer"
        );
    }

    // Cut off from the hostile 40%, still wired to the honest 60%.
    for (position, node) in honest.iter().enumerate() {
        let others: Vec<PeerId> = honest
            .iter()
            .map(|n| n.handle.peer_id())
            .filter(|peer| *peer != node.handle.peer_id())
            .collect();
        let isolated = eventually(|| async {
            let peers = peers_of(&node.handle).await;
            hostile.iter().all(|h| !peers.contains(h)) && others.iter().all(|o| peers.contains(o))
        })
        .await;
        assert!(
            isolated,
            "honest node {position} is not isolated from the hostile peers"
        );
    }

    // A *valid* transaction from a quarantined node reaches no honest node:
    // isolation is total, not a filter on bad content.
    let lure = transfer(&funded_key, [0x07; 32], 5, 0);
    malicious[0]
        .handle
        .mempool()
        .insert(lure.clone())
        .expect("seed");
    let _ = malicious[0].handle.publish_transaction(&lure).await;

    // Honest traffic still flows across the honest 60%.
    let honest_tx = transfer(&funded_key, [0x08; 32], 5, 0);
    publish_until_accepted(&honest[0].handle, &honest_tx).await;
    let hash = honest_tx.txid();
    assert!(
        eventually(|| async { honest.iter().all(|n| n.handle.mempool().contains(&hash)) }).await,
        "an honest transaction did not reach every honest node"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Zero corruption: every honest mempool holds exactly the honest
    // transaction, and no honest state moved.
    for (position, node) in honest.iter().enumerate() {
        assert!(
            !node.handle.mempool().contains(&lure.txid()),
            "node {position} heard the lure"
        );
        assert_eq!(
            node.handle.mempool().len(),
            1,
            "node {position} pooled hostile traffic"
        );
        assert_eq!(
            node.state.state_root().expect("root"),
            roots_before[position]
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_honest_peers_carrying_valid_traffic_are_never_quarantined() {
    let key = generate_signing_key().expect("keygen");
    let funded = [(key.address(), 1_000_000u64)];
    let mut nodes: Vec<SimNode> = (0..3).map(|_| spawn(&funded, None)).collect();
    nodes.push(spawn(&funded, Some(Duration::from_millis(300))));
    connect_all(&nodes).await;
    let mut events: Vec<_> = nodes.iter().map(|n| n.handle.subscribe()).collect();

    for round in 0..5u64 {
        for (index, node) in nodes.iter().enumerate() {
            // Valid blocks with ancient timestamps: the largest "delay" there is.
            let _ = node
                .handle
                .publish_block(&empty_block(round * 10 + index as u64, 1))
                .await;
            // A stale nonce: refused, and honest — it must be ignored, not scored.
            let _ = node
                .handle
                .publish_transaction(&transfer(&key, [1; 32], 1, 5 + round))
                .await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;

    for receiver in &mut events {
        while let Ok(event) = receiver.try_recv() {
            assert!(
                !matches!(event, NodeEvent::PeerQuarantined { .. }),
                "an honest peer was quarantined: {event:?}"
            );
        }
    }
    let slow = nodes[3].handle.peer_id();
    let mut saw_latency = false;
    for node in &nodes[..3] {
        let report = node.handle.peer_report(slow).await.expect("report");
        if let Some(report) = report {
            assert_eq!(report.score, 0.0);
            assert!(!report.quarantined);
            saw_latency |= report.mean_latency.is_some();
        }
    }
    assert!(
        saw_latency,
        "no node recorded the slow peer's block propagation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_fetches_a_block_it_lacks_from_a_peer_that_holds_it() {
    let nodes: Vec<SimNode> = (0..2).map(|_| spawn(&[], None)).collect();
    connect_all(&nodes).await;
    let (holder, asker) = (&nodes[0], &nodes[1]);
    let parent = empty_block(7, 1_756_252_800);
    holder
        .state
        .store_block(&parent, 1, U256::ZERO)
        .expect("store");
    let peer = holder.handle.peer_id();

    let fetched = asker
        .handle
        .request_blocks(peer, vec![parent.header.id()])
        .await
        .expect("fetch");
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].to_bytes(), parent.to_bytes());

    let unknown = asker.handle.request_blocks(peer, vec![[9; 32]]).await;
    assert_eq!(unknown.map(|blocks| blocks.len()).ok(), Some(0));

    let oversized = vec![[1; 32]; MAX_BLOCKS_PER_REQUEST + 1];
    assert!(asker.handle.request_blocks(peer, oversized).await.is_err());

    // Past the per-peer quota the holder answers with nothing.
    let mut refused = false;
    for _ in 0..40 {
        let answer = asker
            .handle
            .request_blocks(peer, vec![parent.header.id()])
            .await
            .expect("request");
        refused |= answer.is_empty();
    }
    assert!(refused, "the holder served past its rate limit");
}
