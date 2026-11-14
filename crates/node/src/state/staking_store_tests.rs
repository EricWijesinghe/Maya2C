//! ADR-062b: the per-record staking store against the rules it replaced.
//!
//! The reference is `maya_staking::Staking` held whole in memory and ended
//! with `Staking::end_epoch`, which credits every delegator at the boundary:
//! exactly what the single `k:state` record did. The store must produce the
//! same validators, delegations, unbonding entries, committee and balances,
//! with delegators' rewards arriving later but in the same amounts.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::Instant;

use maya_staking::{Amount, Effect, Params, Participation, Staking};
use primitive_types::U256;
use tempfile::TempDir;

use crate::state::account::{Account, Address};
use crate::state::db::{Overlay, StateDB};
use crate::state::fees::{FEE_COLLECTOR, FEE_KEY, FeeRecord};
use crate::state::shielded::FEE_SINK;
use crate::state::staking::{STAKING_PREFIX, StakingRecord};
use crate::state::staking_records::{
    DELEGATION_PREFIX, StoredDelegation, StoredValidator, UNBONDING_PREFIX, VALIDATOR_PREFIX,
    decode_unbonding, parse_delegation_key, parse_unbonding_key,
};
use crate::state::staking_store::{PAYOUTS_PER_BLOCK, RECORDS_READ, StakeOp, commit_view};

/// splitmix64: deterministic, so a failure replays exactly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const FUNDS: u64 = 1_000_000_000;

fn addr(tag: u8, n: u64) -> Address {
    let mut a = [tag; 32];
    a[24..].copy_from_slice(&n.to_be_bytes());
    a
}

fn operator(i: u64) -> Address {
    addr(0xA0, i)
}

fn validator(i: u64) -> [u8; 32] {
    addr(0xB0, i)
}

fn delegator(i: u64) -> Address {
    addr(0xC0, i)
}

/// The store and the reference, side by side.
struct Pair {
    db: StateDB,
    _dir: TempDir,
    overlay: Overlay,
    reference: Staking,
    balances: BTreeMap<Address, U256>,
    /// Operations both sides accepted, by kind, so a history that refuses
    /// everything cannot pass as equivalent.
    accepted: BTreeMap<&'static str, usize>,
    /// Payouts left open by boundaries: the deferred path was exercised.
    payouts_opened: usize,
}

fn amount(n: u64) -> Amount {
    Amount::from_u64(n)
}

impl Pair {
    /// `validators` registered and settled, every account funded, a fee
    /// record so the collector is the reward pool.
    fn new(validators: u64, epoch_blocks: u64) -> Self {
        let mut reference = Staking::new(Params::DEVNET).unwrap();
        for i in 0..validators {
            reference
                .register(operator(i), validator(i), amount(10_000 + i * 1_000), 500)
                .unwrap();
        }
        reference.pending_self.clear();
        reference.active = reference.ranked().into_iter().map(|s| s.id).collect();
        let dir = TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        let view = StakingRecord {
            staking: reference.clone(),
            epoch_blocks,
            last_round: 0,
            expected: BTreeMap::new(),
            authored: BTreeMap::new(),
        };
        for (key, value) in view.to_records().unwrap() {
            db.raw_put_for_test(&key, &value).unwrap();
        }
        let fees = FeeRecord {
            base_fee: Amount::from_u64(1),
            min_base_fee: Amount::from_u64(1),
            target_block_bytes: 1_000,
            change_denominator: 8,
        };
        db.raw_put_for_test(FEE_KEY, &fees.encode()).unwrap();
        let mut pair = Self {
            db,
            _dir: dir,
            overlay: Overlay::new(),
            reference,
            balances: BTreeMap::new(),
            accepted: BTreeMap::new(),
            payouts_opened: 0,
        };
        for i in 0..validators.max(8) {
            pair.fund(operator(i));
        }
        pair
    }

    fn fund(&mut self, a: Address) {
        self.db
            .put_account(
                &a,
                &Account {
                    balance: U256::from(FUNDS),
                    nonce: 0,
                },
            )
            .unwrap();
        self.balances.insert(a, U256::from(FUNDS));
    }

