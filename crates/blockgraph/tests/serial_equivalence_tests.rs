//! The claim that replaces two-phase commit: executing the waves reaches the
//! state the serial order would have reached.
//!
//! Everything else in this crate is a bound or an ordering rule, checkable by
//! inspection. This one is a *behavioural* claim about a scheduler, so it is
//! checked against an executable model rather than argued for in a comment.
//!
//! The model is a 64-slot state, one slot per shard, and a transaction that
//! folds its own index into every slot it touches. The fold is deliberately
//! **order-sensitive** — a commutative one (addition, say) would pass even for
//! a scheduler that reordered conflicting transactions, which is precisely the
//! bug worth catching.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_blockgraph::{Access, SHARD_COUNT, schedule};

/// A 64-slot state: one value per shard.
type State = [u64; SHARD_COUNT];

/// Applies transaction `index` to every shard it touches.
///
/// Order-sensitive by construction: mixing in the running value means a
/// different arrival order gives a different result, so the test can tell a
/// correct schedule from a plausible one.
fn apply(state: &mut State, index: usize, access: Access) {
    let mut remaining = access.mask();
    while remaining != 0 {
        let shard = remaining.trailing_zeros() as usize;
        remaining &= remaining - 1;
        let previous = state[shard];
        state[shard] = previous
            .rotate_left(7)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(index as u64 + 1);
    }
}

/// The reference: straight down the agreed order.
fn execute_serially(accesses: &[Access]) -> State {
    let mut state = [0u64; SHARD_COUNT];
    for (index, access) in accesses.iter().enumerate() {
        apply(&mut state, index, *access);
    }
    state
}

/// The scheduled execution, with each wave applied in a caller-chosen order.
///
/// `permute` decides how a wave's transactions are ordered before being
/// applied. A correct schedule reaches the same state for *every* choice, which
/// is what the adversarial orderings below exercise.
fn execute_in_waves(accesses: &[Access], permute: impl Fn(&mut Vec<usize>)) -> State {
    let mut state = [0u64; SHARD_COUNT];
    for wave in schedule(accesses).expect("access sets are non-empty") {
        let mut order: Vec<usize> = wave.indices().to_vec();
        permute(&mut order);
        for index in order {
            apply(&mut state, index, accesses[index]);
        }
    }
    state
}

/// A deterministic pseudo-random access-set generator.
///
/// Not `rand`: this crate has no dependencies and neither does its test, and a
/// seeded generator written here is reproducible across machines in a way a
/// third-party one across versions is not.
fn accesses_from_seed(seed: u64, count: usize, max_shards: u32) -> Vec<Access> {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    (0..count)
        .map(|_| {
            let width = (next() % u64::from(max_shards)) as usize + 1;
            let mut mask = 0u64;
            for _ in 0..width {
                mask |= 1u64 << (next() % SHARD_COUNT as u64);
            }
            Access::from_mask(mask).expect("at least one bit is set")
        })
        .collect()
}

#[test]
fn a_scheduled_execution_matches_the_serial_one() {
    for seed in 1..64u64 {
        let accesses = accesses_from_seed(seed, 200, 4);
        assert_eq!(
            execute_in_waves(&accesses, |_| {}),
            execute_serially(&accesses),
            "seed {seed} diverged"
        );
    }
}

#[test]
fn reversing_every_wave_still_matches_the_serial_execution() {
    // The strongest ordinary check: if any wave contained two conflicting
    // transactions, reversing it would swap them and the fold would diverge.
    for seed in 1..64u64 {
        let accesses = accesses_from_seed(seed, 200, 4);
        assert_eq!(
            execute_in_waves(&accesses, |order| order.reverse()),
            execute_serially(&accesses),
            "seed {seed} diverged under reversal"
        );
    }
}

#[test]
fn rotating_every_wave_still_matches_the_serial_execution() {
    for seed in 1..32u64 {
        let accesses = accesses_from_seed(seed, 150, 6);
        assert_eq!(
            execute_in_waves(&accesses, |order| {
                let midpoint = order.len() / 2;
                order.rotate_left(midpoint);
            }),
            execute_serially(&accesses),
            "seed {seed} diverged under rotation"
        );
    }
}

#[test]
fn wide_access_sets_still_match() {
    // Access sets touching up to half the shards: waves get short and the
    // schedule approaches the serial order, which is the degenerate case.
    for seed in 1..24u64 {
        let accesses = accesses_from_seed(seed, 120, 32);
        assert_eq!(
            execute_in_waves(&accesses, |order| order.reverse()),
            execute_serially(&accesses),
            "seed {seed} diverged with wide access sets"
        );
    }
}

#[test]
fn single_shard_traffic_serialises_completely() {
    // Every transaction on one shard: one wave each, and the schedule is the
    // serial order. Parallelism is zero, and correctness is unchanged.
    let accesses = vec![Access::from_mask(1).expect("non-zero"); 32];
    let waves = schedule(&accesses).expect("valid");
    assert_eq!(waves.len(), 32);
    assert_eq!(
        execute_in_waves(&accesses, |order| order.reverse()),
        execute_serially(&accesses)
    );
}

#[test]
fn the_model_would_notice_a_reordering() {
    // Guards the guard. If `apply` were commutative, every test above would
    // pass against a scheduler that freely reordered conflicting transactions.
    // This asserts the model can tell the difference.
    let one = Access::from_mask(0b1).expect("non-zero");
    let mut forward = [0u64; SHARD_COUNT];
    apply(&mut forward, 0, one);
    apply(&mut forward, 1, one);

    let mut backward = [0u64; SHARD_COUNT];
    apply(&mut backward, 1, one);
    apply(&mut backward, 0, one);

    assert_ne!(forward, backward, "the model must be order-sensitive");
}

#[test]
fn no_wave_contains_two_conflicting_transactions() {
    // The structural property, asserted directly rather than inferred from the
    // state matching — a schedule could in principle be wrong in a way this
    // particular fold does not expose.
    for seed in 1..48u64 {
        let accesses = accesses_from_seed(seed, 200, 5);
        for wave in schedule(&accesses).expect("valid") {
            let indices = wave.indices();
            for (position, &left) in indices.iter().enumerate() {
                for &right in &indices[position + 1..] {
                    assert!(
                        !accesses[left].conflicts_with(accesses[right]),
                        "seed {seed}: {left} and {right} share a shard in one wave"
                    );
                }
            }
        }
    }
}

#[test]
fn conflicting_transactions_keep_their_relative_order() {
    // The other half of the correctness argument: if i < j conflict, then i is
    // in a strictly earlier wave.
    for seed in 1..48u64 {
        let accesses = accesses_from_seed(seed, 120, 5);
        let waves = schedule(&accesses).expect("valid");

        let mut wave_of = vec![usize::MAX; accesses.len()];
        for (number, wave) in waves.iter().enumerate() {
            for &index in wave.indices() {
                wave_of[index] = number;
            }
        }

        for left in 0..accesses.len() {
            for right in left + 1..accesses.len() {
                if accesses[left].conflicts_with(accesses[right]) {
                    assert!(
                        wave_of[left] < wave_of[right],
                        "seed {seed}: {left} and {right} conflict but are not ordered"
                    );
                }
            }
        }
    }
}
