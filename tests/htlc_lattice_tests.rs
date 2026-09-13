//! Lattice HTLCs end to end: two in-process chains, the real watcher, and the
//! forgeries a claim has to survive.
//!
//! ## Three groups
//!
//! 1. **Swaps.** Two `StateDB`s stand in for two chains running the same
//!    verifier. The watchers in `maya-htlc-watcher` drive them through the
//!    same `SwapChain` trait their RPC client implements, so what settles here
//!    is the production decision path with only the transport replaced.
//! 2. **Timeouts.** The expiry height belongs to the refund, and a claim or
//!    refund that loses its race leaves the block valid — invariant 7.
//! 3. **Forgeries.** "Quantum pre-image attack" cannot be simulated, and no
//!    test here pretends to. What can be tested is every check a forger has to
//!    get past: the noise bound (which has no wire encoding to violate), the
//!    equation, the binding to one lock's commitment, the trivial commitment,
//!    and the classical preimage a SHA-256 HTLC would have accepted.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tempfile::TempDir;

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::htlc_payload::{HtlcClaim, HtlcLock, HtlcRefund, LockId};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::htlc::derive_lock_id;
use custom_l1_node::state::{Account, Address, BlockContext, Module, StateDB};

use maya_htlc_lattice::params::{K, L, N};
use maya_htlc_lattice::{LatticeSecret, LockRecord, Opening, Settlement};
use maya_htlc_watcher::{
    BlockRate, InitiatedSwap, Journal, LockView, Margins, Outcome, Phase, RespondRequest,
    SwapChain, WatcherError, Worker,
};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// Context with HTLC-L switched on. Everywhere the node builds one it is off.
fn active(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_htlc_activation(0)
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_789_200_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key).expect("sign");
    tx
}

fn keypair() -> (HybridSigningKey, Address) {
    let key = generate_signing_key().expect("keygen");
    let address = key.address();
    (key, address)
}

fn open_db(funded: &[(Address, u64)]) -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    for (address, balance) in funded {
        db.put_account(
            address,
            &Account {
                balance: *balance,
                nonce: 0,
            },
        )
        .expect("fund");
    }
    (db, dir)
}

/// A chain in a `StateDB`: a height, a pool, and a miner that drops what the
/// state transition refuses, as a block template would.
struct LocalChain {
    db: StateDB,
    height: Mutex<u64>,
    pool: Mutex<Vec<Transaction>>,
    _dir: TempDir,
}

impl LocalChain {
    fn new(funded: &[(Address, u64)]) -> Arc<Self> {
        let (db, dir) = open_db(funded);
        Arc::new(Self {
            db,
            height: Mutex::new(0),
            pool: Mutex::new(Vec::new()),
            _dir: dir,
        })
    }

    fn height(&self) -> u64 {
        *self.height.lock().expect("height")
    }

    fn submit(&self, tx: Transaction) {
        let mut pool = self.pool.lock().expect("pool");
        if !pool.iter().any(|pooled| pooled.txid() == tx.txid()) {
            pool.push(tx);
        }
    }

    /// Mines the pool into the next height. Each transaction is applied as its
    /// own block at that height, so one the chain refuses is dropped rather
    /// than taking the others with it.
    fn mine(&self) {
        let height = {
            let mut height = self.height.lock().expect("height");
            *height += 1;
            *height
        };
        let mut transactions = std::mem::take(&mut *self.pool.lock().expect("pool"));
        transactions.sort_by_key(|tx| (tx.sender(), tx.nonce));
        for tx in transactions {
            let _refused = self.db.apply_block(&block_of(vec![tx]), active(height));
        }
    }

    fn mine_to(&self, height: u64) {
        while self.height() < height {
            self.mine();
        }
    }

    fn balance(&self, address: &Address) -> u64 {
        self.db.get_account(address).expect("account").balance
    }

    fn nonce(&self, address: &Address) -> u64 {
        self.db.get_account(address).expect("account").nonce
    }

    fn lock(&self, id: &LockId) -> LockRecord {
        self.db
            .stored_htlc_lock(id)
            .expect("read")
            .expect("lock exists")
    }

    /// Signs and mines one transaction from `key` at its next nonce.
    fn send(&self, kind: TxKind, key: &HybridSigningKey) {
        self.submit(signed(kind, self.nonce(&key.address()), key));
        self.mine();
    }
}

