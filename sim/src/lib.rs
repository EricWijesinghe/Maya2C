//! Deterministic simulation for `Maya2C`.
//!
//! A seeded, virtual-time harness for testing the things that only go wrong
//! when the network is slow, lossy, reordering or split, and when the disk
//! does not do what it was asked. Everything later chaos, latency, partition
//! and Byzantine work needs runs on this.
//!
//! # The one rule
//!
//! **A run is a function of its seed and nothing else.** No wall clock, no
//! thread scheduling, no hash iteration order, no address, no dependency with
//! its own RNG — the crate has no dependencies at all, for that reason. When
//! a run fails, its seed is printed, and replaying that seed reproduces the
//! failure exactly, on any machine and any toolchain.
//!
//! # Shape
//!
//! | Module | What it decides |
//! |---|---|
//! | [`rng`] | every random choice, from one seed |
//! | [`clock`] | virtual time, which moves only when the scheduler moves it |
//! | [`net`] | latency, loss, reordering, partitions |
//! | [`disk`] | slow, corrupt, torn, lost-on-crash and full writes |
//! | [`world`] | the event queue that ties them together |
//!
//! Each model holds its own stream forked from the seed, so adding a draw in
//! one does not shift the others. That is what lets a recorded seed keep its
//! meaning after an unrelated change.
//!
//! # Use
//!
//! ```
//! use maya_sim::{Duration, Instant, LinkModel, NodeId, World, replay};
//!
//! replay(0x1234, |world: &mut World<u32>| {
//!     world.net_mut().set_link(LinkModel::wide_area());
//!     world.send(NodeId(0), NodeId(1), 42);
//!     world.run_until(Instant::START + Duration::from_secs(1), 1_000, |_, event| {
//!         assert_eq!(event.payload, 42);
//!     });
//! });
//! ```
//!
//! # What it is not
//!
//! It does not intercept `tokio`, so it does not make the node's real async
//! code deterministic — `madsim` does that by replacing the runtime, at the
//! cost of a dependency that owns scheduling. That trade was considered and
//! not taken; see `docs/adr/ADR-006-simulation-harness.md`. What this models
//! is the *environment*, and the code under test is written against the
//! model rather than against a socket.

pub mod clock;
pub mod disk;
pub mod net;
pub mod rng;
pub mod world;

pub use clock::{Clock, Duration, Instant};
pub use disk::{Disk, DiskModel, WriteOutcome};
pub use net::{LinkModel, Network, NodeId, Verdict};
pub use rng::{PPM, SimRng};
pub use world::{Event, RunOutcome, Sent, World};

/// Run `body` in a world built from `seed`, and print the seed if it panics.
///
/// The seed is the whole point of the harness, and a failure that does not
/// name it is a failure nobody can reproduce. The panic is re-raised, so the
/// test still fails — this only makes sure the seed is on the way out.
///
/// # Panics
///
/// Re-panics with whatever `body` panicked with.
pub fn replay<M, F>(seed: u64, body: F)
where
    M: Eq,
    F: FnOnce(&mut World<M>) + std::panic::UnwindSafe,
{
    let mut world = World::<M>::new(seed);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&mut world)));
    if let Err(panic) = result {
        eprintln!(
            "\n=== simulation failed ===\nseed: {seed:#018x} ({seed})\n\
             virtual time: {}\ndelivered/dropped: {:?}\n\
             replay with: maya_sim::replay({seed}, ..)\n",
            world.now(),
            world.counts()
        );
        std::panic::resume_unwind(panic);
    }
}

/// A seed for a run that does not care which one it gets, derived from a
/// counter rather than from the clock so a test binary's *sequence* of seeds
/// is itself reproducible.
///
/// Use this for a soak that explores; use a literal for a regression.
#[must_use]
pub fn next_seed() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // SplitMix64's finalizer, so consecutive counters give unrelated seeds.
    let mut z = n.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn replay_reruns_a_failure_identically() {
        let capture = |seed: u64| {
            let mut w: World<u32> = World::new(seed);
            w.net_mut().set_link(LinkModel::wide_area());
            for i in 0..100 {
                w.send(NodeId(0), NodeId(1), i);
            }
            let mut seen = Vec::new();
            w.run_until(Instant::START + Duration::from_secs(5), 1_000, |_, e| {
                seen.push((e.at.as_nanos(), e.payload));
            });
            seen
        };
        assert_eq!(capture(4242), capture(4242));
    }

    #[test]
    #[should_panic(expected = "deliberate")]
    fn replay_reraises_the_panic() {
        replay(1, |_w: &mut World<u32>| panic!("deliberate"));
    }

    #[test]
    fn next_seed_does_not_repeat_itself() {
        let seeds: std::collections::BTreeSet<u64> = (0..256).map(|_| next_seed()).collect();
        assert_eq!(seeds.len(), 256, "next_seed collided");
    }
}
