//! Trading operations against the block-execution overlay.
//!
//! The arithmetic lives in [`maya_dex`]. This is the part that moves balances,
//! writes records, and decides what happens when the engine says no.
//!
//! ## Two failure classes, and why they are not the same
//!
//! A transaction can be **wrong** — it names a pool that does not exist, its
//! route's legs do not join up, its asset symbol has a control character in it.
//! That is an `Err`, and because a failing transaction fails its whole block,
//! it is also a statement that no honest miner would have included it.
//!
//! A transaction can also merely **lose**. Its slippage bound was missed
//! because somebody else traded first; its arbitrage was taken by a faster
//! arbitrageur. That is not an error and must never be one: two traders racing
//! the same opportunity is the ordinary case, and if the loser's transaction
//! took the block down with it, every block carrying a competitive trade would
//! be invalid.
//!
//! So a lost trade is a **no-op**. The nonce advances, nothing moves, and the
//! record of what happened is that nothing did. Getting this wrong is the
//! difference between a DEX and a denial-of-service surface, which is why it is
//! stated here and repeated at each site.
//!
//! ## When a swap actually happens
//!
//! Not where it sits in the block. `StateDB::stage_swap` takes the trader's
//! input into escrow and adds them to that pool's batch; every batch settles in
//! `StateDB::settle_trading` once the whole block has been staged, at one
//! price per pool. See [`maya_dex::batch`] for what that buys and what it does
//! not.
//!
//! Routes are the exception and execute in place — they are the arbitrage path,
//! and there is nothing to net a multi-hop trade against.

use std::collections::BTreeMap;

use maya_dex::amm::Pool;
use maya_dex::batch::{MAX_BATCH_INTENTS, SwapIntent, clear_batch};
use maya_dex::book::{Book, Order, OrderId, Side, quote_for_base};
use maya_dex::error::DexError;
use maya_dex::fees::FeeSchedule;
use maya_dex::matching::{MatchLimits, match_book};
use maya_dex::types::{Direction, PairId};
use maya_governance::params::ParameterKey;

use crate::core::dex_payload::{
    AssetRegistration, AssetTransfer, LiquidityDeposit, LiquidityWithdrawal, OrderPlacement,
    PoolCreation, SwapRequest, SwapRoute,
};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::asset::{
    AssetId, AssetRecord, decode_balance, derive_asset_id, encode_balance, is_native,
    validate_symbol,
};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::dex::{
    OrderRecord, PoolRecord, SEQUENCE_KEY, asset_key, balance_key, book_prefix, canonical_pair,
    derive_intent_id, derive_lp_asset, derive_order_id, derive_pair_id, lp_asset_key,
    order_count_key, order_index_key, order_key, pool_key,
};
use crate::state::shielded::FEE_SINK;

/// Basis points of every pooled swap that go to the protocol, before
/// governance has said otherwise.
///
/// Zero, and it stays the *starting* value rather than the value: this is now
/// [`ParameterKey::DexProtocolFeeBps`]'s default, and the rate actually charged
/// is read from consensus state at execution time.
///
/// This constant used to say that nothing took a cut "because there is no
/// governance process that could have decided to. A protocol fee set by whoever
/// last edited a constant is not a protocol fee, it is a developer helping
/// themselves." That process now exists, so the sentence has been answered
/// rather than deleted — the rate is zero until a quorum, a strict majority,
/// and a timelock have all said otherwise, and it can never exceed the two
/// percent `maya_governance::params` fixes as its hard ceiling.
///
/// **Read at swap time, not baked into the pool.** The LP rate belongs to the
/// pool — its providers agreed to it when they deposited. The protocol rate
/// belongs to the chain. Storing the chain's rate inside each pool record would
/// mean a governance decision applied only to pools created afterwards, which
/// is not what "the protocol takes a cut" means.
pub const DEFAULT_PROTOCOL_FEE_BPS: u32 = 0;

/// Most resting orders one book may hold.
///
/// There is no fee market on this chain, so the cost of resting an order is one
/// transaction and the escrow it locks. Escrow is the real deterrent — an
/// attacker filling a book has to fund every order — but escrow does not bound
/// the *count*, and the count is what every node pays for in storage and in
/// book reconstruction. Four thousand and ninety-six per book is roughly the
/// mempool's own capacity, which is the rate at which orders can arrive anyway.
///
/// Governed by [`ParameterKey::DexMaxOrdersPerBook`]; this is its default.
pub const MAX_ORDERS_PER_BOOK: u64 = 4_096;

