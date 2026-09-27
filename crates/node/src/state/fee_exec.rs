//! Charging fees per transaction and settling them per block (ADR-029).
//! The rules and the record are in [`crate::state::fees`].

use crate::core::Transaction;
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
        let required = fees.required(size);
        let offered: u128 = tx
            .outputs
            .iter()
            .filter(|o| o.recipient == FEE_COLLECTOR)
            .map(|o| u128::from(o.amount))
            .sum();
        if offered < required {
            return Err(NodeError::FeeTooLow { required, offered });
        }
        overlay.fee_burn += required;
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
        let burn = u64::try_from(overlay.fee_burn).map_err(|_| NodeError::BalanceOverflow)?;
        if burn > 0 {
            let mut collector = self.load(overlay, &FEE_COLLECTOR)?;
            collector.balance = collector.balance.checked_sub(burn).ok_or_else(|| {
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
    pub(crate) fn take_fee_pool(&self, overlay: &mut Overlay) -> Result<u64> {
        if self.fee_record(overlay)?.is_none() {
            return Ok(0);
        }
        let mut collector = self.load(overlay, &FEE_COLLECTOR)?;
        let pool = collector.balance;
        collector.balance = 0;
        overlay.accounts.insert(FEE_COLLECTOR, collector);
        Ok(pool)
    }
}
