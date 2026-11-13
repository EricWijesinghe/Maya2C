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
use crate::model::{Direction, IndexedBlock, IndexedOutput, IndexedTx};
use crate::store::{BlockStore, Seek};

/// Schema applied at startup.
///
/// Inline rather than in a migrations directory so a fresh deployment needs no
/// external tooling to reach a working schema.
pub const SCHEMA: &str = r"
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
    total_out    TEXT NOT NULL,
    signed       BOOLEAN NOT NULL,
    position     INTEGER NOT NULL
);

-- ON DELETE cascade is done in application code rather than a foreign key:
-- rollback_from deletes by height range, and a cascading constraint would make
-- the delete order significant.
--
-- ADR-092: every public list pages newest first by (height, position), so the
-- indexes carry both columns and a page is an index range, not a sort. An
-- ascending index read backwards serves the descending order; the row
-- comparisons the keyset queries use are index conditions on it.
DROP INDEX IF EXISTS transactions_height_idx;
DROP INDEX IF EXISTS transactions_sender_idx;
CREATE INDEX IF NOT EXISTS transactions_order_idx ON transactions (height, position);
CREATE INDEX IF NOT EXISTS transactions_sender_order_idx
    ON transactions (sender, height, position);

-- ADR-092: one row per output, so an address's received transactions are a
-- lookup rather than a scan of every transaction's outputs. Height and
-- position are copied from the transaction so the received side pages in
-- index order too; put_block keeps them in step with the transaction row.
CREATE TABLE IF NOT EXISTS tx_outputs (
    txid      TEXT NOT NULL,
    idx       INTEGER NOT NULL,
    recipient TEXT NOT NULL,
    amount    TEXT NOT NULL,
    height    BIGINT NOT NULL,
    position  INTEGER NOT NULL,
    PRIMARY KEY (txid, idx)
);
CREATE INDEX IF NOT EXISTS tx_outputs_recipient_order_idx
    ON tx_outputs (recipient, height, position);

-- ADR-046: a table created before amounts were 256-bit held a BIGINT. A no-op
-- once the column is already TEXT.
ALTER TABLE transactions ALTER COLUMN total_out TYPE TEXT USING total_out::text;

-- core::describe columns. Additive with defaults, so an index built before them
-- keeps working and old rows read as an unknown kind until re-indexed.
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT '';
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS summary TEXT NOT NULL DEFAULT '';
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS careful BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS explanation TEXT NOT NULL DEFAULT '';

-- Which schema generation filled the index. See INDEX_VERSION.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Generation of what the index stores.
///
/// `ADD COLUMN ... DEFAULT ''` upgrades the shape of an old index but cannot
/// fill in what the node knew at the time: rows indexed before `kind` existed
/// would show a blank kind forever, because the indexer resumes from the tip.
/// Bumping this empties the derived tables once so the indexer re-reads the
/// chain. v2: `kind`, `summary` and `careful` are populated. v3: `explanation`.
/// v4: `tx_outputs` is populated, with each output's height and position
/// (ADR-092). Moving to v4 empties a v3 index, so the explorer must point at a
/// node that still holds the history it wants indexed (an archive node).
pub const INDEX_VERSION: i64 = 4;

/// The keyset queries' bound when there is no cursor: every indexed key sorts
/// before it. One statement then serves both cases, and its generic plan keeps
/// the `(height, position)` index condition that `$n IS NULL OR ...` loses.
const FROM_TIP: Seek = Seek {
    height: i64::MAX,
    position: i64::MAX,
};

// The statements are public so `tests/postgres_store_tests.rs` can EXPLAIN the
// exact SQL that ships. All of them bind every value; none is formatted.

/// `blocks_page`: `$1` height bound, `$2` limit.
pub const BLOCKS_PAGE_SQL: &str =
    "SELECT * FROM blocks WHERE height < $1 ORDER BY height DESC LIMIT $2";

/// `transactions_page`: `$1, $2` the `(height, position)` bound, `$3` limit.
pub const TRANSACTIONS_PAGE_SQL: &str = "SELECT * FROM transactions
     WHERE (height, position) < ($1::BIGINT, $2::BIGINT)
     ORDER BY height DESC, position DESC LIMIT $3";

