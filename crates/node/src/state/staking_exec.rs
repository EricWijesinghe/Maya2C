//! Executing staking transactions and the per-block staking pass (ADR-028).
//!
//! Transactions go through `maya-staking`'s operations; its [`Effect`]s are
//! applied here to accounts. Every block then runs [`StateDB::settle_staking`]:
//! it reads the header's seal to count the anchor slot it filled and the slots
//! skipped since the last one, and at an epoch boundary ends the epoch —
//! downtime judged from those counts, rewards paid, unbonded funds released,
//! the next committee chosen.

use std::sync::Arc;

use maya_dag_bft::{Equivocation, Message};
use maya_staking::{Effect, Participation};

use crate::consensus::bft::auth::MlDsaAuthenticator;
use crate::consensus::bft::{Envelope, unseal};
use crate::core::block::BlockHeader;
use crate::core::staking_payload::StakingAction;
use crate::crypto::PUBLIC_KEY_LEN;
use crate::crypto::keys::VerifyingKey;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::shielded::FEE_SINK;
use crate::state::staking::{
    STATE_KEY, StakingRecord, committee_record, decode_committee, encode_committee, key_record,
    validator_id,
};

/// Most anchor slots one block may account for. A gap this long means the
/// chain made no progress for hours; the cap only bounds a loop.
const MAX_SLOTS_PER_BLOCK: u64 = 1_000_000;

fn stake_err(e: maya_staking::StakeError) -> NodeError {
    NodeError::Network(format!("staking refused: {e:?}"))
}

