//! `Staking::end_epoch` against outputs recorded from the code before
//! ADR-062b's refactor (master 2090300c), not against the refactored code.
//!
//! ADR-062b split `end_epoch` into `close_epoch`, shared with the deferred
//! path the node runs, and moved the validator `retain` after ranking. The
//! node's equivalence test compares the two paths with each other, so a bug
//! common to both would pass it. This fixes the outcome of a scenario with
//! pending stake, downtime, jailing, a tombstone, unbonding releases and a
//! validator emptied out, epoch by epoch, as the pre-refactor code produced
//! it. The digest was recorded by running this file against 2090300c's
//! `src/` and must not be regenerated from the code under test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt::Write;

use maya_staking::{Amount, Params, Participation, Staking};

/// FNV-1a, 64-bit: dependency-free, and only has to be stable.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn id(n: u8) -> [u8; 32] {
    [n; 32]
}

fn amount(n: u64) -> Amount {
    Amount::from_u64(n)
}

/// `(expected, authored)` for every active validator, `down` authoring
/// `authored_down` of 10.
fn participation(s: &Staking, down: &[[u8; 32]], authored_down: u64) -> Participation {
    let expected: BTreeMap<[u8; 32], u64> = s.active.iter().map(|v| (*v, 10)).collect();
    let authored = s
        .active
        .iter()
        .map(|v| (*v, if down.contains(v) { authored_down } else { 10 }))
        .collect();
    Participation {
        rounds: 0,
        expected,
        authored,
    }
}

fn close(s: &mut Staking, down: &[[u8; 32]], authored_down: u64, pool: u64, log: &mut String) {
    let p = participation(s, down, authored_down);
    let out = s.end_epoch(&p, amount(pool)).unwrap();
    writeln!(log, "{out:?}\n{s:?}").unwrap();
}

/// Every epoch's outcome and the whole state after it, as text.
fn history() -> String {
    let mut s = Staking::new(Params::DEVNET).unwrap();
    for v in 1..=6u8 {
        s.register(
            id(0xA0 + v),
            id(v),
            amount(5_000 + u64::from(v) * 700),
            300 * u16::from(v),
        )
        .unwrap();
    }
    for d in 0..12u8 {
        s.delegate(id(0xD0 + d), id(1 + d % 6), amount(100 + u64::from(d) * 37))
            .unwrap();
    }
    let mut log = String::new();
    // Epoch 0: everything pending; nothing earns.
    close(&mut s, &[], 0, 9_999, &mut log);
    // Epoch 1: more stake arrives, one validator is down.
    s.delegate(id(0xD0), id(2), amount(250)).unwrap();
    s.bond_more(id(0xA3), id(3), amount(400)).unwrap();
    s.undelegate(id(0xD1), id(2), amount(50)).unwrap();
    close(&mut s, &[id(4)], 2, 123_457, &mut log);
    // Epoch 2: a double sign, an operator leaving entirely.
    s.slash_double_sign(id(5)).unwrap();
    let all = s.validators[&id(6)].self_bond;
    s.unbond(id(0xA6), id(6), all).unwrap();
    for d in [5u8, 11] {
        let held = s.delegations[&(id(0xD0 + d), id(6))];
        s.undelegate(id(0xD0 + d), id(6), held).unwrap();
    }
    close(&mut s, &[id(1)], 0, 77_777, &mut log);
    // Epochs 3..=11: quiet, until every unbonding entry has been released.
    for e in 3..=11u64 {
        close(&mut s, &[], 0, 10_000 + e, &mut log);
    }
    log
}

/// Recorded against 2090300c `crates/staking/src/` (before ADR-062b).
const RECORDED: u64 = 0x54d5_21a6_62c0_fdd3;

#[test]
fn end_epoch_reproduces_the_outputs_recorded_before_the_refactor() {
    let log = history();
    assert_eq!(
        fnv(log.as_bytes()),
        RECORDED,
        "digest {:#018x} of\n{log}",
        fnv(log.as_bytes())
    );
}
