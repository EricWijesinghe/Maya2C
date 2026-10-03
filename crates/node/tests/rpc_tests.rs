//! End-to-end tests over a live JSON-RPC server.
//!
//! Each test binds a real HTTP server on port 0, lets the OS assign a port, and
//! drives it with a real `jsonrpsee` HTTP client. Nothing is stubbed — the
//! request path is serialization, transport, dispatch, RocksDB, and back.
//!
//! The mining test uses a deliberately easy target: every attempt is a 32 MiB
//! Argon2id pass, so a realistic difficulty would take minutes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use custom_l1_node::consensus::{Chain, ChainConfig, mine_header};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::Mempool;
use custom_l1_node::rpc::{
    AccountInfo, BlockInfo, MiningCandidate, RpcContext, RpcServer, SubmitBlockResult,
    SubmitTransactionResult, serve,
};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use tempfile::TempDir;

mod common;

/// Leading zero bits required by the test chain. Low enough that a single block
/// solves in roughly a second.
const TEST_DIFFICULTY_BITS: u32 = 6;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct TestNode {
    client: HttpClient,
    chain: Arc<Mutex<Chain>>,
    state: Arc<StateDB>,
    _server: RpcServer,
    _dir: TempDir,
}

fn genesis(bits: u32) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_700_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(bits),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

/// Starts a node and an RPC server in front of it.
///
/// `verify_pow` is off by default so most tests avoid Argon2 entirely; the
/// mining test opts back in, since verifying the work is the point there.
async fn start_node(funded: &[(Address, u64)], verify_pow: bool) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open state"));

    for (address, balance) in funded {
        state
            .put_account(
                address,
                &Account {
                    balance: *balance,
                    nonce: 0,
                },
            )
            .expect("fund");
    }

    let genesis_block = genesis(TEST_DIFFICULTY_BITS);
    let config = if verify_pow {
        // The genesis target doubles as this network's difficulty floor.
        ChainConfig::with_pow_limit(genesis_block.header.difficulty_target)
    } else {
        ChainConfig::without_pow_verification()
    };

    let chain = Arc::new(Mutex::new(
        common::open_chain(Arc::clone(&state), genesis_block, config).expect("open chain"),
    ));
    let mempool = Mempool::new(Arc::clone(&state));
    let context = RpcContext::new(Arc::clone(&chain), mempool);

    // Port 0: the OS assigns a free port, so concurrent tests never collide.
    let server = serve("127.0.0.1:0".parse().expect("addr"), context)
        .await
        .expect("start rpc server");

    let client = HttpClientBuilder::default()
        .build(format!("http://{}", server.address))
        .expect("build client");

    TestNode {
        client,
        chain,
        state,
        _server: server,
        _dir: dir,
    }
}

fn address_of(key: &HybridSigningKey) -> Address {
    key.address()
}

fn signed_transfer(from: &HybridSigningKey, to: Address, amount: u64, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: to,
        }],
        nonce,
    );
    tx.sign(from, &common::test_chain()).expect("sign");
    tx
}

