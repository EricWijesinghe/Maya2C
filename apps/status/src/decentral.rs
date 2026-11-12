//! How spread out the current committee's stake is, from `get_validators`.
//!
//! Integer arithmetic only. A threshold is checked by cross-multiplying
//! (`coalition * den > total * num`), never by dividing, so no share is
//! rounded into or out of a threshold. Any value that cannot be read exactly
//! makes the whole measurement absent (MISSING on the page), never a guess.
//!
//! Weights here are current bonded stake (`self_bond + delegated`). The
//! engine votes with weights frozen at each epoch boundary, which the node
//! does not expose over RPC, so these are the live stakes, not those weights.

use primitive_types::U256;
use serde::{Deserialize, Serialize};

/// Basis points in the whole.
const WHOLE_BPS: u64 = 10_000;

/// One seat as `get_validators` reports it. Other fields are ignored.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Seat {
    /// Staking id, hex: the key a watched node's `validator_id` is matched on.
    #[serde(default)]
    pub id: String,
    /// In the current epoch's committee.
    pub in_committee: bool,
    /// Operator's own stake, decimal.
    pub self_bond: String,
    /// Sum of delegations, decimal.
    pub delegated: String,
}

/// The `get_validators` answer, trimmed to what this measures.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct StakeSet {
    /// Current staking epoch.
    pub epoch: u64,
    /// Committee size the node reports; the listing is capped, so it can
    /// be shorter than the committee.
    pub committee: usize,
    /// Registered validators, committee first.
    pub validators: Vec<Seat>,
}

/// The committee's spread, served as JSON.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Decentralization {
    /// Staking epoch the committee belongs to.
    pub epoch: u64,
    /// Committee members with stake.
    pub committee: usize,
    /// Committee stake, decimal.
    pub total_stake: String,
    /// Fewest members holding more than a third of the stake.
    pub nakamoto_third: usize,
    /// Fewest members holding more than two thirds of the stake.
    pub nakamoto_two_thirds: usize,
    /// Largest single member's share, in basis points (floor).
    pub largest_share_bps: u64,
}

/// A seat's stake: `self_bond + delegated`, or `None` if either is not a
/// decimal number or the sum overflows.
#[must_use]
pub fn stake(seat: &Seat) -> Option<U256> {
    let bond = U256::from_dec_str(&seat.self_bond).ok()?;
    let delegated = U256::from_dec_str(&seat.delegated).ok()?;
    bond.checked_add(delegated)
}

/// Fewest leading stakes (largest first) whose sum is strictly more than
/// `num / den` of `total`. `None` when `total` is zero, there are no stakes,
/// or the cross-multiplication overflows.
#[must_use]
pub fn coalition(stakes_desc: &[U256], total: U256, num: u32, den: u32) -> Option<usize> {
    if total.is_zero() {
        return None;
    }
    let limit = total.checked_mul(U256::from(num))?;
    let mut sum = U256::zero();
    for (i, s) in stakes_desc.iter().enumerate() {
        sum = sum.checked_add(*s)?;
        if sum.checked_mul(U256::from(den))? > limit {
            return Some(i + 1);
        }
    }
    None
}

/// Measures the committee. `None` (MISSING) if there is no committee, a
/// stake cannot be read, or the arithmetic does not fit.
#[must_use]
pub fn measure(set: &StakeSet) -> Option<Decentralization> {
    let members: Vec<&Seat> = set.validators.iter().filter(|s| s.in_committee).collect();
    // A truncated listing would undercount the committee: MISSING, not a guess.
    if members.len() != set.committee {
        return None;
    }
    let mut stakes = members
        .into_iter()
        .map(stake)
        .collect::<Option<Vec<U256>>>()?;
    stakes.retain(|s| !s.is_zero());
    stakes.sort_unstable_by(|a, b| b.cmp(a));
    let total = stakes
        .iter()
        .try_fold(U256::zero(), |acc, s| acc.checked_add(*s))?;
    let largest = *stakes.first()?;
    let largest_bps = largest
        .checked_mul(U256::from(WHOLE_BPS))?
        .checked_div(total)?;
    Some(Decentralization {
        epoch: set.epoch,
        committee: stakes.len(),
        total_stake: total.to_string(),
        nakamoto_third: coalition(&stakes, total, 1, 3)?,
        nakamoto_two_thirds: coalition(&stakes, total, 2, 3)?,
        largest_share_bps: largest_bps.low_u64(),
    })
}

/// One committee seat as the readiness measure sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatClaim {
    /// The seat's live stake (`self_bond + delegated`).
    pub stake: U256,
    /// Some watched node saw the seat's peer connected.
    pub observed: bool,
    /// The lowest protocol version any observer heard the peer claim; 0 for
    /// no claim. A claim is what the peer says of itself, never checked.
    pub claimed: u32,
}

