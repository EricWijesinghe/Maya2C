//! Charging fees per transaction and settling them per block (ADR-029).
//! The rules and the record are in [`crate::state::fees`].

use primitive_types::U256;

use crate::core::{Transaction, TxKind};
use crate::error::{NodeError, Result};
use crate::state::db::{Overlay, StateDB};
use crate::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord};
use crate::state::shielded::FEE_SINK;

impl StateDB {
    /// The fee record through `overlay`, or `None` where fees are off.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub(crate) fn fee_record(&self, overlay: &Overlay) -> Result<Option<FeeRecord>> {
        self.record(overlay, FEE_KEY)?
            .map(|bytes| FeeRecord::decode(&bytes))
            .transpose()
    }

    /// The committed fee record, for RPC, wallets and the mempool.
    ///
    /// # Errors
    ///
    /// As [`StateDB::fee_record`].
    pub fn committed_fees(&self) -> Result<Option<FeeRecord>> {
        self.raw_get(FEE_KEY)?
            .map(|bytes| FeeRecord::decode(&bytes))
            .transpose()
    }

    /// Checks `tx` pays the base fee on its size and books the burn. The
    /// payment itself moves as an ordinary output to the collector.
    ///
    /// # Errors
    ///
    /// [`NodeError::FeeTooLow`] if the collector outputs fall short.
    pub(crate) fn charge_fee(&self, overlay: &mut Overlay, tx: &Transaction) -> Result<()> {
        let Some(fees) = self.fee_record(overlay)? else {
            return Ok(());
        };
        let size = tx.to_bytes().len();
        // A mining share that moves nothing pays no base fee: a new miner
        // holds no coin, and Bitcoin's first miners paid none either. Its bytes
        // still count toward the base-fee step, and the per-block share cap
        // bounds how many there can be (ADR-044).
        let required = if matches!(tx.kind, TxKind::MiningShare(_)) && tx.outputs.is_empty() {
            U256::zero()
        } else {
            U256::from(fees.required(size))
        };
        // Saturating: the outputs already passed the transaction's checked
        // output total, so a sum past 2^256 cannot reach here.
        let offered = tx
            .outputs
            .iter()
            .filter(|o| o.recipient == FEE_COLLECTOR)
            .fold(U256::zero(), |sum, o| sum.saturating_add(o.amount));
        if offered < required {
            return Err(NodeError::FeeTooLow { required, offered });
        }
        overlay.fee_burn = overlay
            .fee_burn
            .checked_add(required)
            .ok_or(NodeError::BalanceOverflow)?;
        overlay.fee_bytes = overlay
            .fee_bytes
            .saturating_add(u64::try_from(size).unwrap_or(u64::MAX));
        Ok(())
    }

    /// End of block: burns the base fee the block owed and steps the base fee.
    ///
    /// # Errors
    ///
    /// A damaged record, or a collector holding less than the burn — which
    /// would mean a fee was booked without being paid, and fails the block.
    pub(crate) fn settle_fees(&self, overlay: &mut Overlay) -> Result<()> {
        let Some(fees) = self.fee_record(overlay)? else {
            return Ok(());
        };
        let burn = overlay.fee_burn;
        if !burn.is_zero() {
            let mut collector = self.load(overlay, &FEE_COLLECTOR)?;
            collector.balance =
                collector
                    .balance
                    .checked_sub(burn)
                    .ok_or_else(|| {
                        NodeError::InvariantViolation("fee burn exceeds what was collected".into())
                    })?;
            overlay.accounts.insert(FEE_COLLECTOR, collector);
            let mut sink = self.load(overlay, &FEE_SINK)?;
            sink.balance = sink
                .balance
                .checked_add(burn)
                .ok_or(NodeError::BalanceOverflow)?;
            overlay.accounts.insert(FEE_SINK, sink);
        }
        let next = fees.next(overlay.fee_bytes);
        overlay
            .records
            .insert(FEE_KEY.to_vec(), Some(next.encode()));
        Ok(())
    }

    /// Empties the collector — the tips since the last epoch — for the
    /// staking reward pool. Zero where fees are off.
    ///
    /// # Errors
    ///
    /// Storage failure.
    pub(crate) fn take_fee_pool(&self, overlay: &mut Overlay) -> Result<U256> {
        if self.fee_record(overlay)?.is_none() {
            return Ok(U256::zero());
        }
        let mut collector = self.load(overlay, &FEE_COLLECTOR)?;
        let pool = collector.balance;
        collector.balance = U256::zero();
        overlay.accounts.insert(FEE_COLLECTOR, collector);
        // The whole collector, 256-bit (ADR-046): the staking reward pool is
        // an Amount, so nothing is truncated.
        Ok(pool)
    }
}