    fn apply_reference(&mut self, sender: Option<&Address>, effects: &[Effect]) -> bool {
        let mut next = self.balances.clone();
        for e in effects {
            let (a, v, debit) = match e {
                Effect::Debit(a, v) => (*a, *v, true),
                Effect::Credit(a, v) => (*a, *v, false),
                Effect::Burn(v) => (FEE_SINK, *v, false),
            };
            assert!(!debit || Some(&a) == sender);
            let b = next.entry(a).or_default();
            let v = maya_stateless_core::amount::to_u256(v);
            if debit {
                let Some(left) = b.checked_sub(v) else {
                    return false;
                };
                *b = left;
            } else {
                *b = b.checked_add(v).unwrap();
            }
        }
        self.balances = next;
        true
    }

    /// Runs `op` on both; both must accept it or both refuse it.
    fn op(&mut self, sender: Address, op: StakeOp) -> bool {
        let before = self.reference.clone();
        let reference = match op {
            StakeOp::Register {
                id,
                bond,
                commission_bps,
            } => self.reference.register(sender, id, bond, commission_bps),
            StakeOp::BondMore { validator, amount } => {
                self.reference.bond_more(sender, validator, amount)
            }
            StakeOp::Unbond { validator, amount } => {
                self.reference.unbond(sender, validator, amount)
            }
            StakeOp::Delegate { validator, amount } => {
                self.reference.delegate(sender, validator, amount)
            }
            StakeOp::Undelegate { validator, amount } => {
                self.reference.undelegate(sender, validator, amount)
            }
            StakeOp::Slash(id) => self.reference.slash_double_sign(id),
        };
        let reference_ok = match reference {
            Ok(effects) => self.apply_reference(Some(&sender), &effects),
            Err(_) => false,
        };
        if !reference_ok {
            self.reference = before;
        }
        let node_ok = self
            .db
            .apply_stake_op(&mut self.overlay, &sender, op)
            .is_ok();
        assert_eq!(node_ok, reference_ok, "{op:?} accepted by one side only");
        if node_ok {
            let kind = match op {
                StakeOp::Register { .. } => "register",
                StakeOp::BondMore { .. } => "bond_more",
                StakeOp::Unbond { .. } => "unbond",
                StakeOp::Delegate { .. } => "delegate",
                StakeOp::Undelegate { .. } => "undelegate",
                StakeOp::Slash(_) => "slash",
            };
            *self.accepted.entry(kind).or_default() += 1;
        }
        node_ok
    }

    /// Ends the epoch on both with the same participation and reward pool.
    fn end_epoch(&mut self, rng: &mut Rng) {
        self.end_epoch_with(rng, true);
    }

    /// As [`Self::end_epoch`]; with `downtime` false every validator is present.
    fn end_epoch_with(&mut self, rng: &mut Rng, downtime: bool) {
        let mut expected = BTreeMap::new();
        let mut authored = BTreeMap::new();
        for id in &self.reference.active {
            let e = 10;
            expected.insert(*id, e);
            // Mostly present, sometimes down.
            authored.insert(
                *id,
                if downtime && rng.below(6) == 0 {
                    rng.below(4)
                } else {
                    e
                },
            );
        }
        let pool = rng.below(1_000_000);
        // The reference.
        let previous = self.reference.active.clone();
        let participation = Participation {
            rounds: 0,
            expected: expected.clone(),
            authored: authored.clone(),
        };
        let out = self
            .reference
            .end_epoch(&participation, amount(pool))
            .unwrap();
        if self.reference.active.is_empty() {
            self.reference.active = previous;
        }
        assert!(self.apply_reference(None, &out.effects));
        self.node_end_epoch(expected, authored, pool);
    }

