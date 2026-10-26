//! Staking records under `k:` (ADR-028), committed as their own state layer.
//!
//! | key | value |
//! |---|---|
//! | `k:state` | [`StakingRecord`]: the `maya-staking` state plus the node's slot counts |
//! | `k:key:<id>` | the validator's 1,952-byte ML-DSA-65 verifying key |
//! | `k:cmt:<epoch be>` | that epoch's committee, ids in committee order |
//!
//! One record for the whole staking state is a deliberate v1 choice: every
//! change rewrites it, which is linear in validators and delegations. It keeps
//! the codec to one function pair and the undo journal to one entry per block.
//! A thousand validators is well under a megabyte; revisit before it is not.
//!
//! **Presence is activation.** A chain whose genesis configures no staking has
//! no `k:state`, the layer is absent from its root, and a staking transaction
//! is refused as inactive — the same "never present before activation, so it
//! moves no existing root" rule the `IoT` and threat-intel layers follow.

use std::collections::{BTreeMap, BTreeSet};

use maya_staking::{Amount, Params, Staking, Status, Unbonding, ValidatorRecord};

/// Staking record format. 2 since ADR-046 widened every amount to 256 bits; 3
/// adds the stake added this epoch (earns from the next). A version-1 or -2
/// record is refused, not reinterpreted.
const RECORD_VERSION: u8 = 3;

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Every staking record lives under this prefix.
pub const STAKING_PREFIX: &[u8] = b"k:";

/// The single state record.
pub const STATE_KEY: &[u8] = b"k:state";

const KEY_PREFIX: &[u8] = b"k:key:";
const COMMITTEE_PREFIX: &[u8] = b"k:cmt:";
/// Stake-weighted committees (ADR-040 part 2): each member's stake, frozen
/// at the epoch boundary. Present only on a chain whose genesis asked for
/// it, so a chain without it keeps the state root it always had.
const WEIGHTS_PREFIX: &[u8] = b"k:cmw:";