impl StateDB {
    /// The staking record through `overlay`, or `None` where staking is not
    /// configured.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub(crate) fn staking_record(&self, overlay: &Overlay) -> Result<Option<StakingRecord>> {
        self.record(overlay, STATE_KEY)?
            .map(|bytes| StakingRecord::decode(&bytes))
            .transpose()
    }

    /// The committed staking record, for the consensus driver and RPC.
    ///
    /// # Errors
    ///
    /// As [`StateDB::staking_record`].
    pub fn committed_staking(&self) -> Result<Option<StakingRecord>> {
        self.raw_get(STATE_KEY)?
            .map(|bytes| StakingRecord::decode(&bytes))
            .transpose()
    }

    /// The committed verifying keys of `ids`, in order.
    ///
    /// # Errors
    ///
    /// A missing or malformed key record: every registered id has one.
    pub fn validator_keys(&self, ids: &[[u8; 32]]) -> Result<Vec<VerifyingKey>> {
        ids.iter()
            .map(|id| {
                let bytes = self.raw_get(&key_record(id))?.ok_or_else(|| {
                    NodeError::Storage(format!("no key for validator {}", hex::encode(id)))
                })?;
                let array: [u8; PUBLIC_KEY_LEN] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| NodeError::Decode("validator key length".to_string()))?;
                VerifyingKey::from_bytes(&array)
            })
            .collect()
    }

    /// Applies `effects`. A debit may only name the transaction's sender.
    fn apply_effects(
        &self,
        overlay: &mut Overlay,
        sender: Option<&Address>,
        effects: &[Effect],
    ) -> Result<()> {
        for effect in effects {
            let (address, delta_in) = match effect {
                Effect::Debit(address, amount) => {
                    if Some(address) != sender {
                        return Err(NodeError::Network(
                            "staking tried to debit someone other than the sender".to_string(),
                        ));
                    }
                    (*address, -i128::from(*amount))
                }
                Effect::Credit(address, amount) => (*address, i128::from(*amount)),
                // A burn is value moved to the unspendable sink: supply is
                // conserved, circulation falls (the fee-market convention).
                Effect::Burn(amount) => (FEE_SINK, i128::from(*amount)),
            };
            let mut account = self.load(overlay, &address)?;
            let next = i128::from(account.balance) + delta_in;
            account.balance = u64::try_from(next).map_err(|_| NodeError::InsufficientBalance {
                address: hex::encode(address),
                required: u64::try_from(-delta_in).unwrap_or(u64::MAX),
                available: account.balance,
            })?;
            overlay.accounts.insert(address, account);
        }
        Ok(())
    }

    /// Executes one staking action from `sender`.
    ///
    /// # Errors
    ///
    /// Staking not configured, a refused operation, a bad proof of
    /// possession, bad evidence, or an unfunded debit. Nothing is staged on
    /// error.
    pub(crate) fn apply_staking(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        action: &StakingAction,
    ) -> Result<()> {
        let mut record = self
            .staking_record(overlay)?
            .ok_or_else(|| NodeError::Network("staking is not active on this chain".to_string()))?;
        let s = &mut record.staking;
        let effects = match action {
            StakingAction::Register {
                key,
                bond,
                commission_bps,
                possession,
            } => {
                let verifying = VerifyingKey::from_bytes(key)?;
                verifying.verify(&StakingAction::possession_message(sender), possession)?;
                let id = validator_id(key.as_slice());
                let effects = s
                    .register(*sender, id, *bond, *commission_bps)
                    .map_err(stake_err)?;
                overlay.records.insert(key_record(&id), Some(key.to_vec()));
                effects
            }
            StakingAction::BondMore { validator, amount } => s
                .bond_more(*sender, *validator, *amount)
                .map_err(stake_err)?,
            StakingAction::Unbond { validator, amount } => {
                s.unbond(*sender, *validator, *amount).map_err(stake_err)?
            }
            StakingAction::Delegate { validator, amount } => s
                .delegate(*sender, *validator, *amount)
                .map_err(stake_err)?,
            StakingAction::Undelegate { validator, amount } => s
                .undelegate(*sender, *validator, *amount)
                .map_err(stake_err)?,
            StakingAction::ReportEquivocation { first, second } => {
                let id = self.verify_equivocation(overlay, first, second)?;
                s.slash_double_sign(id).map_err(stake_err)?
            }
        };
        self.apply_effects(overlay, Some(sender), &effects)?;
        overlay
            .records
            .insert(STATE_KEY.to_vec(), Some(record.encode()));
        Ok(())
    }

    /// Checks two `Propose` frames are a real equivocation by a member of the
    /// committee of the epoch they name, and returns its id.
    fn verify_equivocation(
        &self,
        overlay: &Overlay,
        first: &[u8],
        second: &[u8],
    ) -> Result<[u8; 32]> {
        let proposal = |frame: &[u8]| -> Result<(maya_dag_bft::Vertex, Vec<u8>)> {
            match Envelope::decode(frame)?.message {
                Message::Propose { vertex, signature } => Ok((vertex, signature)),
                _ => Err(NodeError::Decode("evidence is not a proposal".to_string())),
            }
        };
        let (a, sa) = proposal(first)?;
        let (b, sb) = proposal(second)?;
        let committee = self
            .record(overlay, &committee_record(a.epoch))?
            .map(|bytes| decode_committee(&bytes))
            .transpose()?
            .ok_or_else(|| {
                NodeError::Network(format!("no committee on record for epoch {}", a.epoch))
            })?;
        let keys: Arc<[VerifyingKey]> = self.validator_keys(&committee)?.into();
        let evidence = Equivocation {
            first: a,
            first_signature: sa,
            second: b,
            second_signature: sb,
        };
        if !evidence.is_valid(&MlDsaAuthenticator::observer(keys)) {
            return Err(NodeError::SignatureVerification);
        }
        committee
            .get(usize::from(evidence.first.author))
            .copied()
            .ok_or_else(|| NodeError::Decode("evidence author outside the committee".to_string()))
    }

    /// The per-block staking pass. A no-op where staking is not configured.
    ///
    /// # Errors
    ///
    /// A damaged record or overflow.
    pub(crate) fn settle_staking(
        &self,
        overlay: &mut Overlay,
        header: &BlockHeader,
        context: BlockContext,
    ) -> Result<()> {
        let Some(mut record) = self.staking_record(overlay)? else {
            return Ok(());
        };
        // The epoch's committee as the engine ran it, not `active`: a
        // mid-epoch tombstone leaves `active` but not the leader schedule.
        let committee = self
            .record(overlay, &committee_record(record.staking.epoch))?
            .map(|bytes| decode_committee(&bytes))
            .transpose()?
            .unwrap_or_default();
        count_slots(&mut record, &committee, header.nonce);
        if context.height.is_multiple_of(record.epoch_blocks) {
            self.end_epoch(overlay, &mut record)?;
        }
        overlay
            .records
            .insert(STATE_KEY.to_vec(), Some(record.encode()));
        Ok(())
    }

    fn end_epoch(&self, overlay: &mut Overlay, record: &mut StakingRecord) -> Result<()> {
        let previous = record.staking.active.clone();
        let participation = Participation {
            rounds: 0,
            expected: std::mem::take(&mut record.expected),
            authored: std::mem::take(&mut record.authored),
        };
        // The epoch's tips (ADR-029); zero where fees are off. Taken out of
        // the collector here and fully paid out or burned by `end_epoch`.
        let pool = self.take_fee_pool(overlay)?;
        let outcome = record
            .staking
            .end_epoch(&participation, pool)
            .map_err(stake_err)?;
        self.apply_effects(overlay, None, &outcome.effects)?;
        // An empty committee would halt the chain for good. Keep the old one
        // and say so in ADR-028; governance, not arithmetic, fixes that state.
        if record.staking.active.is_empty() {
            record.staking.active = previous;
        }
        record.last_round = 0;
        let epoch = record.staking.epoch;
        overlay.records.insert(
            committee_record(epoch),
            Some(encode_committee(&record.staking.active)),
        );
        // Evidence is admissible while the offender's funds can still be
        // reached: keep committees for the unbonding window, drop older ones.
        let keep = record.staking.params.unbonding_epochs.saturating_add(1);
        if let Some(stale) = epoch.checked_sub(keep) {
            overlay.records.insert(committee_record(stale), None);
        }
        Ok(())
    }
}

