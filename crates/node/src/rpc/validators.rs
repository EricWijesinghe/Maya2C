//! `get_validators`: who is registered, who is in this epoch's committee, who
//! is jailed and until when. Read-only, from the committed staking record.
//!
//! Why: on 2026-10-25 all four PC validators sat out one epoch (a downtime jail)
//! and the only way to see it was `get_bft_status` reading `validator: null`
//! on each of them. Operators and the explorer need the reason, not the symptom.

use maya_staking::Status;
use serde::{Deserialize, Serialize};

use crate::state::staking::StakingRecord;

/// Most validators one answer lists; the rest are counted in `total`.
pub const MAX_LISTED: usize = 500;

/// One validator, amounts as decimal strings (they are 256-bit).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidatorInfo {
    /// Validator id, hex.
    pub id: String,
    /// The bonding account, hex.
    pub operator: String,
    /// The operator's own stake.
    pub self_bond: String,
    /// Sum of delegations.
    pub delegated: String,
    /// Commission in basis points.
    pub commission_bps: u16,
    /// `bonded`, `jailed` or `tombstoned`.
    pub status: String,
    /// For `jailed`: the first epoch it is eligible again.
    pub jailed_until: Option<u64>,
    /// In the current epoch's committee.
    pub in_committee: bool,
}

/// The answer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidatorsInfo {
    /// Current staking epoch.
    pub epoch: u64,
    /// Blocks per epoch.
    pub epoch_blocks: u64,
    /// Committee size this epoch.
    pub committee: usize,
    /// Registered validators in all.
    pub total: usize,
    /// Committee members first, then by stake, at most [`MAX_LISTED`].
    pub validators: Vec<ValidatorInfo>,
}

/// Builds the answer from the committed record.
#[must_use]
pub fn validators_info(record: &StakingRecord) -> ValidatorsInfo {
    let s = &record.staking;
    let mut rows: Vec<(bool, ValidatorInfo)> = s
        .validators
        .iter()
        .map(|(id, v)| {
            let in_committee = s.active.contains(id);
            let (status, jailed_until) = match v.status {
                Status::Bonded => ("bonded", None),
                Status::Jailed { until } => ("jailed", Some(until)),
                Status::Tombstoned => ("tombstoned", None),
            };
            (
                in_committee,
                ValidatorInfo {
                    id: hex::encode(id),
                    operator: hex::encode(v.operator),
                    self_bond: v.self_bond.to_string(),
                    delegated: v.delegated.to_string(),
                    commission_bps: v.commission_bps,
                    status: status.to_string(),
                    jailed_until,
                    in_committee,
                },
            )
        })
        .collect();
    // Committee first, then larger stake, then id: stable for a given state.
    rows.sort_by(|(a_in, a), (b_in, b)| {
        b_in.cmp(a_in)
            .then_with(|| stake(b).cmp(&stake(a)))
            .then_with(|| a.id.cmp(&b.id))
    });
    ValidatorsInfo {
        epoch: s.epoch,
        epoch_blocks: record.epoch_blocks,
        committee: s.active.len(),
        total: rows.len(),
        validators: rows.into_iter().map(|(_, v)| v).take(MAX_LISTED).collect(),
    }
}

/// Self bond plus delegations, as a number (decimal strings sort by length
/// first, so they cannot be compared as text).
fn stake(v: &ValidatorInfo) -> primitive_types::U256 {
    let parse = |s: &str| primitive_types::U256::from_dec_str(s).unwrap_or_default();
    parse(&v.self_bond).saturating_add(parse(&v.delegated))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use maya_staking::{Amount, Params, Participation, Staking};

    use super::*;

    fn record(staking: Staking) -> StakingRecord {
        StakingRecord {
            staking,
            epoch_blocks: 3600,
            last_round: 0,
            expected: Default::default(),
            authored: Default::default(),
        }
    }

    #[test]
    fn a_jailed_validator_shows_why_and_until_when() {
        let mut s = Staking::new(Params::DEVNET).unwrap();
        let (a, b) = ([1u8; 32], [2u8; 32]);
        s.register([0xA1; 32], a, Amount::from_u64(5_000), 0)
            .unwrap();
        s.register([0xA2; 32], b, Amount::from_u64(9_000), 0)
            .unwrap();
        s.end_epoch(&Participation::default(), Amount::ZERO)
            .unwrap();
        // Validator `a` authored nothing of its 10 slots: downtime.
        let mut p = Participation::default();
        p.expected.insert(a, 10);
        p.expected.insert(b, 10);
        p.authored.insert(b, 10);
        s.end_epoch(&p, Amount::ZERO).unwrap();

        let info = validators_info(&record(s));
        assert_eq!(info.total, 2);
        assert_eq!(info.committee, 1, "the jailed validator left the committee");
        let first = &info.validators[0];
        assert_eq!(first.id, hex::encode(b));
        assert!(first.in_committee && first.status == "bonded");
        let jailed = &info.validators[1];
        assert_eq!(jailed.id, hex::encode(a));
        assert_eq!(jailed.status, "jailed");
        assert!(jailed.jailed_until.is_some() && !jailed.in_committee);
    }

    #[test]
    fn stake_orders_numerically_not_as_text() {
        let mut s = Staking::new(Params::DEVNET).unwrap();
        s.register([0xA1; 32], [1; 32], Amount::from_u64(99_999), 0)
            .unwrap();
        s.register([0xA2; 32], [2; 32], Amount::from_u64(100_000), 0)
            .unwrap();
        let mut r = record(s);
        r.staking.active.clear(); // neither in committee: ordering falls to stake
        let info = validators_info(&r);
        assert_eq!(info.validators[0].id, hex::encode([2u8; 32]));
    }
}
