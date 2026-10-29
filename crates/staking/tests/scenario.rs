//! A thousand validators join, delegate, leave and get slashed under
//! Byzantine faults (Master Prompt 4 §9), and after every step value is
//! conserved and the committee obeys its rules.
//!
//! Deterministic: a fixed-seed LCG picks every action, so a failure replays.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use maya_staking::{
    Amount, Effect, Params, Participation, StakeError, Staking, Status, concentration,
};

const VALIDATORS: u32 = 1_000;
const EPOCHS: u64 = 30;
const REWARD_PER_EPOCH: u64 = 1_000_000;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// An amount from a small test number.
fn a(n: u64) -> Amount {
    Amount::from_u64(n)
}

/// A test amount as `u128`: every amount this file builds is far below 2^128.
fn wide(x: Amount) -> u128 {
    x.to_u128().expect("test amounts fit u128")
}

fn id(n: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..4].copy_from_slice(&n.to_le_bytes());
    out
}

fn account(n: u32) -> [u8; 32] {
    let mut out = [0xAAu8; 32];
    out[..4].copy_from_slice(&n.to_le_bytes());
    out
}

/// Totals over every effect the module has ever produced.
#[derive(Default)]
struct Ledger {
    debited: u128,
    credited: u128,
    burned: u128,
}

impl Ledger {
    fn apply(&mut self, effects: &[Effect]) {
        for e in effects {
            match e {
                Effect::Debit(_, x) => self.debited += wide(*x),
                Effect::Credit(_, x) => self.credited += wide(*x),
                Effect::Burn(x) => self.burned += wide(*x),
            }
        }
    }
}

fn check(s: &Staking, ledger: &Ledger, rewards_in: u128) {
    assert_eq!(
        ledger.debited + rewards_in,
        wide(s.held().expect("held fits")) + ledger.credited + ledger.burned,
        "value leaked or appeared at epoch {}",
        s.epoch
    );
    assert!(s.active.len() <= usize::from(s.params.max_validators));
    let unique: BTreeSet<_> = s.active.iter().collect();
    assert_eq!(
        unique.len(),
        s.active.len(),
        "a validator twice in the committee"
    );
    for a in &s.active {
        let v = &s.validators[a];
        assert!(
            !matches!(v.status, Status::Tombstoned),
            "tombstoned in committee"
        );
        assert!(v.self_bond >= s.params.min_self_bond);
    }
}

#[test]
#[allow(clippy::too_many_lines)] // one scenario, read top to bottom
fn a_thousand_validators_under_byzantine_faults_conserve_value() {
    let mut rng = Lcg(0x5EED);
    let mut s = Staking::new(Params::DEVNET).unwrap();
    let mut ledger = Ledger::default();
    let mut rewards_in: u128 = 0;
    let mut tombstoned = BTreeSet::new();

    for n in 0..VALIDATORS {
        let bond = 1_000 + rng.below(100_000);
        let effects = s
            .register(
                account(n),
                id(n),
                a(bond),
                u16::try_from(rng.below(2_000)).unwrap(),
            )
            .unwrap();
        ledger.apply(&effects);
    }
    assert_eq!(
        s.register(account(0), id(0), a(5_000), 0),
        Err(StakeError::AlreadyRegistered)
    );
    let first = s
        .end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    ledger.apply(&first.effects);
    assert_eq!(
        first.active.len(),
        usize::from(Params::DEVNET.max_validators)
    );
    check(&s, &ledger, rewards_in);

    for _ in 0..EPOCHS {
        for _ in 0..200 {
            let n = u32::try_from(rng.below(u64::from(VALIDATORS))).unwrap();
            let who = account(10_000 + u32::try_from(rng.below(500)).unwrap());
            let result = match rng.below(6) {
                0 => s.delegate(who, id(n), a(10 + rng.below(50_000))),
                1 => {
                    let held: Vec<_> = s.delegations.keys().copied().collect();
                    if held.is_empty() {
                        continue;
                    }
                    let (d, v) = held[usize::try_from(rng.below(held.len() as u64)).unwrap()];
                    let amount = s.delegations[&(d, v)].to_u64().unwrap();
                    s.undelegate(d, v, a(1 + rng.below(amount)))
                }
                2 => s.bond_more(account(n), id(n), a(rng.below(10_000))),
                3 => s.unbond(account(n), id(n), a(rng.below(200_000))),
                4 => s.delegate(who, id(n), a(rng.below(9))), // below minimum
                _ => s.unbond(account(n + 1), id(n), Amount::ONE), // not the operator
            };
            if let Ok(effects) = result {
                ledger.apply(&effects);
            }
            check(&s, &ledger, rewards_in);
        }
        // Byzantine faults: a few committee members double-sign.
        for _ in 0..rng.below(3) {
            if s.active.is_empty() {
                break;
            }
            let who = s.active[usize::try_from(rng.below(s.active.len() as u64)).unwrap()];
            ledger.apply(&s.slash_double_sign(who).unwrap());
            // A second report is harmless.
            assert!(s.slash_double_sign(who).unwrap().is_empty());
            tombstoned.insert(who);
        }
        // Participation: a tenth of the committee is mostly offline.
        let rounds = 1_000;
        let authored: BTreeMap<_, _> = s
            .active
            .iter()
            .map(|a| {
                (
                    *a,
                    if rng.below(10) == 0 {
                        rng.below(400)
                    } else {
                        900 + rng.below(100)
                    },
                )
            })
            .collect();
        let outcome = s
            .end_epoch(
                &Participation {
                    rounds,
                    authored,
                    ..Participation::default()
                },
                a(REWARD_PER_EPOCH),
            )
            .unwrap();
        rewards_in += u128::from(REWARD_PER_EPOCH);
        ledger.apply(&outcome.effects);
        for j in &outcome.jailed {
            assert!(
                !s.active.contains(j),
                "a validator jailed this epoch is in the next committee"
            );
        }
        check(&s, &ledger, rewards_in);
    }
    for t in &tombstoned {
        assert!(
            s.register(account(0), *t, a(1_000_000), 0).is_err(),
            "a tombstoned id re-registered"
        );
        assert!(!s.active.contains(t));
    }
    let c = concentration(&s.ranked());
    assert!(c.nakamoto_halt >= 1 && c.nakamoto_finalize >= c.nakamoto_halt);
    println!(
        "staking scenario: {} validators, {} epochs, {} tombstoned; \
         final committee {}; Nakamoto (halt/finalize) {}/{}; Gini {} bps; top {} bps",
        VALIDATORS,
        EPOCHS,
        tombstoned.len(),
        s.active.len(),
        c.nakamoto_halt,
        c.nakamoto_finalize,
        c.gini_bps,
        c.top_share_bps
    );
}

