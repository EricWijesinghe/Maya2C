//! The one source of randomness in a simulation.
//!
//! SplitMix64, because it is four lines, has no state beyond a `u64`, and is
//! identical on every platform and every toolchain. That last property is the
//! whole point: a seed that reproduces a failure on one machine has to
//! reproduce it on another, and an RNG that consults the operating system,
//! the clock, or a hash of a pointer cannot promise that.
//!
//! Probabilities are parts per million, as integers. A float probability
//! would be a second thing to disagree about across platforms for no gain —
//! nobody needs a loss rate finer than 0.0001%.

/// Parts per million. `1_000_000` is certainty.
pub const PPM: u32 = 1_000_000;

/// A seeded, reproducible generator.
///
/// Cloning it forks the stream, which is how a model takes a private stream
/// without perturbing anyone else's — see [`crate::World::fork_rng`].
#[derive(Clone, Debug)]
pub struct SimRng {
    state: u64,
}

impl SimRng {
    /// A generator from a seed. The same seed always yields the same stream.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n`, or `0` when `n` is zero.
    ///
    /// Lemire's multiply-shift rather than a modulus: uniform without a
    /// rejection loop, so the number of `next_u64` calls per draw is fixed
    /// and a replay consumes the stream at exactly the same rate.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }

    /// A value in `low..=high`. Panics if `high < low`.
    pub fn between(&mut self, low: u64, high: u64) -> u64 {
        assert!(high >= low, "empty range {low}..={high}");
        low + self.below(high - low + 1)
    }

    /// `true` with probability `ppm` parts per million.
    ///
    /// `0` never fires and `PPM` always does, exactly — a model that asks for
    /// "no loss" must get no loss, not one packet in a million.
    pub fn chance(&mut self, ppm: u32) -> bool {
        if ppm == 0 {
            return false;
        }
        if ppm >= PPM {
            return true;
        }
        (self.below(u64::from(PPM)) as u32) < ppm
    }

    /// A private stream, derived from this one, that advances independently.
    ///
    /// Each model holds its own so that adding a call in one of them does not
    /// shift every later draw in the others — the failure mode that makes a
    /// seed stop reproducing after an unrelated change.
    pub fn fork(&mut self) -> Self {
        Self::new(self.next_u64())
    }

    /// Fisher-Yates, from this stream.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_stream() {
        let mut a = SimRng::new(0x5EED);
        let mut b = SimRng::new(0x5EED);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn a_different_seed_gives_a_different_stream() {
        let mut a = SimRng::new(1);
        let mut b = SimRng::new(2);
        let first: Vec<u64> = (0..16).map(|_| a.next_u64()).collect();
        let second: Vec<u64> = (0..16).map(|_| b.next_u64()).collect();
        assert_ne!(first, second);
    }

    #[test]
    fn below_stays_in_range_and_zero_is_not_a_panic() {
        let mut rng = SimRng::new(7);
        for n in [1u64, 2, 3, 17, 1024, u64::MAX] {
            for _ in 0..200 {
                assert!(rng.below(n) < n, "below({n}) escaped its range");
            }
        }
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn certainty_and_impossibility_are_exact() {
        let mut rng = SimRng::new(11);
        for _ in 0..10_000 {
            assert!(!rng.chance(0), "0 ppm fired");
            assert!(rng.chance(PPM), "1_000_000 ppm did not fire");
        }
    }

    #[test]
    fn a_stated_probability_is_roughly_honoured() {
        // Not a statistics test — a check that `chance` is not stuck.
        let mut rng = SimRng::new(23);
        let hits = (0..100_000).filter(|_| rng.chance(PPM / 10)).count();
        assert!(
            (8_000..12_000).contains(&hits),
            "10% of 100000 landed at {hits}"
        );
    }

    #[test]
    fn forking_does_not_disturb_the_parent_stream() {
        let mut parent = SimRng::new(99);
        let expected: Vec<u64> = {
            let mut probe = SimRng::new(99);
            probe.next_u64(); // the draw `fork` consumes
            (0..8).map(|_| probe.next_u64()).collect()
        };
        let mut child = parent.fork();
        for _ in 0..1000 {
            child.next_u64();
        }
        let actual: Vec<u64> = (0..8).map(|_| parent.next_u64()).collect();
        assert_eq!(actual, expected, "a child's draws moved the parent");
    }

    #[test]
    fn shuffle_is_a_permutation_and_is_reproducible() {
        let mut first: Vec<u32> = (0..64).collect();
        let mut second = first.clone();
        SimRng::new(5).shuffle(&mut first);
        SimRng::new(5).shuffle(&mut second);
        assert_eq!(first, second);
        let mut sorted = first.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..64).collect::<Vec<u32>>());
    }
}
