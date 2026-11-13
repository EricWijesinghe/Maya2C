//! The PostgreSQL store held to the in-memory store's contract (ADR-092).
//!
//! `api_v1_tests.rs` drives the v1 API over [`MemoryStore`]. This file runs one
//! scenario covering every store query the API makes against both stores,
//! so the SQL in `store/postgres.rs` and the logic the API tests rely on cannot
//! drift apart unnoticed.
//!
//! The Postgres half needs `MAYA_EXPLORER_TEST_PG`: an admin URL of the form
//! `postgres://user:password@host:port/postgres`, no query string. Each test
//! creates and drops its own database. CI's nextest job provides one as a
//! service container. When the variable is unset the Postgres tests print
//! `SKIPPED` and return, and only the in-memory half has run.
//!
//! Local run:
//!
//! ```text
//! docker run -d --name explorer-pg -e POSTGRES_PASSWORD=test \
//!     -p 127.0.0.1:55432:5432 postgres:17-alpine
//! MAYA_EXPLORER_TEST_PG=postgres://postgres:test@127.0.0.1:55432/postgres \
//!     cargo test -p maya-explorer --test postgres_store_tests
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicUsize, Ordering};

use maya_explorer::model::{Direction, IndexedBlock, IndexedOutput, IndexedTx};
use maya_explorer::store::memory::MemoryStore;
use maya_explorer::store::postgres::{INDEX_VERSION, PostgresStore};
use maya_explorer::store::{BlockStore, Seek};
use sqlx::{AssertSqlSafe, PgPool, Row};

const PG_ENV: &str = "MAYA_EXPLORER_TEST_PG";
const HEIGHTS: i64 = 4;
/// Above `u64::MAX`, so a column that narrowed amounts would show.
const BIG: &str = "18446744073709551617";

// ---------------------------------------------------------------------------
// a database per test
// ---------------------------------------------------------------------------

struct PgDb {
    admin: String,
    name: String,
    url: String,
}