#[test]
fn unbonded_funds_wait_out_the_delay_and_remain_slashable() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), a(10_000), 0).unwrap();
    s.delegate(account(2), id(1), a(4_000)).unwrap();
    s.undelegate(account(2), id(1), a(4_000)).unwrap();
    // Offence discovered during the delay: the leaving delegation pays too.
    let burned = s.slash_double_sign(id(1)).unwrap();
    assert_eq!(burned, vec![Effect::Burn(a(5_000 + 2_000))]);
    for _ in 0..Params::DEVNET.unbonding_epochs - 1 {
        let out = s
            .end_epoch(&Participation::default(), Amount::ZERO)
            .unwrap();
        assert!(
            !out.effects.iter().any(|e| matches!(e, Effect::Credit(..))),
            "released early"
        );
    }
    let out = s
        .end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    assert!(out.effects.contains(&Effect::Credit(account(2), a(2_000))));
}

#[test]
fn rewards_pay_commission_then_pro_rata_and_burn_only_dust() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), a(3_000), 1_000).unwrap(); // 10% commission
    s.delegate(account(2), id(1), a(1_000)).unwrap();
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    let out = s.end_epoch(&Participation::default(), a(1_000)).unwrap();
    // Commission 100; rest 900 split 3:1 → 675 and 225.
    assert!(
        out.effects
            .contains(&Effect::Credit(account(1), a(100 + 675)))
    );
    assert!(out.effects.contains(&Effect::Credit(account(2), a(225))));
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::Burn(b) if !b.is_zero()))
    );
}

#[test]
fn a_validator_that_misses_its_rounds_is_slashed_and_sits_out() {
    // Six seats, so the committee floor (MIN_COMMITTEE) leaves room to jail.
    let mut s = Staking::new(Params::DEVNET).unwrap();
    for n in 1..=6 {
        s.register(account(n), id(n), a(10_000), 0).unwrap();
    }
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    let mut authored: BTreeMap<_, _> = (1..=6).map(|n| (id(n), 100)).collect();
    authored.insert(id(2), 10);
    let out = s
        .end_epoch(
            &Participation {
                rounds: 100,
                authored,
                ..Participation::default()
            },
            Amount::ZERO,
        )
        .unwrap();
    assert_eq!(out.jailed, vec![id(2)]);
    assert!(!out.active.contains(&id(2)));
    assert_eq!(out.active.len(), 5);
    assert_eq!(s.validators[&id(2)].self_bond, a(9_900));
    // Back after the jail term.
    for _ in 0..Params::DEVNET.jail_epochs {
        s.end_epoch(&Participation::default(), Amount::ZERO)
            .unwrap();
    }
    assert!(s.active.contains(&id(2)));
}

