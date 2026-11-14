//! Staking records under `k:` (ADR-028, ADR-062b), committed as their own
//! state layer.
//!
//! | key | value |
//! |---|---|
//! | `k:state` | `Meta`: parameters, epoch, committee, the epoch's slot counts |
//! | `k:v:<validator>` | one validator (`staking_records::StoredValidator`) |
//! | `k:d:<validator><delegator>` | one delegation (`StoredDelegation`) |
//! | `k:u:<validator><release be><owner>` | one unbonding entry: its amount |
//! | `k:ur:<release be><validator><owner>` | the release index: empty |
//! | `k:pj:<validator><epoch be>` | delegators' rewards owed for an epoch (`PayoutJob`) |
//! | `k:key:<id>` | the validator's 2,592-byte ML-DSA-87 verifying key |
//! | `k:cmt:<epoch be>` | that epoch's committee, ids in committee order |
//!
//! Until ADR-062b all of the first six were one value under `k:state`,
//! decoded and re-encoded whole by every block and every staking
//! transaction, so a crowd of dust delegations made every block slower.
//! Keyed by validator first, one validator's delegations and unbonding funds
//! are each one key range, and the release index puts the entries one epoch
//! boundary pays out in one range too. [`StakingRecord`] is now the in-memory
//! view: the meta record plus whichever records an operation loaded.
//!
//! **Presence is activation.** A chain whose genesis configures no staking has
//! no `k:state`, the layer is absent from its root, and a staking transaction
//! is refused as inactive — the same "never present before activation, so it
//! moves no existing root" rule the `IoT` and threat-intel layers follow.

use std::collections::BTreeMap;

use maya_staking::{Amount, Params, Staking, Status};

/// Staking record format. 2 since ADR-046 widened every amount to 256 bits; 3
/// added the stake added this epoch (earns from the next); 4 is the meta
/// record of the per-record layout (ADR-062b). An older record is refused,
/// not reinterpreted.
const RECORD_VERSION: u8 = 4;

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Every staking record lives under this prefix.
pub const STAKING_PREFIX: &[u8] = b"k:";

/// The meta record (`Meta`).
pub const STATE_KEY: &[u8] = b"k:state";

const KEY_PREFIX: &[u8] = b"k:key:";
const COMMITTEE_PREFIX: &[u8] = b"k:cmt:";
/// Stake-weighted committees (ADR-040 part 2): each member's stake, frozen
/// at the epoch boundary. Present only on a chain whose genesis asked for
/// it, so a chain without it keeps the state root it always had.
const WEIGHTS_PREFIX: &[u8] = b"k:cmw:";

/// The staking state plus what the node counts between epoch boundaries.
///
/// A view: [`StateDB::committed_staking`](crate::state::db::StateDB::committed_staking)
/// fills every validator and leaves delegations and unbonding funds out;
/// an operation loads only the records it touches; `staking_records`
/// converts a complete one to and from the stored records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StakingRecord {
    /// The rules' state, as far as loaded.
    pub staking: Staking,
    /// Blocks per epoch. From genesis.
    pub epoch_blocks: u64,
    /// Round of the last committed anchor in the current epoch (0 before any).
    pub last_round: u64,
    /// Anchor slots each committee member led this epoch.
    pub expected: BTreeMap<[u8; 32], u64>,
    /// Of those, slots whose anchor committed.
    pub authored: BTreeMap<[u8; 32], u64>,
}

/// A validator's id: a domain-separated hash of its verifying key.
#[must_use]
pub fn validator_id(key: &[u8]) -> [u8; 32] {
    blake3::derive_key("maya2c validator id v1", key)
}

/// `k:key:<id>`.
#[must_use]
pub fn key_record(id: &[u8; 32]) -> Vec<u8> {
    [KEY_PREFIX, id.as_slice()].concat()
}

/// `k:cmt:<epoch>`, big-endian so epochs sort.
#[must_use]
pub fn committee_record(epoch: u64) -> Vec<u8> {
    [COMMITTEE_PREFIX, &epoch.to_be_bytes()].concat()
}

/// `k:cmw:<epoch>`: the committee's voting weights, in committee order.
#[must_use]
pub fn committee_weights_record(epoch: u64) -> Vec<u8> {
    [WEIGHTS_PREFIX, &epoch.to_be_bytes()].concat()
}

