//! `maya2c replay` and `maya2c fork` against a real source chain.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::HybridSigningKey;
use l1_wallet::client::NodeClient;
use maya2c_cli::dev::{LocalChain, Options, launch};
use maya2c_cli::fork::{Source, replay, start};

const SOURCE_RPC: u16 = 32_200;
const FORK_RPC: u16 = 32_201;
const WAIT: Duration = Duration::from_secs(60);

fn bin_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_BIN_EXE_maya2c"))
        .parent()
        .unwrap()
        .to_path_buf();
    if !dir
        .join(format!("maya2c-node{}", std::env::consts::EXE_SUFFIX))
        .exists()
    {
        let ok = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "maya2c-node"])
            .status()
            .expect("cargo build");
        assert!(ok.success());
    }
    dir
}

fn transfer(from: &HybridSigningKey, to: [u8; 32], amount: u64, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: to,
        }],
        nonce,
    );
    tx.sign(from).unwrap();
    tx
}

async fn wait_for_balance(client: &NodeClient, address: [u8; 32], amount: u64) {
    let deadline = Instant::now() + WAIT;
    while client
        .get_balance(&hex::encode(address))
        .await
        .map_or(0, |a| a.balance)
        != amount
    {
        assert!(
            Instant::now() < deadline,
            "balance of {} never reached {amount}",
            hex::encode(address)
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The height of the block carrying `txid`.
async fn height_of(client: &NodeClient, txid: &str) -> u64 {
    for h in 1.. {
        let block = client.get_block_by_height(h).await.unwrap();
        if block.transactions.iter().any(|t| t.txid == txid) {
            return h;
        }
    }
    unreachable!()
}

async fn source(work: &std::path::Path) -> LocalChain {
    launch(&Options {
        dir: work.join("source"),
        accounts: 2,
        watch: None,
        rpc_port: SOURCE_RPC,
        p2p_port: 31_200,
        explorer_port: None,
        bin_dir: bin_dir(),
        stop_after_deploys: None,
        node_args: Vec::new(),
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_block_replays_exactly_and_a_fork_diverges_locally() {
    let work = tempfile::tempdir().unwrap();
    let chain = source(work.path()).await;
    let alice = &chain.accounts[0].0;
    let (bob, carol) = ([0x55; 32], [0x66; 32]);

    // History first: four transfers, each waited for, so the replayed block
    // sits on blocks the local copy must execute itself.
    let dave = [0x44; 32];
    for nonce in 0..4u64 {
        let filler = transfer(alice, dave, 10, nonce);
        chain
            .client
            .send_raw_transaction(&hex::encode(filler.to_bytes()))
            .await
            .unwrap();
        wait_for_balance(&chain.client, dave, 10 * (nonce + 1)).await;
    }
    let tx = transfer(alice, bob, 777, 4);
    let txid = hex::encode(tx.txid());
    chain
        .client
        .send_raw_transaction(&hex::encode(tx.to_bytes()))
        .await
        .unwrap();
    wait_for_balance(&chain.client, bob, 777).await;
    let height = height_of(&chain.client, &txid).await;

    // Replay: a fresh local copy up to height - 1, then that block executed
    // here. Success means the state root reproduced.
    let url = format!("http://127.0.0.1:{SOURCE_RPC}");
    let genesis = work.path().join("source").join("genesis.json");
    let replay_source = Source {
        url: url.clone(),
        genesis: genesis.clone(),
        dir: work.path().join("replay"),
        snapshot_depth: None,
    };
    let handle = tokio::runtime::Handle::current();
    let started = Instant::now();
    let replayed = tokio::task::spawn_blocking(move || replay(&replay_source, height, handle))
        .await
        .unwrap()
        .unwrap();
    let replay_time = started.elapsed();
    assert_eq!(replayed.height, height);
    assert!(height >= 5, "the replay executed real history first");
    assert!(replayed.txids.contains(&txid));
    let header = chain
        .client
        .get_block_by_height(height)
        .await
        .unwrap()
        .header;
    assert_eq!(
        replayed.state_root, header.state_root,
        "same result, byte for byte"
    );
    assert!(
        replayed
            .changes
            .iter()
            .any(|c| c.address == bob && c.before == 0 && c.after == 777)
    );

    // Fork: the source's state, extended only here.
    let started = Instant::now();
    let fork = start(
        Source {
            url,
            genesis,
            dir: work.path().join("fork"),
            snapshot_depth: None,
        },
        format!("127.0.0.1:{FORK_RPC}").parse().unwrap(),
    )
    .await
    .unwrap();
    let fork_time = started.elapsed();
    assert!(fork.forked_at >= height);
    let local = NodeClient::connect(&format!("http://127.0.0.1:{FORK_RPC}")).unwrap();
    wait_for_balance(&local, bob, 777).await; // the source's history is there
    let tx = transfer(alice, carol, 888, 5);
    local
        .send_raw_transaction(&hex::encode(tx.to_bytes()))
        .await
        .unwrap();
    wait_for_balance(&local, carol, 888).await;
    assert_eq!(
        chain
            .client
            .get_balance(&hex::encode(carol))
            .await
            .unwrap()
            .balance,
        0,
        "the source never sees the fork's transaction"
    );
    println!(
        "replayed block {height} in {replay_time:?}; forked at {} in {fork_time:?}",
        fork.forked_at
    );
}
