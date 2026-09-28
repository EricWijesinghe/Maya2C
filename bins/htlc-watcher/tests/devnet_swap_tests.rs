//! Intent settlement across two real devnets (Master Prompt 25): a user who
//! wants chain B's coin for chain A's, and a solver who provides it, settled
//! by a SHA-256 hash-locked swap. Two `maya2c-node` processes with different
//! chain ids, each watcher talking to them over JSON-RPC (`RpcChain`), no
//! bridge and no trusted party: the preimage the user reveals to claim on B
//! is what lets the solver claim on A.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use l1_wallet::client::NodeClient;
use maya_htlc_lattice::lock::{HashFunction, Lock, Preimage, SwapSecret};
use maya_htlc_watcher::rpc::RpcChain;
use maya_htlc_watcher::{BlockRate, Journal, Margins, Outcome, Phase, SwapChain};
use maya_htlc_watcher::{InitiatedSwap, RespondRequest, Worker};
use maya2c_cli::dev::{LocalChain, Options, launch};

const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);

fn bin_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    maya2c_cli::dev::fresh_node_dir(&root).expect("building maya2c-node")
}

async fn devnet(dir: &std::path::Path, chain_id: &str, rpc_port: u16) -> LocalChain {
    launch(&Options {
        dir: dir.join(chain_id),
        accounts: 1,
        watch: None,
        rpc_port,
        p2p_port: rpc_port - 1_000,
        explorer_port: None,
        bin_dir: bin_dir(),
        stop_after_deploys: None,
        node_args: Vec::new(),
        chain_id: chain_id.to_string(),
        // Fees on: every lock, claim and allowance pays, as on a real network.
        base_fee: Some(1),
    })
    .await
    .unwrap()
}

fn watcher(
    a: u16,
    b: u16,
    key: HybridSigningKey,
    journals: &std::path::Path,
    name: &str,
) -> Worker {
    let maya: Arc<dyn SwapChain> =
        Arc::new(RpcChain::connect(&format!("http://127.0.0.1:{a}")).unwrap());
    let other: Arc<dyn SwapChain> =
        Arc::new(RpcChain::connect(&format!("http://127.0.0.1:{b}")).unwrap());
    let margins = Margins {
        maya_confirmations: 2,
        counterparty_confirmations: 2,
        submission_blocks: 1,
    };
    Worker::new(
        maya,
        other,
        key,
        margins,
        Journal::open(&journals.join(format!("{name}.json"))).unwrap(),
    )
}

async fn balance(client: &NodeClient, address: [u8; 32]) -> u64 {
    client
        .get_balance(&hex::encode(address))
        .await
        .map_or(0, |a| a.balance)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_intent_settles_across_two_devnets_by_hash_locked_swap() {
    let work = tempfile::tempdir().unwrap();
    let (port_a, port_b) = (32_300, 32_310);
    let chain_a = devnet(work.path(), "maya2c-devnet-a", port_a).await;
    let chain_b = devnet(work.path(), "maya2c-devnet-b", port_b).await;
    let user = chain_a.accounts[0].0.clone();
    let solver = chain_b.accounts[0].0.clone();
    let (user_addr, solver_addr) = (user.address(), solver.address());
    let a0 = balance(&chain_a.client, user_addr).await;
    let b0 = balance(&chain_b.client, solver_addr).await;

    let journals = work.path().join("journals");
    std::fs::create_dir_all(&journals).unwrap();
    let mut user_w = watcher(port_a, port_b, user, &journals, "user");
    let mut solver_w = watcher(port_a, port_b, solver, &journals, "solver");

    // The intent: 4,000 on A for at least 3,000 on B. The user locks first,
    // under a SHA-256 digest only they can open.
    let preimage = Preimage::new([0x5A; 32]);
    let lock = Lock::hash(HashFunction::Sha256, &preimage);
    let started = Instant::now();
    let user_lock = user_w
        .initiate(
            SwapSecret::Hash {
                function: HashFunction::Sha256,
                preimage,
            },
            solver_addr,
            4_000,
            200,
        )
        .await
        .expect("user locks on A");
    // The solver takes the intent: locks 3,000 on B for the user, shorter.
    let mut solver_lock = None;
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    while solver_lock.is_none() {
        assert!(
            Instant::now() < deadline,
            "the solver never saw the user's lock"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        solver_lock = solver_w
            .respond(RespondRequest {
                inbound_lock_id: user_lock,
                lock: lock.clone(),
                min_inbound_amount: 4_000,
                outbound_recipient: user_addr,
                outbound_amount: 3_000,
                outbound_expiry: 100,
                rate: BlockRate::EQUAL,
            })
            .await
            .ok();
    }
    let solver_lock = solver_lock.unwrap();
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let accepted = user_w
            .accept_response(InitiatedSwap {
                commitment_id: lock.id(),
                inbound_lock_id: solver_lock,
                min_inbound_amount: 3_000,
                rate: BlockRate::EQUAL,
            })
            .await;
        if accepted.is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the user never saw the solver's lock: {accepted:?}"
        );
    }

    // Both watchers run until both legs are finished, while the real
    // validators keep making blocks.
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        for w in [&mut user_w, &mut solver_w] {
            let report = w.step().await.expect("step");
            assert!(report.failures.is_empty(), "{:?}", report.failures);
        }
        let done = [&user_w, &solver_w]
            .iter()
            .all(|w| w.journal().swaps().iter().all(|s| s.phase != Phase::Active));
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the swap did not finish in {SETTLE_TIMEOUT:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let elapsed = started.elapsed();

    for w in [&user_w, &solver_w] {
        assert_eq!(
            w.journal().swaps()[0].phase,
            Phase::Finished(Outcome::Swapped)
        );
    }
    // Each side paid its lock, the other side's fee allowance, and fees; each
    // received its counterparty's lock plus an allowance it partly spent on the
    // claim's fee. Neither had an account on the other chain before.
    let user_a = balance(&chain_a.client, user_addr).await;
    let solver_a = balance(&chain_a.client, solver_addr).await;
    let solver_b = balance(&chain_b.client, solver_addr).await;
    let user_b = balance(&chain_b.client, user_addr).await;
    assert!(
        user_a <= a0 - 4_000 && solver_a >= 4_000,
        "A: user {user_a}, solver {solver_a}"
    );
    assert!(
        solver_b <= b0 - 3_000 && user_b >= 3_000,
        "B: solver {solver_b}, user {user_b}"
    );
    println!(
        "intent settled across two fee-charging devnets in {elapsed:?};          fees and allowances paid: user {} on A, solver {} on B",
        a0 - 4_000 - user_a,
        b0 - 3_000 - solver_b
    );
}
