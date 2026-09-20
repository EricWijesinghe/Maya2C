//! PostgreSQL store.
//!
//! ## Runtime queries, not `query!`
//!
//! `sqlx::query!` type-checks SQL against a live database *at compile time*,
//! which means the crate cannot build without either a reachable
//! `DATABASE_URL` or committed `.sqlx` offline metadata. Runtime `query()`
//! trades that checking for a crate that builds anywhere. The trade is
//! deliberate and it is a real loss: a typo in a column name here surfaces at
//! runtime rather than at compile time, so the schema and these queries have to
//! be kept in step by hand.
//!
//! ## Idempotency
//!
//! Every insert is `ON CONFLICT ... DO UPDATE`. The indexer can legitimately
//! re-index a height — after a restart, or when a reorg replaces history — and
//! a plain `INSERT` would fail the whole batch on the primary key.
//!
//! ## Transactions
//!
//! A block and its transactions are written in one SQL transaction. Half a
//! block in the index is worse than none: the explorer would show a block whose
//! transaction list is silently short.

use async_trait::async_trait;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::error::{ExplorerError, Result};
use crate::model::{IndexedBlock, IndexedTx};
use crate::store::BlockStore;

/// Schema applied at startup.
///
/// Inline rather than in a migrations directory so a fresh deployment needs no
/// external tooling to reach a working schema.
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS blocks (
    height            BIGINT PRIMARY KEY,
    id                TEXT NOT NULL,
    prev_hash         TEXT NOT NULL,
    state_root        TEXT NOT NULL,
    timestamp         BIGINT NOT NULL,
    nonce             BIGINT NOT NULL,
    difficulty_target TEXT NOT NULL,
    tx_count          INTEGER NOT NULL,
    work              TEXT NOT NULL
);

-- Lookup by hash is a first-class explorer query, not a scan.
CREATE INDEX IF NOT EXISTS blocks_id_idx ON blocks (id);
CREATE INDEX IF NOT EXISTS blocks_timestamp_idx ON blocks (timestamp DESC);

CREATE TABLE IF NOT EXISTS transactions (
    txid         TEXT PRIMARY KEY,
    height       BIGINT NOT NULL,
    sender       TEXT NOT NULL,
    nonce        BIGINT NOT NULL,
    output_count INTEGER NOT NULL,
    total_out    BIGINT NOT NULL,
    signed       BOOLEAN NOT NULL,
    position     INTEGER NOT NULL
);

-- ON DELETE cascade is done in application code rather than a foreign key:
-- rollback_from deletes by height range, and a cascading constraint would make
-- the delete order significant.
CREATE INDEX IF NOT EXISTS transactions_height_idx ON transactions (height);
CREATE INDEX IF NOT EXISTS transactions_sender_idx ON transactions (sender, height DESC);
"#;

/// A PostgreSQL-backed index.
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Connects and applies the schema.
    ///
    /// # Errors
    ///
    /// Returns [`ExplorerError::Store`] if the database is unreachable or the
    /// schema cannot be applied.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(url)
            .await
            .map_err(|e| ExplorerError::Store(format!("connecting to {url}: {e}")))?;

        sqlx::raw_sql(SCHEMA)
            .execute(&pool)
            .await
            .map_err(|e| ExplorerError::Store(format!("applying schema: {e}")))?;

        Ok(Self { pool })
    }

    /// Wraps an existing pool.
    #[must_use]
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The underlying pool.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// Reads a block row.
fn block_from_row(row: &sqlx::postgres::PgRow) -> Result<IndexedBlock> {
    Ok(IndexedBlock {
        height: row.try_get("height")?,
        id: row.try_get("id")?,
        prev_hash: row.try_get("prev_hash")?,
        state_root: row.try_get("state_root")?,
        timestamp: row.try_get("timestamp")?,
        nonce: row.try_get("nonce")?,
        difficulty_target: row.try_get("difficulty_target")?,
        tx_count: row.try_get("tx_count")?,
        work: row.try_get("work")?,
    })
}

/// Reads a transaction row.
fn tx_from_row(row: &sqlx::postgres::PgRow) -> Result<IndexedTx> {
    Ok(IndexedTx {
        txid: row.try_get("txid")?,
        height: row.try_get("height")?,
        sender: row.try_get("sender")?,
        nonce: row.try_get("nonce")?,
        output_count: row.try_get("output_count")?,
        total_out: row.try_get("total_out")?,
        signed: row.try_get("signed")?,
    })
}