    /// The store's side of an epoch boundary alone: what the cost test times.
    fn node_end_epoch(
        &mut self,
        expected: BTreeMap<[u8; 32], u64>,
        authored: BTreeMap<[u8; 32], u64>,
        pool: u64,
    ) {
        // The pool is whatever the collector holds.
        let mut collector = self.db.load(&self.overlay, &FEE_COLLECTOR).unwrap();
        collector.balance = U256::from(pool);
        self.overlay.accounts.insert(FEE_COLLECTOR, collector);
        let mut view = self.db.staking_view(&self.overlay).unwrap().unwrap();
        view.record.expected = expected;
        view.record.authored = authored;
        let effects = self.db.end_epoch(&mut self.overlay, &mut view).unwrap();
        let writes = commit_view(&view).unwrap();
        self.db
            .stage_staking(&mut self.overlay, None, writes, &effects)
            .unwrap();
    }

    fn open_payouts(&self) -> usize {
        self.records()
            .keys()
            .filter(|k| k.starts_with(crate::state::staking_records::PAYOUT_PREFIX))
            .count()
    }

    fn pay(&mut self, budget: usize) {
        let epoch = self.reference.epoch;
        self.db.pay_owed(&mut self.overlay, epoch, budget).unwrap();
    }

    fn commit(&mut self) {
        self.db.commit_overlay_for_test(&self.overlay).unwrap();
        self.overlay = Overlay::new();
    }

    fn records(&self) -> BTreeMap<Vec<u8>, Vec<u8>> {
        self.db
            .merged_records(&self.overlay, STAKING_PREFIX)
            .unwrap()
    }

    /// The staking state agrees, and every validator's aggregates add up.
    fn check_state(&self) {
        let records = self.records();
        let node = StakingRecord::from_records(&records)
            .unwrap()
            .unwrap()
            .staking;
        let r = &self.reference;
        assert_eq!(node.epoch, r.epoch);
        assert_eq!(node.validators, r.validators);
        assert_eq!(node.delegations, r.delegations);
        assert_eq!(node.active, r.active);
        assert_eq!(node.retired, r.retired);
        assert_eq!(node.pending_self, r.pending_self);
        assert_eq!(node.pending_delegations, r.pending_delegations);
        let sorted = |u: &[maya_staking::Unbonding]| {
            let mut v: Vec<_> = u
                .iter()
                .map(|u| (u.validator, u.release_epoch, u.owner, u.amount))
                .collect();
            v.sort();
            v
        };
        assert_eq!(sorted(&node.unbonding), sorted(&r.unbonding));
        // Aggregates.
        let mut count: BTreeMap<[u8; 32], u64> = BTreeMap::new();
        let mut pending: BTreeMap<[u8; 32], Amount> = BTreeMap::new();
        let mut unbonding: BTreeMap<[u8; 32], Amount> = BTreeMap::new();
        for (key, value) in &records {
            if key.starts_with(DELEGATION_PREFIX) {
                let (_, v) = parse_delegation_key(key).unwrap();
                let d = StoredDelegation::decode(value).unwrap();
                *count.entry(v).or_default() += 1;
                let p = pending.entry(v).or_insert(Amount::ZERO);
                *p = p.saturating_add(d.pending_at(r.epoch));
            } else if key.starts_with(UNBONDING_PREFIX) {
                let (v, _, _) = parse_unbonding_key(key).unwrap();
                let u = unbonding.entry(v).or_insert(Amount::ZERO);
                *u = u.saturating_add(decode_unbonding(value).unwrap());
            }
        }
        for (key, value) in &records {
            if let Some(id) = key.strip_prefix(VALIDATOR_PREFIX) {
                let id: [u8; 32] = id.try_into().unwrap();
                let v = StoredValidator::decode(value).unwrap();
                assert_eq!(v.delegations, count.get(&id).copied().unwrap_or(0));
                assert_eq!(
                    v.pending_delegated_at(r.epoch),
                    pending.get(&id).copied().unwrap_or(Amount::ZERO)
                );
                assert_eq!(
                    v.unbonding,
                    unbonding.get(&id).copied().unwrap_or(Amount::ZERO)
                );
            }
        }
    }

    /// With every payout drained, balances and held stake agree too.
    fn check_balances(&mut self) {
        self.pay(usize::MAX);
        let open = self
            .records()
            .keys()
            .filter(|k| k.starts_with(crate::state::staking_records::PAYOUT_PREFIX))
            .count();
        assert_eq!(open, 0, "every payout drained");
        for (a, b) in &self.balances {
            let node = self.db.load(&self.overlay, a).unwrap().balance;
            assert_eq!(&node, b, "balance of {}", hex::encode(a));
        }
        self.commit();
        assert_eq!(self.db.staking_held().unwrap(), self.reference.held());
    }
}