// Address branches, as macros so the three statements stay `&'static str`.
// `$1` address, `$2, $3` the bound, `$4` limit. Each branch is an index range
// read newest first and stopped at the limit, so an address with no history
// costs two empty index probes rather than a walk of the table.
macro_rules! sent_branch {
    () => {
        "SELECT t.* FROM transactions t
         WHERE t.sender = $1 AND (t.height, t.position) < ($2::BIGINT, $3::BIGINT)
         ORDER BY t.height DESC, t.position DESC LIMIT $4"
    };
}
// DISTINCT ON collapses several outputs of one transaction to the same
// address; without it they would count twice against the limit and end a
// page early, which the API reads as the last page.
macro_rules! received_branch {
    () => {
        "SELECT t.* FROM (
             SELECT DISTINCT ON (o.height, o.position) o.txid, o.height, o.position
             FROM tx_outputs o
             WHERE o.recipient = $1 AND (o.height, o.position) < ($2::BIGINT, $3::BIGINT)
             ORDER BY o.height DESC, o.position DESC LIMIT $4
         ) r JOIN transactions t ON t.txid = r.txid"
    };
}

/// Transactions the address sent.
pub const ADDRESS_SENT_SQL: &str = sent_branch!();
/// Transactions that paid the address.
pub const ADDRESS_RECEIVED_SQL: &str = concat!(
    received_branch!(),
    " ORDER BY t.height DESC, t.position DESC"
);
/// Both. UNION, not UNION ALL: a transaction that sends from and pays the
/// same address is one row.
pub const ADDRESS_BOTH_SQL: &str = concat!(
    "SELECT * FROM ((",
    sent_branch!(),
    ") UNION (",
    received_branch!(),
    ")) u ORDER BY height DESC, position DESC LIMIT $4"
);

/// The address statement for `direction`.
#[must_use]
pub const fn address_sql(direction: Direction) -> &'static str {
    match direction {
        Direction::Sent => ADDRESS_SENT_SQL,
        Direction::Received => ADDRESS_RECEIVED_SQL,
        Direction::Both => ADDRESS_BOTH_SQL,
    }
}

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

        rebuild_if_stale(&pool).await?;

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

/// Empties the derived tables when the index was built by an older generation.
///
/// Blocks and transactions are a cache of the node, so clearing them loses
/// nothing; the indexer then rebuilds from genesis (or the pruned window).
/// Runs once per upgrade and records the new generation in the same
/// transaction, so a crash mid-rebuild retries on the next start.
async fn rebuild_if_stale(pool: &PgPool) -> Result<()> {
    let stored: Option<String> = sqlx::query("SELECT value FROM meta WHERE key = 'index_version'")
        .fetch_optional(pool)
        .await?
        .map(|row| row.try_get("value"))
        .transpose()?;

    if stored.as_deref() == Some(INDEX_VERSION.to_string().as_str()) {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    sqlx::query("TRUNCATE blocks, transactions, tx_outputs")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO meta (key, value) VALUES ('index_version', $1)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(INDEX_VERSION.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
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
        kind: row.try_get("kind")?,
        summary: row.try_get("summary")?,
        careful: row.try_get("careful")?,
        explanation: row.try_get("explanation")?,
        outputs: Vec::new(),
    })
}

/// Reads a transaction row together with its `(height, position)` key.
fn placed_tx_from_row(row: &sqlx::postgres::PgRow) -> Result<(Seek, IndexedTx)> {
    let seek = Seek {
        height: row.try_get("height")?,
        position: i64::from(row.try_get::<i32, _>("position")?),
    };
    Ok((seek, tx_from_row(row)?))
}

