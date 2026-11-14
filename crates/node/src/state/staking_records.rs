//! One state record per validator, delegation, unbonding entry and owed
//! payout (ADR-062b), and the conversion between them and a complete
//! in-memory [`StakingRecord`].
//!
//! Keys put the validator first, so everything a slash must reach is two key
//! ranges, `k:d:<validator>` and `k:u:<validator>`, and nothing else is read.
//! The release index orders unbonding entries by the epoch they are paid at,
//! so an epoch boundary reads exactly the entries it releases.
//!
//! Each validator record carries aggregates over its own delegations and
//! unbonding entries (count, pending stake, unbonding total). They are what
//! lets the epoch boundary pay, rank and retire validators without reading a
//! single delegation; `staking_store` keeps them exact by computing every
//! change as a difference over the records an operation loaded.

use std::collections::BTreeMap;

use maya_staking::{Amount, ValidatorRecord};
#[cfg(test)]
use maya_staking::{Staking, Status, Unbonding};

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::staking::{
    Meta, STATE_KEY, StakingRecord, put_amount, put_status, put_u16, put_u64, read_amount,
    read_status, read_u16,
};

/// `k:v:<validator>`.
pub(crate) const VALIDATOR_PREFIX: &[u8] = b"k:v:";
/// `k:d:<validator><delegator>`.
pub(crate) const DELEGATION_PREFIX: &[u8] = b"k:d:";
/// `k:u:<validator><release epoch be><owner>`.
pub(crate) const UNBONDING_PREFIX: &[u8] = b"k:u:";
/// `k:ur:<release epoch be><validator><owner>`: the release index.
pub(crate) const RELEASE_PREFIX: &[u8] = b"k:ur:";
/// `k:pj:<validator><epoch be>`: rewards owed to a validator's delegators.
pub(crate) const PAYOUT_PREFIX: &[u8] = b"k:pj:";

/// `(validator, release epoch, owner)`: one unbonding entry's identity.
pub(crate) type UnbondingKey = ([u8; 32], u64, [u8; 32]);

fn decode_err(what: &str) -> NodeError {
    NodeError::Decode(format!("staking {what}"))
}

/// `k:v:<id>`.
pub(crate) fn validator_key(id: &[u8; 32]) -> Vec<u8> {
    [VALIDATOR_PREFIX, id].concat()
}

/// `k:d:<validator>`: every delegation to one validator.
pub(crate) fn delegations_of(validator: &[u8; 32]) -> Vec<u8> {
    [DELEGATION_PREFIX, validator].concat()
}

/// `k:d:<validator><delegator>`.
pub(crate) fn delegation_key(delegator: &[u8; 32], validator: &[u8; 32]) -> Vec<u8> {
    [DELEGATION_PREFIX, validator, delegator].concat()
}

/// `k:u:<validator>`: every unbonding entry from one validator.
pub(crate) fn unbondings_of(validator: &[u8; 32]) -> Vec<u8> {
    [UNBONDING_PREFIX, validator].concat()
}

/// `k:u:<validator><release be><owner>`.
pub(crate) fn unbonding_key((validator, release, owner): &UnbondingKey) -> Vec<u8> {
    [UNBONDING_PREFIX, validator, &release.to_be_bytes(), owner].concat()
}

/// `k:ur:<release be><validator><owner>`.
pub(crate) fn release_key((validator, release, owner): &UnbondingKey) -> Vec<u8> {
    [RELEASE_PREFIX, &release.to_be_bytes(), validator, owner].concat()
}

/// `k:pj:<validator>`: every payout owed for one validator, oldest first.
pub(crate) fn payouts_of(validator: &[u8; 32]) -> Vec<u8> {
    [PAYOUT_PREFIX, validator].concat()
}

/// `k:pj:<validator><epoch be>`.
pub(crate) fn payout_key(validator: &[u8; 32], epoch: u64) -> Vec<u8> {
    [PAYOUT_PREFIX, validator, &epoch.to_be_bytes()].concat()
}

fn tail<'a>(key: &'a [u8], prefix: &[u8], len: usize) -> Result<&'a [u8]> {
    key.strip_prefix(prefix)
        .filter(|t| t.len() == len)
        .ok_or_else(|| decode_err("record key"))
}

