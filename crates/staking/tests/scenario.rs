//! A thousand validators join, delegate, leave and get slashed under
//! Byzantine faults (Master Prompt 4 §9), and after every step value is
//! conserved and the committee obeys its rules.
//!
//! Deterministic: a fixed-seed LCG picks every action, so a failure replays.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use maya_staking::{Effect, Params, Participation, StakeError, Staking, Status, concentration};

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
                Effect::Debit(_, a) => self.debited += u128::from(*a),
                Effect::Credit(_, a) => self.credited += u128::from(*a),
                Effect::Burn(a) => self.burned += u128::from(*a),
            }
        }
    }
}

fn check(s: &Staking, ledger: &Ledger, rewards_in: u128) {
    assert_eq!(
        ledger.debited + rewards_in,
        s.held() + ledger.credited + ledger.burned,
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
                bond,
                u16::try_from(rng.below(2_000)).unwrap(),
            )
            .unwrap();
        ledger.apply(&effects);
    }
    assert_eq!(
        s.register(account(0), id(0), 5_000, 0),
        Err(StakeError::AlreadyRegistered)
    );
    let first = s.end_epoch(&Participation::default(), 0).unwrap();
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
                0 => s.delegate(who, id(n), 10 + rng.below(50_000)),
                1 => {
                    let held: Vec<_> = s.delegations.keys().copied().collect();
                    if held.is_empty() {
                        continue;
                    }
                    let (d, v) = held[usize::try_from(rng.below(held.len() as u64)).unwrap()];
                    let amount = s.delegations[&(d, v)];
                    s.undelegate(d, v, 1 + rng.below(amount))
                }
                2 => s.bond_more(account(n), id(n), rng.below(10_000)),
                3 => s.unbond(account(n), id(n), rng.below(200_000)),
                4 => s.delegate(who, id(n), rng.below(9)), // below minimum
                _ => s.unbond(account(n + 1), id(n), 1),   // not the operator
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
                REWARD_PER_EPOCH,
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
            s.register(account(0), *t, 1_000_000, 0).is_err(),
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
    s.register(account(1), id(1), 10_000, 0).unwrap();
    s.delegate(account(2), id(1), 4_000).unwrap();
    s.undelegate(account(2), id(1), 4_000).unwrap();
    // Offence discovered during the delay: the leaving delegation pays too.
    let burned = s.slash_double_sign(id(1)).unwrap();
    assert_eq!(burned, vec![Effect::Burn(5_000 + 2_000)]);
    for _ in 0..Params::DEVNET.unbonding_epochs - 1 {
        let out = s.end_epoch(&Participation::default(), 0).unwrap();
        assert!(
            !out.effects.iter().any(|e| matches!(e, Effect::Credit(..))),
            "released early"
        );
    }
    let out = s.end_epoch(&Participation::default(), 0).unwrap();
    assert!(out.effects.contains(&Effect::Credit(account(2), 2_000)));
}

#[test]
fn rewards_pay_commission_then_pro_rata_and_burn_only_dust() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), 3_000, 1_000).unwrap(); // 10% commission
    s.delegate(account(2), id(1), 1_000).unwrap();
    s.end_epoch(&Participation::default(), 0).unwrap();
    let out = s.end_epoch(&Participation::default(), 1_000).unwrap();
    // Commission 100; rest 900 split 3:1 → 675 and 225.
    assert!(out.effects.contains(&Effect::Credit(account(1), 100 + 675)));
    assert!(out.effects.contains(&Effect::Credit(account(2), 225)));
    assert!(
        !out.effects
            .iter()
            .any(|e| matches!(e, Effect::Burn(b) if *b > 0))
    );
}

#[test]
fn a_validator_that_misses_its_rounds_is_slashed_and_sits_out() {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    s.register(account(1), id(1), 10_000, 0).unwrap();
    s.register(account(2), id(2), 10_000, 0).unwrap();
    s.end_epoch(&Participation::default(), 0).unwrap();
    let authored = BTreeMap::from([(id(1), 100), (id(2), 10)]);
    let out = s
        .end_epoch(
            &Participation {
                rounds: 100,
                authored,
                ..Participation::default()
            },
            0,
        )
        .unwrap();
    assert_eq!(out.jailed, vec![id(2)]);
    assert_eq!(out.active, vec![id(1)]);
    assert_eq!(s.validators[&id(2)].self_bond, 9_900);
    // Back after the jail term.
    for _ in 0..Params::DEVNET.jail_epochs {
        s.end_epoch(&Participation::default(), 0).unwrap();
    }
    assert!(s.active.contains(&id(2)));
}