#[async_trait]
impl SwapChain for LocalChain {
    async fn tip_height(&self) -> Result<u64, WatcherError> {
        Ok(self.height())
    }

    async fn lock(&self, id: &LockId) -> Result<Option<LockView>, WatcherError> {
        self.db
            .stored_htlc_lock(id)
            .map(|record| record.as_ref().map(LockView::from_record))
            .map_err(|e| WatcherError::Rpc(e.to_string()))
    }

    async fn next_nonce(&self, address: &[u8; 32]) -> Result<u64, WatcherError> {
        Ok(self.nonce(address))
    }

    async fn broadcast(&self, raw: &[u8]) -> Result<(), WatcherError> {
        let tx = Transaction::from_bytes(raw).map_err(|e| WatcherError::Refused(e.to_string()))?;
        tx.verify().map_err(|e| WatcherError::Refused(e.to_string()))?;
        self.submit(tx);
        Ok(())
    }
}

fn margins() -> Margins {
    Margins {
        maya_confirmations: 2,
        counterparty_confirmations: 2,
        submission_blocks: 1,
    }
}

fn worker(
    maya: &Arc<LocalChain>,
    counterparty: &Arc<LocalChain>,
    key: HybridSigningKey,
    journal: &TempDir,
    name: &str,
) -> Worker {
    let maya: Arc<dyn SwapChain> = maya.clone();
    let counterparty: Arc<dyn SwapChain> = counterparty.clone();
    let journal = Journal::open(&journal.path().join(format!("{name}.json"))).expect("journal");
    Worker::new(maya, counterparty, key, margins(), journal)
}

/// Steps every worker and mines both chains until every swap has finished.
async fn settle(workers: &mut [&mut Worker], chains: &[&LocalChain], rounds: usize) {
    for _ in 0..rounds {
        for worker in workers.iter_mut() {
            let report = worker.step().await.expect("step");
            assert!(report.failures.is_empty(), "{:?}", report.failures);
        }
        for chain in chains {
            chain.mine();
        }
        let done = workers.iter().all(|worker| {
            worker
                .journal()
                .swaps()
                .iter()
                .all(|swap| swap.phase != Phase::Active)
        });
        if done {
            return;
        }
    }
    panic!("the swaps did not finish in {rounds} rounds");
}

fn outcome_of(worker: &Worker) -> Phase {
    worker.journal().swaps()[0].phase
}

fn lock_kind(recipient: Address, amount: u64, expiry_height: u64, secret: &LatticeSecret) -> TxKind {
    TxKind::HtlcLock(Box::new(HtlcLock {
        recipient,
        amount,
        expiry_height,
        commitment: secret.commitment().expect("commit"),
    }))
}

fn claim_kind(lock_id: LockId, opening: Opening) -> TxKind {
    TxKind::HtlcClaim(Box::new(HtlcClaim { lock_id, opening }))
}

fn refund_kind(lock_id: LockId) -> TxKind {
    TxKind::HtlcRefund(HtlcRefund { lock_id })
}

// ---------------------------------------------------------------------------
// 1. swaps
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_swap_between_two_chains_settles_both_legs() {
    let (alice_key, alice) = keypair();
    let (bob_key, bob) = keypair();
    let chain_a = LocalChain::new(&[(alice, 10_000)]);
    let chain_b = LocalChain::new(&[(bob, 10_000)]);
    let journals = TempDir::new().expect("journals");
    // Both watchers see A as "maya": it holds the long lock Alice funds.
    let mut alice_watcher = worker(&chain_a, &chain_b, alice_key, &journals, "alice");
    let mut bob_watcher = worker(&chain_a, &chain_b, bob_key, &journals, "bob");

    let secret = LatticeSecret::from_entropy([1; 32]);
    let commitment = secret.commitment().expect("commit");
    let alice_lock = alice_watcher
        .initiate(secret, bob, 4_000, 80)
        .await
        .expect("initiate");
    chain_a.mine();

    let bob_lock = bob_watcher
        .respond(RespondRequest {
            inbound_lock_id: alice_lock,
            commitment: commitment.clone(),
            min_inbound_amount: 4_000,
            outbound_recipient: alice,
            outbound_amount: 3_000,
            outbound_expiry: 40,
            rate: BlockRate::EQUAL,
        })
        .await
        .expect("respond");
    chain_b.mine();

    alice_watcher
        .accept_response(InitiatedSwap {
            commitment_id: commitment.id(),
            inbound_lock_id: bob_lock,
            min_inbound_amount: 3_000,
            rate: BlockRate::EQUAL,
        })
        .await
        .expect("accept");

    settle(&mut [&mut alice_watcher, &mut bob_watcher], &[&chain_a, &chain_b], 20).await;

    assert_eq!(outcome_of(&alice_watcher), Phase::Finished(Outcome::Swapped));
    assert_eq!(outcome_of(&bob_watcher), Phase::Finished(Outcome::Swapped));
    assert_eq!((chain_a.balance(&alice), chain_a.balance(&bob)), (6_000, 4_000));
    assert_eq!((chain_b.balance(&bob), chain_b.balance(&alice)), (7_000, 3_000));
    // Both chains hold the same opening, published by Alice's claim on B and
    // reused by Bob's watcher on A.
    assert_eq!(
        chain_a.lock(&alice_lock).revealed_opening(),
        chain_b.lock(&bob_lock).revealed_opening()
    );
}

