//! Settling share credits on chain.
//!
//! ## The state machine
//!
//! ```text
//!                    treasury refuses
//!   Pending ──────────────────────────────► Failed   (credits returned)
//!      │
//!      │ signed against a reserved nonce
//!      ▼
//!   Signed ─── broadcast ──► Submitted ─── buried ──► Confirmed
//!                                 ▲                │
//!                                 └── reorg ───────┘
//! ```
//!
//! Nothing moves backwards out of `Submitted` except into `Submitted` again.
//! Once the bytes are on the network the pool's only options are to wait or to
//! rebroadcast *those* bytes; signing replacement bytes for the same nonce
//! would be a second transaction competing with the first, and whichever won
//! the pool would have double-counted the outcome.
//!
//! ## How the pool knows a payout landed
//!
//! There is no `get_transaction` RPC to ask. What there is, is
//! `src/state/db.rs:418`: a transaction is valid only when `tx.nonce` equals
//! the sender's current nonce exactly, so transactions from one account are
//! strictly sequential. The pool is the only spender from the treasury, so the
//! account's nonce advancing past a batch's reserved nonce means *that batch's
//! transaction* was included — there is nothing else it could have been.
//!
//! A reorg that unwinds the payout moves the nonce back down, which the watcher
//! reads as "not landed after all" and rebroadcasts the same bytes into the
//! nonce slot that is free again.
//!
//! ## One batch at a time
//!
//! Sequential nonces make pipelining a trap: several batches in flight means
//! predicting nonces for transactions not yet accepted, and one rejection
//! strands every batch behind it. The engine therefore starts a new batch only
//! when nothing is open.

use std::sync::Arc;

use async_trait::async_trait;
use custom_l1_node::state::Address;

use crate::config::PoolConfig;
use crate::error::Result;
use crate::ledger::{BlockState, FoundBlock, ShareLedger};
use crate::model::{PayoutBatch, PayoutEntry, PayoutState, now_millis};
use crate::pplns;
use crate::treasury::Treasury;

/// What the payout engine needs from the chain.
///
/// A trait rather than [`crate::node::NodeClient`] directly, because the code
/// below decides who gets paid and how much, and a path that can only be
/// exercised against a live node is a path that is exercised in production for
/// the first time.
#[async_trait]
pub trait ChainView: Send + Sync {
    /// Height of the active tip.
    async fn chain_height(&self) -> Result<u64>;

    /// Id of the block the active chain holds at `height`, if it has one.
    async fn block_id_at(&self, height: u64) -> Result<Option<String>>;

    /// The treasury's balance and next usable nonce.
    async fn account(&self, address: &Address) -> Result<(u64, u64)>;

    /// Broadcasts signed transaction bytes, returning the transaction id.
    async fn broadcast(&self, raw: &[u8]) -> Result<String>;

    /// The chain's genesis block id, used as the chain tag for signatures (ADR-036).
    async fn chain_tag(&self) -> Result<custom_l1_node::core::ChainTag>;
}

/// What one settlement pass did.
///
/// Returned rather than logged so the caller can move it straight into
/// [`crate::metrics`] — a counter incremented inside this module would be a
/// second place that knows how settlement is shaped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettleReport {
    /// Blocks whose credits became payable.
    pub matured: usize,
    /// Blocks lost to a reorg, whose credits were reversed.
    pub orphaned: usize,
    /// Batches broadcast this pass.
    pub broadcast: usize,
    /// Batches that reached the confirmation depth.
    pub confirmed: usize,
    /// Value moved by batches broadcast this pass, in base units.
    pub value_broadcast: u64,
}

/// Turns share credits into transactions.
pub struct PayoutEngine {
    /// The share ledger.
    ledger: Arc<dyn ShareLedger>,
    /// The chain.
    chain: Arc<dyn ChainView>,
    /// The signing key and its limits.
    treasury: Treasury,
    /// Policy.
    config: PoolConfig,
}

impl PayoutEngine {
    /// Builds an engine.
    #[must_use]
    pub fn new(
        ledger: Arc<dyn ShareLedger>,
        chain: Arc<dyn ChainView>,
        treasury: Treasury,
        config: PoolConfig,
    ) -> Self {
        Self {
            ledger,
            chain,
            treasury,
            config,
        }
    }

    /// The account payouts are spent from.
    #[must_use]
    pub fn treasury_address(&self) -> Address {
        self.treasury.address()
    }