fn random_op(pair: &mut Pair, rng: &mut Rng, validators: &mut u64, delegators: u64) {
    let v = validator(rng.below(*validators + 1));
    let d = delegator(rng.below(delegators));
    let held = pair.reference.delegations.get(&(d, v)).copied();
    match rng.below(10) {
        0..=3 => {
            let a = amount(10 + rng.below(3_000));
            pair.op(
                d,
                StakeOp::Delegate {
                    validator: v,
                    amount: a,
                },
            );
        }
        4..=5 => {
            let a = match held {
                Some(h) if rng.below(2) == 0 => h,
                Some(h) => amount(1 + rng.below(h.saturating_to_u64().max(1))),
                None => amount(1 + rng.below(100)),
            };
            pair.op(
                d,
                StakeOp::Undelegate {
                    validator: v,
                    amount: a,
                },
            );
        }
        6 => {
            let i = rng.below(*validators);
            let a = amount(1 + rng.below(2_000));
            let op = if rng.below(2) == 0 {
                StakeOp::BondMore {
                    validator: validator(i),
                    amount: a,
                }
            } else {
                StakeOp::Unbond {
                    validator: validator(i),
                    amount: a,
                }
            };
            pair.op(operator(i), op);
        }
        7 => {
            if rng.below(8) == 0 {
                let i = rng.below(*validators);
                pair.op(addr(0xEE, 0), StakeOp::Slash(validator(i)));
            }
        }
        8 => {
            let i = *validators;
            if i < 7 {
                let bond = amount(1_000 + rng.below(20_000));
                if pair.op(
                    operator(i),
                    StakeOp::Register {
                        id: validator(i),
                        bond,
                        commission_bps: 1_000,
                    },
                ) {
                    *validators += 1;
                }
            }
        }
        _ => pair.pay(1 + usize::try_from(rng.below(4)).unwrap()),
    }
}

#[test]
fn random_histories_match_the_single_record_rules() {
    let (mut slashes, mut registers) = (0, 0);
    for seed in 1..=6u64 {
        let mut rng = Rng(seed);
        let mut validators = 5;
        let delegators = 24;
        let mut pair = Pair::new(validators, 600);
        for i in 0..delegators {
            pair.fund(delegator(i));
        }
        pair.fund(addr(0xEE, 0));
        for step in 0..900u64 {
            random_op(&mut pair, &mut rng, &mut validators, delegators);
            if step % 37 == 36 {
                pair.end_epoch(&mut rng);
                pair.payouts_opened += pair.open_payouts();
            }
            if step % 11 == 0 {
                pair.commit();
            }
            pair.check_state();
            if step % 150 == 149 {
                pair.check_balances();
            }
        }
        pair.check_balances();
        eprintln!(
            "seed {seed}: accepted {:?}, payouts opened {}",
            pair.accepted, pair.payouts_opened
        );
        for kind in ["delegate", "undelegate", "bond_more", "unbond"] {
            let n = pair.accepted.get(kind).copied().unwrap_or(0);
            assert!(n >= 5, "seed {seed}: only {n} {kind}");
        }
        assert!(
            pair.payouts_opened >= 10,
            "seed {seed}: the deferred path ran"
        );
        slashes += pair.accepted.get("slash").copied().unwrap_or(0);
        registers += pair.accepted.get("register").copied().unwrap_or(0);
    }
    assert!(
        slashes >= 1 && registers >= 1,
        "slashes {slashes}, registers {registers}"
    );
}

