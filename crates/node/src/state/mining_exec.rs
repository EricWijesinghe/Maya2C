//! Executing mining shares and closing mining epochs (ADR-044).
//!
//! A share is checked in this order: mining enabled, right epoch, block cap,
//! not already spent, then the proof of work. The cheap refusals come first
//! so the 2 ms DAG check only runs for a share that could pay.

use std::sync::Arc;

use crate::core::block::BlockHeader;
use crate::core::mining_payload::MiningShare;
use crate::crypto::dag::cache::Cache;
use crate::crypto::dag::hashimoto::hashimoto_light;
use crate::crypto::dag::registry::{CacheRegistry, DagConfig};
use crate::crypto::pow::meets_target;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::fees::FEE_COLLECTOR;
use crate::state::mining::{
    DagProfile, MINING_KEY, MINING_RESERVE, MiningRecord, seen_key, share_header,
};

/// Marks a spent share. The key carries the meaning; the value is a byte.
const SEEN: &[u8] = &[1];

fn refuse(reason: &str) -> NodeError {
    NodeError::Mining(reason.to_string())
}

impl StateDB {
    /// The mining record as of `overlay`, if this chain mines.
    ///
    /// # Errors
    ///
    /// A damaged record.
    pub(crate) fn mining_record(&self, overlay: &Overlay) -> Result<Option<MiningRecord>> {
        self.record(overlay, MINING_KEY)?
            .map(|bytes| MiningRecord::decode(&bytes))
            .transpose()
    }

    /// The committed mining record, for RPC and miners.
    ///
    /// # Errors
    ///
    /// A damaged record.
    pub fn committed_mining(&self) -> Result<Option<MiningRecord>> {
        self.raw_get(MINING_KEY)?
            .map(|bytes| MiningRecord::decode(&bytes))
            .transpose()
    }

    /// The DAG cache shares of `record`'s epoch are verified against.
    fn share_cache(&self, record: &MiningRecord) -> Result<Arc<Cache>> {
        let registry = self
            .mining_dag
            .get_or_init(|| Arc::new(CacheRegistry::new(dag_config(record.dag))));
        let params = record.dag.params();
        let start = record
            .epoch
            .checked_mul(record.epoch_blocks)
            .ok_or_else(|| NodeError::Mining("epoch start overflow".into()))?;
        registry.cache_for_epoch(start / params.epoch_length.max(1))
    }

    /// Pays one share from the reserve, or refuses it.
    ///
    /// # Errors
    ///
    /// Mining off, a wrong epoch, a full block, a spent share, or work that
    /// misses the target. Each fails the transaction.
    pub(crate) fn apply_mining_share(
        &self,
        overlay: &mut Overlay,
        miner: &Address,
        share: &MiningShare,
    ) -> Result<()> {
        let Some(mut record) = self.mining_record(overlay)? else {
            return Err(refuse("this chain does not mine"));
        };
        if share.epoch != record.epoch {
            return Err(refuse("wrong epoch"));
        }
        if overlay.mining_shares >= record.max_shares_per_block {
            return Err(refuse("block share cap reached"));
        }
        let header = share_header(
            &self.get_chain_tag()?.0,
            record.epoch,
            &record.anchor,
            miner,
        );
        let key = seen_key(&header, share.nonce);
        if self.record(overlay, &key)?.is_some() {
            return Err(refuse("already paid"));
        }
        let cache = self.share_cache(&record)?;
        let proof = hashimoto_light(&cache, &header, share.nonce);
        if !meets_target(&proof.result, &record.target) {
            return Err(refuse("work does not meet the share target"));
        }

        let paid = self.pay_from_reserve(overlay, miner, record.share_reward())?;
        record.shares = record.shares.saturating_add(1);
        record.paid = record.paid.saturating_add(paid);
        overlay.mining_shares += 1;
        overlay.records.insert(key, Some(SEEN.to_vec()));
        overlay
            .records
            .insert(MINING_KEY.to_vec(), Some(record.encode()));
        Ok(())
    }

    /// End of block. On an epoch boundary: the committee's part goes to the
    /// fee collector, where staking pays it out with the tips, then the target
    /// retargets and the next epoch is anchored to this block. Runs before
    /// `settle_staking`, which empties the collector.
    ///
    /// # Errors
    ///
    /// A damaged record or overflow.
    pub(crate) fn settle_mining(
        &self,
        overlay: &mut Overlay,
        header: &BlockHeader,
        context: BlockContext,
    ) -> Result<()> {
        let Some(record) = self.mining_record(overlay)? else {
            return Ok(());
        };
        if context.height == 0 || !context.height.is_multiple_of(record.epoch_blocks) {
            return Ok(());
        }
        self.pay_from_reserve(
            overlay,
            &FEE_COLLECTOR,
            record.validator_reward(record.epoch),
        )?;
        let next = MiningRecord {
            epoch: record
                .epoch
                .checked_add(1)
                .ok_or_else(|| NodeError::Mining("epoch counter overflow".into()))?,
            anchor: header.id(),
            target: record.retarget(),
            shares: 0,
            paid: 0,
            ..record
        };
        overlay
            .records
            .insert(MINING_KEY.to_vec(), Some(next.encode()));
        Ok(())
    }

    /// Moves up to `amount` from the reserve to `to` and returns what moved.
    /// A transfer, never a mint: once the reserve is empty, emission ends.
    fn pay_from_reserve(&self, overlay: &mut Overlay, to: &Address, amount: u64) -> Result<u64> {
        let mut reserve = self.load(overlay, &MINING_RESERVE)?;
        let paid = amount.min(reserve.balance);
        if paid == 0 {
            return Ok(0);
        }
        reserve.balance -= paid;
        overlay.accounts.insert(MINING_RESERVE, reserve);
        let mut payee = self.load(overlay, to)?;
        payee.balance = payee
            .balance
            .checked_add(paid)
            .ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(*to, payee);
        Ok(paid)
    }
}

/// Shares use the DAG from height 0; the activation fields only matter to
/// the block-PoW path, which mining never touches.
fn dag_config(profile: DagProfile) -> DagConfig {
    DagConfig {
        params: profile.params(),
        activation_height: 0,
        activation_target: [0xff; 32],
    }
}