    /// Credits a found block's PPLNS window.
    ///
    /// `sequence` is the ledger position of the share that solved the block,
    /// and the window is measured backwards from there rather than from the
    /// ledger tip — shares that arrived afterwards belong to the next block.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Treasury`](crate::error::PoolError::Treasury) if the window is empty or the split
    /// cannot be performed exactly, and propagates ledger failures.
    pub fn credit_block(
        &self,
        block_id: &str,
        height: u64,
        sequence: u64,
        finder: Address,
        network_target: &[u8; 32],
    ) -> Result<Vec<PayoutEntry>> {
        let window_size = pplns::window_size(network_target, self.config.pplns_factor)?;
        let window = self.ledger.window(sequence, window_size)?;
        let reward = self.config.miner_share(self.config.reward_per_block);
        let credits = pplns::split(&window, reward)?;

        let block = FoundBlock {
            id: block_id.to_string(),
            height,
            sequence,
            reward,
            finder,
            found_at_millis: now_millis(),
            state: BlockState::Immature,
        };

        self.ledger.record_block(&block, &credits)?;
        Ok(credits)
    }

    /// Runs one settlement pass.
    ///
    /// # Errors
    ///
    /// Propagates ledger failures. A chain that is unreachable degrades rather
    /// than fails: maturation and batch progress simply do not advance this
    /// pass, and the credits already in the ledger are untouched.
    pub async fn settle(&self) -> Result<SettleReport> {
        let height = self.chain.chain_height().await?;

        let mut report = self.resolve_blocks(height).await?;
        let progress = self.advance_batches(height).await?;
        report.broadcast += progress.broadcast;
        report.confirmed += progress.confirmed;
        report.value_broadcast += progress.value_broadcast;

        if self.ledger.open_batches()?.is_empty() {
            let started = self.start_batch().await?;
            report.broadcast += started.broadcast;
            report.value_broadcast += started.value_broadcast;
        }

        Ok(report)
    }

    /// Matures or orphans every immature block that is deep enough to judge.
    async fn resolve_blocks(&self, height: u64) -> Result<SettleReport> {
        let mut report = SettleReport::default();

        for block in self.ledger.immature_blocks()? {
            if height < block.height.saturating_add(self.config.confirmations) {
                continue;
            }

            // The id at that height decides it. Comparing heights alone would
            // mature a block a reorg had already replaced with someone else's.
            match self.chain.block_id_at(block.height).await? {
                Some(id) if id == block.id => {
                    self.ledger.mature_block(&block.id)?;
                    report.matured += 1;
                }
                Some(_) => {
                    self.ledger.orphan_block(&block.id)?;
                    report.orphaned += 1;
                }
                // The chain has no block at a height it claims to be past.
                // Leave it immature and look again next pass rather than
                // guessing in either direction.
                None => {}
            }
        }

        Ok(report)
    }

    /// Moves every open batch as far along as the chain allows.
    async fn advance_batches(&self, height: u64) -> Result<SettleReport> {
        let mut report = SettleReport::default();
        let (_, account_nonce) = self.chain.account(&self.treasury.address()).await?;

        for mut batch in self.ledger.open_batches()? {
            match batch.state {
                PayoutState::Pending | PayoutState::Signed => {
                    // Signed bytes already exist and are rebroadcast verbatim.
                    // This is the crash-recovery path, and it is why a batch is
                    // written down before it is signed.
                    let txid = self.chain.broadcast(&batch.signed_tx).await?;
                    batch.txid = Some(txid);
                    batch.state = PayoutState::Submitted;
                    self.ledger.put_batch(&batch)?;
                    report.broadcast += 1;
                    report.value_broadcast = report.value_broadcast.saturating_add(batch.total());
                }
                PayoutState::Submitted => {
                    let landed = account_nonce > batch.nonce;
                    match (landed, batch.included_height) {
                        (true, None) => {
                            batch.included_height = Some(height);
                            self.ledger.put_batch(&batch)?;
                        }
                        (true, Some(included))
                            if height >= included.saturating_add(self.config.confirmations) =>
                        {
                            batch.state = PayoutState::Confirmed;
                            self.ledger.put_batch(&batch)?;
                            report.confirmed += 1;
                        }
                        (false, Some(_)) => {
                            // A reorg unwound it. The nonce slot is free again,
                            // so the same bytes go back out — not new ones.
                            batch.included_height = None;
                            batch.state = PayoutState::Signed;
                            self.ledger.put_batch(&batch)?;
                        }
                        _ => {}
                    }
                }
                // Terminal states never appear in `open_batches`.
                PayoutState::Confirmed | PayoutState::Orphaned | PayoutState::Failed => {}
            }
        }

        Ok(report)
    }