fn id_at(bytes: &[u8], at: usize) -> [u8; 32] {
    let mut id = [0; 32];
    id.copy_from_slice(&bytes[at..at + 32]);
    id
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut be = [0; 8];
    be.copy_from_slice(&bytes[at..at + 8]);
    u64::from_be_bytes(be)
}

/// `(delegator, validator)` from a delegation key.
pub(crate) fn parse_delegation_key(key: &[u8]) -> Result<([u8; 32], [u8; 32])> {
    let t = tail(key, DELEGATION_PREFIX, 64)?;
    Ok((id_at(t, 32), id_at(t, 0)))
}

/// The entry an unbonding key names.
pub(crate) fn parse_unbonding_key(key: &[u8]) -> Result<UnbondingKey> {
    let t = tail(key, UNBONDING_PREFIX, 72)?;
    Ok((id_at(t, 0), u64_at(t, 32), id_at(t, 40)))
}

/// The entry a release-index key names.
pub(crate) fn parse_release_key(key: &[u8]) -> Result<UnbondingKey> {
    let t = tail(key, RELEASE_PREFIX, 72)?;
    Ok((id_at(t, 8), u64_at(t, 0), id_at(t, 40)))
}

/// `(validator, epoch)` from a payout key.
pub(crate) fn parse_payout_key(key: &[u8]) -> Result<([u8; 32], u64)> {
    let t = tail(key, PAYOUT_PREFIX, 40)?;
    Ok((id_at(t, 0), u64_at(t, 32)))
}

/// One validator and the aggregates over its own records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredValidator {
    /// The rules' record.
    pub(crate) record: ValidatorRecord,
    /// The epoch the two pending sums belong to; in any other they are zero.
    pub(crate) pending_epoch: u64,
    /// Self bond added in `pending_epoch`.
    pub(crate) pending_self: Amount,
    /// Sum of the pending part of every delegation to it in `pending_epoch`.
    pub(crate) pending_delegated: Amount,
    /// Delegation records to it.
    pub(crate) delegations: u64,
    /// Sum of its unbonding entries.
    pub(crate) unbonding: Amount,
}

impl StoredValidator {
    /// Self bond added in `epoch`.
    pub(crate) fn pending_self_at(&self, epoch: u64) -> Amount {
        if self.pending_epoch == epoch {
            self.pending_self
        } else {
            Amount::ZERO
        }
    }

    /// Delegated stake added in `epoch`.
    pub(crate) fn pending_delegated_at(&self, epoch: u64) -> Amount {
        if self.pending_epoch == epoch {
            self.pending_delegated
        } else {
            Amount::ZERO
        }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let v = &self.record;
        let mut out = Vec::with_capacity(32 + 32 * 5 + 2 + 9 + 16);
        out.extend_from_slice(&v.operator);
        put_amount(&mut out, v.self_bond);
        put_amount(&mut out, v.delegated);
        put_u16(&mut out, v.commission_bps);
        put_status(&mut out, v.status);
        put_u64(&mut out, self.pending_epoch);
        put_amount(&mut out, self.pending_self);
        put_amount(&mut out, self.pending_delegated);
        put_u64(&mut out, self.delegations);
        put_amount(&mut out, self.unbonding);
        out
    }

    /// # Errors
    ///
    /// Malformed bytes, or a pending sum above the stake it is part of.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        let stored = Self {
            record: ValidatorRecord {
                operator: r.read_array()?,
                self_bond: read_amount(&mut r)?,
                delegated: read_amount(&mut r)?,
                commission_bps: read_u16(&mut r)?,
                status: read_status(&mut r)?,
            },
            pending_epoch: r.read_u64()?,
            pending_self: read_amount(&mut r)?,
            pending_delegated: read_amount(&mut r)?,
            delegations: r.read_u64()?,
            unbonding: read_amount(&mut r)?,
        };
        r.finish()?;
        // A corrupt record must not make stake look larger than it is.
        if stored.pending_self > stored.record.self_bond
            || stored.pending_delegated > stored.record.delegated
        {
            return Err(decode_err("pending stake out of range"));
        }
        Ok(stored)
    }
}

/// One delegation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StoredDelegation {
    /// What is delegated. Never zero: an emptied delegation is deleted.
    pub(crate) amount: Amount,
    /// The part added in `pending_epoch`, which earns from the next.
    pub(crate) pending: Amount,
    /// The epoch `pending` belongs to; in any other it is zero.
    pub(crate) pending_epoch: u64,
    /// The first epoch whose owed payout this delegation has not received.
    pub(crate) next_payout_epoch: u64,
}