/// ADR-046: stake, delegations and rewards past `u64::MAX` (about 18.4 coins at
/// 18 decimals) bond, earn pro rata and slash without loss.
#[test]
fn stakes_past_u64_bond_earn_and_slash_exactly() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    let big = Amount::from_u128(1u128 << 66); // even, so a 50% slash floors exactly
    let pool = Amount::from_u128(u128::from(u64::MAX) * 4);
    s.register(account(1), id(1), big, 0).unwrap();
    s.delegate(account(2), id(1), big).unwrap();
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    let out = s.end_epoch(&Participation::default(), pool).unwrap();
    // Equal stakes, no commission: the pool splits exactly in half.
    let half = Amount::from_u128(u128::from(u64::MAX) * 2);
    assert!(out.effects.contains(&Effect::Credit(account(1), half)));
    assert!(out.effects.contains(&Effect::Credit(account(2), half)));

    let before = s.held().unwrap();
    let burned = s.slash_double_sign(id(1)).unwrap();
    let [Effect::Burn(b)] = burned.as_slice() else {
        panic!("one burn, got {burned:?}");
    };
    assert_eq!(s.held().unwrap().checked_add(*b), Some(before));
    // DEVNET double-sign slash is 50% of each holding: half of 2 × big.
    assert_eq!(*b, big);
}

#[test]
fn boundary_block_delegation_earns_from_the_next_epoch() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), a(1_000), 0).unwrap();
    // Settle the registration so the operator's bond is eligible in epoch 1.
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    // A delegation in the boundary block of epoch 1 must not share epoch 1.
    s.delegate(account(2), id(1), a(1_000)).unwrap();
    let out = s.end_epoch(&Participation::default(), a(1_000)).unwrap();
    assert!(out.effects.contains(&Effect::Credit(account(1), a(1_000))));
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::Credit(who, _) if *who == account(2)))
    );
    // In epoch 2 the delegation is eligible: the pool splits 1:1.
    let out = s.end_epoch(&Participation::default(), a(1_000)).unwrap();
    assert!(out.effects.contains(&Effect::Credit(account(1), a(500))));
    assert!(out.effects.contains(&Effect::Credit(account(2), a(500))));
}

#[test]
fn bond_added_in_the_boundary_block_earns_from_the_next_epoch() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), a(1_000), 0).unwrap();
    s.delegate(account(2), id(1), a(1_000)).unwrap();
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    // Operator tops up its bond in the boundary block: no share in that epoch.
    s.bond_more(account(1), id(1), a(2_000)).unwrap();
    let out = s.end_epoch(&Participation::default(), a(2_000)).unwrap();
    assert!(out.effects.contains(&Effect::Credit(account(1), a(1_000))));
    assert!(out.effects.contains(&Effect::Credit(account(2), a(1_000))));
    let out = s.end_epoch(&Participation::default(), a(4_000)).unwrap();
    // Operator 3_000 vs delegator 1_000 → 3:1 of 4_000.
    assert!(out.effects.contains(&Effect::Credit(account(1), a(3_000))));
    assert!(out.effects.contains(&Effect::Credit(account(2), a(1_000))));
}

#[test]
fn unbond_takes_pending_stake_before_earned_stake() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), a(1_000), 0).unwrap();
    s.register(account(2), id(2), a(1_000), 0).unwrap();
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    // 1_000 earned + 1_000 pending on operator 1; withdraw 500.
    s.bond_more(account(1), id(1), a(1_000)).unwrap();
    s.unbond(account(1), id(1), a(500)).unwrap();
    // Earned left: 1_500 - 500 pending = 1_000, equal to operator 2's bond.
    let out = s.end_epoch(&Participation::default(), a(2_000)).unwrap();
    assert!(out.effects.contains(&Effect::Credit(account(1), a(1_000))));
    assert!(out.effects.contains(&Effect::Credit(account(2), a(1_000))));
}

#[test]
fn undelegate_takes_pending_stake_before_earned_stake() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), a(1_000), 0).unwrap();
    s.register(account(2), id(2), a(1_000), 0).unwrap();
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    // Delegator 3 holds 1_000 earned, then adds 1_000 pending; withdraws 500.
    s.delegate(account(3), id(1), a(1_000)).unwrap();
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    s.delegate(account(3), id(1), a(1_000)).unwrap();
    s.undelegate(account(3), id(1), a(500)).unwrap();
    // Delegator 3 earns 1_000 (1_500 held, 500 of it pending).
    let out = s.end_epoch(&Participation::default(), a(3_000)).unwrap();
    assert!(out.effects.contains(&Effect::Credit(account(3), a(1_000))));
}