#[test]
fn a_payout_owed_over_several_epochs_is_paid_once_and_in_full() {
    // Payouts outlive their epoch when the pass is starved: three boundaries
    // in a row with no budget, then delegations touched out of key order.
    let mut rng = Rng(77);
    let mut pair = Pair::new(4, 600);
    for i in 0..40 {
        pair.fund(delegator(i));
        let v = validator(i % 4);
        assert!(pair.op(
            delegator(i),
            StakeOp::Delegate {
                validator: v,
                amount: amount(500 + i)
            }
        ));
    }
    pair.commit();
    for _ in 0..3 {
        pair.end_epoch(&mut rng);
        pair.check_state();
    }
    let open = pair
        .records()
        .keys()
        .filter(|k| k.starts_with(crate::state::staking_records::PAYOUT_PREFIX))
        .count();
    assert!(open >= 4, "payouts are open across epochs: {open}");
    for i in (0..40).rev().step_by(3) {
        pair.op(
            delegator(i),
            StakeOp::Undelegate {
                validator: validator(i % 4),
                amount: amount(500 + i),
            },
        );
    }
    pair.pay(5);
    pair.check_state();
    pair.check_balances();
}

/// `(records read, wall time)` for `f`.
fn measured<T>(f: impl FnOnce() -> T) -> (u64, std::time::Duration, T) {
    RECORDS_READ.with(|c| c.set(0));
    let start = Instant::now();
    let out = f();
    (
        RECORDS_READ.with(std::cell::Cell::get),
        start.elapsed(),
        out,
    )
}

/// A chain with `crowd` minimum delegations to its first validator, seeded
/// straight into state (built through the rules, written as records).
fn crowded(crowd: u64) -> Pair {
    crowded_with(crowd, 600)
}

/// As [`crowded`], with epochs of `epoch_blocks`.
fn crowded_with(crowd: u64, epoch_blocks: u64) -> Pair {
    let mut pair = Pair::new(4, epoch_blocks);
    let min = Params::DEVNET.min_delegation;
    for i in 0..crowd {
        pair.reference
            .delegate(addr(0xD0, i), validator(0), min)
            .unwrap();
    }
    pair.reference.pending_delegations.clear();
    let view = StakingRecord {
        staking: pair.reference.clone(),
        epoch_blocks,
        last_round: 0,
        expected: BTreeMap::new(),
        authored: BTreeMap::new(),
    };
    for (key, value) in view.to_records().unwrap() {
        pair.db.raw_put_for_test(&key, &value).unwrap();
    }
    pair.fund(delegator(0));
    pair
}

#[test]
fn past_the_ceiling_a_new_delegation_record_is_refused_and_a_top_up_is_not() {
    // Two-block epochs: the ceiling is 1,024 × 2 / 2 = 1,024 records.
    let ceiling = crate::state::staking_store::max_delegation_records(2);
    let mut pair = crowded_with(ceiling, 2);
    let fresh = StakeOp::Delegate {
        validator: validator(1),
        amount: amount(1_000),
    };
    let refused = pair
        .db
        .apply_stake_op(&mut pair.overlay, &delegator(0), fresh)
        .expect_err("a new record past the ceiling");
    assert!(refused.to_string().contains("ceiling"), "{refused}");
    // An existing delegation can still grow.
    pair.fund(addr(0xD0, 0));
    let top_up = StakeOp::Delegate {
        validator: validator(0),
        amount: amount(1_000),
    };
    pair.db
        .apply_stake_op(&mut pair.overlay, &addr(0xD0, 0), top_up)
        .unwrap();
    // One record leaving makes room for one new one.
    let leave = StakeOp::Undelegate {
        validator: validator(0),
        amount: Params::DEVNET.min_delegation,
    };
    pair.db
        .apply_stake_op(&mut pair.overlay, &addr(0xD0, 1), leave)
        .unwrap();
    pair.db
        .apply_stake_op(&mut pair.overlay, &delegator(0), fresh)
        .unwrap();
}

