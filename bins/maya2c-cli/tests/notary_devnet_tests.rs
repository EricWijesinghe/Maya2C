//! Notary end to end on a devnet (ADR-094): a real node, the CLI's own
//! `setup`, `stamp` and `verify`, and the stamp found in a block.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use maya2c_cli::dev::{CHAIN_ID, DEV_PASSWORD, Options, launch};
use maya2c_cli::notary::{Finality, SCHEMA, hash_file, setup, stamp, verify};

const WAIT: Duration = Duration::from_secs(60);

fn bin_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    maya2c_cli::dev::fresh_node_dir(&root).expect("building maya2c-node")
}

/// Two distinct ports the OS reports free, so parallel runs never collide on
/// a fixed pair. shortcut: a port can be taken between this call and the
/// node's bind; upgrade if the node ever accepts port 0 and reports its bind.
fn free_ports() -> (u16, u16) {
    let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    (
        a.local_addr().unwrap().port(),
        b.local_addr().unwrap().port(),
    )
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stamp_lands_in_a_block_and_verifies_with_a_receipt() {
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("notary");
    let (rpc, p2p) = free_ports();
    let chain = launch(&Options {
        dir: dir.clone(),
        accounts: 1,
        watch: None,
        rpc_port: rpc,
        p2p_port: p2p,
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
    let url = format!("http://127.0.0.1:{rpc}");
    let genesis = client.get_network().await.unwrap().genesis;

    // Back to back, no wait in between: each call returns only once its
    // transaction is in a block, so the next one signs with the next nonce.
    Box::pin(setup(&keystore, DEV_PASSWORD, &url, Some(&genesis)))
        .await
        .unwrap();
    let file = work.path().join("lease.txt");
    write(&file, b"lease agreement, page 1");
    let txid = Box::pin(stamp(&keystore, DEV_PASSWORD, &url, Some(&genesis), &file))
        .await
        .unwrap();
    let other = work.path().join("annex.txt");
    write(&other, b"annex");
    let other_txid = Box::pin(stamp(&keystore, DEV_PASSWORD, &url, Some(&genesis), &other))
        .await
        .unwrap();
    assert_eq!(
        client.get_balance(&address).await.unwrap().nonce,
        3,
        "setup and both stamps landed"
    );
    let second = verify(&other, &other_txid, &url, Some(&genesis), 1)
        .await
        .unwrap();
    assert_eq!(second.hash, hex::encode(hash_file(&other).unwrap()));

    // A node whose genesis is not the expected one is refused before any scan.
    let wrong = "00".repeat(32);
    let refused = format!(
        "{:#}",
        verify(&file, &txid, &url, Some(&wrong), 1)
            .await
            .unwrap_err()
    );
    assert!(refused.contains("refusing"), "{refused}");

    let receipt = verify(&file, &txid, &url, Some(&genesis), 1).await.unwrap();
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

    // A checkpoint reaches the stamp's block, and the receipt then says final.
    let deadline = Instant::now() + WAIT;
    let fin = loop {
        let r = verify(&file, &txid, &url, Some(&genesis), 1).await.unwrap();
        if r.finality == Finality::Final || Instant::now() >= deadline {
            break r;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert_eq!(fin.finality, Finality::Final, "{fin:?}");
    assert_eq!(fin.height, receipt.height);

    // One changed byte, and the same receipt no longer verifies.
    write(&file, b"lease agreement, page 2");
    let refused = format!(
        "{:#}",
        verify(&file, &txid, &url, Some(&genesis), 1)
            .await
            .unwrap_err()
    );
    assert!(refused.contains("does not match"), "{refused}");
}