#[test]
fn undelegations_cannot_grow_the_unbonding_queue_for_free() {
    // Pre-audit CRITICAL-1 (self-run): one-unit undelegations each added a row
    // to the staking record until no node could decode it.
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), a(10_000), 0).unwrap();
    s.delegate(account(2), id(1), a(1_000)).unwrap();
    assert_eq!(
        s.undelegate(account(2), id(1), Amount::ZERO),
        Err(StakeError::BelowMinimum)
    );
    assert_eq!(
        s.undelegate(account(2), id(1), a(1)),
        Err(StakeError::BelowMinimum)
    );
    assert_eq!(
        s.unbond(account(1), id(1), Amount::ZERO),
        Err(StakeError::BelowMinimum)
    );
    // Undelegations releasing in the same epoch fold into one row.
    for _ in 0..50 {
        s.undelegate(account(2), id(1), a(10)).unwrap();
    }
    assert_eq!(s.unbonding.len(), 1);
    assert_eq!(s.unbonding[0].amount, a(500));
    // Leaving a remainder below the minimum is refused; leaving whole is not.
    assert_eq!(
        s.undelegate(account(2), id(1), a(495)),
        Err(StakeError::BelowMinimum)
    );
    s.undelegate(account(2), id(1), a(500)).unwrap();
    assert_eq!(s.unbonding.len(), 1);
    assert_eq!(s.unbonding[0].amount, a(1_000));
    assert!(!s.delegations.contains_key(&(account(2), id(1))));
}

/// Pre-audit S-033: three of five seats down in one epoch. Jailing all three
/// would leave a two-seat committee, so only the worst one is jailed; every
/// down validator is still slashed and unpaid.
#[test]
fn downtime_jailing_never_shrinks_the_committee_below_four_seats() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    for n in 1..=5 {
        s.register(account(n), id(n), a(10_000), 0).unwrap();
    }
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    // Validators 3, 4 and 5 are down; 4 took part least.
    let authored = BTreeMap::from([
        (id(1), 100),
        (id(2), 100),
        (id(3), 30),
        (id(4), 0),
        (id(5), 20),
    ]);
    let out = s
        .end_epoch(
            &Participation {
                rounds: 100,
                authored,
                ..Participation::default()
            },
            a(1_000),
        )
        .unwrap();
    assert_eq!(out.jailed, vec![id(4)]);
    assert_eq!(out.active.len(), staking_min_committee());
    for n in 3..=5 {
        assert_eq!(
            s.validators[&id(n)].self_bond,
            a(9_900),
            "validator {n} slashed"
        );
    }
    // Only the two that took part are paid.
    let paid: Vec<_> = out
        .effects
        .iter()
        .filter_map(|e| match e {
            Effect::Credit(to, _) => Some(*to),
            _ => None,
        })
        .collect();
    assert!(
        paid.iter().all(|to| *to == account(1) || *to == account(2)),
        "{paid:?}"
    );
}

fn staking_min_committee() -> usize {
    maya_staking::MIN_COMMITTEE
}

/// Security review of S-033: the floor is counted after the slash. Two
/// thin bonds fall below `min_self_bond` when slashed, leaving four eligible,
/// so the third down validator (the worst) must stay seated, not be jailed.
#[test]
fn the_committee_floor_is_counted_after_the_slash() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    for n in 1..=4 {
        s.register(account(n), id(n), a(10_000), 0).unwrap();
    }
    // Just above the 1,000 minimum: a 1% slash drops both below it.
    s.register(account(5), id(5), a(1_005), 0).unwrap();
    s.register(account(6), id(6), a(1_005), 0).unwrap();
    s.end_epoch(&Participation::default(), Amount::ZERO)
        .unwrap();
    let authored = BTreeMap::from([
        (id(1), 100),
        (id(2), 100),
        (id(3), 100),
        (id(4), 0),
        (id(5), 10),
        (id(6), 10),
    ]);
    let out = s
        .end_epoch(
            &Participation {
                rounds: 100,
                authored,
                ..Participation::default()
            },
            Amount::ZERO,
        )
        .unwrap();
    assert!(
        out.active.contains(&id(4)),
        "the floor keeps the worst offender seated"
    );
    assert_eq!(out.active.len(), staking_min_committee());
    // The thin bonds are jailed for free: they no longer count.
    assert!(out.jailed.contains(&id(5)) && out.jailed.contains(&id(6)));
}