#[test]
fn a_crowd_of_minimum_delegations_neither_blocks_nor_slows_a_new_one() {
    // Before ADR-062b the crowd lived in one value: past the record cap every
    // new delegation was refused, and below it each one decoded the crowd.
    let crowd = 100_000;
    let mut small = crowded(10);
    let mut large = crowded(crowd);
    let delegate = StakeOp::Delegate {
        validator: validator(1),
        amount: amount(1_000),
    };
    let (small_reads, small_time, ok) = measured(|| {
        small
            .db
            .apply_stake_op(&mut small.overlay, &delegator(0), delegate)
    });
    ok.unwrap();
    let (large_reads, large_time, ok) = measured(|| {
        large
            .db
            .apply_stake_op(&mut large.overlay, &delegator(0), delegate)
    });
    ok.expect("a new delegation is accepted beside 100,000 dust delegations");
    eprintln!(
        "delegate beside 10 delegations: {small_reads} records, {small_time:?}; beside {crowd}: {large_reads} records, {large_time:?}"
    );
    assert_eq!(
        small_reads, large_reads,
        "records read do not grow with the crowd"
    );
    // Into the crowded validator itself: still one delegation record.
    let into_crowd = StakeOp::Delegate {
        validator: validator(0),
        amount: amount(1_000),
    };
    let (reads, _, ok) = measured(|| {
        large
            .db
            .apply_stake_op(&mut large.overlay, &delegator(0), into_crowd)
    });
    ok.unwrap();
    assert_eq!(reads, small_reads);
}

#[test]
fn per_block_staking_work_does_not_grow_with_total_delegations() {
    let mut rng = Rng(5);
    let mut rows = Vec::new();
    for crowd in [1_000, 100_000] {
        let mut pair = crowded(crowd);
        // A block that is not a boundary: open, count, commit, pay.
        let (plain, plain_time, ()) = measured(|| {
            let view = pair.db.staking_view(&pair.overlay).unwrap().unwrap();
            let writes = commit_view(&view).unwrap();
            pair.db
                .stage_staking(&mut pair.overlay, None, writes, &[])
                .unwrap();
            pair.pay(PAYOUTS_PER_BLOCK);
        });
        // The boundary itself: every validator, no delegation. Only the
        // store's side is timed; the in-memory reference walks every one.
        let full: BTreeMap<[u8; 32], u64> =
            pair.reference.active.iter().map(|id| (*id, 10)).collect();
        let pool = rng.below(1_000_000);
        let (boundary, boundary_time, ()) =
            measured(|| pair.node_end_epoch(full.clone(), full.clone(), pool));
        // The blocks after it pay the crowd at most PAYOUTS_PER_BLOCK each.
        let (paying, paying_time, ()) = measured(|| pair.pay(PAYOUTS_PER_BLOCK));
        eprintln!(
            "{crowd} delegations: plain block {plain} records {plain_time:?}; boundary {boundary} records {boundary_time:?}; next block's payouts {paying} records {paying_time:?}"
        );
        rows.push((plain, boundary, paying));
    }
    assert_eq!(rows[0].0, rows[1].0, "a plain block reads the same");
    assert_eq!(rows[0].1, rows[1].1, "the boundary reads the same");
    // The pass reads up to its budget of delegations plus its payout keys.
    let bound = PAYOUTS_PER_BLOCK as u64 + 8;
    assert!(rows[0].2 <= bound && rows[1].2 <= bound, "{rows:?}");
}

#[test]
fn a_slash_reaches_every_delegation_of_a_crowded_validator() {
    // Complete, and a key range: what a slash reads is the offender's own
    // records, and it is the one staking operation whose cost they set.
    let crowd = 100_000;
    let mut pair = crowded(crowd);
    let (reads, time, ok) = measured(|| {
        pair.db.apply_stake_op(
            &mut pair.overlay,
            &addr(0xEE, 0),
            StakeOp::Slash(validator(0)),
        )
    });
    ok.unwrap();
    eprintln!("slash of a validator with {crowd} delegations: {reads} records, {time:?}");
    let bps = u64::from(Params::DEVNET.double_sign_slash_bps);
    let min = Params::DEVNET.min_delegation.saturating_to_u64();
    let left = min - min * bps / 10_000;
    let records = pair.records();
    let cut = records
        .iter()
        .filter(|(k, _)| {
            k.starts_with(&crate::state::staking_records::delegations_of(&validator(
                0,
            )))
        })
        .all(|(_, v)| StoredDelegation::decode(v).unwrap().amount == amount(left));
    assert!(cut, "every delegation was cut");
    assert!(reads >= crowd);
}
