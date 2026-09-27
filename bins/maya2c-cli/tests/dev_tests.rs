//! `maya2c dev` end to end: a real node, pre-funded accounts, and the
//! nft-game contract redeployed when its file changes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use l1_wallet::client::NodeClient;
use maya2c_cli::dev::{DEV_BALANCE, Options, run};

const RPC_PORT: u16 = 32_100;

fn bin_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_BIN_EXE_maya2c"))
        .parent()
        .unwrap()
        .to_path_buf();
    let node = dir.join(format!("maya2c-node{}", std::env::consts::EXE_SUFFIX));
    if !node.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "maya2c-node"])
            .status()
            .expect("cargo build");
        assert!(status.success(), "building maya2c-node");
    }
    dir
}

/// A valid module with different bytes: an appended custom section.
fn edited(code: &[u8], tag: u8) -> Vec<u8> {
    [code, &[0x00, 0x05, 0x04, b'd', b'e', b'v', tag]].concat()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dev_starts_a_funded_chain_and_redeploys_on_save() {
    let work = tempfile::tempdir().unwrap();
    let watch = work.path().join("contract.wasm");
    let code = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target-contracts/wasm32-unknown-unknown/release/nft_game.wasm"),
    )
    .unwrap();
    std::fs::write(&watch, &code).unwrap();
    let options = Options {
        dir: work.path().join("dev"),
        accounts: 3,
        watch: Some(watch.clone()),
        rpc_port: RPC_PORT,
        p2p_port: 31_100,
        explorer_port: None,
        bin_dir: bin_dir(),
        stop_after_deploys: Some(2),
        node_args: Vec::new(),
    };

    // Once the first deploy has landed: check an account is funded, then
    // "save" a changed module. Returns the funded balance it saw.
    let genesis_path = options.dir.join("genesis.json");
    let saver = tokio::spawn(async move {
        let client = NodeClient::connect(&format!("http://127.0.0.1:{RPC_PORT}")).unwrap();
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let Ok(text) = std::fs::read_to_string(&genesis_path) else {
                continue;
            };
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            let address = |i: usize| {
                json["allocations"][i]["address"]
                    .as_str()
                    .unwrap()
                    .to_string()
            };
            if client
                .get_balance(&address(0))
                .await
                .is_ok_and(|a| a.nonce >= 1)
            {
                let funded = client.get_balance(&address(2)).await.unwrap().balance;
                std::fs::write(&watch, edited(&code, b'2')).unwrap();
                return funded;
            }
        }
    });
    let report = run(&options).await.unwrap();
    let funded = saver.await.unwrap();

    assert_eq!(funded, DEV_BALANCE, "every dev account is pre-funded");
    assert_eq!(report.accounts.len(), 3);
    assert_eq!(report.deployments.len(), 2);
    assert_ne!(
        report.deployments[0].0, report.deployments[1].0,
        "a new module is a new contract"
    );
    println!(
        "maya2c dev: RPC ready {:?}, first block {:?}, deploys {:?}",
        report.rpc_ready,
        report.first_block,
        report.deployments.iter().map(|d| d.1).collect::<Vec<_>>()
    );
}
