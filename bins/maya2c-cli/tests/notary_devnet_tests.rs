//! Notary end to end on a devnet (ADR-094): a real node, the CLI's own
//! `setup`, `stamp` and `verify`, and the stamp found in a block.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use l1_wallet::client::NodeClient;
use maya2c_cli::dev::{CHAIN_ID, DEV_PASSWORD, Options, launch};
use maya2c_cli::notary::{SCHEMA, hash_file, setup, stamp, verify};

const RPC: u16 = 32_460;
const WAIT: Duration = Duration::from_secs(60);

fn bin_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    maya2c_cli::dev::fresh_node_dir(&root).expect("building maya2c-node")
}

/// Waits until `address`'s nonce exceeds `past`, i.e. a transaction landed.
async fn wait_nonce(client: &NodeClient, address: &str, past: u64, what: &str) {
    let deadline = Instant::now() + WAIT;
    while client.get_balance(address).await.unwrap().nonce <= past {
        assert!(Instant::now() < deadline, "{what} never landed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stamp_lands_in_a_block_and_verifies_with_a_receipt() {
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("notary");
    let chain = launch(&Options {
        dir: dir.clone(),
        accounts: 1,
        watch: None,
        rpc_port: RPC,
        p2p_port: 31_460,
        explorer_port: None,
        bin_dir: bin_dir(),
        stop_after_deploys: None,
        node_args: Vec::new(),
        chain_id: CHAIN_ID.to_string(),
        base_fee: None,
    })
    .await
    .unwrap();
    let client = &chain.client;
    let address = chain.accounts[0].1.clone();
    let keystore = dir.join("account-0.key");
    let url = format!("http://127.0.0.1:{RPC}");
    let genesis = client.get_network().await.unwrap().genesis;

    Box::pin(setup(&keystore, DEV_PASSWORD, &url, Some(&genesis)))
        .await
        .unwrap();
    wait_nonce(client, &address, 0, "register_did").await;

    let file = work.path().join("lease.txt");
    write(&file, b"lease agreement, page 1");
    let txid = Box::pin(stamp(&keystore, DEV_PASSWORD, &url, Some(&genesis), &file))
        .await
        .unwrap();
    wait_nonce(client, &address, 1, "the stamp").await;

    let receipt = verify(&file, &txid, &url, 1).await.unwrap();
    assert_eq!(receipt.hash, hex::encode(hash_file(&file).unwrap()));
    assert_eq!(receipt.txid, txid);
    assert_eq!(receipt.sender, address);
    assert_eq!(receipt.schema, SCHEMA);
    assert!(receipt.height >= 1, "{receipt:?}");
    let block = client.get_block_by_height(receipt.height).await.unwrap();
    assert_eq!(receipt.block_hash, block.header.id, "the real block's id");
    assert_eq!(receipt.timestamp, block.header.timestamp);
    assert!(
        block.transactions.iter().any(|t| t.txid == txid),
        "the stamp is in the block the receipt names"
    );
    println!(
        "notary on a devnet: {}",
        serde_json::to_string(&receipt).unwrap()
    );

    // One changed byte, and the same receipt no longer verifies.
    write(&file, b"lease agreement, page 2");
    let refused = format!("{:#}", verify(&file, &txid, &url, 1).await.unwrap_err());
    assert!(refused.contains("does not match"), "{refused}");
}