impl StoredDelegation {
    /// The part added in `epoch`.
    pub(crate) fn pending_at(&self, epoch: u64) -> Amount {
        if self.pending_epoch == epoch {
            self.pending
        } else {
            Amount::ZERO
        }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(80);
        put_amount(&mut out, self.amount);
        put_amount(&mut out, self.pending);
        put_u64(&mut out, self.pending_epoch);
        put_u64(&mut out, self.next_payout_epoch);
        out
    }

    /// # Errors
    ///
    /// Malformed bytes, a zero delegation, or pending stake above it.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        let d = Self {
            amount: read_amount(&mut r)?,
            pending: read_amount(&mut r)?,
            pending_epoch: r.read_u64()?,
            next_payout_epoch: r.read_u64()?,
        };
        r.finish()?;
        if d.amount.is_zero() || d.pending > d.amount {
            return Err(decode_err("delegation out of range"));
        }
        Ok(d)
    }
}

/// One validator's delegators' rewards for one epoch, not yet all paid
/// (`maya_staking::Payout`, plus who is still owed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PayoutJob {
    /// The validator's reward after commission.
    pub(crate) rest: Amount,
    /// Its earning stake that epoch.
    pub(crate) total: Amount,
    /// What is still held for the delegators; burned once none is owed.
    pub(crate) owed: Amount,
    /// Delegation records that existed at the boundary and are not paid yet.
    pub(crate) remaining: u64,
    /// The last delegator the per-block pass paid, in key order.
    pub(crate) cursor: Option<[u8; 32]>,
}

impl PayoutJob {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 * 4 + 9);
        put_amount(&mut out, self.rest);
        put_amount(&mut out, self.total);
        put_amount(&mut out, self.owed);
        put_u64(&mut out, self.remaining);
        match &self.cursor {
            None => out.push(0),
            Some(d) => {
                out.push(1);
                out.extend_from_slice(d);
            }
        }
        out
    }

    /// # Errors
    ///
    /// Malformed bytes, or a job nobody is owed from (it would never end).
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        let job = Self {
            rest: read_amount(&mut r)?,
            total: read_amount(&mut r)?,
            owed: read_amount(&mut r)?,
            remaining: r.read_u64()?,
            cursor: match r.read_u8()? {
                0 => None,
                1 => Some(r.read_array()?),
                t => return Err(decode_err(&format!("payout cursor tag {t}"))),
            },
        };
        r.finish()?;
        if job.remaining == 0 || job.total.is_zero() {
            return Err(decode_err("payout with nobody owed"));
        }
        Ok(job)
    }
}

/// An unbonding entry's amount.
pub(crate) fn decode_unbonding(bytes: &[u8]) -> Result<Amount> {
    let mut r = ByteReader::new(bytes);
    let amount = read_amount(&mut r)?;
    r.finish()?;
    Ok(amount)
}

/// Native coin one staking record holds, by its key: a validator its self
/// bond (its `delegated` is the sum of delegation records), a delegation or
/// an unbonding entry its amount, a payout what it still owes. Every other
/// `k:` record holds nothing. The conservation check folds these.
///
/// # Errors
///
/// A record that does not decode.
pub(crate) fn held_by(key: &[u8], value: &[u8]) -> Result<Amount> {
    if key.starts_with(VALIDATOR_PREFIX) {
        Ok(StoredValidator::decode(value)?.record.self_bond)
    } else if key.starts_with(DELEGATION_PREFIX) {
        Ok(StoredDelegation::decode(value)?.amount)
    } else if key.starts_with(UNBONDING_PREFIX) {
        decode_unbonding(value)
    } else if key.starts_with(PAYOUT_PREFIX) {
        Ok(PayoutJob::decode(value)?.owed)
    } else {
        Ok(Amount::ZERO)
    }
}

/// The validator a record of a complete view belongs to.
fn aggregate<'a>(
    stored: &'a mut BTreeMap<[u8; 32], StoredValidator>,
    id: &[u8; 32],
) -> Result<&'a mut StoredValidator> {
    stored
        .get_mut(id)
        .ok_or_else(|| NodeError::Decode("staking record of an unknown validator".into()))
}

