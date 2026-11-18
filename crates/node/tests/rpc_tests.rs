//! End-to-end tests over a live JSON-RPC server.
//!
//! Each test binds a real HTTP server on port 0, lets the OS assign a port, and
//! drives it with a real `jsonrpsee` HTTP client. Nothing is stubbed — the
//! request path is serialization, transport, dispatch, RocksDB, and back.
//!
//! The mining test uses a deliberately easy target: every attempt is a 32 MiB
//! Argon2id pass, so a realistic difficulty would take minutes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use primitive_types::U256;
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
            version: 0,
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
    start_node_on(funded, verify_pow, None).await
}

/// As [`start_node`], naming the chain so text addresses have a network.
async fn start_node_on(
    funded: &[(Address, u64)],
    verify_pow: bool,
    network: Option<&str>,
) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open state"));

    for (address, balance) in funded {
        state
            .put_account(
                address,
                &Account {
                    balance: U256::from(*balance),
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
    let mut context = RpcContext::new(Arc::clone(&chain), mempool);
    if let Some(name) = network {
        context = context.with_network(name);
    }

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
            amount: primitive_types::U256::from(amount),
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
    assert_eq!(account.balance, U256::from(5_000));
    assert_eq!(account.nonce, 0);
}

/// ADR-081: a text address names a network, and the node refuses one for
/// another. Hex keeps working, and the reply carries the text form.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_balance_reads_text_addresses_for_its_own_network_only() {
    use maya_address::{Network, encode};

    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let node = start_node_on(&[(alice_addr, 7)], false, Some("maya-testnet-1")).await;
    let text = encode(Network::Testnet, &alice_addr);

    let by_text: AccountInfo = node
        .client
        .request("get_balance", rpc_params![text.clone()])
        .await
        .expect("text address on its own network");
    assert_eq!(by_text.balance, U256::from(7));
    assert_eq!(by_text.address_text.as_deref(), Some(text.as_str()));

    let by_hex: AccountInfo = node
        .client
        .request("get_balance", rpc_params![hex::encode(alice_addr)])
        .await
        .expect("hex still accepted");
    assert_eq!(by_hex.balance, U256::from(7));
    assert_eq!(by_hex.address_text.as_deref(), Some(text.as_str()));

    let mainnet = encode(Network::Mainnet, &alice_addr);
    let wrong: Result<AccountInfo, _> = node
        .client
        .request("get_balance", rpc_params![mainnet])
        .await;
    let message = wrong.expect_err("mainnet address on testnet").to_string();
    assert!(message.contains("expects 'tmaya'"), "{message}");

    let mut typo = text.clone();
    typo.pop();
    typo.push(if text.ends_with('q') { 'p' } else { 'q' });
    let typoed: Result<AccountInfo, _> =
        node.client.request("get_balance", rpc_params![typo]).await;
    assert!(typoed.is_err(), "a mistyped address was accepted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_balance_reports_zero_for_an_unknown_account() {
    let node = start_node(&[], false).await;

    let account: AccountInfo = node
        .client
        .request("get_balance", rpc_params![hex::encode([9u8; 32])])
        .await
        .expect("get_balance");

    assert_eq!(account.balance, U256::from(0));
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

/// Pre-audit S-012: the per-IP limiter charges one token per HTTP request, so
/// a batch would carry any number of calls for one token. Batches are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_request_is_refused_so_one_token_buys_one_call() {
    let node = start_node(&[], false).await;
    let mut batch = jsonrpsee::core::params::BatchRequestBuilder::new();
    for _ in 0..3 {
        batch
            .insert("get_balance", rpc_params![hex::encode([9u8; 32])])
            .expect("batch entry");
    }

    let answer = node.client.batch_request::<AccountInfo>(batch).await;

    let served = answer.map_or(0, |response| response.num_successful_calls());
    assert_eq!(served, 0, "calls in a batch were served");
    let single: Result<AccountInfo, _> = node
        .client
        .request("get_balance", rpc_params![hex::encode([9u8; 32])])
        .await;
    assert!(single.is_ok(), "a single call stopped working: {single:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_node_version_reports_this_build() {
    let node = start_node(&[], false).await;
    let v: serde_json::Value = node
        .client
        .request("get_node_version", rpc_params![])
        .await
        .expect("get_node_version");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        v["protocol"],
        custom_l1_node::upgrade::SUPPORTED_PROTOCOL_VERSION
    );
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
async fn get_mempool_info_reports_pooled_count_and_bytes() {
    let alice = generate_signing_key().expect("keygen");
    let node = start_node(&[(address_of(&alice), 1_000)], false).await;

    let empty: serde_json::Value = node
        .client
        .request("get_mempool_info", rpc_params![])
        .await
        .expect("get_mempool_info on an empty pool");
    assert_eq!(empty, serde_json::json!({ "size": 0, "bytes": 0 }));

    let tx = signed_transfer(&alice, [2u8; 32], 250, 0);
    let raw = hex::encode(tx.to_bytes());
    let submitted: SubmitTransactionResult = node
        .client
        .request("send_raw_transaction", rpc_params![raw])
        .await
        .expect("send_raw_transaction");
    assert!(submitted.accepted);

    let pooled: serde_json::Value = node
        .client
        .request("get_mempool_info", rpc_params![])
        .await
        .expect("get_mempool_info after one transaction");
    assert_eq!(pooled["size"], 1);
    assert_eq!(pooled["bytes"], tx.to_bytes().len());
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
    tx.outputs[0].amount = primitive_types::U256::from(900);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_validators_is_null_on_a_chain_without_staking() {
    let node = start_node(&[], false).await;

    let info: Option<serde_json::Value> = node
        .client
        .request("get_validators", rpc_params![])
        .await
        .expect("get_validators");

    assert!(info.is_none(), "{info:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_name_reads_the_committed_record_and_refuses_an_impossible_label() {
    use custom_l1_node::rpc::NameInfo;
    use custom_l1_node::state::names::NameRecord;

    let node = start_node(&[], false).await;
    let record = NameRecord {
        label: b"alice".to_vec(),
        owner: [0xAA; 32],
        target: [0xBB; 32],
        registered_at: 3,
        expires_at: 31_536_003,
    };
    // `nm:n:` ‖ blake3(label), the key ADR-135 P-3 names.
    let key = [&b"nm:n:"[..], blake3::hash(b"alice").as_bytes()].concat();
    node.state
        .raw_put_for_test(&key, &record.encode())
        .expect("seed name");

    for query in ["alice", "alice.maya"] {
        let info: Option<NameInfo> = node
            .client
            .request("get_name", rpc_params![query])
            .await
            .expect("get_name");
        let info = info.expect("registered");
        assert_eq!(info.label, "alice");
        assert_eq!(info.owner, "aa".repeat(32));
        assert_eq!(info.target, Some("bb".repeat(32)));
        assert_eq!(info.expires_at, 31_536_003);
        assert_eq!(info.status, "active");
        assert_eq!(info.checked_at, 1, "judged at the tip plus one");
    }
    let free: Option<NameInfo> = node
        .client
        .request("get_name", rpc_params!["bobby"])
        .await
        .expect("get_name");
    assert!(free.is_none());
    for bad in ["ab", "Alice", "a--b", "-ab"] {
        let refused: Result<Option<NameInfo>, _> =
            node.client.request("get_name", rpc_params![bad]).await;
        assert!(refused.is_err(), "{bad} must be refused, not looked up");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_name_is_reported_in_grace_without_a_target() {
    use custom_l1_node::rpc::NameInfo;
    use custom_l1_node::state::names::NameRecord;

    let node = start_node(&[], false).await;
    // Expired at block 0, so the next block (1) is past it: in grace.
    let record = NameRecord {
        label: b"carol".to_vec(),
        owner: [0xAA; 32],
        target: [0xBB; 32],
        registered_at: 0,
        expires_at: 0,
    };
    let key = [&b"nm:n:"[..], blake3::hash(b"carol").as_bytes()].concat();
    node.state
        .raw_put_for_test(&key, &record.encode())
        .expect("seed name");

    let info: Option<NameInfo> = node
        .client
        .request("get_name", rpc_params!["carol"])
        .await
        .expect("get_name");
    let info = info.expect("the record is still in state");
    assert_eq!(info.status, "grace");
    assert_eq!(info.target, None, "an expired name resolves to nothing");
    assert_eq!(info.owner, "aa".repeat(32), "the owner may still renew");
    let json = serde_json::to_value(&info).expect("json");
    assert!(json.get("target").is_none(), "{json}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_v4_reads_serve_an_asset_an_nft_and_a_schedule_from_committed_state() {
    use custom_l1_node::rpc::{AssetInfo, NftCollectionInfo, NftItemInfo, ScheduleInfo};
    use custom_l1_node::state::asset::{AssetRecord, AssetRecordV2};
    use custom_l1_node::state::nft::{NftCollection, NftItem};
    use custom_l1_node::state::scheduled::ScheduleRecord;

    let node = start_node(&[], false).await;
    let (asset, collection, schedule) = ([0x51; 32], [0x52; 32], [0x53; 32]);
    let put = |key: Vec<u8>, value: &[u8]| node.state.raw_put_for_test(&key, value).expect("seed");
    put(
        custom_l1_node::state::dex::asset_key(&asset),
        &AssetRecordV2 {
            base: AssetRecord {
                creator: [0xAA; 32],
                total_supply: U256::from(1_000),
                symbol: *b"GOLD\0\0\0\0",
            },
            cap: U256::from(5_000),
            authority: [0xAB; 32],
        }
        .encode(),
    );
    put(
        [&b"n:c:"[..], &collection[..]].concat(),
        &NftCollection {
            creator: [0xAC; 32],
            max_items: 10,
            minted: 1,
            metadata_hash: [0x01; 32],
        }
        .encode(),
    );
    put(
        [&b"n:i:"[..], &collection[..], &0_u64.to_be_bytes()[..]].concat(),
        &NftItem {
            owner: [0xAD; 32],
            metadata_hash: [0x02; 32],
        }
        .encode(),
    );
    put(
        [&b"sp:s:"[..], &schedule[..]].concat(),
        &ScheduleRecord {
            payer: [0xAE; 32],
            payee: [0xAF; 32],
            amount: U256::from(7),
            next_due: 40,
            interval: 20,
            remaining: 3,
            escrow: U256::from(24),
            missed: 0,
        }
        .encode(),
    );

    let a: Option<AssetInfo> = node
        .client
        .request("get_asset", rpc_params![hex::encode(asset)])
        .await
        .expect("get_asset");
    let a = a.expect("seeded");
    assert_eq!((a.symbol.as_str(), a.cap), ("GOLD", U256::from(5_000)));
    assert_eq!(a.mint_authority, Some("ab".repeat(32)));
    let c: Option<NftCollectionInfo> = node
        .client
        .request("get_nft_collection", rpc_params![hex::encode(collection)])
        .await
        .expect("get_nft_collection");
    assert_eq!(c.expect("seeded").minted, 1);
    let item: Option<NftItemInfo> = node
        .client
        .request("get_nft_item", rpc_params![hex::encode(collection), 0_u64])
        .await
        .expect("get_nft_item");
    assert_eq!(item.expect("seeded").owner, "ad".repeat(32));
    let missing: Option<NftItemInfo> = node
        .client
        .request("get_nft_item", rpc_params![hex::encode(collection), 1_u64])
        .await
        .expect("get_nft_item");
    assert!(missing.is_none(), "an unminted index is null");
    let s: Option<ScheduleInfo> = node
        .client
        .request("get_schedule", rpc_params![hex::encode(schedule)])
        .await
        .expect("get_schedule");
    let s = s.expect("seeded");
    assert_eq!((s.remaining, s.escrow), (3, U256::from(24)));
    assert_eq!(s.payee, "af".repeat(32));

    let none: Option<AssetInfo> = node
        .client
        .request("get_asset", rpc_params!["00".repeat(32)])
        .await
        .expect("get_asset");
    assert!(none.is_none());
    for method in ["get_asset", "get_nft_collection", "get_schedule"] {
        let bad: Result<Option<serde_json::Value>, _> =
            node.client.request(method, rpc_params!["abcd"]).await;
        assert!(bad.is_err(), "{method}: a short id is refused");
    }
}

#[test]
fn a_name_is_active_to_its_expiry_then_in_grace_then_lapsed() {
    use custom_l1_node::rpc::NameInfo;
    use custom_l1_node::state::names::{GRACE_BLOCKS, NameRecord};

    let record = NameRecord {
        label: b"dave".to_vec(),
        owner: [0xAA; 32],
        target: [0xBB; 32],
        registered_at: 1,
        expires_at: 100,
    };
    let cases = [
        (100, "active", true),
        (101, "grace", false),
        (100 + GRACE_BLOCKS, "grace", false),
        (101 + GRACE_BLOCKS, "lapsed", false),
    ];
    for (height, status, resolves) in cases {
        let info = NameInfo::at(&record, height);
        assert_eq!(info.status, status, "at {height}");
        assert_eq!(info.target.is_some(), resolves, "at {height}");
        assert_eq!(info.checked_at, height);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_range_stops_at_the_tip_and_matches_the_single_block_answer() {
    let node = start_node(&[], false).await;

    let range: Vec<BlockInfo> = node
        .client
        .request("get_blocks_by_height_range", rpc_params![0u64, 5u64])
        .await
        .expect("range");
    let single: BlockInfo = node
        .client
        .request("get_block_by_height", rpc_params![0u64])
        .await
        .expect("single");

    // Only genesis exists: the answer is short, not an error and not padded.
    assert_eq!(range.len(), 1);
    assert_eq!(range[0].raw, single.raw);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_range_past_the_tip_or_of_a_bad_size_is_refused() {
    let node = start_node(&[], false).await;

    for params in [
        rpc_params![42u64, 1u64],
        rpc_params![0u64, 0u64],
        rpc_params![0u64, 33u64],
    ] {
        let result: Result<Vec<BlockInfo>, _> = node
            .client
            .request("get_blocks_by_height_range", params)
            .await;
        assert!(result.is_err(), "must be refused");
    }
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

    assert_eq!(alice_info.balance, U256::from(600));
    assert_eq!(alice_info.nonce, 1);
    assert_eq!(bob_info.balance, U256::from(400));
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
            // ADR-046: amounts cross the wire as decimal strings so a u256
            // balance survives JSON (a JSON number tops out at 2^53).
            reply["balance"]
                .as_str()
                .expect("balance")
                .parse::<u64>()
                .expect("balance fits u64 in this fixture")
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
                c["before"]
                    .as_str()
                    .expect("before")
                    .parse()
                    .expect("before u64"),
                c["after"]
                    .as_str()
                    .expect("after")
                    .parse()
                    .expect("after u64"),
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
            Vec::new(),
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