/// How much of the committee's stake claims a protocol version.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Readiness {
    /// The protocol version measured.
    pub protocol: u32,
    /// Committee members measured.
    pub committee: usize,
    /// Their stake, decimal.
    pub total_stake: String,
    /// Members whose peers claim `protocol` or later.
    pub ready_seats: usize,
    /// Their share of the stake, in basis points (floor).
    pub ready_bps: u64,
    /// Members some watched node can see connected.
    pub observed_seats: usize,
    /// Their share of the stake, in basis points (floor).
    pub observed_bps: u64,
}

/// Stake sum of the seats `keep` selects; `None` if it overflows.
fn stake_where(seats: &[SeatClaim], keep: impl Fn(&SeatClaim) -> bool) -> Option<U256> {
    seats
        .iter()
        .filter(|s| keep(s))
        .try_fold(U256::zero(), |acc, s| acc.checked_add(s.stake))
}

/// Share of `part` in `total`, in basis points, floored.
fn bps_of(part: U256, total: U256) -> Option<u64> {
    part.checked_mul(U256::from(WHOLE_BPS))?
        .checked_div(total)
        .map(|v| v.low_u64())
}

/// Share of the committee's stake whose peers claim `protocol` or later.
/// Integer arithmetic. `None` (MISSING) for an empty or weightless committee, or
/// when the arithmetic does not fit.
#[must_use]
pub fn readiness(seats: &[SeatClaim], protocol: u32) -> Option<Readiness> {
    let total = stake_where(seats, |_| true)?;
    if seats.is_empty() || total.is_zero() {
        return None;
    }
    let ready = stake_where(seats, |s| s.claimed >= protocol)?;
    let observed = stake_where(seats, |s| s.observed)?;
    Some(Readiness {
        protocol,
        committee: seats.len(),
        total_stake: total.to_string(),
        ready_seats: seats.iter().filter(|s| s.claimed >= protocol).count(),
        ready_bps: bps_of(ready, total)?,
        observed_seats: seats.iter().filter(|s| s.observed).count(),
        observed_bps: bps_of(observed, total)?,
    })
}

/// The host part of a URL: `127.0.0.1`, `node.example`, `[::1]`.
#[must_use]
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    if let Some(stripped) = authority.strip_prefix('[') {
        return stripped
            .split_once(']')
            .map_or(authority, |(h, _)| &authority[..h.len() + 2]);
    }
    authority.split(':').next().unwrap_or(authority)
}

