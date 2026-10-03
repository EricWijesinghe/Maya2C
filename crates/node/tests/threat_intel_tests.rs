//! Threat intel end to end: a live gossip mesh under a coordinated flood, the
//! evidence it yields, the blocks that record it, and the firewall worker on
//! every honest host.
//!
//! ## What "a global DDoS" can mean here
//!
//! The attack simulated is a coordinated flood of *forged frames*: four hostile
//! identities pushing transactions whose signatures do not verify and blocks
//! whose bodies are not the ones their headers commit to. Those leave evidence a
//! third party can check. A flood of *valid* bytes leaves none, and
//! [`a_flood_of_valid_transactions_produces_no_evidence`] pins that the
//! registry does not pretend otherwise — volume is the XDP token bucket's
//! problem, not consensus's.
//!
//! ## "Within 2 DAG rounds"
//!
//! Nothing in consensus has rounds, so the bound is in blocks: evidence
//! recorded at height `h` is enforced by every honest host's worker before
//! height `h + 2`. Here it is enforced at `h` itself.
//!
//! The memory transport has no IP addresses, so each worker is handed a
//! documentation address per peer where the node would supply its own
//! connection addresses; everything upstream of that map — capture, evidence,
//! verification, the indicator, the decay — is the production path.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use libp2p::identity::Keypair;
use libp2p::{Multiaddr, PeerId};
use tempfile::TempDir;
use tokio::sync::broadcast::error::RecvError;

use custom_l1_node::core::transaction::TxOutput;
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::network::{GuardConfig, Mempool, Node, NodeEvent, NodeHandle};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_threat_firewall::{DryRunSink, FirewallError, Observation, ThreatSource, Worker};
use maya_threat_intel::score::HALF_LIFE_BLOCKS;
use maya_threat_intel::{
    AttackAttestation, Author, OffenceKind, SignedGossip, author_of_peer_id, peer_id_bytes,
};

mod common;

const NETWORK_SIZE: usize = 10;
const ATTACKERS: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);
/// The height whose block records the evidence.
const EVIDENCE_HEIGHT: u64 = 1;
/// The brief's bound, in blocks.
const ENFORCEMENT_BOUND_BLOCKS: u64 = 2;
/// Valid transactions in the control flood.
const VALID_FLOOD: u64 = 32;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// Quarantines that outlast every test, as in `byzantine_guard_tests.rs`.
fn guard() -> GuardConfig {
    GuardConfig {
        first_quarantine: Duration::from_secs(600),
        ..GuardConfig::DEFAULT
    }
}