/// Most order fills one block may produce across every book.
///
/// A shared budget rather than a per-book one, because what has to be bounded
/// is the work a single block costs every node, and a per-book ceiling
/// multiplied by an unbounded number of books is not a ceiling.
///
/// Governed by [`ParameterKey::DexMaxFillsPerBlock`]; this is its default.
pub const MAX_FILLS_PER_BLOCK: usize = 1_024;

impl StateDB {
    // ---------------------------------------------------------------- reads

    /// Reads an asset's record from committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn get_asset(&self, asset: &AssetId) -> Result<Option<AssetRecord>> {
        match self.raw_get(&asset_key(asset))? {
            Some(bytes) => Ok(Some(AssetRecord::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Reads a pool's record from committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn get_pool(&self, pair: &PairId) -> Result<Option<PoolRecord>> {
        match self.raw_get(&pool_key(pair))? {
            Some(bytes) => Ok(Some(PoolRecord::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Reads a resting order from committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn get_order(&self, id: &OrderId) -> Result<Option<OrderRecord>> {
        let Some(key) = self.raw_get(&order_index_key(id))? else {
            return Ok(None);
        };
        match self.raw_get(&key)? {
            Some(bytes) => Ok(Some(OrderRecord::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Units of `asset` held by `address` in committed state.
    ///
    /// The native coin is read from the account record, not from the
    /// multi-asset keyspace — it has exactly one home. See
    /// [`crate::state::asset`].
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn asset_balance(&self, asset: &AssetId, address: &Address) -> Result<u64> {
        if is_native(asset) {
            return Ok(self.get_account(address)?.balance);
        }
        match self.raw_get(&balance_key(asset, address))? {
            Some(bytes) => decode_balance(&bytes),
            None => Ok(0),
        }
    }

    // ------------------------------------------------------- overlay access

    /// Reads a balance through the overlay.
    fn load_asset_balance(
        &self,
        overlay: &Overlay,
        asset: &AssetId,
        address: &Address,
    ) -> Result<u64> {
        if is_native(asset) {
            return Ok(match overlay.accounts.get(address) {
                Some(account) => account.balance,
                None => self.get_account(address)?.balance,
            });
        }
        match self.record(overlay, &balance_key(asset, address))? {
            Some(bytes) => decode_balance(&bytes),
            None => Ok(0),
        }
    }

    /// Writes a balance through the overlay.
    fn store_balance(
        &self,
        overlay: &mut Overlay,
        asset: &AssetId,
        address: &Address,
        amount: u64,
    ) -> Result<()> {
        if is_native(asset) {
            let mut account = match overlay.accounts.get(address) {
                Some(existing) => *existing,
                None => self.get_account(address)?,
            };
            account.balance = amount;
            overlay.accounts.insert(*address, account);
            return Ok(());
        }

        let key = balance_key(asset, address);
        if amount == 0 {
            // An absent key and a stored zero must not both be reachable, or
            // two states with identical balances would have different roots.
            Self::delete_record(overlay, key);
        } else {
            Self::put_record(overlay, key, encode_balance(amount).to_vec());
        }
        Ok(())
    }

    /// Credits `amount` of `asset` to `address`.
    fn credit_asset(
        &self,
        overlay: &mut Overlay,
        asset: &AssetId,
        address: &Address,
        amount: u64,
    ) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        let balance = self.load_asset_balance(overlay, asset, address)?;
        let updated =
            maya_ledger_math::credit(balance, amount).ok_or(NodeError::BalanceOverflow)?;
        self.store_balance(overlay, asset, address, updated)
    }

    /// Debits `amount` of `asset` from `address`.
    fn debit_asset(
        &self,
        overlay: &mut Overlay,
        asset: &AssetId,
        address: &Address,
        amount: u64,
    ) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        let balance = self.load_asset_balance(overlay, asset, address)?;
        let updated = maya_ledger_math::debit(balance, amount).ok_or_else(|| {
            NodeError::InsufficientAssetBalance {
                asset: hex::encode(asset),
                address: hex::encode(address),
                required: amount,
                available: balance,
            }
        })?;
        self.store_balance(overlay, asset, address, updated)
    }

    /// Applies the chain's current protocol rate to a pool.
    ///
    /// The stored record carries the LP rate its providers agreed to and a
    /// zero protocol rate. The chain's cut is read here, at execution, so a
    /// governance decision reaches every pool rather than only the ones
    /// created after it.
    fn priced(&self, overlay: &Overlay, record: &PoolRecord) -> Result<PoolRecord> {
        let protocol_bps = u32::try_from(self.parameter(overlay, ParameterKey::DexProtocolFeeBps)?)
            .unwrap_or(u32::MAX);

        let fees = FeeSchedule::new(record.pool.fees.lp_bps, protocol_bps)
            .map_err(|error| trade_error("pool pricing", error))?;
        Ok(PoolRecord {
            pool: Pool {
                fees,
                ..record.pool
            },
            ..*record
        })
    }

    /// Reads a pool through the overlay, requiring that it exists.
    fn require_pool(&self, overlay: &Overlay, pair: &PairId) -> Result<PoolRecord> {
        match self.record(overlay, &pool_key(pair))? {
            Some(bytes) => PoolRecord::decode(&bytes),
            None => Err(NodeError::UnknownPool(hex::encode(pair))),
        }
    }

    /// Writes a pool through the overlay.
    fn store_pool(overlay: &mut Overlay, pair: &PairId, record: &PoolRecord) {
        Self::put_record(overlay, pool_key(pair), record.encode());
    }

    /// Whether an asset can be held: registered, or a pool's share asset.
    fn asset_exists(&self, overlay: &Overlay, asset: &AssetId) -> Result<bool> {
        if is_native(asset) {
            return Ok(true);
        }
        if self.record(overlay, &asset_key(asset))?.is_some() {
            return Ok(true);
        }
        Ok(self.record(overlay, &lp_asset_key(asset))?.is_some())
    }

    /// Consumes the next order sequence number.
    ///
    /// Monotonic across the whole chain. It only has to be unique and agreed
    /// upon, and one counter is one number to get right; a counter per book
    /// would be a hundred.
    fn next_sequence(&self, overlay: &mut Overlay) -> Result<u64> {
        let current = match self.record(overlay, SEQUENCE_KEY)? {
            Some(bytes) => decode_balance(&bytes)?,
            None => 0,
        };
        let next = current.checked_add(1).ok_or(NodeError::BalanceOverflow)?;
        Self::put_record(
            overlay,
            SEQUENCE_KEY.to_vec(),
            encode_balance(next).to_vec(),
        );
        Ok(current)
    }

    /// Adjusts a book's resting order count.
    fn adjust_order_count(&self, overlay: &mut Overlay, pair: &PairId, delta: i64) -> Result<u64> {
        let key = order_count_key(pair);
        let current = match self.record(overlay, &key)? {
            Some(bytes) => decode_balance(&bytes)?,
            None => 0,
        };
        let updated = if delta >= 0 {
            current
                .checked_add(delta.unsigned_abs())
                .ok_or(NodeError::BalanceOverflow)?
        } else {
            // Saturating rather than checked: a count that has drifted below
            // the number of orders is a bug worth not compounding, and
            // refusing the cancellation would strand the escrow.
            current.saturating_sub(delta.unsigned_abs())
        };

        if updated == 0 {
            Self::delete_record(overlay, key);
        } else {
            Self::put_record(overlay, key, encode_balance(updated).to_vec());
        }
        Ok(updated)
    }

    /// Rejects a transaction whose deadline has passed.
    ///
    /// The defence against a held transaction: a miner who sits on a swap for
    /// an hour cannot then execute it against a market that has moved.
    fn check_deadline(deadline: u64, context: BlockContext) -> Result<()> {
        if deadline != 0 && context.height > deadline {
            return Err(NodeError::DeadlineExpired {
                deadline,
                height: context.height,
            });
        }
        Ok(())
    }

    // ---------------------------------------------------------- assets

    /// Registers an asset, crediting its whole supply to `sender`.
    pub(crate) fn register_asset(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        registration: &AssetRegistration,
    ) -> Result<()> {
        validate_symbol(&registration.symbol)?;
        if registration.total_supply == 0 {
            return Err(NodeError::InvalidAssetSymbol {
                reason: "an asset with no supply would be a name and nothing else".to_string(),
            });
        }

        let asset = derive_asset_id(sender, nonce, &registration.symbol);
        if self.record(overlay, &asset_key(&asset))?.is_some() {
            return Err(NodeError::AssetExists(hex::encode(asset)));
        }

        let record = AssetRecord {
            creator: *sender,
            total_supply: registration.total_supply,
            symbol: registration.symbol,
        };
        Self::put_record(overlay, asset_key(&asset), record.encode().to_vec());
        self.credit_asset(overlay, &asset, sender, registration.total_supply)
    }

    /// Moves units of a non-native asset.
    pub(crate) fn transfer_asset(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        transfer: &AssetTransfer,
    ) -> Result<()> {
        if is_native(&transfer.asset) {
            return Err(NodeError::NativeAssetTransfer);
        }
        if transfer.amount == 0 {
            return Err(NodeError::InsufficientAssetBalance {
                asset: hex::encode(transfer.asset),
                address: hex::encode(sender),
                required: 1,
                available: 0,
            });
        }
        if !self.asset_exists(overlay, &transfer.asset)? {
            return Err(NodeError::UnknownAsset(hex::encode(transfer.asset)));
        }

        // Debit first, so a transfer to oneself nets to zero rather than
        // minting — the same ordering rule the native transfer path uses.
        self.debit_asset(overlay, &transfer.asset, sender, transfer.amount)?;
        self.credit_asset(
            overlay,
            &transfer.asset,
            &transfer.recipient,
            transfer.amount,
        )
    }

    // ----------------------------------------------------------- liquidity

    /// Creates a pool and seeds it in one operation.
    pub(crate) fn create_pool(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        creation: &PoolCreation,
    ) -> Result<()> {
        let (base_asset, quote_asset) = canonical_pair(&creation.asset_a, &creation.asset_b)?;
        // The sender named their two amounts against their own ordering of the
        // assets; the pair has its own. Mapping here rather than asking the
        // sender to know the canonical order is what keeps a mistake about it
        // from being a deposit at the wrong ratio.
        let (base_amount, quote_amount) = if base_asset == creation.asset_a {
            (creation.amount_a, creation.amount_b)
        } else {
            (creation.amount_b, creation.amount_a)
        };

        for asset in [&base_asset, &quote_asset] {
            if !self.asset_exists(overlay, asset)? {
                return Err(NodeError::UnknownAsset(hex::encode(asset)));
            }
        }

        // The pool stores its own LP rate and a zero protocol rate. The
        // chain's cut is not the pool's to remember — it is read from the
        // parameter table at swap time, so a governance decision reaches every
        // pool rather than only the ones created after it.
        let fees = FeeSchedule::new(creation.lp_fee_bps, DEFAULT_PROTOCOL_FEE_BPS)
            .map_err(|error| trade_error("pool creation", error))?;
        let pair = derive_pair_id(&base_asset, &quote_asset, creation.lp_fee_bps);
        if self.record(overlay, &pool_key(&pair))?.is_some() {
            return Err(NodeError::PoolExists(hex::encode(pair)));
        }

        let delta = Pool::empty(fees)
            .add_liquidity(base_amount, quote_amount)
            .map_err(|error| trade_error("pool creation", error))?;

        self.debit_asset(overlay, &base_asset, sender, delta.base)?;
        self.debit_asset(overlay, &quote_asset, sender, delta.quote)?;

        let lp_asset = derive_lp_asset(&pair);
        // The share asset has no registration transaction, so this index is
        // what tells it apart from thirty-two bytes somebody invented.
        Self::put_record(overlay, lp_asset_key(&lp_asset), pair.to_vec());
        self.credit_asset(overlay, &lp_asset, sender, delta.shares)?;

        Self::store_pool(
            overlay,
            &pair,
            &PoolRecord {
                base_asset,
                quote_asset,
                pool: delta.pool,
            },
        );
        Ok(())
    }

    /// Deposits into an existing pool.
    pub(crate) fn add_liquidity(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        deposit: &LiquidityDeposit,
    ) -> Result<()> {
        let mut record = self.require_pool(overlay, &deposit.pair)?;
        let delta = record
            .pool
            .add_liquidity(deposit.base_desired, deposit.quote_desired)
            .map_err(|error| trade_error("deposit", error))?;

        // A deposit executes in place, so its bound is an error rather than a
        // no-op: there is no later settlement stage at which it could be
        // dropped, and the alternative is taking the assets anyway.
        if delta.shares < deposit.min_shares {
            return Err(NodeError::SlippageExceeded {
                what: "liquidity shares",
                expected: deposit.min_shares,
                actual: delta.shares,
            });
        }

        self.debit_asset(overlay, &record.base_asset, sender, delta.base)?;
        self.debit_asset(overlay, &record.quote_asset, sender, delta.quote)?;
        self.credit_asset(
            overlay,
            &derive_lp_asset(&deposit.pair),
            sender,
            delta.shares,
        )?;

        record.pool = delta.pool;
        Self::store_pool(overlay, &deposit.pair, &record);
        Ok(())
    }

    /// Redeems shares for a slice of both reserves.
    pub(crate) fn remove_liquidity(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        withdrawal: &LiquidityWithdrawal,
    ) -> Result<()> {
        let mut record = self.require_pool(overlay, &withdrawal.pair)?;
        let delta = record
            .pool
            .remove_liquidity(withdrawal.shares)
            .map_err(|error| trade_error("withdrawal", error))?;

        if delta.base < withdrawal.min_base {
            return Err(NodeError::SlippageExceeded {
                what: "base withdrawn",
                expected: withdrawal.min_base,
                actual: delta.base,
            });
        }
        if delta.quote < withdrawal.min_quote {
            return Err(NodeError::SlippageExceeded {
                what: "quote withdrawn",
                expected: withdrawal.min_quote,
                actual: delta.quote,
            });
        }

        // Burn first. Crediting before the burn would let a redemption of more
        // shares than the sender holds pay out before it failed.
        self.debit_asset(
            overlay,
            &derive_lp_asset(&withdrawal.pair),
            sender,
            withdrawal.shares,
        )?;
        self.credit_asset(overlay, &record.base_asset, sender, delta.base)?;
        self.credit_asset(overlay, &record.quote_asset, sender, delta.quote)?;

        record.pool = delta.pool;
        Self::store_pool(overlay, &withdrawal.pair, &record);
        Ok(())
    }

    // --------------------------------------------------------------- swaps

    /// Takes a swap into escrow and adds it to its pool's batch.
    ///
    /// Nothing is priced here. The batch settles in
    /// [`StateDB::settle_trading`], after every transaction in the block has
    /// been staged, at one price for everyone — which is the whole reason a
    /// swap does not execute where it sits.
    pub(crate) fn stage_swap(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        request: &SwapRequest,
        context: BlockContext,
    ) -> Result<()> {
        Self::check_deadline(request.deadline, context)?;

        let direction = Direction::from_tag(request.direction).ok_or_else(|| {
            NodeError::Decode(format!("unknown swap direction {}", request.direction))
        })?;
        if request.amount_in == 0 {
            return Err(NodeError::Trade {
                operation: "swap",
                reason: DexError::ZeroAmount.to_string(),
            });
        }

        let record = self.require_pool(overlay, &request.pair)?;
        let staged = overlay.swaps.entry(request.pair).or_default();
        if staged.len() >= MAX_BATCH_INTENTS {
            return Err(NodeError::TooManySwaps {
                pair: hex::encode(request.pair),
                limit: MAX_BATCH_INTENTS,
            });
        }

        staged.push(SwapIntent {
            id: derive_intent_id(sender, nonce),
            trader: *sender,
            direction,
            amount_in: request.amount_in,
            min_out: request.min_out,
        });
        overlay.touched_pairs.insert(request.pair);

        // The input leaves the trader now and is held by the batch. Taking it
        // at settlement instead would mean a trader could be in two batches at
        // once with one balance.
        let input = input_asset(&record, direction);
        self.debit_asset(overlay, &input, sender, request.amount_in)
    }

    /// Executes an atomic multi-hop swap.
    ///
    /// Every leg is computed before anything is written, so a route that misses
    /// its bound leaves no trace at all. It is a no-op rather than an error for
    /// the reason at the top of this module: two arbitrageurs racing one
    /// opportunity is normal, and the loser must not invalidate the block.
    pub(crate) fn execute_route(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        route: &SwapRoute,
        context: BlockContext,
    ) -> Result<()> {
        Self::check_deadline(route.deadline, context)?;
        if route.amount_in == 0 {
            return Err(NodeError::Trade {
                operation: "route",
                reason: DexError::ZeroAmount.to_string(),
            });
        }

        // Computed against copies. Nothing below writes until every leg has
        // succeeded and the bound has been met.
        let mut pools: Vec<(PairId, PoolRecord)> = Vec::with_capacity(route.legs.len());
        let mut fees: Vec<(AssetId, u64)> = Vec::new();
        // What the route ends up holding after the leg just executed, and what
        // it started from. The second is not derivable from the first once the
        // path has moved on, and the sender is debited in it.
        let mut current_asset: Option<AssetId> = None;
        let mut route_input: Option<AssetId> = None;
        let mut amount = route.amount_in;

        for (index, leg) in route.legs.iter().enumerate() {
            let direction = Direction::from_tag(leg.direction).ok_or_else(|| {
                NodeError::Decode(format!("unknown swap direction {}", leg.direction))
            })?;

            // A leg may hit a pool an earlier leg already moved — a route that
            // revisits a pool is legal and prices the second visit against the
            // first, which is what makes a two-leg round trip lose money
            // rather than being free.
            let mut record =
                if let Some((_, record)) = pools.iter().find(|(pair, _)| *pair == leg.pair) {
                    *record
                } else {
                    let stored = self.require_pool(overlay, &leg.pair)?;
                    self.priced(overlay, &stored)?
                };

            let input = input_asset(&record, direction);
            match current_asset {
                None => route_input = Some(input),
                Some(expected) if expected == input => {}
                Some(_) => return Err(NodeError::RouteDiscontinuity { leg: index }),
            }

            let outcome = match record.pool.swap_exact_in(direction, amount) {
                Ok(outcome) => outcome,
                // A leg the curve cannot price is a route that does not pay,
                // not a malformed transaction.
                Err(_) => return Ok(()),
            };

            if outcome.protocol_fee > 0 {
                fees.push((input, outcome.protocol_fee));
            }
            record.pool = outcome.pool;
            amount = outcome.amount_out;
            current_asset = Some(output_asset(&record, direction));

            match pools.iter_mut().find(|(pair, _)| *pair == leg.pair) {
                Some(slot) => slot.1 = record,
                None => pools.push((leg.pair, record)),
            }
        }

        if amount < route.min_out {
            // Lost the race. Nothing has been written, and nothing will be.
            return Ok(());
        }

        // Both are set by the first leg, and the decoder rejects a route with
        // no legs, so neither can be absent here.
        let (Some(final_asset), Some(first_asset)) = (current_asset, route_input) else {
            return Err(NodeError::RouteDiscontinuity { leg: 0 });
        };

        self.debit_asset(overlay, &first_asset, sender, route.amount_in)?;
        self.credit_asset(overlay, &final_asset, sender, amount)?;
        for (asset, fee) in &fees {
            self.credit_asset(overlay, asset, &FEE_SINK, *fee)?;
        }
        for (pair, record) in &pools {
            // The chain's protocol rate is never written into a stored pool:
            // it belongs to the parameter table, and a copy in each record
            // would outlive the decision that set it.
            let stored = PoolRecord {
                pool: Pool {
                    fees: FeeSchedule::new(record.pool.fees.lp_bps, DEFAULT_PROTOCOL_FEE_BPS)
                        .map_err(|error| trade_error("pool pricing", error))?,
                    ..record.pool
                },
                ..*record
            };
            Self::store_pool(overlay, pair, &stored);
        }
        Ok(())
    }

    // -------------------------------------------------------------- orders

    /// Rests a limit order, taking its escrow.
    pub(crate) fn place_order(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        placement: &OrderPlacement,
        context: BlockContext,
    ) -> Result<()> {
        let side = Side::from_tag(placement.side)
            .ok_or_else(|| NodeError::Decode(format!("unknown order side {}", placement.side)))?;
        if placement.price == 0 {
            return Err(NodeError::Trade {
                operation: "order placement",
                reason: DexError::ZeroPrice.to_string(),
            });
        }
        if placement.amount == 0 {
            return Err(NodeError::Trade {
                operation: "order placement",
                reason: DexError::ZeroAmount.to_string(),
            });
        }
        if placement.expiry != 0 && placement.expiry < context.height {
            return Err(NodeError::DeadlineExpired {
                deadline: placement.expiry,
                height: context.height,
            });
        }

        let record = self.require_pool(overlay, &placement.pair)?;
        let id = derive_order_id(sender, nonce);
        if self.record(overlay, &order_index_key(&id))?.is_some() {
            return Err(NodeError::Trade {
                operation: "order placement",
                reason: "an order with this identifier is already resting".to_string(),
            });
        }

        let ceiling = self.parameter(overlay, ParameterKey::DexMaxOrdersPerBook)?;
        let count = self.adjust_order_count(overlay, &placement.pair, 1)?;
        if count > ceiling {
            return Err(NodeError::Trade {
                operation: "order placement",
                reason: format!("book already holds {ceiling} orders"),
            });
        }

        // A bid escrows quote, rounded up; an ask escrows the base it is
        // selling. Rounding up is what guarantees the escrow can cover every
        // fill the order can produce.
        let (escrow_asset, escrow) = match side {
            Side::Bid => (
                record.quote_asset,
                quote_for_base(placement.amount, placement.price, Side::Bid)
                    .map_err(|error| trade_error("order placement", error))?,
            ),
            Side::Ask => (record.base_asset, placement.amount),
        };
        if escrow == 0 {
            return Err(NodeError::Trade {
                operation: "order placement",
                reason: "order is too small to escrow anything".to_string(),
            });
        }
        self.debit_asset(overlay, &escrow_asset, sender, escrow)?;

        let sequence = self.next_sequence(overlay)?;
        let order = OrderRecord {
            pair: placement.pair,
            order: Order {
                id,
                owner: *sender,
                side,
                price: placement.price,
                amount: placement.amount,
                remaining: placement.amount,
                sequence,
                expiry: placement.expiry,
            },
            escrow,
        };

        let key = order_key(&placement.pair, side, placement.price, sequence);
        Self::put_record(overlay, order_index_key(&id), key.clone());
        Self::put_record(overlay, key, order.encode());
        overlay.touched_pairs.insert(placement.pair);
        Ok(())
    }

    /// Withdraws a resting order and refunds what is left of its escrow.
    pub(crate) fn cancel_order(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        id: &OrderId,
    ) -> Result<()> {
        let index = order_index_key(id);
        let Some(key) = self.record(overlay, &index)? else {
            return Err(NodeError::UnknownOrder(hex::encode(id)));
        };
        let Some(bytes) = self.record(overlay, &key)? else {
            return Err(NodeError::UnknownOrder(hex::encode(id)));
        };
        let record = OrderRecord::decode(&bytes)?;

        if record.order.owner != *sender {
            return Err(NodeError::NotOrderOwner {
                order: hex::encode(id),
                owner: hex::encode(record.order.owner),
                claimant: hex::encode(sender),
            });
        }

        let pool = self.require_pool(overlay, &record.pair)?;
        self.refund_order(overlay, &pool, &record)?;
        Self::delete_record(overlay, key);
        Self::delete_record(overlay, index);
        self.adjust_order_count(overlay, &record.pair, -1)?;
        Ok(())
    }

    /// Returns an order's remaining escrow to its owner.
    fn refund_order(
        &self,
        overlay: &mut Overlay,
        pool: &PoolRecord,
        record: &OrderRecord,
    ) -> Result<()> {
        let asset = record.escrow_asset(pool);
        self.credit_asset(overlay, &asset, &record.order.owner, record.escrow)
    }

    // ------------------------------------------------------- end of block

    /// Settles every pool the block touched: batches first, then books.
    ///
    /// Pools are visited in identifier order, which is a property of the set
    /// and not of the block, so no arrangement of transactions changes the
    /// order this runs in.
    pub(crate) fn settle_trading(
        &self,
        overlay: &mut Overlay,
        context: BlockContext,
    ) -> Result<()> {
        let pairs: Vec<PairId> = overlay.touched_pairs.iter().copied().collect();
        for pair in pairs {
            self.settle_batch(overlay, &pair)?;
            self.settle_book(overlay, &pair, context)?;
        }
        Ok(())
    }

    /// Clears one pool's swaps at a single price.
    fn settle_batch(&self, overlay: &mut Overlay, pair: &PairId) -> Result<()> {
        let Some(intents) = overlay.swaps.remove(pair) else {
            return Ok(());
        };
        if intents.is_empty() {
            return Ok(());
        }

        let mut record = self.require_pool(overlay, pair)?;

        // Priced with the chain's current protocol rate, which the stored
        // record deliberately does not carry.
        let priced = self.priced(overlay, &record)?;
        let outcome = match clear_batch(&priced.pool, &intents) {
            Ok(outcome) => outcome,
            // The pool cannot price this batch at all — it has been drained of
            // liquidity, most likely by a withdrawal in the same block. Every
            // trader gets their input back. Refusing the block instead would
            // let a withdrawal invalidate everybody else's transactions.
            Err(_) => {
                for intent in &intents {
                    let asset = input_asset(&record, intent.direction);
                    self.credit_asset(overlay, &asset, &intent.trader, intent.amount_in)?;
                }
                return Ok(());
            }
        };

        for trade in &outcome.cleared {
            let asset = output_asset(&record, trade.direction);
            self.credit_asset(overlay, &asset, &trade.trader, trade.amount_out)?;
        }

        // A skipped intent is a no-op: the input it was holding goes straight
        // back. Its transaction is still valid and still spent its nonce.
        for id in &outcome.skipped {
            let Some(intent) = intents.iter().find(|intent| intent.id == *id) else {
                continue;
            };
            let asset = input_asset(&record, intent.direction);
            self.credit_asset(overlay, &asset, &intent.trader, intent.amount_in)?;
        }

        if let (Some(direction), true) = (outcome.protocol_fee_direction, outcome.protocol_fee > 0)
        {
            let asset = input_asset(&record, direction);
            self.credit_asset(overlay, &asset, &FEE_SINK, outcome.protocol_fee)?;
        }

        // The reserves move; the stored fee schedule stays the pool's own, so
        // the chain's rate is never written into a record that outlives the
        // decision that set it.
        record.pool = Pool {
            fees: record.pool.fees,
            ..outcome.pool
        };
        Self::store_pool(overlay, pair, &record);
        Ok(())
    }

    /// Crosses one book and settles the fills.
    fn settle_book(
        &self,
        overlay: &mut Overlay,
        pair: &PairId,
        context: BlockContext,
    ) -> Result<()> {
        let records = self.load_order_records(overlay, pair)?;
        if records.is_empty() {
            return Ok(());
        }

        let pool = self.require_pool(overlay, pair)?;
        let mut book = Book::new();
        for record in records.values() {
            book.insert(record.order)
                .map_err(|error| trade_error("book reconstruction", error))?;
        }

        let ceiling = usize::try_from(self.parameter(overlay, ParameterKey::DexMaxFillsPerBlock)?)
            .unwrap_or(usize::MAX);
        let budget = ceiling.saturating_sub(overlay.fills);
        if budget == 0 {
            return Ok(());
        }
        let limits = MatchLimits {
            // The ceiling is a whole-block budget, so a book reached late in
            // the pass gets what is left rather than a fresh allowance.
            max_fills: u32::try_from(budget.min(u32::MAX as usize)).unwrap_or(u32::MAX),
            height: context.height,
        };

        let outcome = match_book(&book, pool.pool.fees, limits)
            .map_err(|error| trade_error("matching", error))?;
        overlay.fills = overlay.fills.saturating_add(outcome.fills.len());

        // Working copies. Escrow is decremented fill by fill, then whatever
        // survives is either rewritten with the order or refunded with it.
        let mut working = records.clone();

        for fill in &outcome.fills {
            let bid_base = fill.base_to_bidder();
            let ask_quote = fill.quote_to_asker();

            self.credit_asset(overlay, &pool.base_asset, &fill.bid_owner, bid_base)?;
            self.credit_asset(overlay, &pool.quote_asset, &fill.ask_owner, ask_quote)?;

            let fee_asset = match fill.maker_side {
                Side::Ask => pool.base_asset,
                Side::Bid => pool.quote_asset,
            };
            self.credit_asset(overlay, &fee_asset, &FEE_SINK, fill.taker_fee)?;

            // The bid's escrow paid the quote; the ask's paid the base. Both
            // are decremented by exactly what left them, so what remains is
            // exact rather than recomputed.
            if let Some(record) = working.get_mut(&fill.bid_id) {
                record.escrow = record.escrow.saturating_sub(fill.quote);
            }
            if let Some(record) = working.get_mut(&fill.ask_id) {
                record.escrow = record.escrow.saturating_sub(fill.base);
            }
        }

        let surviving: BTreeMap<OrderId, Order> = outcome
            .book
            .orders()
            .into_iter()
            .map(|order| (order.id, order))
            .collect();

        for (id, mut record) in working {
            if let Some(order) = surviving.get(&id) {
                if order.remaining == record.order.remaining {
                    // Untouched: leave the stored record exactly as it is,
                    // so a block that matched nothing writes nothing.
                    continue;
                }
                record.order.remaining = order.remaining;
                Self::put_record(
                    overlay,
                    order_key(
                        pair,
                        record.order.side,
                        record.order.price,
                        record.order.sequence,
                    ),
                    record.encode(),
                );
            } else {
                // Filled out, or expired. Either way it is gone and
                // whatever escrow it still holds belongs to its owner —
                // a bid that executed at a maker's better price has some.
                self.refund_order(overlay, &pool, &record)?;
                Self::delete_record(
                    overlay,
                    order_key(
                        pair,
                        record.order.side,
                        record.order.price,
                        record.order.sequence,
                    ),
                );
                Self::delete_record(overlay, order_index_key(&id));
                self.adjust_order_count(overlay, pair, -1)?;
            }
        }

        Ok(())
    }

    /// Every resting order on one pair, read through the overlay.
    ///
    /// The committed scan is merged with the block's own writes, so an order
    /// placed and an order cancelled earlier in the same block are both visible
    /// to the matching pass. Reading only committed state would match against a
    /// book that no longer exists.
    fn load_order_records(
        &self,
        overlay: &Overlay,
        pair: &PairId,
    ) -> Result<BTreeMap<OrderId, OrderRecord>> {
        let prefix = book_prefix(pair);
        let mut records = BTreeMap::new();

        for (key, value) in self.scan_prefix(&prefix)? {
            if overlay.records.contains_key(&key) {
                // Superseded below by whatever this block did to it.
                continue;
            }
            let record = OrderRecord::decode(&value)?;
            records.insert(record.order.id, record);
        }

        for (key, value) in &overlay.records {
            if !key.starts_with(&prefix) {
                continue;
            }
            if let Some(bytes) = value {
                let record = OrderRecord::decode(bytes)?;
                records.insert(record.order.id, record);
            }
        }

        Ok(records)
    }
}

/// The asset a trade in `direction` supplies to the pool.
fn input_asset(record: &PoolRecord, direction: Direction) -> AssetId {
    match direction {
        Direction::BaseToQuote => record.base_asset,
        Direction::QuoteToBase => record.quote_asset,
    }
}

/// The asset a trade in `direction` receives from the pool.
fn output_asset(record: &PoolRecord, direction: Direction) -> AssetId {
    match direction {
        Direction::BaseToQuote => record.quote_asset,
        Direction::QuoteToBase => record.base_asset,
    }
}

/// Wraps an engine refusal with the operation that provoked it.
fn trade_error(operation: &'static str, error: DexError) -> NodeError {
    NodeError::Trade {
        operation,
        reason: error.to_string(),
    }
}