/// The one host every URL names, when there are at least two URLs and they
/// all name the same host.
#[must_use]
pub fn one_host<'a>(hosts: &[&'a str]) -> Option<&'a str> {
    let first = *hosts.first()?;
    (hosts.len() >= 2 && hosts.iter().all(|h| *h == first)).then_some(first)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn seat(bond: u64, in_committee: bool) -> Seat {
        Seat {
            in_committee,
            self_bond: bond.to_string(),
            delegated: "0".into(),
            ..Seat::default()
        }
    }

    fn set(bonds: &[u64]) -> StakeSet {
        StakeSet {
            epoch: 7,
            committee: bonds.len(),
            validators: bonds.iter().map(|b| seat(*b, true)).collect(),
        }
    }

    #[test]
    fn equal_weights_need_a_head_count_over_each_threshold() {
        // Four equal seats: 2/4 > 1/3, 2/4 is not > 2/3, 3/4 is.
        let m = measure(&set(&[5, 5, 5, 5])).unwrap();
        assert_eq!((m.nakamoto_third, m.nakamoto_two_thirds), (2, 3));
        assert_eq!(m.largest_share_bps, 2_500);
        assert_eq!(m.committee, 4);
    }

    #[test]
    fn two_thirds_is_strict() {
        // Six equal seats: 4/6 is exactly 2/3, not more, so 5 are needed.
        let m = measure(&set(&[1, 1, 1, 1, 1, 1])).unwrap();
        assert_eq!(m.nakamoto_two_thirds, 5);
        assert_eq!(m.nakamoto_third, 3);
    }

    #[test]
    fn one_whale_alone_passes_both_thresholds() {
        let m = measure(&set(&[70, 10, 10, 10])).unwrap();
        assert_eq!((m.nakamoto_third, m.nakamoto_two_thirds), (1, 1));
        assert_eq!(m.largest_share_bps, 7_000);
    }

    #[test]
    fn three_equal_seats_split_the_thresholds() {
        // 1/3 is not more than 1/3; 2/3 is not more than 2/3.
        let m = measure(&set(&[1, 1, 1])).unwrap();
        assert_eq!((m.nakamoto_third, m.nakamoto_two_thirds), (2, 3));
        assert_eq!(m.largest_share_bps, 3_333);
    }

    #[test]
    fn a_single_member_is_its_own_coalition() {
        let m = measure(&set(&[9])).unwrap();
        assert_eq!((m.nakamoto_third, m.nakamoto_two_thirds), (1, 1));
        assert_eq!(m.largest_share_bps, 10_000);
    }

    #[test]
    fn order_of_the_input_does_not_change_the_answer() {
        let a = measure(&set(&[10, 40, 25, 25])).unwrap();
        let b = measure(&set(&[25, 25, 40, 10])).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn non_committee_and_zero_stake_seats_do_not_count() {
        let mut s = set(&[5, 5, 5]);
        s.validators.push(seat(1_000, false));
        s.validators.push(seat(0, true));
        s.committee = 4; // the zero-stake seat is listed and counted as a member
        let m = measure(&s).unwrap();
        assert_eq!(m.committee, 3);
        assert_eq!(m.total_stake, "15");
    }

    #[test]
    fn a_truncated_listing_is_absent_not_undercounted() {
        let mut s = set(&[5, 5, 5, 5]);
        s.committee = 6; // the node says six; only four were listed
        assert!(measure(&s).is_none());
    }

    #[test]
    fn missing_data_is_absent_not_estimated() {
        assert!(measure(&StakeSet::default()).is_none());
        assert!(measure(&set(&[0, 0])).is_none());
        let mut bad = set(&[5, 5]);
        bad.validators[1].self_bond = "not a number".into();
        assert!(measure(&bad).is_none());
    }

    #[test]
    fn stakes_past_u256_are_absent_not_wrapped() {
        let max = U256::MAX.to_string();
        let s = StakeSet {
            epoch: 1,
            committee: 2,
            validators: vec![
                Seat {
                    in_committee: true,
                    self_bond: max.clone(),
                    delegated: "0".into(),
                    ..Seat::default()
                },
                Seat {
                    in_committee: true,
                    self_bond: "1".into(),
                    delegated: "0".into(),
                    ..Seat::default()
                },
            ],
        };
        assert!(measure(&s).is_none());
    }

    #[test]
    fn stake_sums_both_parts_of_a_seat() {
        let s = Seat {
            in_committee: true,
            self_bond: "3".into(),
            delegated: "4".into(),
            ..Seat::default()
        };
        assert_eq!(stake(&s), Some(U256::from(7u64)));
    }

    #[test]
    fn host_is_read_from_plain_and_bracketed_urls() {
        assert_eq!(host_of("http://127.0.0.1:8545"), "127.0.0.1");
        assert_eq!(host_of("https://node.example/rpc?x=1"), "node.example");
        assert_eq!(host_of("http://[::1]:8545/"), "[::1]");
        assert_eq!(host_of("node.example:9"), "node.example");
    }

    #[test]
    fn one_host_needs_two_urls_on_the_same_host() {
        assert_eq!(one_host(&["127.0.0.1", "127.0.0.1"]), Some("127.0.0.1"));
        assert_eq!(one_host(&["127.0.0.1"]), None);
        assert_eq!(one_host(&["a.example", "b.example"]), None);
        assert_eq!(one_host(&[]), None);
    }

    fn claim(stake: u64, observed: bool, claimed: u32) -> SeatClaim {
        SeatClaim {
            stake: U256::from(stake),
            observed,
            claimed,
        }
    }

    #[test]
    fn an_empty_or_weightless_committee_has_no_readiness() {
        assert_eq!(readiness(&[], 3), None);
        assert_eq!(readiness(&[claim(0, true, 3)], 3), None);
    }

    #[test]
    fn a_fully_claimed_committee_is_all_ready() {
        let r = readiness(&[claim(5, true, 3), claim(5, true, 4)], 3).unwrap();
        assert_eq!((r.ready_bps, r.ready_seats), (10_000, 2));
        assert_eq!((r.observed_bps, r.observed_seats), (10_000, 2));
    }

    #[test]
    fn an_old_claim_or_no_claim_is_not_ready() {
        // Claimed 2 against a target of 3, and one seat never heard from.
        let r = readiness(&[claim(5, true, 2), claim(5, false, 0)], 3).unwrap();
        assert_eq!((r.ready_bps, r.ready_seats), (0, 0));
        assert_eq!((r.observed_bps, r.observed_seats), (5_000, 1));
    }

    #[test]
    fn readiness_is_weighted_by_stake_and_floored() {
        // One seat of 1 ready against 3 of 4: 2500 bps.
        let r = readiness(&[claim(1, true, 3), claim(3, true, 2)], 3).unwrap();
        assert_eq!(r.ready_bps, 2_500);
        // 1 of 3 is 3333.33 bps: floored, never rounded up to a threshold.
        let r = readiness(&[claim(1, true, 3), claim(2, true, 2)], 3).unwrap();
        assert_eq!(r.ready_bps, 3_333);
    }

    #[test]
    fn an_overflowing_stake_sum_is_missing_not_guessed() {
        let huge = SeatClaim {
            stake: U256::MAX,
            observed: true,
            claimed: 3,
        };
        assert_eq!(readiness(&[huge, claim(1, true, 3)], 3), None);
    }
}
