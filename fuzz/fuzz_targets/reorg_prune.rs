//! Fuzzes the pure fork-choice and reorg-depth rules.
//!
//! A reorganisation is decided by cumulative work, and cumulative work is the
//! sum of per-header work derived from each target. Fork choice relies on
//! four facts, which this target asserts for arbitrary targets:
//!
//! - work is non-increasing in the target, so a harder header never counts
//!   for less than an easier one;
//! - cumulative work is order-independent, because it saturates at `U256::MAX`
//!   and a saturating sum of non-negative terms is `min(MAX, total)`;
//! - extending a branch never lowers its work;
//! - a retarget stays between one and the floor, so no branch can be made
//!   unsatisfiable or easier than the network's limit.
//!
//! The prune horizon bounds reorg depth: a height refused at the horizon must
//! stay refused at every lower height.
//!
//! Input layout: `count` (1 byte, 1..=8 targets), `timespan` (8 bytes LE),
//! `height` (8 bytes LE), `horizon` (8 bytes LE), then 32-byte targets.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::consensus::U256;
use custom_l1_node::consensus::chain::below_prune_horizon;
use custom_l1_node::consensus::difficulty::{cumulative_work, retarget, work_from_target};
use libfuzzer_sys::fuzz_target;

const HEADER_FIELDS: usize = 1 + 8 + 8 + 8;
const MAX_TARGETS: usize = 8;

fuzz_target!(|data: &[u8]| {
    if data.len() < HEADER_FIELDS {
        return;
    }
    let take = usize::from(data[0]) % MAX_TARGETS + 1;
    let timespan = u64::from_le_bytes(data[1..9].try_into().expect("sized"));
    let height = u64::from_le_bytes(data[9..17].try_into().expect("sized"));
    let horizon = u64::from_le_bytes(data[17..25].try_into().expect("sized"));

    let targets: Vec<[u8; 32]> = data[HEADER_FIELDS..]
        .chunks_exact(32)
        .take(take)
        .map(|chunk| <[u8; 32]>::try_from(chunk).expect("sized"))
        .collect();
    if targets.is_empty() {
        return;
    }

    // Work falls as the target rises: a lexicographically smaller target is a
    // harder one, so its work is at least as large.
    for a in &targets {
        for b in &targets {
            if a <= b {
                assert!(
                    work_from_target(a) >= work_from_target(b),
                    "work rose with the target"
                );
            }
        }
    }

    // Saturating sums of non-negative terms do not depend on order.
    let forward = cumulative_work(targets.iter());
    let backward = cumulative_work(targets.iter().rev());
    assert!(forward == backward, "cumulative work depends on order");

    // Extending a branch never lowers its work.
    let mut previous = U256::ZERO;
    for end in 1..=targets.len() {
        let current = cumulative_work(targets[..end].iter());
        assert!(current >= previous, "extending a branch lowered its work");
        previous = current;
    }

    // A retarget lands in [1, limit], with the limit the largest target seen.
    // A zero limit is a configuration error, not a retarget input, so skip it.
    let previous_target = targets[0];
    let limit = *targets.iter().max().expect("non-empty");
    if limit != [0u8; 32] {
        let next = retarget(&previous_target, timespan, &limit);
        assert!(next != [0u8; 32], "retarget produced a zero target");
        assert!(next <= limit, "retarget exceeded the floor");
    }

    // Refused at a horizon implies refused at every lower height.
    if below_prune_horizon(height, horizon) {
        assert!(below_prune_horizon(height.saturating_sub(1), horizon));
    }
});