/// Reads an output row.
fn output_from_row(row: &sqlx::postgres::PgRow) -> Result<IndexedOutput> {
    Ok(IndexedOutput {
        txid: row.try_get("txid")?,
        idx: row.try_get("idx")?,
        recipient: row.try_get("recipient")?,
        amount: row.try_get("amount")?,
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
        // transactions, and a pure upsert would leave the surplus behind. The
        // outputs go with them, or a surplus output would still match an
        // address query.
        sqlx::query(
            "DELETE FROM tx_outputs WHERE txid IN
               (SELECT txid FROM transactions WHERE height = $1)",
        )
        .bind(block.height)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM transactions WHERE height = $1")
            .bind(block.height)
            .execute(&mut *tx)
            .await?;

        for (position, transaction) in transactions.iter().enumerate() {
            sqlx::query(
                "INSERT INTO transactions
                   (txid, height, sender, nonce, output_count, total_out, signed, position,
                    kind, summary, careful, explanation)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                 ON CONFLICT (txid) DO UPDATE SET
                   height = EXCLUDED.height,
                   sender = EXCLUDED.sender,
                   nonce = EXCLUDED.nonce,
                   output_count = EXCLUDED.output_count,
                   total_out = EXCLUDED.total_out,
                   signed = EXCLUDED.signed,
                   position = EXCLUDED.position,
                   kind = EXCLUDED.kind,
                   summary = EXCLUDED.summary,
                   careful = EXCLUDED.careful,
                   explanation = EXCLUDED.explanation",
            )
            .bind(&transaction.txid)
            .bind(transaction.height)
            .bind(&transaction.sender)
            .bind(transaction.nonce)
            .bind(transaction.output_count)
            .bind(&transaction.total_out)
            .bind(transaction.signed)
            .bind(position as i32)
            .bind(&transaction.kind)
            .bind(&transaction.summary)
            .bind(transaction.careful)
            .bind(&transaction.explanation)
            .execute(&mut *tx)
            .await?;

            for output in &transaction.outputs {
                sqlx::query(
                    "INSERT INTO tx_outputs (txid, idx, recipient, amount, height, position)
                     VALUES ($1, $2, $3, $4, $5, $6)
                     ON CONFLICT (txid, idx) DO UPDATE SET
                       recipient = EXCLUDED.recipient,
                       amount = EXCLUDED.amount,
                       height = EXCLUDED.height,
                       position = EXCLUDED.position",
                )
                .bind(&output.txid)
                .bind(output.idx)
                .bind(&output.recipient)
                .bind(&output.amount)
                .bind(transaction.height)
                .bind(position as i32)
                .execute(&mut *tx)
                .await?;
            }
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

    async fn transaction_placed(&self, txid: &str) -> Result<Option<(Seek, IndexedTx)>> {
        let row = sqlx::query("SELECT * FROM transactions WHERE txid = $1")
            .bind(txid)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(placed_tx_from_row).transpose()
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

    async fn blocks_page(&self, before: Option<Seek>, limit: i64) -> Result<Vec<IndexedBlock>> {
        let before = before.unwrap_or(FROM_TIP);
        let rows = sqlx::query(BLOCKS_PAGE_SQL)
            .bind(before.height)
            .bind(limit.max(0))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(block_from_row).collect()
    }

    async fn transactions_page(
        &self,
        before: Option<Seek>,
        limit: i64,
    ) -> Result<Vec<(Seek, IndexedTx)>> {
        let before = before.unwrap_or(FROM_TIP);
        let rows = sqlx::query(TRANSACTIONS_PAGE_SQL)
            .bind(before.height)
            .bind(before.position)
            .bind(limit.max(0))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(placed_tx_from_row).collect()
    }

    async fn transactions_by_address(
        &self,
        address: &str,
        direction: Direction,
        before: Option<Seek>,
        limit: i64,
    ) -> Result<Vec<(Seek, IndexedTx)>> {
        let before = before.unwrap_or(FROM_TIP);
        let rows = sqlx::query(address_sql(direction))
            .bind(address)
            .bind(before.height)
            .bind(before.position)
            .bind(limit.max(0))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(placed_tx_from_row).collect()
    }

    async fn outputs_of(&self, txid: &str) -> Result<Vec<IndexedOutput>> {
        let rows = sqlx::query(
            "SELECT txid, idx, recipient, amount FROM tx_outputs WHERE txid = $1 ORDER BY idx",
        )
        .bind(txid)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(output_from_row).collect()
    }

    async fn oldest_height(&self) -> Result<Option<i64>> {
        let row = sqlx::query("SELECT MIN(height) AS height FROM blocks")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get::<Option<i64>, _>("height")?)
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

        sqlx::query(
            "DELETE FROM tx_outputs WHERE txid IN
               (SELECT txid FROM transactions WHERE height >= $1)",
        )
        .bind(height)
        .execute(&mut *tx)
        .await?;
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
