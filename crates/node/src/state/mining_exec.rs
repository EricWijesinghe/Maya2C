//! Executing mining shares and closing mining epochs (ADR-044).
//!
//! A share is checked in this order: mining enabled, right epoch, block cap,
//! not already spent, then the proof of work. The cheap refusals come first
//! so the 2 ms DAG check only runs for a share that could pay.

use maya_ledger_math::Amount;
use maya_stateless_core::amount::{to_amount, to_u256};
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
    DagProfile, MINING_KEY, MINING_RESERVE, MiningRecord, seen_epoch_prefix, seen_key, share_header,
};

/// Marks a spent share. The key carries the meaning; the value is a byte.
const SEEN: &[u8] = &[1];

/// The DAG epoch a mining epoch hashes against: the one holding its first block.
fn dag_epoch(record: &MiningRecord) -> Result<u64> {
    let start = record
        .epoch
        .checked_mul(record.epoch_blocks)
        .ok_or_else(|| NodeError::Mining("epoch start overflow".into()))?;
    Ok(start / record.dag.params().epoch_length.max(1))
}

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

    /// The committed share header `miner` must hash, and the DAG epoch whose
    /// cache it hashes against. A miner needs nothing else: the chain tag,
    /// anchor and payee are already folded into the header (ADR-044).
    ///
    /// # Errors
    ///
    /// A damaged record, or an epoch start that overflows.
    pub fn committed_share_work(
        &self,
        record: &MiningRecord,
        miner: &Address,
    ) -> Result<([u8; 32], u64)> {
        let header = share_header(
            &self.get_chain_tag()?.0,
            record.epoch,
            &record.anchor,
            miner,
        );
        Ok((header, dag_epoch(record)?))
    }

    /// How many shares of `epoch` the committed spent set holds. Zero for
    /// every closed epoch once its boundary block commits; RPC and the pruning
    /// test read it.
    ///
    /// # Errors
    ///
    /// Storage failure.
    pub fn committed_seen_shares(&self, epoch: u64) -> Result<usize> {
        Ok(self.scan_prefix(&seen_epoch_prefix(epoch))?.len())
    }

    /// The DAG cache shares of `record`'s epoch are verified against.
    fn share_cache(&self, record: &MiningRecord) -> Result<Arc<Cache>> {
        let registry = self
            .mining_dag
            .get_or_init(|| Arc::new(CacheRegistry::new(dag_config(record.dag))));
        registry.cache_for_epoch(dag_epoch(record)?)
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
        // Epoch 0's anchor is fixed before launch, so its work could be done
        // in advance and land first. Mining opens at the first boundary, whose
        // anchor is a block nobody could predict (security review, ADR-044).
        if record.epoch == 0 {
            return Err(refuse("mining opens at the first epoch boundary"));
        }
        if share.epoch != record.epoch {
            return Err(refuse("wrong epoch"));
        }
        // A share that pays nothing would still take a block slot fee-free
        // and spend the miner's nonce (found on the local mining devnet).
        let reward = record.share_reward();
        if reward.is_zero() {
            return Err(refuse("this epoch's miner budget is spent"));
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
        let key = seen_key(record.epoch, &header, share.nonce);
        if self.record(overlay, &key)?.is_some() {
            return Err(refuse("already paid"));
        }
        let cache = self.share_cache(&record)?;
        let proof = hashimoto_light(&cache, &header, share.nonce);
        if !meets_target(&proof.result, &record.target) {
            return Err(refuse("work does not meet the share target"));
        }

        let paid = self.pay_from_reserve(overlay, miner, reward)?;
        if paid.is_zero() {
            return Err(refuse("the mining reserve is empty"));
        }
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
        let Some(mut record) = self.mining_record(overlay)? else {
            return Ok(());
        };
        if record.epoch > 0
            && record.spent_after == 0
            && !record.paid.is_zero()
            && record.share_reward().is_zero()
        {
            // Shares past the budget are refused from here on, so record how
            // early it ran out: the retarget's measure of excess demand.
            let opened = record.epoch.saturating_mul(record.epoch_blocks);
            record.spent_after = context
                .height
                .saturating_sub(opened)
                .clamp(1, record.epoch_blocks);
            overlay
                .records
                .insert(MINING_KEY.to_vec(), Some(record.encode()));
        }
        if context.height == 0 || !context.height.is_multiple_of(record.epoch_blocks) {
            return Ok(());
        }
        self.pay_from_reserve(
            overlay,
            &FEE_COLLECTOR,
            record.validator_reward(record.epoch),
        )?;
        self.prune_seen(overlay, record.epoch)?;
        let next = MiningRecord {
            epoch: record
                .epoch
                .checked_add(1)
                .ok_or_else(|| NodeError::Mining("epoch counter overflow".into()))?,
            anchor: header.id(),
            target: record.retarget(),
            shares: 0,
            paid: Amount::ZERO,
            spent_after: 0,
            ..record
        };
        overlay
            .records
            .insert(MINING_KEY.to_vec(), Some(next.encode()));
        Ok(())
    }

    /// Admission check for the mempool: would `share` pay if it landed in the
    /// next block? The same rules as execution, against committed state and a
    /// throwaway overlay, so the mempool cannot drift from consensus.
    ///
    /// # Errors
    ///
    /// As [`Self::apply_mining_share`].
    pub(crate) fn check_mining_share(&self, miner: &Address, share: &MiningShare) -> Result<()> {
        self.apply_mining_share(&mut Overlay::default(), miner, share)
    }

    /// Deletes a closed epoch's spent-share set, committed and staged. Its
    /// shares are refused by the epoch check from now on, so the set guards
    /// nothing and would otherwise grow with the chain.
    fn prune_seen(&self, overlay: &mut Overlay, epoch: u64) -> Result<()> {
        let prefix = seen_epoch_prefix(epoch);
        let staged: Vec<Vec<u8>> = overlay
            .records
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix))
            .map(|(key, _)| key.clone())
            .collect();
        let committed = self.scan_prefix(&prefix)?.into_iter().map(|(key, _)| key);
        for key in staged.into_iter().chain(committed) {
            overlay.records.insert(key, None);
        }
        Ok(())
    }

    /// Moves up to `amount` from the reserve to `to` and returns what moved.
    /// A transfer, never a mint: once the reserve is empty, emission ends.
    fn pay_from_reserve(
        &self,
        overlay: &mut Overlay,
        to: &Address,
        amount: Amount,
    ) -> Result<Amount> {
        let mut reserve = self.load(overlay, &MINING_RESERVE)?;
        // Both 256-bit (ADR-046): pays `amount`, or whatever the reserve has.
        let paid = reserve.balance.min(to_u256(amount));
        if paid.is_zero() {
            return Ok(Amount::ZERO);
        }
        // `paid` is at most the reserve; checked so a later edit fails closed.
        reserve.balance = reserve
            .balance
            .checked_sub(paid)
            .ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(MINING_RESERVE, reserve);
        let mut payee = self.load(overlay, to)?;
        payee.balance = payee
            .balance
            .checked_add(paid)
            .ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(*to, payee);
        Ok(to_amount(paid))
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