// ---------------------------------------------------------------------------
// get_balance
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_balance_returns_funded_account_state() {
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let node = start_node(&[(alice_addr, 5_000)], false).await;

    let account: AccountInfo = node
        .client
        .request("get_balance", rpc_params![hex::encode(alice_addr)])
        .await
        .expect("get_balance");

    assert_eq!(account.address, hex::encode(alice_addr));
    assert_eq!(account.balance, 5_000);
    assert_eq!(account.nonce, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_balance_reports_zero_for_an_unknown_account() {
    let node = start_node(&[], false).await;

    let account: AccountInfo = node
        .client
        .request("get_balance", rpc_params![hex::encode([9u8; 32])])
        .await
        .expect("get_balance");

    assert_eq!(account.balance, 0);
    assert_eq!(account.nonce, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_balance_rejects_a_malformed_address() {
    let node = start_node(&[], false).await;

    // Not hex.
    let bad_hex: Result<AccountInfo, _> = node
        .client
        .request("get_balance", rpc_params!["zzzz"])
        .await;
    assert!(bad_hex.is_err());

    // Valid hex, wrong length.
    let wrong_length: Result<AccountInfo, _> = node
        .client
        .request("get_balance", rpc_params!["aabb"])
        .await;
    assert!(wrong_length.is_err());
}

// ---------------------------------------------------------------------------
// send_raw_transaction — key generation, signing, broadcast
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generated_key_can_sign_a_transaction_the_node_accepts() {
    // Full path: generate a key, derive its address, sign, encode, broadcast.
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [2u8; 32];

    let node = start_node(&[(alice_addr, 1_000)], false).await;

    let tx = signed_transfer(&alice, bob_addr, 250, 0);
    let expected_txid = hex::encode(tx.txid());

    // The wire encoding must survive a round trip before it is ever sent.
    let raw = hex::encode(tx.to_bytes());
    let decoded = Transaction::from_bytes(&hex::decode(&raw).expect("hex")).expect("decode");
    assert_eq!(decoded, tx, "wire encoding must round-trip exactly");
    assert!(
        decoded.verify(&common::test_chain()).is_ok(),
        "signature must survive encoding"
    );

    let result: SubmitTransactionResult = node
        .client
        .request("send_raw_transaction", rpc_params![raw.clone()])
        .await
        .expect("send_raw_transaction");

    assert_eq!(result.txid, expected_txid);
    assert!(result.accepted);

    // It is really in the node's pool, not just acknowledged.
    let pooled = node
        .client
        .request::<SubmitTransactionResult, _>("send_raw_transaction", rpc_params![raw])
        .await
        .expect("resubmit");
    assert!(
        !pooled.accepted,
        "a duplicate must be reported as already known, not re-accepted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_node_rejects_a_transaction_signed_by_an_unfunded_key() {
    let pauper = generate_signing_key().expect("keygen");
    let node = start_node(&[], false).await;

    let tx = signed_transfer(&pauper, [2u8; 32], 500, 0);
    let result: Result<SubmitTransactionResult, _> = node
        .client
        .request(
            "send_raw_transaction",
            rpc_params![hex::encode(tx.to_bytes())],
        )
        .await;

    assert!(result.is_err(), "unfunded transfer must be refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_node_rejects_a_tampered_transaction() {
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let node = start_node(&[(alice_addr, 1_000)], false).await;

    let mut tx = signed_transfer(&alice, [2u8; 32], 100, 0);
    // Raise the amount after signing; the signature no longer covers it.
    tx.outputs[0].amount = 900;

    let result: Result<SubmitTransactionResult, _> = node
        .client
        .request(
            "send_raw_transaction",
            rpc_params![hex::encode(tx.to_bytes())],
        )
        .await;

    assert!(result.is_err(), "a broken signature must be refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_node_rejects_malformed_transaction_bytes() {
    let node = start_node(&[], false).await;

    for payload in ["", "00", "deadbeef"] {
        let result: Result<SubmitTransactionResult, _> = node
            .client
            .request("send_raw_transaction", rpc_params![payload])
            .await;
        assert!(result.is_err(), "{payload:?} should not decode");
    }
}

// ---------------------------------------------------------------------------
// get_block_by_height
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_block_by_height_returns_genesis() {
    let node = start_node(&[], false).await;

    let block: BlockInfo = node
        .client
        .request("get_block_by_height", rpc_params![0u64])
        .await
        .expect("get_block_by_height");

    assert_eq!(block.height, 0);
    assert!(block.transactions.is_empty());

    // The raw encoding must decode back to the same block.
    let decoded = Block::from_bytes(&hex::decode(&block.raw).expect("hex")).expect("decode");
    assert_eq!(hex::encode(decoded.header.id()), block.header.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_block_by_height_reports_a_missing_height() {
    let node = start_node(&[], false).await;

    let result: Result<BlockInfo, _> = node
        .client
        .request("get_block_by_height", rpc_params![42u64])
        .await;

    assert!(result.is_err(), "height beyond the tip must not resolve");
}

// ---------------------------------------------------------------------------
// get_mining_candidate + submit_block
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_mining_candidate_describes_the_next_block() {
    let node = start_node(&[], false).await;

    let candidate: MiningCandidate = node
        .client
        .request("get_mining_candidate", rpc_params![])
        .await
        .expect("get_mining_candidate");

    assert_eq!(candidate.height, 1);
    assert_eq!(candidate.header.nonce, 0, "candidates start unsolved");

    let genesis_id = {
        let chain = node.chain.lock().expect("lock");
        hex::encode(chain.genesis())
    };
    assert_eq!(candidate.header.prev_hash, genesis_id);

    // header_bytes must be exactly what the node itself would hash. If these
    // disagreed, a miner would produce work the node cannot verify.
    let expected_target = target_from_leading_zero_bits(TEST_DIFFICULTY_BITS);
    assert_eq!(candidate.difficulty_target, hex::encode(expected_target));
    assert_eq!(
        hex::decode(&candidate.header_bytes).expect("hex").len(),
        custom_l1_node::core::HEADER_LEN
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mined_candidate_is_accepted_over_rpc() {
    // The full miner round trip with proof-of-work verification enabled:
    // fetch a candidate, actually solve it, submit it, confirm the tip moved.
    let node = start_node(&[], true).await;

    let candidate: MiningCandidate = node
        .client
        .request("get_mining_candidate", rpc_params![])
        .await
        .expect("get_mining_candidate");

    // Reconstruct the header from the exact bytes the node handed out.
    let header_bytes = hex::decode(&candidate.header_bytes).expect("hex");
    let header = BlockHeader::from_bytes(&header_bytes).expect("decode header");

    let cancel = std::sync::atomic::AtomicBool::new(false);
    let solved = mine_header(&header, 4, &cancel, None)
        .expect("mining must not fail")
        .expect("a solution exists at this difficulty");

    let block = Block::new(solved.header, Vec::new());
    let result: SubmitBlockResult = node
        .client
        .request("submit_block", rpc_params![hex::encode(block.to_bytes())])
        .await
        .expect("submit_block");

    assert_eq!(result.outcome, "extended");
    assert_eq!(result.height, 1);
    assert_eq!(result.tip, hex::encode(block.header.id()));

    // The chain really advanced, and the block is now retrievable by height.
    let stored: BlockInfo = node
        .client
        .request("get_block_by_height", rpc_params![1u64])
        .await
        .expect("get_block_by_height");
    assert_eq!(stored.header.id, result.tip);

    // The next candidate builds on the block we just submitted.
    let next: MiningCandidate = node
        .client
        .request("get_mining_candidate", rpc_params![])
        .await
        .expect("next candidate");
    assert_eq!(next.height, 2);
    assert_eq!(next.header.prev_hash, result.tip);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_block_rejects_insufficient_proof_of_work() {
    let node = start_node(&[], true).await;

    let candidate: MiningCandidate = node
        .client
        .request("get_mining_candidate", rpc_params![])
        .await
        .expect("get_mining_candidate");

    // Submit a header that provably misses the target. At the test's 6-bit
    // difficulty an arbitrary nonce *hits* 1 time in 64, so nonce 0 alone
    // made this test fail about that often; step until the check says no.
    let header_bytes = hex::decode(&candidate.header_bytes).expect("hex");
    let mut header = BlockHeader::from_bytes(&header_bytes).expect("decode header");
    while header.meets_difficulty().expect("pow hash") {
        header.nonce += 1;
    }
    let block = Block::new(header, Vec::new());

    let result: Result<SubmitBlockResult, _> = node
        .client
        .request("submit_block", rpc_params![hex::encode(block.to_bytes())])
        .await;

    assert!(result.is_err(), "unsolved header must be refused");
}

// ---------------------------------------------------------------------------
// state consistency across the RPC boundary
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn balances_seen_over_rpc_track_committed_state() {
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [3u8; 32];
    let node = start_node(&[(alice_addr, 1_000)], false).await;

    // Commit a block directly, bypassing RPC, then read the result back over it.
    let mut block = Block::new(
        genesis(TEST_DIFFICULTY_BITS).header,
        vec![signed_transfer(&alice, bob_addr, 400, 0)],
    );
    block.header.state_root = node
        .state
        .preview_root(&block, BlockContext::GENESIS)
        .expect("preview");
    node.state
        .apply_block_journaled(&block, &[7u8; 32], BlockContext::GENESIS)
        .expect("apply");

    let alice_info: AccountInfo = node
        .client
        .request("get_balance", rpc_params![hex::encode(alice_addr)])
        .await
        .expect("alice");
    let bob_info: AccountInfo = node
        .client
        .request("get_balance", rpc_params![hex::encode(bob_addr)])
        .await
        .expect("bob");

    assert_eq!(alice_info.balance, 600);
    assert_eq!(alice_info.nonce, 1);
    assert_eq!(bob_info.balance, 400);
}

// ---------------------------------------------------------------------------
// historical balances (the Mesh Data API's reads)
// ---------------------------------------------------------------------------

/// Commits `transactions` as the next block through the chain, as the node does.
fn commit_next(node: &TestNode, transactions: Vec<Transaction>) {
    let mut chain = node.chain.lock().expect("chain");
    let timestamp = 1_700_000_000 + chain.height() + 1;
    let block = chain
        .candidate_block(timestamp, transactions)
        .expect("candidate");
    chain.insert_block(block).expect("insert");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn balances_at_past_heights_undo_each_later_blocks_changes() {
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [4u8; 32];
    let node = start_node(&[(alice_addr, 1_000)], false).await;
    commit_next(&node, vec![signed_transfer(&alice, bob_addr, 300, 0)]);
    commit_next(&node, vec![signed_transfer(&alice, bob_addr, 200, 1)]);

    let at = |address: Address, height: u64| {
        let client = node.client.clone();
        async move {
            let reply: serde_json::Value = client
                .request(
                    "get_balance_at_height",
                    rpc_params![hex::encode(address), height],
                )
                .await
                .expect("get_balance_at_height");
            reply["balance"].as_u64().expect("balance")
        }
    };
    assert_eq!(
        (
            at(alice_addr, 0).await,
            at(alice_addr, 1).await,
            at(alice_addr, 2).await
        ),
        (1_000, 700, 500)
    );
    assert_eq!((at(bob_addr, 0).await, at(bob_addr, 1).await), (0, 300));

    let changes: serde_json::Value = node
        .client
        .request("get_balance_changes", rpc_params![2u64])
        .await
        .expect("get_balance_changes");
    let moved: Vec<(String, u64, u64)> = changes["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .map(|c| {
            (
                c["address"].as_str().expect("address").to_string(),
                c["before"].as_u64().expect("before"),
                c["after"].as_u64().expect("after"),
            )
        })
        .collect();
    assert!(moved.contains(&(hex::encode(alice_addr), 700, 500)));
    assert!(moved.contains(&(hex::encode(bob_addr), 300, 500)));

    let above: Result<serde_json::Value, _> = node
        .client
        .request(
            "get_balance_at_height",
            rpc_params![hex::encode(alice_addr), 9u64],
        )
        .await;
    assert!(above.is_err(), "a height above the tip is refused");
}

// ---------------------------------------------------------------------------
// rate limiting
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_over_the_rate_limit_is_refused_with_429() {
    use custom_l1_node::rpc::limit::RateLimiter;

    let serve_limited = |rate: u32, burst: u32| async move {
        let dir = TempDir::new().expect("temp dir");
        let state = Arc::new(StateDB::open(dir.path()).expect("open state"));
        let chain = Arc::new(Mutex::new(
            common::open_chain(
                Arc::clone(&state),
                genesis(TEST_DIFFICULTY_BITS),
                ChainConfig::without_pow_verification(),
            )
            .expect("open chain"),
        ));
        let context = RpcContext::new(Arc::clone(&chain), Mempool::new(state));
        let server = custom_l1_node::rpc::serve_metered(
            "127.0.0.1:0".parse().expect("addr"),
            context,
            Arc::new(custom_l1_node::metrics::Metrics::new()),
            Arc::new(RateLimiter::new(rate, burst)),
        )
        .await
        .expect("serve");
        let client = HttpClientBuilder::default()
            .build(format!("http://{}", server.address))
            .expect("client");
        (client, server, dir)
    };
    let burst_of = |client: HttpClient| async move {
        let mut ok = 0;
        for _ in 0..10 {
            let reply: Result<AccountInfo, _> = client
                .request("get_balance", rpc_params![hex::encode([1u8; 32])])
                .await;
            ok += usize::from(reply.is_ok());
        }
        ok
    };

    // 2 per second with a burst of 3: ten immediate calls get about three
    // through; the rest are refused before they reach a handler.
    let (client, _server, _dir) = serve_limited(2, 3).await;
    let admitted = burst_of(client).await;
    assert!((3..=4).contains(&admitted), "admitted {admitted} of 10");

    // Zero disables the limiter (`rpc.rate_limit_per_second = 0`).
    let (client, _server2, _dir2) = serve_limited(0, 0).await;
    assert_eq!(burst_of(client).await, 10);
}