/// Threat intel switched on. Everywhere the node builds a context it is off.
fn active(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_threat_intel_activation(0)
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

fn open_db(funded: &[(Address, u64)]) -> (Arc<StateDB>, TempDir) {
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
    (Arc::new(state), dir)
}

fn spawn(funded: &[(Address, u64)]) -> SimNode {
    let (state, dir) = open_db(funded);
    let mut node = Node::new_memory_with_guard(Arc::clone(&state), guard()).expect("build node");
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

/// Dials every pair once and waits for a complete mesh.
async fn connect_all(nodes: &[SimNode]) {
    for (index, node) in nodes.iter().enumerate() {
        for other in nodes.iter().skip(index + 1) {
            node.handle.dial(other.address.clone()).await.expect("dial");
        }
    }
    for node in nodes {
        let wanted = nodes.len() - 1;
        assert!(
            eventually(|| async {
                node.handle
                    .connected_peers()
                    .await
                    .unwrap_or_default()
                    .len()
                    >= wanted
            })
            .await,
            "mesh never formed"
        );
    }
}

/// Every [`NodeEvent::AttackEvidence`] a node emits from now on.
fn collect_evidence(handle: &NodeHandle) -> Arc<Mutex<Vec<AttackAttestation>>> {
    let store = Arc::new(Mutex::new(Vec::new()));
    let mut events = handle.subscribe();
    let sink = Arc::clone(&store);
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(NodeEvent::AttackEvidence(attestation)) => {
                    sink.lock().expect("evidence").push(*attestation);
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
    store
}

fn author_of(peer: &PeerId) -> Author {
    author_of_peer_id(&peer.to_bytes()).expect("node identities are ed25519")
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_789_300_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

fn transfer(key: &HybridSigningKey, recipient: Address, amount: u64, nonce: u64) -> Transaction {
    let outputs = vec![TxOutput { amount, recipient }];
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(key, &common::test_chain()).expect("sign");
    tx
}

/// Signed by `forger`, claiming `victim`'s key: decodes, does not verify.
fn forged(forger: &HybridSigningKey, victim: &HybridSigningKey, nonce: u64) -> Transaction {
    let mut tx = transfer(forger, [0x0F; 32], 1, nonce);
    tx.public_key = Box::new(victim.public_key());
    tx
}

/// A block whose body is not the one its header commits to.
fn substituted_body(nonce: u64, filler: Transaction) -> Block {
    let mut honest = block_of(vec![]);
    honest.header.nonce = nonce;
    Block {
        header: honest.header,
        transactions: vec![filler],
    }
}

/// A firewall worker's view of one honest node: that node's own state, its
/// tip, and the address book the node would supply.
struct LocalSource {
    state: Arc<StateDB>,
    height: AtomicU64,
    addresses: Vec<(Author, IpAddr)>,
}

#[async_trait]
impl ThreatSource for LocalSource {
    async fn observe(&self) -> Result<Observation, FirewallError> {
        let indicators = self
            .state
            .stored_threat_indicators()
            .map_err(|e| FirewallError::Rpc(e.to_string()))?;
        Ok(Observation {
            height: self.height.load(Ordering::SeqCst),
            indicators,
            addresses: self.addresses.clone(),
        })
    }
}

/// Evidence exactly as a node would have captured it, signed by `keypair`.
fn attest(
    keypair: &Keypair,
    kind: OffenceKind,
    frame: Vec<u8>,
    sequence: u64,
) -> AttackAttestation {
    let mut gossip = SignedGossip {
        author: author_of(&PeerId::from(keypair.public())),
        sequence_number: sequence,
        data: frame,
        signature: [0; 64],
    };
    let signature = keypair
        .sign(&gossip.signed_bytes(kind.topic()))
        .expect("sign");
    gossip.signature = signature
        .try_into()
        .expect("an ed25519 signature is 64 bytes");
    AttackAttestation::new(kind, gossip).expect("under the evidence cap")
}

fn attest_tx(attestation: &AttackAttestation, nonce: u64, key: &HybridSigningKey) -> Transaction {
    signed(
        TxKind::AttestAttack(Box::new(attestation.clone())),
        nonce,
        key,
    )
}

// ---------------------------------------------------------------------------
// the attack
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_coordinated_flood_of_forged_frames_is_quarantined_network_wide_within_two_blocks() {
    // Arrange
    let reporter = generate_signing_key().expect("keygen");
    let forger = generate_signing_key().expect("keygen");
    let victim = generate_signing_key().expect("keygen");
    let funded = [
        (reporter.address(), 1_000_000u64),
        (victim.address(), 1_000_000u64),
    ];
    let nodes: Vec<SimNode> = (0..NETWORK_SIZE).map(|_| spawn(&funded)).collect();
    connect_all(&nodes).await;
    let (attackers, honest) = nodes.split_at(ATTACKERS);
    let hostile: Vec<Author> = attackers
        .iter()
        .map(|node| author_of(&node.handle.peer_id()))
        .collect();
    let evidence: Vec<_> = honest
        .iter()
        .map(|node| collect_evidence(&node.handle))
        .collect();
    let addresses: Vec<(Author, IpAddr)> = nodes
        .iter()
        .zip(1u8..)
        .map(|(node, last)| {
            (
                author_of(&node.handle.peer_id()),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)),
            )
        })
        .collect();
    let hostile_ips: BTreeSet<IpAddr> = addresses[..ATTACKERS].iter().map(|(_, ip)| *ip).collect();
    let payloads: Vec<(Transaction, Block)> = (0..ATTACKERS as u64)
        .map(|index| {
            let filler = transfer(&forger, [0x0E; 32], 1, index);
            (
                forged(&forger, &victim, index),
                substituted_body(index, filler),
            )
        })
        .collect();

    // Act: the flood, until every honest node holds evidence against every
    // attacker. Attackers keep sending; the first publish can land before the
    // mesh has exchanged subscriptions.
    let convicted = eventually(|| async {
        for (attacker, (tx, block)) in attackers.iter().zip(&payloads) {
            let _ = attacker.handle.publish_transaction(tx).await;
            let _ = attacker.handle.publish_block(block).await;
        }
        evidence.iter().all(|store| {
            let seen = store.lock().expect("evidence");
            hostile
                .iter()
                .all(|author| seen.iter().any(|e| e.gossip.author == *author))
        })
    })
    .await;

    // Assert: evidence against every attacker, and against nobody else.
    assert!(
        convicted,
        "an honest node never produced evidence against every attacker"
    );
    for (position, store) in evidence.iter().enumerate() {
        let seen = store.lock().expect("evidence");
        assert!(
            seen.iter().all(|e| hostile.contains(&e.gossip.author)),
            "honest node {position} produced evidence against an honest peer"
        );
    }

    // Act: one reporter records one attestation per attacker at height 1 —
    // each twice, as two observers of the same offence would.
    let chosen: Vec<AttackAttestation> = hostile
        .iter()
        .map(|author| {
            evidence[0]
                .lock()
                .expect("evidence")
                .iter()
                .find(|e| e.gossip.author == *author)
                .cloned()
                .expect("seen")
        })
        .collect();
    let transactions: Vec<Transaction> = chosen
        .iter()
        .chain(&chosen)
        .zip(0u64..)
        .map(|(attestation, nonce)| attest_tx(attestation, nonce, &reporter))
        .collect();
    let recorded = block_of(transactions);
    let roots: BTreeSet<[u8; 32]> = honest
        .iter()
        .map(|node| {
            node.state
                .apply_block(&recorded, active(EVIDENCE_HEIGHT))
                .expect("every honest node accepts the evidence")
        })
        .collect();

    // Assert: one agreed state, one offence per attacker despite duplicates.
    assert_eq!(
        roots.len(),
        1,
        "honest nodes disagree about the recorded state"
    );
    for author in &hostile {
        let indicator = honest[0]
            .state
            .stored_threat_indicator(author)
            .expect("read")
            .expect("recorded");
        assert_eq!(indicator.offences, 1, "a duplicate attestation was counted");
        assert!(indicator.is_active(EVIDENCE_HEIGHT));
    }

    // Act: every honest host's worker ticks at the recording height.
    let mut workers: Vec<(Arc<LocalSource>, Worker)> = honest
        .iter()
        .map(|node| {
            let source = Arc::new(LocalSource {
                state: Arc::clone(&node.state),
                height: AtomicU64::new(EVIDENCE_HEIGHT),
                addresses: addresses.clone(),
            });
            let worker = Worker::new(
                Arc::clone(&source) as Arc<dyn ThreatSource>,
                Box::new(DryRunSink::default()),
            );
            (source, worker)
        })
        .collect();
    for (position, (_, worker)) in workers.iter_mut().enumerate() {
        let step = worker.step().await.expect("observe");
        // Assert: exactly the attackers, network-wide, inside the bound.
        assert!(step.height < EVIDENCE_HEIGHT + ENFORCEMENT_BOUND_BLOCKS);
        assert_eq!(
            worker.enforced(),
            &hostile_ips,
            "honest host {position} did not quarantine exactly the attackers"
        );
        assert!(step.failures.is_empty());
    }

    // Act: the next block changes nothing, on any host.
    let next = block_of(vec![]);
    for node in honest {
        node.state
            .apply_block(&next, active(EVIDENCE_HEIGHT + 1))
            .expect("empty block");
    }
    for (source, worker) in &mut workers {
        source.height.store(EVIDENCE_HEIGHT + 1, Ordering::SeqCst);
        let step = worker.step().await.expect("observe");
        assert!(step.blocked.is_empty() && step.unblocked.is_empty());
        assert_eq!(worker.enforced(), &hostile_ips);
    }

    // Act: a node that never saw the attack joins, holding the same chain, and
    // is told what the tip obliges it to refuse.
    let newcomer = spawn(&funded);
    newcomer
        .state
        .apply_block(&recorded, active(EVIDENCE_HEIGHT))
        .expect("the newcomer accepts the evidence");
    newcomer
        .state
        .apply_block(&next, active(EVIDENCE_HEIGHT + 1))
        .expect("empty block");
    let mitigations = newcomer
        .state
        .active_mitigations(EVIDENCE_HEIGHT + 1)
        .expect("mitigations");
    assert_eq!(mitigations.len(), ATTACKERS);
    newcomer
        .handle
        .enforce_mitigations(&mitigations)
        .await
        .expect("enforce");
    let hostile_peers: Vec<PeerId> = attackers.iter().map(|n| n.handle.peer_id()).collect();
    let honest_peer = honest[0].handle.peer_id();
    for node in attackers.iter().chain(&honest[..1]) {
        let _ = node.handle.dial(newcomer.address.clone()).await;
    }

    // Assert: the honest peer gets in, and no convicted attacker does — at the
    // libp2p layer, on a node whose own guard never scored them.
    assert!(
        eventually(|| async {
            newcomer
                .handle
                .connected_peers()
                .await
                .unwrap_or_default()
                .contains(&honest_peer)
        })
        .await,
        "an honest peer could not reach the newcomer"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    let admitted = newcomer.handle.connected_peers().await.expect("peers");
    assert!(
        hostile_peers.iter().all(|peer| !admitted.contains(peer)),
        "a convicted attacker connected to a node that never saw the attack"
    );

    // Act: once the indicators lift, the newcomer re-admits the attackers.
    newcomer
        .handle
        .enforce_mitigations(&[])
        .await
        .expect("acquit");
    let _ = attackers[0].handle.dial(newcomer.address.clone()).await;

    // Assert
    assert!(
        eventually(|| async {
            let _ = attackers[0].handle.dial(newcomer.address.clone()).await;
            newcomer
                .handle
                .connected_peers()
                .await
                .unwrap_or_default()
                .contains(&hostile_peers[0])
        })
        .await,
        "an acquitted peer was not re-admitted"
    );

    // Act: a half-life on, the quarantine lifts by itself — no transaction
    // releases anyone, the height does.
    for (source, worker) in &mut workers {
        source
            .height
            .store(EVIDENCE_HEIGHT + HALF_LIFE_BLOCKS, Ordering::SeqCst);
        let step = worker.step().await.expect("observe");
        let unblocked: BTreeSet<IpAddr> = step.unblocked.into_iter().collect();
        assert_eq!(unblocked, hostile_ips);
        assert!(worker.enforced().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_of_valid_transactions_produces_no_evidence() {
    // Arrange
    let sender = generate_signing_key().expect("keygen");
    let funded = [(sender.address(), 1_000_000u64)];
    let nodes: Vec<SimNode> = (0..2).map(|_| spawn(&funded)).collect();
    connect_all(&nodes).await;
    let evidence = collect_evidence(&nodes[1].handle);
    let flood: Vec<Transaction> = (0..VALID_FLOOD)
        .map(|nonce| transfer(&sender, [0x0A; 32], 1, nonce))
        .collect();

    // Act
    let delivered = eventually(|| async {
        for tx in &flood {
            let _ = nodes[0].handle.publish_transaction(tx).await;
        }
        flood
            .iter()
            .all(|tx| nodes[1].handle.mempool().contains(&tx.txid()))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Assert
    assert!(delivered, "the valid flood never arrived");
    assert!(
        evidence.lock().expect("evidence").is_empty(),
        "valid traffic was attested as an attack"
    );
}

// ---------------------------------------------------------------------------
// evidence that must not convict
// ---------------------------------------------------------------------------

#[test]
fn evidence_that_does_not_convict_its_author_is_refused_and_moves_no_root() {
    // Arrange
    let reporter = generate_signing_key().expect("keygen");
    let forger = generate_signing_key().expect("keygen");
    let victim = generate_signing_key().expect("keygen");
    let (db, _dir) = open_db(&[
        (reporter.address(), 1_000_000),
        (victim.address(), 1_000_000),
    ]);
    let mempool = Mempool::new(Arc::clone(&db));
    let author = Keypair::generate_ed25519();
    let bystander = Keypair::generate_ed25519();
    let genuine = attest(
        &author,
        OffenceKind::InvalidSignature,
        forged(&forger, &victim, 0).to_bytes(),
        1,
    );

    let mut corrupted = genuine.clone();
    corrupted.gossip.signature[0] ^= 1;
    let mut framed = genuine.clone();
    framed.gossip.author = author_of(&PeerId::from(bystander.public()));
    let mut moved = genuine.clone();
    moved.kind = OffenceKind::TxRootMismatch;
    let cases = [
        ("a corrupted signature", corrupted),
        ("a bystander's name on someone else's signature", framed),
        ("evidence moved to another topic", moved),
        (
            "a transaction whose signature verifies",
            attest(
                &author,
                OffenceKind::InvalidSignature,
                transfer(&victim, [0x0B; 32], 1, 0).to_bytes(),
                2,
            ),
        ),
        (
            "a block whose body matches its root",
            attest(
                &author,
                OffenceKind::TxRootMismatch,
                block_of(vec![]).to_bytes(),
                3,
            ),
        ),
        (
            "bytes that do not decode",
            attest(&author, OffenceKind::InvalidSignature, vec![0xFF; 16], 4),
        ),
    ];
    let root = db.state_root().expect("root");

    for (label, attestation) in cases {
        // Act
        let tx = attest_tx(&attestation, 0, &reporter);
        let admitted = mempool.validate(&tx);
        let applied = db.apply_block(&block_of(vec![tx]), active(1));

        // Assert
        assert!(
            matches!(admitted, Err(NodeError::ThreatIntel(_))),
            "the mempool admitted {label}: {admitted:?}"
        );
        assert!(
            matches!(applied, Err(NodeError::ThreatIntel(_))),
            "a block carrying {label} applied: {applied:?}"
        );
        assert_eq!(
            db.state_root().expect("root"),
            root,
            "{label} moved the root"
        );
    }

    // Genuine evidence is admitted, and still refused while the branch is dark.
    let genuine_tx = attest_tx(&genuine, 0, &reporter);
    mempool
        .validate(&genuine_tx)
        .expect("genuine evidence is admitted");
    let dark = db.apply_block(&block_of(vec![genuine_tx]), BlockContext::at_height(1));
    assert!(matches!(dark, Err(NodeError::ThreatIntel(_))), "{dark:?}");
    assert_eq!(db.state_root().expect("root"), root);
}

#[test]
fn repeated_evidence_is_a_no_op_and_a_reverted_block_takes_its_indicator_with_it() {
    // Arrange
    let reporter = generate_signing_key().expect("keygen");
    let forger = generate_signing_key().expect("keygen");
    let victim = generate_signing_key().expect("keygen");
    let (db, _dir) = open_db(&[(reporter.address(), 1_000_000)]);
    let author = Keypair::generate_ed25519();
    let offender = author_of(&PeerId::from(author.public()));
    let genuine = attest(
        &author,
        OffenceKind::InvalidSignature,
        forged(&forger, &victim, 0).to_bytes(),
        1,
    );
    let before = db.state_root().expect("root");

    // Act: the same evidence twice in one block, then once more a block later.
    let mut first = block_of(vec![
        attest_tx(&genuine, 0, &reporter),
        attest_tx(&genuine, 1, &reporter),
    ]);
    first.header.state_root = db.preview_root(&first, active(1)).expect("preview");
    db.apply_block_journaled(&first, &[0x01; 32], active(1))
        .expect("a duplicate is a no-op, not an error");
    let recorded = db
        .stored_threat_indicator(&offender)
        .expect("read")
        .expect("recorded");

    let mut second = block_of(vec![attest_tx(&genuine, 2, &reporter)]);
    second.header.state_root = db.preview_root(&second, active(2)).expect("preview");
    db.apply_block_journaled(&second, &[0x02; 32], active(2))
        .expect("a later copy is a no-op too");

    // Assert
    assert_eq!(recorded.offences, 1);
    assert_eq!(recorded.first_height, 1);
    assert_eq!(
        db.stored_threat_indicator(&offender).expect("read"),
        Some(recorded)
    );

    // Act: a reorg takes both blocks back.
    db.revert_block(&[0x02; 32]).expect("revert");
    db.revert_block(&[0x01; 32]).expect("revert");

    // Assert: no indicator survives the block that made it (invariant 8's rule).
    assert_eq!(db.stored_threat_indicator(&offender).expect("read"), None);
    assert_eq!(db.state_root().expect("root"), before);
}

#[test]
fn an_ed25519_peer_id_is_the_author_key_it_inlines() {
    let keypair = Keypair::generate_ed25519();
    let key = keypair
        .public()
        .try_into_ed25519()
        .expect("ed25519")
        .to_bytes();
    let peer = PeerId::from(keypair.public());
    assert_eq!(peer.to_bytes(), peer_id_bytes(&key).to_vec());
    assert_eq!(author_of_peer_id(&peer.to_bytes()), Some(key));
}