#[tokio::test]
async fn the_watcher_claims_from_the_revelation_alone() {
    // Alice runs no watcher. Bob's claims as soon as her opening is in B's
    // state, and does not wait for it to be buried.
    let (alice_key, alice) = keypair();
    let (bob_key, bob) = keypair();
    let chain_a = LocalChain::new(&[(alice, 10_000)]);
    let chain_b = LocalChain::new(&[(bob, 10_000)]);
    let journals = TempDir::new().expect("journals");
    let mut bob_watcher = worker(&chain_a, &chain_b, bob_key, &journals, "bob");

    let secret = LatticeSecret::from_entropy([2; 32]);
    chain_a.send(lock_kind(bob, 4_000, 80, &secret), &alice_key);
    let alice_lock = derive_lock_id(&alice, 0);

    let bob_lock = bob_watcher
        .respond(RespondRequest {
            inbound_lock_id: alice_lock,
            commitment: secret.commitment().expect("commit"),
            min_inbound_amount: 4_000,
            outbound_recipient: alice,
            outbound_amount: 3_000,
            outbound_expiry: 40,
            rate: BlockRate::EQUAL,
        })
        .await
        .expect("respond");
    chain_b.mine();

    chain_b.send(claim_kind(bob_lock, secret.opening()), &alice_key);
    // The reveal is the tip: one confirmation, below the two a swap needs to
    // count as finished.
    let Settlement::Claimed { height, .. } = chain_b.lock(&bob_lock).settlement else {
        panic!("alice's claim did not settle");
    };
    assert_eq!(height, chain_b.height());

    bob_watcher.step().await.expect("step");
    chain_a.mine();
    assert_eq!(chain_a.balance(&bob), 4_000);
}

#[tokio::test]
async fn a_watcher_refuses_a_lock_that_does_not_pay_it_or_pairs_too_tightly() {
    let (alice_key, alice) = keypair();
    let (bob_key, bob) = keypair();
    let (_, carol) = keypair();
    let chain_a = LocalChain::new(&[(alice, 10_000)]);
    let chain_b = LocalChain::new(&[(bob, 10_000)]);
    let journals = TempDir::new().expect("journals");
    let mut bob_watcher = worker(&chain_a, &chain_b, bob_key, &journals, "bob");

    let secret = LatticeSecret::from_entropy([3; 32]);
    chain_a.send(lock_kind(carol, 4_000, 80, &secret), &alice_key);
    chain_a.send(lock_kind(bob, 4_000, 40, &secret), &alice_key);
    let request = |inbound_lock_id| RespondRequest {
        inbound_lock_id,
        commitment: secret.commitment().expect("commit"),
        min_inbound_amount: 4_000,
        outbound_recipient: alice,
        outbound_amount: 3_000,
        outbound_expiry: 40,
        rate: BlockRate::EQUAL,
    };

    let pays_carol = bob_watcher.respond(request(derive_lock_id(&alice, 0))).await;
    assert!(matches!(pays_carol, Err(WatcherError::Refused(_))));
    let too_tight = bob_watcher.respond(request(derive_lock_id(&alice, 1))).await;
    assert!(matches!(too_tight, Err(WatcherError::Pairing(_))));
    // Nothing was funded and nothing journalled.
    assert_eq!(chain_b.nonce(&bob), 0);
    assert!(bob_watcher.journal().swaps().is_empty());
}