#[async_trait]
impl BlockStore for PostgresStore {
    async fn put_block(&self, block: &IndexedBlock, transactions: &[IndexedTx]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO blocks
               (height, id, prev_hash, state_root, timestamp, nonce,
                difficulty_target, tx_count, work)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (height) DO UPDATE SET
               id = EXCLUDED.id,
               prev_hash = EXCLUDED.prev_hash,
               state_root = EXCLUDED.state_root,
               timestamp = EXCLUDED.timestamp,
               nonce = EXCLUDED.nonce,
               difficulty_target = EXCLUDED.difficulty_target,
               tx_count = EXCLUDED.tx_count,
               work = EXCLUDED.work",
        )
        .bind(block.height)
        .bind(&block.id)
        .bind(&block.prev_hash)
        .bind(&block.state_root)
        .bind(block.timestamp)
        .bind(block.nonce)
        .bind(&block.difficulty_target)
        .bind(block.tx_count)
        .bind(&block.work)
        .execute(&mut *tx)
        .await?;

        // Clear first: a reorg can replace a height with a block holding fewer
        // transactions, and a pure upsert would leave the surplus behind.
        sqlx::query("DELETE FROM transactions WHERE height = $1")
            .bind(block.height)
            .execute(&mut *tx)
            .await?;

        for (position, transaction) in transactions.iter().enumerate() {
            sqlx::query(
                "INSERT INTO transactions
                   (txid, height, sender, nonce, output_count, total_out, signed, position)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (txid) DO UPDATE SET
                   height = EXCLUDED.height,
                   sender = EXCLUDED.sender,
                   nonce = EXCLUDED.nonce,
                   output_count = EXCLUDED.output_count,
                   total_out = EXCLUDED.total_out,
                   signed = EXCLUDED.signed,
                   position = EXCLUDED.position",
            )
            .bind(&transaction.txid)
            .bind(transaction.height)
            .bind(&transaction.sender)
            .bind(transaction.nonce)
            .bind(transaction.output_count)
            .bind(transaction.total_out)
            .bind(transaction.signed)
            .bind(position as i32)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn latest_height(&self) -> Result<Option<i64>> {
        let row = sqlx::query("SELECT MAX(height) AS height FROM blocks")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get::<Option<i64>, _>("height")?)
    }

    async fn latest_blocks(&self, limit: i64) -> Result<Vec<IndexedBlock>> {
        let rows = sqlx::query("SELECT * FROM blocks ORDER BY height DESC LIMIT $1")
            .bind(limit.max(0))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(block_from_row).collect()
    }

    async fn block_by_height(&self, height: i64) -> Result<Option<IndexedBlock>> {
        let row = sqlx::query("SELECT * FROM blocks WHERE height = $1")
            .bind(height)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(block_from_row).transpose()
    }

    async fn block_by_id(&self, id: &str) -> Result<Option<IndexedBlock>> {
        let row = sqlx::query("SELECT * FROM blocks WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(block_from_row).transpose()
    }

    async fn transaction(&self, txid: &str) -> Result<Option<IndexedTx>> {
        let row = sqlx::query("SELECT * FROM transactions WHERE txid = $1")
            .bind(txid)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(tx_from_row).transpose()
    }

    async fn transactions_in_block(&self, height: i64) -> Result<Vec<IndexedTx>> {
        let rows =
            sqlx::query("SELECT * FROM transactions WHERE height = $1 ORDER BY position ASC")
                .bind(height)
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(tx_from_row).collect()
    }

    async fn transactions_by_sender(&self, sender: &str, limit: i64) -> Result<Vec<IndexedTx>> {
        let rows = sqlx::query(
            "SELECT * FROM transactions WHERE sender = $1
             ORDER BY height DESC, position ASC LIMIT $2",
        )
        .bind(sender)
        .bind(limit.max(0))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(tx_from_row).collect()
    }

    async fn block_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM blocks")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("count")?)
    }

    async fn transaction_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM transactions")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("count")?)
    }

    async fn rollback_from(&self, height: i64) -> Result<u64> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM transactions WHERE height >= $1")
            .bind(height)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM blocks WHERE height >= $1")
            .bind(height)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(result.rows_affected())
    }
}