impl StakingRecord {
    /// Every stored record a complete view is, in key order: genesis and
    /// tests. Aggregates are computed here; every delegation is paid up to
    /// the current epoch and no payout is owed.
    ///
    /// # Errors
    ///
    /// A sum past 2²⁵⁶ − 1, which conservation makes unreachable.
    pub(crate) fn to_records(&self) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
        let s = &self.staking;
        let overflow = || NodeError::Decode("staking aggregate overflow".into());
        let mut records = BTreeMap::new();
        let mut stored: BTreeMap<[u8; 32], StoredValidator> = BTreeMap::new();
        for (id, v) in &s.validators {
            stored.insert(
                *id,
                StoredValidator {
                    record: v.clone(),
                    pending_epoch: s.epoch,
                    pending_self: s.pending_self.get(id).copied().unwrap_or(Amount::ZERO),
                    pending_delegated: Amount::ZERO,
                    delegations: 0,
                    unbonding: Amount::ZERO,
                },
            );
        }
        for (key @ (delegator, validator), amount) in &s.delegations {
            let pending = s
                .pending_delegations
                .get(key)
                .copied()
                .unwrap_or(Amount::ZERO);
            let v = aggregate(&mut stored, validator)?;
            v.delegations += 1;
            v.pending_delegated = v
                .pending_delegated
                .checked_add(pending)
                .ok_or_else(overflow)?;
            let d = StoredDelegation {
                amount: *amount,
                pending,
                pending_epoch: s.epoch,
                next_payout_epoch: s.epoch,
            };
            records.insert(delegation_key(delegator, validator), d.encode());
        }
        for u in &s.unbonding {
            let v = aggregate(&mut stored, &u.validator)?;
            v.unbonding = v.unbonding.checked_add(u.amount).ok_or_else(overflow)?;
            let key = (u.validator, u.release_epoch, u.owner);
            let mut amount = Vec::new();
            put_amount(&mut amount, u.amount);
            records.insert(unbonding_key(&key), amount);
            records.insert(release_key(&key), Vec::new());
        }
        for (id, v) in &stored {
            records.insert(validator_key(id), v.encode());
        }
        let meta = Meta {
            params: s.params,
            epoch: s.epoch,
            active: s.active.clone(),
            epoch_blocks: self.epoch_blocks,
            last_round: self.last_round,
            expected: self.expected.clone(),
            authored: self.authored.clone(),
            delegation_records: s.delegations.len() as u64,
        };
        records.insert(STATE_KEY.to_vec(), meta.encode());
        Ok(records)
    }

    /// The complete view `records` (every `k:` record, any extra keys
    /// ignored) amount to, unbonding entries in key order. Owed payouts are
    /// not part of the view. `None` without a meta record.
    ///
    /// # Errors
    ///
    /// A record that does not decode, or one of an unknown validator.
    #[cfg(test)]
    pub(crate) fn from_records(records: &BTreeMap<Vec<u8>, Vec<u8>>) -> Result<Option<Self>> {
        let Some(meta) = records.get(STATE_KEY) else {
            return Ok(None);
        };
        let mut view = Meta::decode(meta)?.view()?;
        let s: &mut Staking = &mut view.staking;
        let epoch = s.epoch;
        for (key, value) in records {
            if let Some(id) = key.strip_prefix(VALIDATOR_PREFIX) {
                let id: [u8; 32] = id.try_into().map_err(|_| decode_err("validator key"))?;
                let v = StoredValidator::decode(value)?;
                if v.record.status == Status::Tombstoned {
                    s.retired.insert(id);
                }
                let pending = v.pending_self_at(epoch);
                if !pending.is_zero() {
                    s.pending_self.insert(id, pending);
                }
                s.validators.insert(id, v.record);
            }
        }
        for (key, value) in records {
            if key.starts_with(DELEGATION_PREFIX) {
                let pair @ (_, validator) = parse_delegation_key(key)?;
                if !s.validators.contains_key(&validator) {
                    return Err(decode_err("delegation to an unknown validator"));
                }
                let d = StoredDelegation::decode(value)?;
                s.delegations.insert(pair, d.amount);
                let pending = d.pending_at(epoch);
                if !pending.is_zero() {
                    s.pending_delegations.insert(pair, pending);
                }
            } else if key.starts_with(UNBONDING_PREFIX) {
                let (validator, release_epoch, owner) = parse_unbonding_key(key)?;
                s.unbonding.push(Unbonding {
                    owner,
                    validator,
                    amount: decode_unbonding(value)?,
                    release_epoch,
                });
            }
        }
        Ok(Some(view))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use maya_staking::Params;

    fn populated() -> StakingRecord {
        let mut staking = Staking::new(Params::DEVNET).unwrap();
        staking
            .register([1; 32], [9; 32], Amount::from_u64(5_000), 100)
            .unwrap();
        staking
            .register([2; 32], [8; 32], Amount::from_u64(7_000), 0)
            .unwrap();
        staking
            .delegate([3; 32], [9; 32], Amount::from_u64(400))
            .unwrap();
        staking
            .undelegate([3; 32], [9; 32], Amount::from_u64(100))
            .unwrap();
        staking.slash_double_sign([8; 32]).unwrap();
        staking.active = vec![[9; 32]];
        StakingRecord {
            staking,
            epoch_blocks: 600,
            last_round: 44,
            expected: BTreeMap::from([([9; 32], 11)]),
            authored: BTreeMap::from([([9; 32], 10)]),
        }
    }

    #[test]
    fn a_complete_view_round_trips_through_its_records() {
        let view = populated();
        let records = view.to_records().unwrap();
        let back = StakingRecord::from_records(&records).unwrap().unwrap();
        assert_eq!(back, view);
        assert!(!back.staking.pending_delegations.is_empty());
        assert!(back.staking.retired.contains(&[8; 32]));
        assert_eq!(back.to_records().unwrap(), records);
    }

    #[test]
    fn records_are_one_per_validator_delegation_and_unbonding_entry() {
        let records = populated().to_records().unwrap();
        let under = |p: &[u8]| records.keys().filter(|k| k.starts_with(p)).count();
        assert_eq!(under(VALIDATOR_PREFIX), 2);
        assert_eq!(under(DELEGATION_PREFIX), 1);
        assert_eq!(under(UNBONDING_PREFIX), 1);
        assert_eq!(under(RELEASE_PREFIX), 1);
        let v = StoredValidator::decode(&records[&validator_key(&[9; 32])]).unwrap();
        assert_eq!(v.delegations, 1);
        assert_eq!(v.unbonding, Amount::from_u64(100));
    }

    #[test]
    fn keys_parse_back_to_what_built_them() {
        let entry = ([7; 32], 0x0102_0304_0506_0708, [5; 32]);
        assert_eq!(parse_unbonding_key(&unbonding_key(&entry)).unwrap(), entry);
        assert_eq!(parse_release_key(&release_key(&entry)).unwrap(), entry);
        assert_eq!(
            parse_delegation_key(&delegation_key(&[1; 32], &[2; 32])).unwrap(),
            ([1; 32], [2; 32])
        );
        assert_eq!(
            parse_payout_key(&payout_key(&[3; 32], 9)).unwrap(),
            ([3; 32], 9)
        );
        assert!(delegation_key(&[1; 32], &[2; 32]).starts_with(&delegations_of(&[2; 32])));
        // The release index and the unbonding entries are separate ranges.
        assert!(!release_key(&entry).starts_with(UNBONDING_PREFIX));
        assert!(parse_delegation_key(b"k:d:short").is_err());
    }

    #[test]
    fn records_that_would_overstate_stake_are_refused() {
        let d = StoredDelegation {
            amount: Amount::from_u64(10),
            pending: Amount::from_u64(11),
            pending_epoch: 0,
            next_payout_epoch: 0,
        };
        assert!(StoredDelegation::decode(&d.encode()).is_err());
        let zero = StoredDelegation {
            amount: Amount::ZERO,
            pending: Amount::ZERO,
            ..d
        };
        assert!(StoredDelegation::decode(&zero.encode()).is_err());
        let mut v =
            StoredValidator::decode(&populated().to_records().unwrap()[&validator_key(&[9; 32])])
                .unwrap();
        v.pending_delegated = v.record.delegated.saturating_add(Amount::from_u64(1));
        assert!(StoredValidator::decode(&v.encode()).is_err());
        let job = PayoutJob {
            rest: Amount::from_u64(5),
            total: Amount::from_u64(5),
            owed: Amount::from_u64(5),
            remaining: 0,
            cursor: Some([1; 32]),
        };
        assert!(PayoutJob::decode(&job.encode()).is_err());
        let live = PayoutJob {
            remaining: 2,
            ..job
        };
        assert_eq!(PayoutJob::decode(&live.encode()).unwrap(), live);
    }
}