// ---------------------------------------------------------------------------
// 2. timeouts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unrevealed_swap_refunds_both_sides() {
    let (alice_key, alice) = keypair();
    let (bob_key, bob) = keypair();
    let (carol_key, _) = keypair();
    let chain_a = LocalChain::new(&[(alice, 10_000)]);
    let chain_b = LocalChain::new(&[(bob, 10_000)]);
    let journals = TempDir::new().expect("journals");
    let mut bob_watcher = worker(&chain_a, &chain_b, bob_key, &journals, "bob");

    let secret = LatticeSecret::from_entropy([4; 32]);
    chain_a.send(lock_kind(bob, 4_000, 30, &secret), &alice_key);
    let alice_lock = derive_lock_id(&alice, 0);
    bob_watcher
        .respond(RespondRequest {
            inbound_lock_id: alice_lock,
            commitment: secret.commitment().expect("commit"),
            min_inbound_amount: 4_000,
            outbound_recipient: alice,
            outbound_amount: 3_000,
            outbound_expiry: 15,
            rate: BlockRate::EQUAL,
        })
        .await
        .expect("respond");

    // Alice vanishes. Bob's watcher refunds B once it expires.
    for _ in 0..20 {
        bob_watcher.step().await.expect("step");
        chain_b.mine();
    }
    assert_eq!(chain_b.balance(&bob), 10_000);

    // A refund is permissionless and pays the sender, whoever submits it.
    chain_a.mine_to(30);
    chain_a.send(refund_kind(alice_lock), &carol_key);
    settle(&mut [&mut bob_watcher], &[&chain_a, &chain_b], 5).await;

    assert_eq!(chain_a.balance(&alice), 10_000);
    assert_eq!(outcome_of(&bob_watcher), Phase::Finished(Outcome::Unwound));
}

#[test]
fn the_expiry_height_belongs_to_the_refund_and_the_loser_is_a_no_op() {
    let (alice_key, alice) = keypair();
    let (_, bob) = keypair();
    let (db, _dir) = open_db(&[(alice, 10_000)]);
    let secret = LatticeSecret::from_entropy([5; 32]);
    let lock_id = derive_lock_id(&alice, 0);
    let apply = |kind, nonce, height| {
        db.apply_block(&block_of(vec![signed(kind, nonce, &alice_key)]), active(height))
    };

    apply(lock_kind(bob, 4_000, 10, &secret), 0, 1).expect("lock");
    assert_eq!(db.get_account(&alice).expect("account").balance, 6_000);

    // A refund one block early and a claim at the expiry both lose. Both
    // blocks are valid: an `Err` here would let the loser void the winner's.
    apply(refund_kind(lock_id), 1, 9).expect("early refund is a valid no-op");
    apply(claim_kind(lock_id, secret.opening()), 2, 10).expect("late claim is a valid no-op");
    let record = db.stored_htlc_lock(&lock_id).expect("read").expect("lock");
    assert_eq!(record.settlement, Settlement::Open);
    assert_eq!(db.get_account(&bob).expect("account").balance, 0);

    apply(refund_kind(lock_id), 3, 10).expect("refund at the expiry");
    assert_eq!(db.get_account(&alice).expect("account").balance, 10_000);
    // Settled means settled: a valid opening inside any window claims nothing.
    apply(claim_kind(lock_id, secret.opening()), 4, 5).expect("valid no-op");
    assert_eq!(db.get_account(&bob).expect("account").balance, 0);
}