/// Encodes voting weights (one `u64` per member, committee order).
#[must_use]
pub fn encode_weights(weights: &[u64]) -> Vec<u8> {
    let mut out = (weights.len() as u64).to_le_bytes().to_vec();
    for w in weights {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

/// Decodes voting weights.
///
/// # Errors
///
/// [`NodeError::Decode`] on malformed bytes.
pub fn decode_weights(bytes: &[u8]) -> Result<Vec<u64>> {
    let mut r = ByteReader::new(bytes);
    let n = r.read_collection_len(8)?;
    let weights = (0..n).map(|_| r.read_u64()).collect::<Result<Vec<_>>>()?;
    r.finish()?;
    Ok(weights)
}

/// Each member's total stake (self bond plus delegations), committee order:
/// the weights an epoch votes with.
///
/// The DAG-BFT engine weighs votes in `u64` (summed in `u128`). Stakes are
/// 256-bit (ADR-046), so when the largest stake does not fit a `u64`, every
/// weight is shifted right by the same number of bits, which keeps each
/// member's share to within 2^-63 of the largest stake. While every stake fits
/// a `u64` the shift is zero and the weights are the stakes, exactly as before.
/// A member with any stake keeps a weight of at least one.
#[must_use]
pub fn stake_weights(staking: &maya_staking::Staking, active: &[[u8; 32]]) -> Vec<u64> {
    let stakes: Vec<maya_staking::Amount> = active
        .iter()
        .map(|id| {
            staking.validators.get(id).map_or(
                maya_staking::Amount::ZERO,
                maya_staking::ValidatorRecord::total,
            )
        })
        .collect();
    let largest = stakes
        .iter()
        .copied()
        .max()
        .unwrap_or(maya_staking::Amount::ZERO);
    let shift = largest.bits().saturating_sub(u64::BITS);
    stakes
        .iter()
        .map(|stake| {
            if stake.is_zero() {
                0
            } else {
                // Below 2^64 after the shift, by the choice of `shift`.
                stake.shr(shift).saturating_to_u64().max(1)
            }
        })
        .collect()
}

/// Encodes a committee (ids in order).
#[must_use]
pub fn encode_committee(ids: &[[u8; 32]]) -> Vec<u8> {
    let mut out = (ids.len() as u64).to_le_bytes().to_vec();
    for id in ids {
        out.extend_from_slice(id);
    }
    out
}

/// Decodes a committee.
///
/// # Errors
///
/// [`NodeError::Decode`] on malformed bytes.
pub fn decode_committee(bytes: &[u8]) -> Result<Vec<[u8; 32]>> {
    let mut r = ByteReader::new(bytes);
    let n = r.read_collection_len(32)?;
    let ids = (0..n).map(|_| r.read_array()).collect::<Result<Vec<_>>>()?;
    r.finish()?;
    Ok(ids)
}

pub(crate) fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A 256-bit amount, 32 bytes little-endian (ADR-046).
pub(crate) fn put_amount(out: &mut Vec<u8>, v: Amount) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn read_amount(r: &mut ByteReader<'_>) -> Result<Amount> {
    Ok(Amount::from_le_bytes(r.read_array()?))
}

fn put_params(out: &mut Vec<u8>, p: &Params) {
    put_amount(out, p.min_self_bond);
    put_amount(out, p.min_delegation);
    put_u16(out, p.max_validators);
    put_u64(out, p.unbonding_epochs);
    put_u16(out, p.double_sign_slash_bps);
    put_u16(out, p.downtime_slash_bps);
    put_u16(out, p.downtime_threshold_bps);
    put_u64(out, p.jail_epochs);
    put_u16(out, p.max_commission_bps);
}

pub(crate) fn read_u16(r: &mut ByteReader<'_>) -> Result<u16> {
    Ok(u16::from_le_bytes(r.read_array()?))
}

fn read_params(r: &mut ByteReader<'_>) -> Result<Params> {
    Ok(Params {
        min_self_bond: read_amount(r)?,
        min_delegation: read_amount(r)?,
        max_validators: read_u16(r)?,
        unbonding_epochs: r.read_u64()?,
        double_sign_slash_bps: read_u16(r)?,
        downtime_slash_bps: read_u16(r)?,
        downtime_threshold_bps: read_u16(r)?,
        jail_epochs: r.read_u64()?,
        max_commission_bps: read_u16(r)?,
    })
}

pub(crate) fn put_status(out: &mut Vec<u8>, s: Status) {
    match s {
        Status::Bonded => out.push(0),
        Status::Jailed { until } => {
            out.push(1);
            put_u64(out, until);
        }
        Status::Tombstoned => out.push(2),
    }
}

pub(crate) fn read_status(r: &mut ByteReader<'_>) -> Result<Status> {
    match r.read_u8()? {
        0 => Ok(Status::Bonded),
        1 => Ok(Status::Jailed {
            until: r.read_u64()?,
        }),
        2 => Ok(Status::Tombstoned),
        t => Err(NodeError::Decode(format!("staking status tag {t}"))),
    }
}

fn put_counts(out: &mut Vec<u8>, m: &BTreeMap<[u8; 32], u64>) {
    put_u64(out, m.len() as u64);
    for (id, n) in m {
        out.extend_from_slice(id);
        put_u64(out, *n);
    }
}

fn read_counts(r: &mut ByteReader<'_>) -> Result<BTreeMap<[u8; 32], u64>> {
    let n = r.read_record_len(40)?;
    (0..n)
        .map(|_| Ok((r.read_array()?, r.read_u64()?)))
        .collect()
}

/// The `k:state` record: everything about staking that is not one
/// validator's, one delegation's, one unbonding entry's or one payout's.
/// Linear in the committee, never in validators or delegations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Meta {
    /// The rules' parameters.
    pub(crate) params: Params,
    /// Current epoch.
    pub(crate) epoch: u64,
    /// The committee for the current epoch, in committee order.
    pub(crate) active: Vec<[u8; 32]>,
    /// Blocks per epoch. From genesis.
    pub(crate) epoch_blocks: u64,
    /// Round of the last committed anchor in the current epoch (0 before any).
    pub(crate) last_round: u64,
    /// Anchor slots each committee member led this epoch.
    pub(crate) expected: BTreeMap<[u8; 32], u64>,
    /// Of those, slots whose anchor committed.
    pub(crate) authored: BTreeMap<[u8; 32], u64>,
    /// Delegation records in state, against the ceiling in `staking_store`.
    pub(crate) delegation_records: u64,
}

impl Meta {
    /// Encodes the record. Canonical: every map is written in key order.
    #[must_use]
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = vec![RECORD_VERSION];
        put_params(&mut out, &self.params);
        put_u64(&mut out, self.epoch);
        out.extend_from_slice(&encode_committee(&self.active));
        put_u64(&mut out, self.epoch_blocks);
        put_u64(&mut out, self.last_round);
        put_counts(&mut out, &self.expected);
        put_counts(&mut out, &self.authored);
        put_u64(&mut out, self.delegation_records);
        out
    }

    /// Decodes the record.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong version, malformed bytes, parameters
    /// `maya-staking` refuses, or a zero epoch length.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != RECORD_VERSION {
            return Err(NodeError::Decode("staking record version".to_string()));
        }
        let params = read_params(&mut r)?;
        if !params.is_valid() {
            return Err(NodeError::Decode("staking params out of range".to_string()));
        }
        let epoch = r.read_u64()?;
        let n = r.read_record_len(32)?;
        let active = (0..n).map(|_| r.read_array()).collect::<Result<_>>()?;
        let meta = Self {
            params,
            epoch,
            active,
            epoch_blocks: r.read_u64()?,
            last_round: r.read_u64()?,
            expected: read_counts(&mut r)?,
            authored: read_counts(&mut r)?,
            delegation_records: r.read_u64()?,
        };
        r.finish()?;
        if meta.epoch_blocks == 0 {
            return Err(NodeError::Decode(
                "staking epoch_blocks is zero".to_string(),
            ));
        }
        Ok(meta)
    }

    /// An in-memory view holding this record and nothing else: no
    /// validator, delegation or unbonding entry is loaded yet.
    ///
    /// # Errors
    ///
    /// Parameters `maya-staking` refuses (checked at decode already).
    pub(crate) fn view(&self) -> Result<StakingRecord> {
        let mut staking = Staking::new(self.params)
            .map_err(|e| NodeError::Decode(format!("staking params: {e:?}")))?;
        staking.epoch = self.epoch;
        staking.active.clone_from(&self.active);
        Ok(StakingRecord {
            staking,
            epoch_blocks: self.epoch_blocks,
            last_round: self.last_round,
            expected: self.expected.clone(),
            authored: self.authored.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_meta_record_round_trips_byte_for_byte() {
        let meta = Meta {
            params: Params::DEVNET,
            epoch: 3,
            active: vec![[9; 32], [1; 32]],
            epoch_blocks: 600,
            last_round: 44,
            expected: BTreeMap::from([([9; 32], 11)]),
            authored: BTreeMap::from([([9; 32], 10)]),
            delegation_records: 7,
        };
        let bytes = meta.encode();
        assert_eq!(Meta::decode(&bytes).unwrap(), meta);
        assert_eq!(Meta::decode(&bytes).unwrap().encode(), bytes);
        assert!(Meta::decode(&bytes[..bytes.len() - 1]).is_err());
        let mut old = bytes.clone();
        old[0] = 3;
        assert!(
            Meta::decode(&old).is_err(),
            "a v3 single-record state is refused"
        );
    }

    #[test]
    fn a_validator_id_is_the_hash_of_its_key_not_the_key() {
        assert_ne!(validator_id(&[1; 1952]), validator_id(&[2; 1952]));
        assert_eq!(validator_id(&[1; 1952]).len(), 32);
    }
}