impl PgDb {
    /// A fresh, empty database, or `None` when no server is configured.
    async fn create(label: &str) -> Option<Self> {
        let Ok(admin) = std::env::var(PG_ENV) else {
            eprintln!(
                "SKIPPED: {PG_ENV} is not set, so the Postgres half of `{label}` did not run"
            );
            return None;
        };
        assert!(
            !admin.contains('?'),
            "{PG_ENV} must not carry a query string"
        );
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        // Our own identifier, never input: [a-z0-9_] only.
        let name = format!(
            "maya_explorer_{label}_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let (base, _) = admin.rsplit_once('/').expect("URL ends in /<database>");
        let url = format!("{base}/{name}");
        let db = Self { admin, name, url };
        db.admin_exec(&format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", db.name))
            .await;
        db.admin_exec(&format!("CREATE DATABASE {}", db.name)).await;
        Some(db)
    }

    async fn admin_exec(&self, sql: &str) {
        let pool = PgPool::connect(&self.admin)
            .await
            .expect("admin connection");
        sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
            .execute(&pool)
            .await
            .expect(sql);
        pool.close().await;
    }

    /// Connects the way the explorer does at startup: schema, then the
    /// generation check.
    async fn store(&self) -> PostgresStore {
        PostgresStore::connect(&self.url, 4)
            .await
            .expect("connect and apply schema")
    }

    async fn drop_db(self) {
        self.admin_exec(&format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        ))
        .await;
    }
}

// ---------------------------------------------------------------------------
// fixture
// ---------------------------------------------------------------------------

fn addr(n: u8) -> String {
    hex::encode([n; 32])
}

fn hash(n: u8) -> String {
    hex::encode([n; 32])
}

fn txid(height: i64, position: i64) -> String {
    let mut bytes = [0xee_u8; 32];
    bytes[0] = height as u8;
    bytes[1] = position as u8;
    hex::encode(bytes)
}

fn tx(height: i64, position: i64, sender: u8, outputs: &[(u8, &str)]) -> IndexedTx {
    let txid = txid(height, position);
    let outputs: Vec<IndexedOutput> = outputs
        .iter()
        .enumerate()
        .map(|(idx, (to, amount))| IndexedOutput {
            txid: txid.clone(),
            idx: idx as i32,
            recipient: addr(*to),
            amount: (*amount).to_string(),
        })
        .collect();
    IndexedTx {
        txid,
        height,
        sender: addr(sender),
        nonce: position,
        output_count: outputs.len() as i32,
        total_out: BIG.to_string(),
        signed: position != 1,
        kind: "transfer".to_string(),
        summary: format!("pay at {height}/{position}"),
        careful: position == 0,
        explanation: format!("explained {height}/{position}"),
        outputs,
    }
}

fn block(height: i64, id: u8, tx_count: usize) -> IndexedBlock {
    IndexedBlock {
        height,
        id: hash(id),
        prev_hash: hash(height.saturating_sub(1) as u8),
        state_root: hash(0xab),
        timestamp: 1_700_000_000 + height * 15,
        nonce: height,
        difficulty_target: hash(0xff),
        tx_count: tx_count as i32,
        work: BIG.to_string(),
    }
}

/// Position 0: addr 1 pays addr 2 (above 2^64) and addr 3. Position 1: addr 2
/// pays addr 1. Even heights add position 2: addr 4 pays itself.
fn txs_at(height: i64) -> Vec<IndexedTx> {
    let mut txs = vec![
        tx(height, 0, 1, &[(2, BIG), (3, "5")]),
        tx(height, 1, 2, &[(1, "7")]),
    ];
    if height % 2 == 0 {
        txs.push(tx(height, 2, 4, &[(4, "1")]));
    }
    txs
}

async fn seed(store: &dyn BlockStore) {
    for height in 1..=HEIGHTS {
        let txs = txs_at(height);
        store
            .put_block(&block(height, height as u8, txs.len()), &txs)
            .await
            .expect("seed block");
    }
}

/// Reads drop `outputs` (the Postgres store keeps them in their own table and
/// the API reads them through `outputs_of`), so compare without them.
fn bare(mut tx: IndexedTx) -> IndexedTx {
    tx.outputs.clear();
    tx
}

fn ids(page: &[(Seek, IndexedTx)]) -> Vec<String> {
    page.iter().map(|(_, tx)| tx.txid.clone()).collect()
}

/// Every transaction newest first, keyset-walked `limit` at a time.
async fn walk_address(
    store: &dyn BlockStore,
    address: &str,
    direction: Direction,
    limit: i64,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut before = None;
    loop {
        let page = store
            .transactions_by_address(address, direction, before, limit)
            .await
            .expect("address page");
        assert!(page.len() as i64 <= limit, "page over its limit");
        let Some((last, _)) = page.last() else {
            return seen;
        };
        before = Some(*last);
        seen.extend(ids(&page));
    }
}

async fn walk_all(store: &dyn BlockStore, limit: i64) -> Vec<(Seek, String)> {
    let mut seen = Vec::new();
    let mut before = None;
    loop {
        let page = store
            .transactions_page(before, limit)
            .await
            .expect("transactions page");
        assert!(page.len() as i64 <= limit, "page over its limit");
        let Some((last, _)) = page.last() else {
            return seen;
        };
        before = Some(*last);
        seen.extend(page.into_iter().map(|(at, tx)| (at, tx.txid)));
    }
}

/// `(height, position)` of every fixture transaction matching `keep`, newest
/// first: the order the API promises.
fn expected(keep: impl Fn(&IndexedTx) -> bool) -> Vec<String> {
    (1..=HEIGHTS)
        .rev()
        .flat_map(|h| txs_at(h).into_iter().rev())
        .filter(|tx| keep(tx))
        .map(|tx| tx.txid)
        .collect()
}

// ---------------------------------------------------------------------------
// the contract
// ---------------------------------------------------------------------------

async fn empty_store_answers_empty(store: &dyn BlockStore) {
    assert_eq!(store.latest_height().await.unwrap(), None);
    assert_eq!(store.oldest_height().await.unwrap(), None);
    assert_eq!(store.block_count().await.unwrap(), 0);
    assert_eq!(store.transaction_count().await.unwrap(), 0);
    assert!(store.blocks_page(None, 10).await.unwrap().is_empty());
    assert!(store.transactions_page(None, 10).await.unwrap().is_empty());
}

async fn lookups(store: &dyn BlockStore) {
    assert_eq!(store.latest_height().await.unwrap(), Some(HEIGHTS));
    assert_eq!(store.oldest_height().await.unwrap(), Some(1));
    assert_eq!(store.block_count().await.unwrap(), HEIGHTS);
    assert_eq!(store.transaction_count().await.unwrap(), 10);

    let latest: Vec<i64> = store
        .latest_blocks(2)
        .await
        .unwrap()
        .iter()
        .map(|b| b.height)
        .collect();
    assert_eq!(latest, vec![4, 3]);
    assert!(store.latest_blocks(0).await.unwrap().is_empty());
    assert!(store.latest_blocks(-5).await.unwrap().is_empty());

    assert_eq!(
        store.block_by_height(3).await.unwrap(),
        Some(block(3, 3, 2)),
        "every block column round-trips"
    );
    assert_eq!(
        store.block_by_id(&hash(2)).await.unwrap().unwrap().height,
        2
    );
    assert_eq!(store.block_by_height(99).await.unwrap(), None);

    let stored = store.transaction(&txid(2, 0)).await.unwrap().unwrap();
    assert_eq!(
        bare(stored),
        bare(txs_at(2).remove(0)),
        "every tx column round-trips"
    );
    assert_eq!(store.transaction(&txid(9, 9)).await.unwrap(), None);

    let in_block: Vec<String> = store
        .transactions_in_block(2)
        .await
        .unwrap()
        .into_iter()
        .map(|tx| tx.txid)
        .collect();
    assert_eq!(in_block, vec![txid(2, 0), txid(2, 1), txid(2, 2)]);

    let by_sender: Vec<String> = store
        .transactions_by_sender(&addr(1), 3)
        .await
        .unwrap()
        .into_iter()
        .map(|tx| tx.txid)
        .collect();
    assert_eq!(by_sender, vec![txid(4, 0), txid(3, 0), txid(2, 0)]);

    let outputs = store.outputs_of(&txid(3, 0)).await.unwrap();
    assert_eq!(
        outputs,
        txs_at(3).remove(0).outputs,
        "outputs in order, exact amounts"
    );
    assert!(store.outputs_of(&txid(9, 9)).await.unwrap().is_empty());
}

async fn pagination(store: &dyn BlockStore) {
    let first: Vec<i64> = store
        .blocks_page(None, 3)
        .await
        .unwrap()
        .iter()
        .map(|b| b.height)
        .collect();
    assert_eq!(first, vec![4, 3, 2]);
    let rest = store
        .blocks_page(
            Some(Seek {
                height: 2,
                position: 0,
            }),
            3,
        )
        .await
        .unwrap();
    assert_eq!(rest.iter().map(|b| b.height).collect::<Vec<_>>(), vec![1]);
    let past_end = Seek {
        height: 1,
        position: 0,
    };
    assert!(
        store
            .blocks_page(Some(past_end), 3)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.transactions_page(None, 0).await.unwrap().is_empty());
    assert!(store.transactions_page(None, -1).await.unwrap().is_empty());

    for limit in [1, 3, 100] {
        let walked = walk_all(store, limit).await;
        let txids: Vec<String> = walked.iter().map(|(_, id)| id.clone()).collect();
        assert_eq!(
            txids,
            expected(|_| true),
            "limit {limit}: every tx once, newest first"
        );
        for (at, id) in &walked {
            assert_eq!(
                id,
                &txid(at.height, at.position),
                "cursor is the row's own place"
            );
        }
    }

    let both = |n: u8| {
        move |tx: &IndexedTx| {
            tx.sender == addr(n) || tx.outputs.iter().any(|o| o.recipient == addr(n))
        }
    };
    for limit in [1, 3, 100] {
        assert_eq!(
            walk_address(store, &addr(4), Direction::Both, limit).await,
            vec![txid(4, 2), txid(2, 2)],
            "self-payment listed once"
        );
        assert_eq!(
            walk_address(store, &addr(2), Direction::Both, limit).await,
            expected(both(2))
        );
    }
    assert_eq!(
        walk_address(store, &addr(2), Direction::Sent, 3).await,
        expected(|tx| tx.sender == addr(2))
    );
    assert_eq!(
        walk_address(store, &addr(1), Direction::Received, 3).await,
        expected(|tx| tx.outputs.iter().any(|o| o.recipient == addr(1)))
    );
    assert!(
        walk_address(store, &addr(3), Direction::Sent, 3)
            .await
            .is_empty()
    );
    assert!(
        walk_address(store, &addr(9), Direction::Both, 3)
            .await
            .is_empty()
    );
}

/// Every string a client controls goes in as a bound parameter; quoting
/// tricks must find nothing and change nothing.
async fn hostile_input_is_data(store: &dyn BlockStore) {
    let hostile = [
        "'; DROP TABLE transactions; --",
        "x' OR '1'='1",
        "' OR 1=1 --",
        "%",
        "\\",
    ];
    for input in hostile {
        assert_eq!(store.transaction(input).await.unwrap(), None, "{input}");
        assert_eq!(store.block_by_id(input).await.unwrap(), None, "{input}");
        assert!(
            store
                .transactions_by_sender(input, 50)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(store.outputs_of(input).await.unwrap().is_empty());
        let page = store
            .transactions_by_address(input, Direction::Both, None, 50)
            .await
            .unwrap();
        assert!(page.is_empty(), "{input}");
    }
    assert_eq!(store.block_count().await.unwrap(), HEIGHTS);
    assert_eq!(store.transaction_count().await.unwrap(), 10);
}

async fn reorg_and_rollback(store: &dyn BlockStore) {
    // Height 4 replaced by a block holding only position 0.
    let replacement = vec![tx(4, 0, 1, &[(2, BIG), (3, "5")])];
    store
        .put_block(&block(4, 0x44, 1), &replacement)
        .await
        .unwrap();
    assert_eq!(
        store.block_by_height(4).await.unwrap().unwrap().id,
        hash(0x44)
    );
    assert_eq!(store.transaction_count().await.unwrap(), 8);
    assert_eq!(store.transaction(&txid(4, 2)).await.unwrap(), None);
    assert!(store.outputs_of(&txid(4, 2)).await.unwrap().is_empty());
    assert_eq!(
        walk_address(store, &addr(4), Direction::Both, 10).await,
        vec![txid(2, 2)],
        "an abandoned output no longer matches"
    );

    assert_eq!(store.rollback_from(3).await.unwrap(), 2);
    assert_eq!(store.latest_height().await.unwrap(), Some(2));
    assert_eq!(store.transaction_count().await.unwrap(), 5);
    assert_eq!(store.transaction(&txid(3, 0)).await.unwrap(), None);
    assert!(store.outputs_of(&txid(3, 0)).await.unwrap().is_empty());
    assert_eq!(
        walk_address(store, &addr(3), Direction::Received, 10).await,
        vec![txid(2, 0), txid(1, 0)]
    );
}

async fn contract(store: &dyn BlockStore) {
    empty_store_answers_empty(store).await;
    seed(store).await;
    // Re-indexing a height must not duplicate anything.
    let again = txs_at(2);
    store
        .put_block(&block(2, 2, again.len()), &again)
        .await
        .unwrap();
    lookups(store).await;
    pagination(store).await;
    hostile_input_is_data(store).await;
    reorg_and_rollback(store).await;
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_store_meets_the_contract() {
    contract(&MemoryStore::new()).await;
}

#[tokio::test]
async fn postgres_store_meets_the_contract() {
    let Some(db) = PgDb::create("contract").await else {
        return;
    };
    let store = db.store().await;
    contract(&store).await;
    store.pool().close().await;
    db.drop_db().await;
}

#[tokio::test]
async fn postgres_restart_keeps_the_index_and_a_stale_generation_rebuilds_it() {
    let Some(db) = PgDb::create("generation").await else {
        return;
    };
    let store = db.store().await;
    seed(&store).await;
    store.pool().close().await;

    // A restart re-applies the schema: every statement must be a no-op on an
    // index of the current generation.
    let store = db.store().await;
    assert_eq!(store.block_count().await.unwrap(), HEIGHTS);
    assert_eq!(store.outputs_of(&txid(1, 0)).await.unwrap().len(), 2);

    sqlx::query("UPDATE meta SET value = $1 WHERE key = 'index_version'")
        .bind((INDEX_VERSION - 1).to_string())
        .execute(store.pool())
        .await
        .unwrap();
    store.pool().close().await;

    let store = db.store().await;
    assert_eq!(
        store.block_count().await.unwrap(),
        0,
        "older generation emptied"
    );
    assert_eq!(store.transaction_count().await.unwrap(), 0);
    assert!(store.outputs_of(&txid(1, 0)).await.unwrap().is_empty());
    let version: String = sqlx::query("SELECT value FROM meta WHERE key = 'index_version'")
        .fetch_one(store.pool())
        .await
        .unwrap()
        .try_get("value")
        .unwrap();
    assert_eq!(version, INDEX_VERSION.to_string());
    store.pool().close().await;
    db.drop_db().await;
}