#[test]
fn a_late_reveal_costs_the_initiator_both_legs() {
    // The case the initiator's watcher refuses to create
    // (`htlc-watcher/tests/policy_tests.rs`). A claim after the expiry does
    // nothing on its chain, but its body still carries the opening.
    let (alice_key, alice) = keypair();
    let (bob_key, bob) = keypair();
    let chain_a = LocalChain::new(&[(alice, 10_000)]);
    let chain_b = LocalChain::new(&[(bob, 10_000)]);
    let secret = LatticeSecret::from_entropy([6; 32]);

    chain_a.send(lock_kind(bob, 4_000, 60, &secret), &alice_key);
    chain_b.send(lock_kind(alice, 3_000, 20, &secret), &bob_key);
    let (alice_lock, bob_lock) = (derive_lock_id(&alice, 0), derive_lock_id(&bob, 0));

    chain_b.mine_to(20);
    let late = signed(claim_kind(bob_lock, secret.opening()), 0, &alice_key);
    chain_b.submit(late.clone());
    chain_b.mine();
    assert_eq!(chain_b.lock(&bob_lock).settlement, Settlement::Open);

    // Bob reads the opening out of the transaction, refunds B, and claims A.
    let TxKind::HtlcClaim(published) = Transaction::from_bytes(&late.to_bytes())
        .expect("decode")
        .kind
    else {
        panic!("not a claim");
    };
    chain_b.send(refund_kind(bob_lock), &bob_key);
    chain_a.send(claim_kind(alice_lock, published.opening.clone()), &bob_key);

    assert_eq!(chain_b.balance(&bob), 10_000);
    assert_eq!(chain_a.balance(&bob), 4_000);
    assert_eq!((chain_a.balance(&alice), chain_b.balance(&alice)), (6_000, 0));
}

// ---------------------------------------------------------------------------
// 3. forgeries
// ---------------------------------------------------------------------------

struct Locked {
    db: StateDB,
    _dir: TempDir,
    alice_key: HybridSigningKey,
    bob: Address,
    secret: LatticeSecret,
    lock_id: LockId,
}

fn locked() -> Locked {
    let (alice_key, alice) = keypair();
    let (_, bob) = keypair();
    let (db, dir) = open_db(&[(alice, 10_000)]);
    let secret = LatticeSecret::from_entropy([7; 32]);
    db.apply_block(
        &block_of(vec![signed(lock_kind(bob, 4_000, 100, &secret), 0, &alice_key)]),
        active(1),
    )
    .expect("lock");
    Locked {
        db,
        _dir: dir,
        alice_key,
        bob,
        secret,
        lock_id: derive_lock_id(&alice, 0),
    }
}

impl Locked {
    /// Submits a claim at height 2 and returns what the recipient holds after.
    fn claim_with(&self, opening: Opening, lock_id: LockId, nonce: u64) -> u64 {
        self.db
            .apply_block(
                &block_of(vec![signed(claim_kind(lock_id, opening), nonce, &self.alice_key)]),
                active(2),
            )
            .expect("a losing claim leaves the block valid");
        self.db.get_account(&self.bob).expect("account").balance
    }
}

#[test]
fn a_wrong_opening_claims_nothing_and_the_right_one_still_can() {
    let fixture = locked();
    let wrong = LatticeSecret::from_entropy([8; 32]).opening();
    assert_eq!(fixture.claim_with(wrong, fixture.lock_id, 1), 0);
    assert_eq!(fixture.claim_with(fixture.secret.opening(), fixture.lock_id, 2), 4_000);
}

#[test]
fn the_zero_opening_claims_nothing() {
    let fixture = locked();
    let zero = Opening::new(&vec![0; L * N], &vec![0; K * N]).expect("bounded");
    assert_eq!(fixture.claim_with(zero, fixture.lock_id, 1), 0);
}

#[test]
fn an_opening_is_bound_to_its_own_commitment() {
    // A second lock under a different secret. The first lock's opening is
    // valid arithmetic and still claims nothing but the first lock.
    let fixture = locked();
    let other = LatticeSecret::from_entropy([9; 32]);
    fixture
        .db
        .apply_block(
            &block_of(vec![signed(
                lock_kind(fixture.bob, 1_000, 100, &other),
                1,
                &fixture.alice_key,
            )]),
            active(1),
        )
        .expect("second lock");
    let other_lock = derive_lock_id(&fixture.alice_key.address(), 1);

    assert_eq!(fixture.claim_with(fixture.secret.opening(), other_lock, 2), 0);
    assert_eq!(fixture.claim_with(other.opening(), other_lock, 3), 1_000);
}

/// A kind's payload bytes, tag first.
fn payload(kind: &TxKind) -> Vec<u8> {
    let mut bytes = Vec::new();
    kind.encode_into(&mut bytes);
    bytes
}

#[test]
fn out_of_bound_noise_has_no_wire_encoding() {
    // Tag, lock id, then the opening: its first nibble set to 9 is a
    // coefficient of -5, one past the bound. There is no byte string for it
    // that decodes, so it never reaches the verifier.
    let fixture = locked();
    let mut bytes = payload(&claim_kind(fixture.lock_id, fixture.secret.opening()));
    bytes[1 + 32] = (bytes[1 + 32] & 0xf0) | 0x09;
    let error = TxKind::decode(&mut ByteReader::new(&bytes)).expect_err("refused");
    assert!(error.to_string().contains("not canonically encoded"), "{error}");
}