    /// Selects, signs, records, and broadcasts one batch.
    async fn start_batch(&self) -> Result<SettleReport> {
        let mut report = SettleReport::default();

        let entries = self.select_entries()?;
        if entries.is_empty() {
            return Ok(report);
        }

        let (balance, nonce) = self.chain.account(&self.treasury.address()).await?;
        let id = self.ledger.next_batch_id()?;
        let chain_tag = self.chain.chain_tag().await?;

        let transaction = match self
            .treasury
            .sign_payout(&entries, nonce, balance, &self.config, &chain_tag)
        {
            Ok(transaction) => transaction,
            Err(error) => {
                // The treasury refused: unfunded, over a cap, or over the output
                // limit. Write the refusal down so an operator can see it, and
                // leave the credits with the miners.
                let refused = PayoutBatch {
                    id,
                    nonce,
                    entries,
                    signed_tx: Vec::new(),
                    txid: None,
                    state: PayoutState::Failed,
                    created_at_millis: now_millis(),
                    included_height: None,
                };
                // The refusal is written down and then raised. It is not folded
                // into the report: every case here is a policy or custody
                // condition that no retry clears, so the daemon has to see an
                // error rather than a counter that ticked.
                self.ledger.put_batch(&refused)?;
                return Err(error);
            }
        };

        let batch = PayoutBatch {
            id,
            nonce,
            entries,
            signed_tx: transaction.to_bytes(),
            txid: Some(hex::encode(transaction.txid())),
            state: PayoutState::Signed,
            created_at_millis: now_millis(),
            included_height: None,
        };

        // Recorded *before* broadcast, and the debit happens in the same write.
        // A crash immediately after this leaves a batch that the next pass
        // rebroadcasts; a crash immediately before leaves nothing, and the
        // credits are still the miners'. There is no window in which the pool
        // has spent and does not know it.
        self.ledger.create_batch(&batch)?;

        let mut batch = batch;
        let txid = self.chain.broadcast(&batch.signed_tx).await?;
        batch.txid = Some(txid);
        batch.state = PayoutState::Submitted;
        self.ledger.put_batch(&batch)?;

        report.broadcast += 1;
        report.value_broadcast = batch.total();
        Ok(report)
    }

    /// Picks the miners a batch will pay.
    ///
    /// Ordered by amount descending so a capped batch clears the largest debts
    /// first, then by address so the selection is deterministic.
    fn select_entries(&self) -> Result<Vec<PayoutEntry>> {
        let mut candidates: Vec<PayoutEntry> = self
            .ledger
            .balances()?
            .into_iter()
            .filter(|(_, balance)| balance.unpaid >= self.config.min_payout)
            .map(|(miner, balance)| PayoutEntry {
                miner,
                amount: balance.unpaid,
            })
            .collect();

        candidates.sort_by(|a, b| b.amount.cmp(&a.amount).then_with(|| a.miner.cmp(&b.miner)));
        candidates.truncate(self.config.max_outputs_per_batch);
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use std::sync::Mutex;

    use crate::error::PoolError;

    use custom_l1_node::crypto::hybrid::generate_signing_key;
    use custom_l1_node::crypto::pow::target_from_leading_zero_bits;

    use crate::ledger::memory::MemoryLedger;

    const ALICE: Address = [0xAA; 32];
    const BOB: Address = [0xBB; 32];

    /// A chain the tests drive by hand.
    #[derive(Default)]
    struct FakeChain {
        /// Mutable chain state.
        state: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        height: u64,
        /// Block id at each height.
        blocks: Vec<(u64, String)>,
        balance: u64,
        nonce: u64,
        /// Raw transactions broadcast, in order.
        broadcasts: Vec<Vec<u8>>,
        /// Whether the next broadcast should fail.
        refuse_broadcast: bool,
    }

    impl FakeChain {
        fn with(height: u64, balance: u64) -> Self {
            Self {
                state: Mutex::new(FakeState {
                    height,
                    balance,
                    ..FakeState::default()
                }),
            }
        }

        fn put_block(&self, height: u64, id: &str) {
            self.state
                .lock()
                .unwrap()
                .blocks
                .push((height, id.to_string()));
        }

        fn set_height(&self, height: u64) {
            self.state.lock().unwrap().height = height;
        }

        fn set_nonce(&self, nonce: u64) {
            self.state.lock().unwrap().nonce = nonce;
        }

        fn broadcasts(&self) -> Vec<Vec<u8>> {
            self.state.lock().unwrap().broadcasts.clone()
        }
    }

    #[async_trait]
    impl ChainView for FakeChain {
        async fn chain_height(&self) -> Result<u64> {
            Ok(self.state.lock().unwrap().height)
        }

        async fn block_id_at(&self, height: u64) -> Result<Option<String>> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .blocks
                .iter()
                .find(|(stored, _)| *stored == height)
                .map(|(_, id)| id.clone()))
        }