/// Counts the anchor slot `nonce` seals and every slot skipped since the last.
fn count_slots(record: &mut StakingRecord, active: &[[u8; 32]], nonce: u64) {
    let (epoch, round) = unseal(nonce);
    if epoch != record.staking.epoch
        || active.is_empty()
        || round <= record.last_round
        || round % 2 != 0
    {
        return;
    }
    let n = active.len() as u64;
    let leader = |r: u64| active[usize::try_from((r / 2) % n).unwrap_or(0)];
    let first = (record.last_round + 2).max(2);
    let slots = ((round - first) / 2 + 1).min(MAX_SLOTS_PER_BLOCK);
    for k in 0..slots {
        *record.expected.entry(leader(first + 2 * k)).or_insert(0) += 1;
    }
    *record.authored.entry(leader(round)).or_insert(0) += 1;
    record.last_round = round;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use maya_staking::{Params, Staking};
    use std::collections::BTreeMap;

    fn committee(n: u8) -> Vec<[u8; 32]> {
        (0..n).map(|i| [i; 32]).collect()
    }

    fn record(active: usize) -> StakingRecord {
        let mut staking = Staking::new(Params::DEVNET).unwrap();
        staking.active = (0..active)
            .map(|i| [u8::try_from(i).unwrap(); 32])
            .collect();
        StakingRecord {
            staking,
            epoch_blocks: 10,
            last_round: 0,
            expected: BTreeMap::default(),
            authored: BTreeMap::default(),
        }
    }

    #[test]
    fn a_skipped_anchor_counts_against_its_leader_only() {
        let mut r = record(3);
        count_slots(&mut r, &committee(3), crate::consensus::bft::seal(0, 2)); // leader 1
        count_slots(&mut r, &committee(3), crate::consensus::bft::seal(0, 6)); // 4 skipped (leader 2), 6 leader 0
        assert_eq!(r.expected[&[1; 32]], 1);
        assert_eq!(r.expected[&[2; 32]], 1);
        assert_eq!(r.expected[&[0; 32]], 1);
        assert!(!r.authored.contains_key(&[2; 32]));
        assert_eq!(r.authored[&[0; 32]], 1);
    }

    #[test]
    fn a_seal_from_another_epoch_or_an_old_round_counts_nothing() {
        let mut r = record(3);
        count_slots(&mut r, &committee(3), crate::consensus::bft::seal(0, 4));
        let before = r.clone();
        count_slots(&mut r, &committee(3), crate::consensus::bft::seal(1, 8));
        count_slots(&mut r, &committee(3), crate::consensus::bft::seal(0, 2));
        assert_eq!(r, before);
    }
}