#[test]
fn a_trivially_openable_commitment_cannot_be_locked() {
    // t = 0: s = 0, e = 0 would open it, so anyone could claim.
    let secret = LatticeSecret::from_entropy([10; 32]);
    let mut bytes = payload(&lock_kind([2; 32], 1, 10, &secret));
    let t = 1 + 32 + 8 + 8 + 32;
    bytes[t..].fill(0);
    let error = TxKind::decode(&mut ByteReader::new(&bytes)).expect_err("refused");
    assert!(error.to_string().contains("trivially openable"), "{error}");
}

#[test]
fn a_sha256_preimage_is_not_a_claim() {
    // What a classical HTLC's claim carries: a lock id and 32 bytes.
    let mut bytes = payload(&refund_kind([1; 32]));
    bytes[0] = payload(&claim_kind([1; 32], LatticeSecret::from_entropy([0; 32]).opening()))[0];
    bytes.extend_from_slice(&[0xab; 32]);
    assert!(TxKind::decode(&mut ByteReader::new(&bytes)).is_err());
}

// ---------------------------------------------------------------------------
// 4. consensus wiring
// ---------------------------------------------------------------------------

#[test]
fn nothing_executes_before_activation() {
    let (alice_key, alice) = keypair();
    let (db, _dir) = open_db(&[(alice, 10_000)]);
    let secret = LatticeSecret::from_entropy([11; 32]);
    let block = block_of(vec![signed(lock_kind([2; 32], 1_000, 100, &secret), 0, &alice_key)]);
    // `at_height` is what the node builds: HTLC_L_ACTIVATION_HEIGHT, u64::MAX.
    assert!(db.apply_block(&block, BlockContext::at_height(5)).is_err());
    assert_eq!(db.get_account(&alice).expect("account").balance, 10_000);
}

#[test]
fn a_lock_escrows_its_amount_under_the_state_root() {
    let (alice_key, alice) = keypair();
    let (db, _dir) = open_db(&[(alice, 10_000)]);
    let before = db.state_root().expect("root");
    let secret = LatticeSecret::from_entropy([12; 32]);

    // The invariant guard's conservation check runs inside apply_block: the
    // coin leaving the account has to reappear as `h:lk:` escrow.
    db.apply_block(
        &block_of(vec![signed(lock_kind([2; 32], 2_500, 100, &secret), 0, &alice_key)]),
        active(1),
    )
    .expect("conserved");
    assert_eq!(db.get_account(&alice).expect("account").balance, 7_500);
    assert_ne!(db.state_root().expect("root"), before);

    for bad_expiry in [0, 1] {
        let block = block_of(vec![signed(
            lock_kind([2; 32], 1, bad_expiry, &secret),
            1,
            &alice_key,
        )]);
        assert!(db.apply_block(&block, active(1)).is_err(), "expiry {bad_expiry}");
    }
}

#[test]
fn only_a_new_lock_can_be_halted_by_the_breaker() {
    let secret = LatticeSecret::from_entropy([13; 32]);
    assert_eq!(
        Module::of(&lock_kind([0; 32], 1, 1, &secret)),
        Some(Module::Htlc)
    );
    // A halted claim beside a live refund would hand the swap to the refunder.
    assert_eq!(Module::of(&claim_kind([0; 32], secret.opening())), None);
    assert_eq!(Module::of(&refund_kind([0; 32])), None);
}

#[test]
fn every_htlc_kind_round_trips_through_a_transaction() {
    let (key, _) = keypair();
    let secret = LatticeSecret::from_entropy([14; 32]);
    for kind in [
        lock_kind([3; 32], 9, 99, &secret),
        claim_kind([4; 32], secret.opening()),
        refund_kind([5; 32]),
    ] {
        let tx = signed(kind, 0, &key);
        let decoded = Transaction::from_bytes(&tx.to_bytes()).expect("decode");
        assert_eq!(decoded.txid(), tx.txid());
        assert_eq!(decoded.kind, tx.kind);
        decoded.verify().expect("signature survives");
    }
}