/// The staking state plus what the node counts between epoch boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StakingRecord {
    /// The rules' state.
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

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A 256-bit amount, 32 bytes little-endian (ADR-046).
fn put_amount(out: &mut Vec<u8>, v: Amount) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn read_amount(r: &mut ByteReader<'_>) -> Result<Amount> {
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

fn read_u16(r: &mut ByteReader<'_>) -> Result<u16> {
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

fn put_status(out: &mut Vec<u8>, s: Status) {
    match s {
        Status::Bonded => out.push(0),
        Status::Jailed { until } => {
            out.push(1);
            put_u64(out, until);
        }
        Status::Tombstoned => out.push(2),
    }
}

fn read_status(r: &mut ByteReader<'_>) -> Result<Status> {
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
    let n = r.read_collection_len(40)?;
    (0..n)
        .map(|_| Ok((r.read_array()?, r.read_u64()?)))
        .collect()
}

/// Reads the pending sections. Keys must be strictly ascending (one canonical
/// encoding per map), entries non-zero, and each entry no larger than the
/// stake it belongs to, so a corrupt record cannot make stake look larger.
fn decode_pending(r: &mut ByteReader<'_>, staking: &mut Staking) -> Result<()> {
    let refused = || NodeError::Decode("staking pending stake out of order or range".into());
    let n = r.read_collection_len(32 + 32)?;
    let mut prev: Option<[u8; 32]> = None;
    for _ in 0..n {
        let id: [u8; 32] = r.read_array()?;
        if prev.is_some_and(|p| p >= id) {
            return Err(refused());
        }
        prev = Some(id);
        let amount = read_amount(r)?;
        let bond = staking.validators.get(&id).map(|v| v.self_bond);
        if amount.is_zero() || bond.is_none_or(|b| amount > b) {
            return Err(refused());
        }
        staking.pending_self.insert(id, amount);
    }
    let n = r.read_collection_len(32 + 32 + 32)?;
    let mut prev: Option<([u8; 32], [u8; 32])> = None;
    for _ in 0..n {
        let key: ([u8; 32], [u8; 32]) = (r.read_array()?, r.read_array()?);
        if prev.is_some_and(|p| p >= key) {
            return Err(refused());
        }
        prev = Some(key);
        let amount = read_amount(r)?;
        let held = staking.delegations.get(&key).copied();
        if amount.is_zero() || held.is_none_or(|h| amount > h) {
            return Err(refused());
        }
        staking.pending_delegations.insert(key, amount);
    }
    Ok(())
}

impl StakingRecord {
    /// Encodes the record. Canonical: every map is written in key order.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let s = &self.staking;
        // Record version 2: every amount is 256-bit (ADR-046).
        let mut out = vec![RECORD_VERSION];
        put_params(&mut out, &s.params);
        put_u64(&mut out, s.epoch);
        put_u64(&mut out, s.validators.len() as u64);
        for (id, v) in &s.validators {
            out.extend_from_slice(id);
            out.extend_from_slice(&v.operator);
            put_amount(&mut out, v.self_bond);
            put_amount(&mut out, v.delegated);
            put_u16(&mut out, v.commission_bps);
            put_status(&mut out, v.status);
        }
        put_u64(&mut out, s.delegations.len() as u64);
        for ((d, v), amount) in &s.delegations {
            out.extend_from_slice(d);
            out.extend_from_slice(v);
            put_amount(&mut out, *amount);
        }
        put_u64(&mut out, s.unbonding.len() as u64);
        for u in &s.unbonding {
            out.extend_from_slice(&u.owner);
            out.extend_from_slice(&u.validator);
            put_amount(&mut out, u.amount);
            put_u64(&mut out, u.release_epoch);
        }
        out.extend_from_slice(&encode_committee(&s.active));
        put_u64(&mut out, s.retired.len() as u64);
        for id in &s.retired {
            out.extend_from_slice(id);
        }
        put_u64(&mut out, s.pending_self.len() as u64);
        for (id, amount) in &s.pending_self {
            out.extend_from_slice(id);
            put_amount(&mut out, *amount);
        }
        put_u64(&mut out, s.pending_delegations.len() as u64);
        for ((d, v), amount) in &s.pending_delegations {
            out.extend_from_slice(d);
            out.extend_from_slice(v);
            put_amount(&mut out, *amount);
        }
        put_u64(&mut out, self.epoch_blocks);
        put_u64(&mut out, self.last_round);
        put_counts(&mut out, &self.expected);
        put_counts(&mut out, &self.authored);
        out
    }

    /// Decodes a record.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong version, malformed bytes, or
    /// parameters `maya-staking` refuses.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != RECORD_VERSION {
            return Err(NodeError::Decode("staking record version".to_string()));
        }
        let params = read_params(&mut r)?;
        let mut staking = Staking::new(params)
            .map_err(|e| NodeError::Decode(format!("staking params: {e:?}")))?;
        staking.epoch = r.read_u64()?;
        // Id, operator, two amounts, commission, status tag.
        let n = r.read_collection_len(32 + 32 + 32 + 32 + 2 + 1)?;
        for _ in 0..n {
            let id: [u8; 32] = r.read_array()?;
            let record = ValidatorRecord {
                operator: r.read_array()?,
                self_bond: read_amount(&mut r)?,
                delegated: read_amount(&mut r)?,
                commission_bps: read_u16(&mut r)?,
                status: read_status(&mut r)?,
            };
            staking.validators.insert(id, record);
        }
        let n = r.read_collection_len(32 + 32 + 32)?;
        for _ in 0..n {
            let key = (r.read_array()?, r.read_array()?);
            staking.delegations.insert(key, read_amount(&mut r)?);
        }
        let n = r.read_collection_len(32 + 32 + 32 + 8)?;
        for _ in 0..n {
            staking.unbonding.push(Unbonding {
                owner: r.read_array()?,
                validator: r.read_array()?,
                amount: read_amount(&mut r)?,
                release_epoch: r.read_u64()?,
            });
        }
        let n = r.read_collection_len(32)?;
        staking.active = (0..n).map(|_| r.read_array()).collect::<Result<_>>()?;
        let n = r.read_collection_len(32)?;
        staking.retired = (0..n)
            .map(|_| r.read_array())
            .collect::<Result<BTreeSet<_>>>()?;
        decode_pending(&mut r, &mut staking)?;
        let record = Self {
            staking,
            epoch_blocks: r.read_u64()?,
            last_round: r.read_u64()?,
            expected: read_counts(&mut r)?,
            authored: read_counts(&mut r)?,
        };
        r.finish()?;
        if record.epoch_blocks == 0 {
            return Err(NodeError::Decode(
                "staking epoch_blocks is zero".to_string(),
            ));
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_populated_record_round_trips_byte_for_byte() {
        let mut staking = Staking::new(Params::DEVNET).unwrap();
        staking
            .register(
                [1; 32],
                [9; 32],
                maya_ledger_math::Amount::from_u64(5_000),
                100,
            )
            .unwrap();
        staking
            .register(
                [2; 32],
                [8; 32],
                maya_ledger_math::Amount::from_u64(7_000),
                0,
            )
            .unwrap();
        staking
            .delegate([3; 32], [9; 32], maya_ledger_math::Amount::from_u64(400))
            .unwrap();
        staking
            .undelegate([3; 32], [9; 32], maya_ledger_math::Amount::from_u64(100))
            .unwrap();
        staking.slash_double_sign([8; 32]).unwrap();
        staking.active = vec![[9; 32]];
        let record = StakingRecord {
            staking,
            epoch_blocks: 600,
            last_round: 44,
            expected: BTreeMap::from([([9; 32], 11)]),
            authored: BTreeMap::from([([9; 32], 10)]),
        };
        let bytes = record.encode();
        let back = StakingRecord::decode(&bytes).unwrap();
        assert_eq!(back, record);
        assert!(!back.staking.pending_delegations.is_empty());
        assert_eq!(back.encode(), bytes);
        assert!(StakingRecord::decode(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn a_validator_id_is_the_hash_of_its_key_not_the_key() {
        assert_ne!(validator_id(&[1; 1952]), validator_id(&[2; 1952]));
        assert_eq!(validator_id(&[1; 1952]).len(), 32);
    }
}

#[cfg(test)]
mod pending_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// A valid record with no pending stake, cut before its trailing
    /// sections, so each test can append its own pending sections.
    fn prefix() -> Vec<u8> {
        let mut staking = Staking::new(Params::DEVNET).unwrap();
        staking
            .register([1; 32], [8; 32], Amount::from_u64(5_000), 0)
            .unwrap();
        staking
            .register([2; 32], [9; 32], Amount::from_u64(5_000), 0)
            .unwrap();
        staking
            .delegate([3; 32], [9; 32], Amount::from_u64(400))
            .unwrap();
        staking.pending_self.clear();
        staking.pending_delegations.clear();
        let record = StakingRecord {
            staking,
            epoch_blocks: 600,
            last_round: 0,
            expected: BTreeMap::new(),
            authored: BTreeMap::new(),
        };
        let bytes = record.encode();
        // Pending counts (16), epoch_blocks, last_round, two empty count maps.
        bytes[..bytes.len() - 48].to_vec()
    }

    /// `((delegator, validator), amount)`.
    type PendingDelegation = (([u8; 32], [u8; 32]), u64);

    fn with_pending(
        prefix: &[u8],
        self_pending: &[([u8; 32], u64)],
        delegated_pending: &[PendingDelegation],
    ) -> Vec<u8> {
        let mut out = prefix.to_vec();
        put_u64(&mut out, self_pending.len() as u64);
        for (id, amount) in self_pending {
            out.extend_from_slice(id);
            put_amount(&mut out, Amount::from_u64(*amount));
        }
        put_u64(&mut out, delegated_pending.len() as u64);
        for ((d, v), amount) in delegated_pending {
            out.extend_from_slice(d);
            out.extend_from_slice(v);
            put_amount(&mut out, Amount::from_u64(*amount));
        }
        put_u64(&mut out, 600);
        put_u64(&mut out, 0);
        put_u64(&mut out, 0);
        put_u64(&mut out, 0);
        out
    }

    #[test]
    fn a_pending_self_section_decodes_and_round_trips() {
        let bytes = with_pending(&prefix(), &[([9; 32], 300)], &[]);
        let record = StakingRecord::decode(&bytes).unwrap();
        assert_eq!(
            record.staking.pending_self.get(&[9; 32]),
            Some(&Amount::from_u64(300))
        );
        assert_eq!(record.encode(), bytes);
    }

    #[test]
    fn a_pending_delegation_section_decodes_and_round_trips() {
        let bytes = with_pending(&prefix(), &[], &[(([3; 32], [9; 32]), 150)]);
        let record = StakingRecord::decode(&bytes).unwrap();
        assert_eq!(record.encode(), bytes);
    }

    #[test]
    fn out_of_order_pending_keys_are_refused() {
        let bytes = with_pending(&prefix(), &[([9; 32], 300), ([8; 32], 300)], &[]);
        assert!(StakingRecord::decode(&bytes).is_err());
    }

    #[test]
    fn duplicate_pending_keys_are_refused() {
        let bytes = with_pending(&prefix(), &[([8; 32], 300), ([8; 32], 300)], &[]);
        assert!(StakingRecord::decode(&bytes).is_err());
    }

    #[test]
    fn pending_above_the_bond_is_refused() {
        let bytes = with_pending(&prefix(), &[([8; 32], 5_001)], &[]);
        assert!(StakingRecord::decode(&bytes).is_err());
        let at_bond = with_pending(&prefix(), &[([8; 32], 5_000)], &[]);
        assert!(StakingRecord::decode(&at_bond).is_ok());
    }

    #[test]
    fn pending_above_the_delegation_is_refused() {
        let bytes = with_pending(&prefix(), &[], &[(([3; 32], [9; 32]), 401)]);
        assert!(StakingRecord::decode(&bytes).is_err());
    }

    #[test]
    fn pending_for_an_unknown_validator_or_delegation_is_refused() {
        let unknown_validator = with_pending(&prefix(), &[([7; 32], 1)], &[]);
        assert!(StakingRecord::decode(&unknown_validator).is_err());
        let unknown_delegation = with_pending(&prefix(), &[], &[(([4; 32], [9; 32]), 1)]);
        assert!(StakingRecord::decode(&unknown_delegation).is_err());
    }

    #[test]
    fn zero_pending_entries_are_refused() {
        let bytes = with_pending(&prefix(), &[([8; 32], 0)], &[]);
        assert!(StakingRecord::decode(&bytes).is_err());
    }
}
