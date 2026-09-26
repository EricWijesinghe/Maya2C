//! Zipf sampling over `n` ranks by inverse CDF on a precomputed table.
//!
//! Integer weights (`1e12 / rank^s`, computed once with floats at build time
//! of the table, then fixed) so sampling is a binary search over integers and
//! reproducible across machines. Ranks map to account keys directly.

/// A Zipf(n, s) sampler.
#[derive(Clone, Debug)]
pub struct Zipf {
    /// Cumulative integer weights; `cdf[i]` covers ranks `0..=i` of the table.
    cdf: Vec<u64>,
    /// Accounts the table stands for (ranks beyond the table share its tail).
    n: u64,
}

/// Table size: ranks beyond this are uniform over the tail, which keeps the
/// table small for 100M-account workloads while the head stays exact.
const TABLE: u64 = 65_536;

impl Zipf {
    /// Sampler over `n` ranks with exponent `s_x100 / 100`.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn new(n: u64, s_x100: u32) -> Self {
        let n = n.max(1);
        let size = n.min(TABLE);
        let s = f64::from(s_x100) / 100.0;
        let mut cdf = Vec::with_capacity(size as usize + 1);
        let mut acc = 0u64;
        for rank in 1..=size {
            acc += (1e12 / (rank as f64).powf(s)).max(1.0) as u64;
            cdf.push(acc);
        }
        if n > size {
            // The tail: every remaining rank at the last head weight.
            let last = (1e12 / (size as f64).powf(s)).max(1.0) as u64;
            acc += last.saturating_mul(n - size);
            cdf.push(acc);
        }
        Self { cdf, n }
    }

    /// Maps a uniform 64-bit draw to a rank in `0..n`.
    pub fn sample(&self, uniform: u64) -> u64 {
        let total = *self.cdf.last().unwrap_or(&1);
        let point = uniform % total;
        let i = u64::try_from(self.cdf.partition_point(|c| *c <= point)).unwrap_or(u64::MAX);
        let head = self.n.min(TABLE);
        if i < head {
            i
        } else {
            // Uniform over the tail, using the draw's high bits.
            head + (uniform >> 20) % (self.n - head).max(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_zero_is_the_most_frequent_and_uniform_is_flat() {
        let z = Zipf::new(1_000, 100);
        let mut counts = vec![0u32; 1_000];
        let mut x = 7u64;
        for _ in 0..200_000 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            counts[usize::try_from(z.sample(x >> 1)).unwrap_or(0)] += 1;
        }
        assert!(counts[0] > counts[1] && counts[1] > counts[10] && counts[10] > counts[500]);
        let flat = Zipf::new(1_000, 0);
        let mut c0 = 0;
        for i in 0..100_000u64 {
            if flat.sample(i.wrapping_mul(0x9E37_79B9_7F4A_7C15)) == 0 {
                c0 += 1;
            }
        }
        assert!((50..=150).contains(&c0), "{c0}");
    }

    #[test]
    fn samples_stay_in_range_for_huge_populations() {
        let z = Zipf::new(100_000_000, 100);
        for i in 0..10_000u64 {
            assert!(z.sample(i.wrapping_mul(0x2545_F491_4F6C_DD1D)) < 100_000_000);
        }
    }
}
