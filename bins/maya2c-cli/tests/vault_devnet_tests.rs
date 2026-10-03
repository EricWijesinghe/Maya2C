//! A PQ vault end to end on a devnet (ADR-030; Master Prompt 28): a real node,
//! hybrid ML-DSA + SLH-DSA keys, every step a signed transaction through the
//! mempool and DAG-BFT block production.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use custom_l1_node::core::vault_payload::{VaultAction, VaultConfig};
use custom_l1_node::core::{Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::HybridSigningKey;
use l1_wallet::client::NodeClient;
use maya2c_cli::dev::{CHAIN_ID, LocalChain, Options, launch};
use maya2c_cli::vault::{RISK_LABEL, status};

const RPC: u16 = 32_400;
const DELAY: u64 = 5;
const LIMIT: u64 = 1_000;
const WAIT: Duration = Duration::from_secs(60);

fn bin_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    maya2c_cli::dev::fresh_node_dir(&root).expect("building maya2c-node")
}

async fn nonce(client: &NodeClient, key: &HybridSigningKey) -> u64 {
    client
        .get_balance(&hex::encode(key.address()))
        .await
        .unwrap()
        .nonce
}

/// Signs and submits, then waits until the sender's nonce moves past it.
async fn land(client: &NodeClient, key: &HybridSigningKey, kind: TxKind, outputs: Vec<TxOutput>) {
    let n = nonce(client, key).await;
    let mut tx = Transaction::with_kind(kind, n);
    tx.outputs = outputs;
    // ADR-036: sign for the devnet's own genesis.
    let chain = client.get_chain_info().await.unwrap();
    tx.sign(key, &chain).unwrap();
    client
        .send_raw_transaction(&hex::encode(tx.to_bytes()))
        .await
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while nonce(client, key).await == n {
        assert!(Instant::now() < deadline, "transaction never landed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn height(client: &NodeClient) -> u64 {
    let mut h = 0;
    while client.get_block_by_height(h + 1).await.is_ok() {
        h += 1;
    }
    h
}

async fn balance(client: &NodeClient, a: [u8; 32]) -> u64 {
    client
        .get_balance(&hex::encode(a))
        .await
        .map_or(0, |a| a.balance)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vault_on_a_devnet_delays_cancels_and_pays() {
    let work = tempfile::tempdir().unwrap();
    let chain: LocalChain = launch(&Options {
        dir: work.path().join("vault"),
        accounts: 2,
        watch: None,
        rpc_port: RPC,
        p2p_port: 31_400,
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
    let (owner, guardian) = (&chain.accounts[0].0, &chain.accounts[1].0);
    let (thief, bob) = ([0x7E; 32], [0xB0; 32]);
    let owner_addr = owner.address();
    let vault = |a: VaultAction| TxKind::Vault(Box::new(a));
    let url = format!("http://127.0.0.1:{RPC}");

    land(
        client,
        owner,
        vault(VaultAction::Configure(VaultConfig {
            delay_blocks: DELAY,
            limit: LIMIT,
            guardians: vec![guardian.address()],
        })),
        vec![],
    )
    .await;
    let shown = status(&url, &hex::encode(owner_addr))
        .await
        .unwrap()
        .expect("a vault");
    assert_eq!(shown["delay_blocks"], DELAY);
    assert!(RISK_LABEL.contains("weakest of its source chain"));

    // An over-limit transfer is refused at admission, with the reason.
    let n = nonce(client, owner).await;
    let mut big = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: LIMIT + 1,
            recipient: thief,
        }],
        n,
    );
    big.sign(owner, &client.get_chain_info().await.unwrap())
        .unwrap();
    let refused = format!(
        "{:#}",
        client
            .send_raw_transaction(&hex::encode(big.to_bytes()))
            .await
            .unwrap_err()
    );
    assert!(refused.contains("instant limit"), "{refused}");

    // A thief with the owner's key requests most of the balance; a guardian
    // cancels it before it matures.
    let before = balance(client, owner_addr).await;
    land(
        client,
        owner,
        vault(VaultAction::Request {
            to: thief,
            amount: 900_000_000,
        }),
        vec![],
    )
    .await;
    assert_eq!(balance(client, owner_addr).await, before - 900_000_000);
    land(
        client,
        guardian,
        vault(VaultAction::Cancel {
            owner: owner_addr,
            id: 0,
        }),
        vec![],
    )
    .await;
    assert_eq!(balance(client, owner_addr).await, before);
    assert_eq!(balance(client, thief).await, 0);

    // The owner's own withdrawal waits out the delay, then anyone executes it.
    land(
        client,
        owner,
        vault(VaultAction::Request {
            to: bob,
            amount: 5_000,
        }),
        vec![],
    )
    .await;
    let requested = height(client).await;
    let deadline = Instant::now() + WAIT;
    while height(client).await < requested + DELAY {
        assert!(Instant::now() < deadline, "the chain stopped");
        // Idle chains make blocks only with transactions: keep one coming.
        land(
            client,
            guardian,
            TxKind::Transfer,
            vec![TxOutput {
                amount: 1,
                recipient: guardian.address(),
            }],
        )
        .await;
    }
    land(
        client,
        guardian,
        vault(VaultAction::Execute {
            owner: owner_addr,
            id: 1,
        }),
        vec![],
    )
    .await;
    assert_eq!(balance(client, bob).await, 5_000);
    let shown = status(&url, &hex::encode(owner_addr))
        .await
        .unwrap()
        .unwrap();
    // Settled requests are deleted; only open ones are reported.
    assert!(shown["requests"].as_array().unwrap().is_empty());
    assert_eq!(shown["open_requests"], 0);
    println!("vault on a devnet: {shown}");

    // The custodian's statement over the whole run reconciles for both sides,
    // and shows the thief's cancelled attempt as escrow out and back.
    let tip = height(client).await;
    let (owner_hex, bob_hex) = (hex::encode(owner_addr), hex::encode(bob));
    let report = maya2c_cli::custody_report::fetch(&url, &[owner_hex, bob_hex], 1, tip)
        .await
        .unwrap();
    let (owner_s, bob_s) = (&report.accounts[0], &report.accounts[1]);
    assert_eq!(owner_s.closing, balance(client, owner_addr).await);
    assert!(
        owner_s.vault.is_object(),
        "the owner's vault policy is in the statement"
    );
    assert_eq!((bob_s.opening, bob_s.closing), (0, 5_000));
    assert!(
        owner_s
            .movements
            .iter()
            .any(|m| m.before - m.after >= 900_000_000),
        "the stolen-key request's escrow is a recorded movement"
    );
    println!(
        "custody report {}..={tip}: {} + {} movements, sha256 {}",
        report.from,
        owner_s.movements.len(),
        bob_s.movements.len(),
        report.digest().unwrap()
    );
}