        async fn account(&self, _address: &Address) -> Result<(u64, u64)> {
            let state = self.state.lock().unwrap();
            Ok((state.balance, state.nonce))
        }

        async fn broadcast(&self, raw: &[u8]) -> Result<String> {
            let mut state = self.state.lock().unwrap();
            if state.refuse_broadcast {
                return Err(PoolError::Chain("refused".to_string()));
            }
            state.broadcasts.push(raw.to_vec());
            Ok(format!("tx{}", state.broadcasts.len()))
        }

        async fn chain_tag(&self) -> Result<custom_l1_node::core::ChainTag> {
            // Test chain: use a fixed tag
            Ok(custom_l1_node::core::ChainTag::from_genesis([0; 32]))
        }
    }

    fn config() -> PoolConfig {
        PoolConfig {
            reward_per_block: 1_000,
            fee_rate: 0.0,
            confirmations: 3,
            min_payout: 10,
            ..PoolConfig::default()
        }
    }

    fn engine(
        ledger: Arc<MemoryLedger>,
        chain: Arc<FakeChain>,
        config: PoolConfig,
    ) -> PayoutEngine {
        let treasury = Treasury::from_key(generate_signing_key().expect("key"));
        PayoutEngine::new(ledger, chain, treasury, config)
    }

    /// Seeds a ledger with shares from two miners and credits a found block.
    fn seeded() -> (Arc<MemoryLedger>, Arc<FakeChain>, PayoutEngine) {
        let ledger = Arc::new(MemoryLedger::new());
        for _ in 0..5 {
            ledger.append_share(&ALICE, "rig", 100, 0).unwrap();
            ledger.append_share(&BOB, "rig", 100, 0).unwrap();
        }

        let chain = Arc::new(FakeChain::with(100, 1_000_000));
        let engine = engine(Arc::clone(&ledger), Arc::clone(&chain), config());
        (ledger, chain, engine)
    }

    #[test]
    fn a_found_block_credits_its_window_as_immature() {
        let (ledger, _chain, engine) = seeded();
        let tip = ledger.tip_sequence().unwrap().unwrap();

        let credits = engine
            .credit_block(
                "block-a",
                100,
                tip,
                ALICE,
                &target_from_leading_zero_bits(8),
            )
            .unwrap();

        assert_eq!(credits.len(), 2);
        assert_eq!(ledger.balance(&ALICE).unwrap().immature, 500);
        assert_eq!(ledger.balance(&BOB).unwrap().immature, 500);
        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 0);
    }

    #[test]
    fn the_operator_fee_comes_off_before_the_split() {
        let ledger = Arc::new(MemoryLedger::new());
        ledger.append_share(&ALICE, "rig", 100, 0).unwrap();

        let chain = Arc::new(FakeChain::with(100, 1_000_000));
        let engine = engine(
            Arc::clone(&ledger),
            chain,
            PoolConfig {
                fee_rate: 0.1,
                ..config()
            },
        );

        engine
            .credit_block("block-a", 100, 0, ALICE, &target_from_leading_zero_bits(8))
            .unwrap();

        assert_eq!(ledger.balance(&ALICE).unwrap().immature, 900);
    }

    #[tokio::test]
    async fn credits_mature_once_the_block_is_buried() {
        let (ledger, chain, engine) = seeded();
        let tip = ledger.tip_sequence().unwrap().unwrap();
        engine
            .credit_block(
                "block-a",
                100,
                tip,
                ALICE,
                &target_from_leading_zero_bits(8),
            )
            .unwrap();
        chain.put_block(100, "block-a");

        // Not yet deep enough.
        chain.set_height(102);
        assert_eq!(engine.settle().await.unwrap().matured, 0);
        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 0);

        chain.set_height(103);
        assert_eq!(engine.settle().await.unwrap().matured, 1);
        assert_eq!(ledger.balance(&ALICE).unwrap().immature, 0);
    }

    #[tokio::test]
    async fn a_block_replaced_by_a_reorg_is_orphaned_rather_than_paid() {
        // The reason blocks are keyed by id: at this height the chain now holds
        // somebody else's block, and the shares behind ours earned nothing.
        let (ledger, chain, engine) = seeded();
        let tip = ledger.tip_sequence().unwrap().unwrap();
        engine
            .credit_block(
                "block-a",
                100,
                tip,
                ALICE,
                &target_from_leading_zero_bits(8),
            )
            .unwrap();

        chain.put_block(100, "someone-elses-block");
        chain.set_height(110);

        let report = engine.settle().await.unwrap();
        assert_eq!(report.orphaned, 1);
        assert_eq!(report.matured, 0);
        assert_eq!(ledger.balance(&ALICE).unwrap().immature, 0);
        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 0);
    }

    #[tokio::test]
    async fn a_matured_balance_is_paid_in_one_batch() {
        let (ledger, chain, engine) = seeded();
        let tip = ledger.tip_sequence().unwrap().unwrap();
        engine
            .credit_block(
                "block-a",
                100,
                tip,
                ALICE,
                &target_from_leading_zero_bits(8),
            )
            .unwrap();
        chain.put_block(100, "block-a");
        chain.set_height(110);

        let report = engine.settle().await.unwrap();
        assert_eq!(report.matured, 1);
        assert_eq!(report.broadcast, 1);
        assert_eq!(report.value_broadcast, 1_000);

        assert_eq!(chain.broadcasts().len(), 1);
        assert_eq!(ledger.balance(&ALICE).unwrap().paid, 500);
        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 0);
    }

    #[tokio::test]
    async fn a_second_pass_does_not_pay_the_same_credits_again() {
        // The failure this guards is the expensive one. The first batch is
        // still open, so no second batch may start.
        let (ledger, chain, engine) = seeded();
        let tip = ledger.tip_sequence().unwrap().unwrap();
        engine
            .credit_block(
                "block-a",
                100,
                tip,
                ALICE,
                &target_from_leading_zero_bits(8),
            )
            .unwrap();
        chain.put_block(100, "block-a");
        chain.set_height(110);

        engine.settle().await.unwrap();
        engine.settle().await.unwrap();
        engine.settle().await.unwrap();

        assert_eq!(chain.broadcasts().len(), 1);
        assert_eq!(ledger.balance(&ALICE).unwrap().paid, 500);
    }

    #[tokio::test]
    async fn a_batch_confirms_once_its_nonce_is_consumed_and_buried() {
        let (ledger, chain, engine) = seeded();
        let tip = ledger.tip_sequence().unwrap().unwrap();
        engine
            .credit_block(
                "block-a",
                100,
                tip,
                ALICE,
                &target_from_leading_zero_bits(8),
            )
            .unwrap();
        chain.put_block(100, "block-a");
        chain.set_height(110);
        engine.settle().await.unwrap();

        // The treasury nonce advancing is the only evidence available that the
        // payout landed, and it is sufficient: the pool is the sole spender.
        chain.set_nonce(1);
        chain.set_height(111);
        assert_eq!(engine.settle().await.unwrap().confirmed, 0);

        chain.set_height(114);
        assert_eq!(engine.settle().await.unwrap().confirmed, 1);
        assert!(ledger.open_batches().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_reorged_payout_is_rebroadcast_byte_for_byte() {
        let (ledger, chain, engine) = seeded();
        let tip = ledger.tip_sequence().unwrap().unwrap();
        engine
            .credit_block(
                "block-a",
                100,
                tip,
                ALICE,
                &target_from_leading_zero_bits(8),
            )
            .unwrap();
        chain.put_block(100, "block-a");
        chain.set_height(110);
        engine.settle().await.unwrap();

        chain.set_nonce(1);
        chain.set_height(111);
        engine.settle().await.unwrap();

        // The reorg puts the nonce back, freeing the slot.
        chain.set_nonce(0);
        chain.set_height(112);
        engine.settle().await.unwrap();
        engine.settle().await.unwrap();

        let broadcasts = chain.broadcasts();
        assert_eq!(broadcasts.len(), 2, "the batch went back out");
        assert_eq!(
            broadcasts[0], broadcasts[1],
            "and it was the same bytes, not a fresh signature over the same nonce"
        );
        assert_eq!(ledger.balance(&ALICE).unwrap().paid, 500);
    }

    #[tokio::test]
    async fn an_unfunded_treasury_fails_the_batch_and_leaves_the_credits_alone() {
        let ledger = Arc::new(MemoryLedger::new());
        ledger.append_share(&ALICE, "rig", 100, 0).unwrap();

        let chain = Arc::new(FakeChain::with(100, 0));
        let engine = engine(Arc::clone(&ledger), Arc::clone(&chain), config());
        engine
            .credit_block("block-a", 100, 0, ALICE, &target_from_leading_zero_bits(8))
            .unwrap();
        chain.put_block(100, "block-a");
        chain.set_height(110);

        let error = engine.settle().await.unwrap_err();
        assert!(matches!(error, PoolError::Treasury(_)));

        // The miner is still owed. An empty treasury must not consume credits.
        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 1_000);
        assert!(chain.broadcasts().is_empty());
    }

    #[tokio::test]
    async fn a_balance_below_the_minimum_payout_waits() {
        // A payout smaller than the transaction carrying it destroys value on
        // the way to the miner.
        let ledger = Arc::new(MemoryLedger::new());
        ledger.append_share(&ALICE, "rig", 100, 0).unwrap();

        let chain = Arc::new(FakeChain::with(100, 1_000_000));
        let engine = engine(
            Arc::clone(&ledger),
            Arc::clone(&chain),
            PoolConfig {
                min_payout: 5_000,
                ..config()
            },
        );
        engine
            .credit_block("block-a", 100, 0, ALICE, &target_from_leading_zero_bits(8))
            .unwrap();
        chain.put_block(100, "block-a");
        chain.set_height(110);

        engine.settle().await.unwrap();
        assert!(chain.broadcasts().is_empty());
        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 1_000);
    }

    #[tokio::test]
    async fn a_batch_signed_but_never_broadcast_is_broadcast_on_the_next_pass() {
        // The crash-recovery path: the daemon died between `create_batch` and
        // the broadcast, so the batch exists, the credits are debited, and
        // nothing is on the network.
        let ledger = Arc::new(MemoryLedger::new());
        ledger.append_share(&ALICE, "rig", 100, 0).unwrap();

        let chain = Arc::new(FakeChain::with(100, 1_000_000));
        let engine = engine(Arc::clone(&ledger), Arc::clone(&chain), config());
        engine
            .credit_block("block-a", 100, 0, ALICE, &target_from_leading_zero_bits(8))
            .unwrap();
        ledger.mature_block("block-a").unwrap();

        let treasury = Treasury::from_key(generate_signing_key().unwrap());
        let entries = vec![PayoutEntry {
            miner: ALICE,
            amount: 1_000,
        }];
        let test_tag = custom_l1_node::core::ChainTag::from_genesis([0; 32]);
        let signed = treasury
            .sign_payout(&entries, 0, 1_000_000, &config(), &test_tag)
            .unwrap();
        let orphan_batch = PayoutBatch {
            id: ledger.next_batch_id().unwrap(),
            nonce: 0,
            entries,
            signed_tx: signed.to_bytes(),
            txid: None,
            state: PayoutState::Signed,
            created_at_millis: 0,
            included_height: None,
        };
        ledger.create_batch(&orphan_batch).unwrap();

        engine.settle().await.unwrap();

        assert_eq!(chain.broadcasts().len(), 1);
        assert_eq!(chain.broadcasts()[0], orphan_batch.signed_tx);
    }

    #[test]
    fn selection_clears_the_largest_debts_first_and_is_deterministic() {
        let ledger = Arc::new(MemoryLedger::new());
        let chain = Arc::new(FakeChain::with(1, 1_000));
        let engine = engine(
            Arc::clone(&ledger),
            chain,
            PoolConfig {
                max_outputs_per_batch: 2,
                ..config()
            },
        );

        for (miner, amount) in [(ALICE, 100u64), (BOB, 300), ([0xCC; 32], 200)] {
            ledger
                .record_block(
                    &FoundBlock {
                        id: hex::encode(miner),
                        height: 1,
                        sequence: 0,
                        reward: amount,
                        finder: miner,
                        found_at_millis: 0,
                        state: BlockState::Immature,
                    },
                    &[PayoutEntry { miner, amount }],
                )
                .unwrap();
            ledger.mature_block(&hex::encode(miner)).unwrap();
        }

        let entries = engine.select_entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].amount, 300);
        assert_eq!(entries[1].amount, 200);
        assert_eq!(entries, engine.select_entries().unwrap());
    }
}
