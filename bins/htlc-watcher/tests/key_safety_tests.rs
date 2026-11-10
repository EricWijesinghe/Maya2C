//! What the watcher signs when the chain under it changes, and what it does
//! not sign when it cannot remember having signed. An in-process chain that
//! checks signatures against its genesis, as a node does (ADR-036).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use custom_l1_node::core::{ChainTag, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use maya_htlc_lattice::Address;
use maya_htlc_lattice::lock::{HashFunction, Preimage, SwapSecret};
use maya_htlc_watcher::submit::{Pending, Purpose};
use maya_htlc_watcher::{
    ChainSide, Journal, LockId, LockView, Margins, SwapChain, WatcherError, Worker,
};

const OLD: ChainTag = ChainTag::from_genesis([0x01; 32]);
const NEW: ChainTag = ChainTag::from_genesis([0x02; 32]);
const ID: [u8; 32] = [0x33; 32];

/// A chain whose genesis can change under the watcher. Like `RpcChain`, the
/// tag it hands out for signing is cached until a rejected broadcast reveals
/// the change, which it reports once as `GenesisChanged`.
struct Chain {
    live: Mutex<ChainTag>,
    cached: Mutex<ChainTag>,
    sent: Mutex<Vec<Vec<u8>>>,
}

impl Chain {
    fn new() -> Self {
        Self {
            live: Mutex::new(OLD),
            cached: Mutex::new(OLD),
            sent: Mutex::new(Vec::new()),
        }
    }

    fn regenesis(&self) {
        *self.live.lock().unwrap() = NEW;
    }

    fn sent(&self) -> Vec<Vec<u8>> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait]
impl SwapChain for Chain {
    async fn tip_height(&self) -> maya_htlc_watcher::Result<u64> {
        Ok(10)
    }

    async fn lock(&self, _id: &LockId) -> maya_htlc_watcher::Result<Option<LockView>> {
        Ok(None)
    }

    async fn next_nonce(&self, _address: &Address) -> maya_htlc_watcher::Result<u64> {
        Ok(0)
    }

    async fn broadcast(&self, raw: &[u8]) -> maya_htlc_watcher::Result<()> {
        self.sent.lock().unwrap().push(raw.to_vec());
        let tx = Transaction::from_bytes(raw).map_err(|e| WatcherError::Rpc(e.to_string()))?;
        let live = *self.live.lock().unwrap();
        if tx.verify(&live).is_ok() {
            return Ok(());
        }
        let mut cached = self.cached.lock().unwrap();
        if *cached != live {
            let old = std::mem::replace(&mut *cached, live);
            return Err(WatcherError::GenesisChanged {
                cached: hex::encode(old.0),
                live: hex::encode(live.0),
            });
        }
        Err(WatcherError::Refused("bad signature".into()))
    }

    async fn chain_tag(&self) -> maya_htlc_watcher::Result<ChainTag> {
        Ok(*self.cached.lock().unwrap())
    }
}

fn claim() -> Purpose {
    Purpose::Claim(ID)
}

async fn submit(
    p: &mut Pending,
    chain: &Chain,
    key: &HybridSigningKey,
) -> maya_htlc_watcher::Result<()> {
    p.submit(ChainSide::Maya, chain, key, claim(), TxKind::Transfer)
        .await
}

fn verifies_on_new_chain(raw: &[u8]) -> bool {
    Transaction::from_bytes(raw).unwrap().verify(&NEW).is_ok()
}

#[tokio::test]
async fn a_regenesis_found_on_rebroadcast_drops_what_was_signed_for_the_old_chain() {
    let (chain, key) = (Chain::new(), generate_signing_key().unwrap());
    let mut p = Pending::default();
    p.refresh(ChainSide::Maya, &chain, &key).await.unwrap();
    submit(&mut p, &chain, &key).await.unwrap();
    assert!(p.in_flight(claim()));

    chain.regenesis();
    p.refresh(ChainSide::Maya, &chain, &key).await.unwrap();
    // Bytes signed under the old genesis can never be mined here. Kept in
    // flight, the claim is never re-signed, and every later transaction sits
    // behind a nonce that will never be filled.
    assert!(
        !p.in_flight(claim()),
        "a transaction dead on the new chain stays in flight"
    );
    assert_eq!(
        p.next_nonce(ChainSide::Maya).unwrap(),
        0,
        "nonce gap left behind"
    );

    submit(&mut p, &chain, &key).await.unwrap();
    assert!(verifies_on_new_chain(chain.sent().last().unwrap()));
}

#[tokio::test]
async fn a_regenesis_found_on_submit_is_reported_and_the_retry_signs_for_the_new_chain() {
    let (chain, key) = (Chain::new(), generate_signing_key().unwrap());
    let mut p = Pending::default();
    p.refresh(ChainSide::Maya, &chain, &key).await.unwrap();
    chain.regenesis();

    let err = submit(&mut p, &chain, &key).await.unwrap_err();
    assert!(
        matches!(err, WatcherError::GenesisChanged { .. }),
        "{err:?}"
    );
    assert!(
        !p.in_flight(claim()),
        "the old-genesis claim stays in flight"
    );

    p.refresh(ChainSide::Maya, &chain, &key).await.unwrap();
    assert_eq!(p.next_nonce(ChainSide::Maya).unwrap(), 0);
    submit(&mut p, &chain, &key).await.unwrap();
    let sent = chain.sent();
    assert_eq!(
        sent.len(),
        2,
        "one dead transaction, one live; nothing rebroadcast twice"
    );
    assert!(!verifies_on_new_chain(&sent[0]));
    assert!(verifies_on_new_chain(&sent[1]));
}

#[tokio::test]
async fn a_lock_is_never_broadcast_when_its_swap_cannot_be_journalled() {
    let dir = tempfile::tempdir().unwrap();
    // The journal's directory does not exist, so every save fails.
    let journal = Journal::open(&dir.path().join("missing").join("journal.json")).unwrap();
    let chain = Arc::new(Chain::new());
    let margins = Margins {
        maya_confirmations: 2,
        counterparty_confirmations: 2,
        submission_blocks: 1,
    };
    let mut worker = Worker::new(
        chain.clone(),
        Arc::new(Chain::new()),
        generate_signing_key().unwrap(),
        margins,
        journal,
    );
    let secret = SwapSecret::Hash {
        function: HashFunction::Sha256,
        preimage: Preimage::new([0x5A; 32]),
    };
    let err = worker
        .initiate(secret, [9; 32], primitive_types::U256::from(1_000), 200)
        .await
        .unwrap_err();
    assert!(matches!(err, WatcherError::Journal { .. }), "{err:?}");
    // A funded lock nobody journalled is a lock nobody refunds.
    assert!(
        chain.sent().is_empty(),
        "a lock was broadcast without a journal entry"
    );
}
