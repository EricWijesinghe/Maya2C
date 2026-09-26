//! **SIM.** A simulated synthesis-storage-sequencing channel. No chemistry
//! happens here: this stands in for the lab so the codec can be tested
//! against the damage a real one does, and every report it produces says so.
//!
//! The model is the one DNA-storage papers test against:
//!
//! - **Dropout** -- a strand is never synthesised, or is lost in storage, and
//!   no read of it exists.
//! - **Coverage** -- each surviving strand is read several times.
//! - **Per-base errors** in each read, independently: substitution, insertion
//!   and deletion, at rates [`Channel`] takes.
//!
//! Its randomness is `xoshiro256**` from a seed, so a failing run replays
//! exactly. That is a simulator's generator, not a cryptographic one, and it
//! never touches key material.

/// Printed at the head of every report this module produces.
pub const BANNER: &str =
    "SIM: simulated DNA synthesis, storage and sequencing -- no chemistry was involved";

/// Always `true`: this module is a simulator.
pub const SIMULATED: bool = true;

/// The error model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Channel {
    /// Probability a strand yields no reads at all.
    pub dropout: f64,
    /// Reads per surviving strand.
    pub coverage: usize,
    /// Per-base substitution probability.
    pub substitution: f64,
    /// Per-base insertion probability.
    pub insertion: f64,
    /// Per-base deletion probability.
    pub deletion: f64,
    /// Seed for the channel's randomness.
    pub seed: u64,
}

impl Channel {
    /// Illumina-like sequencing error rates over a pool that lost 15% of its
    /// strands, read at 5× coverage.
    #[must_use]
    pub const fn fifteen_percent_loss(seed: u64) -> Self {
        Self {
            dropout: 0.15,
            coverage: 5,
            substitution: 0.005,
            insertion: 0.000_5,
            deletion: 0.000_5,
            seed,
        }
    }
}

/// What a channel run did, for the report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Strands in.
    pub strands: usize,
    /// Strands that dropped out.
    pub dropped: usize,
    /// Reads out.
    pub reads: usize,
    /// Substitutions introduced.
    pub substitutions: usize,
    /// Insertions introduced.
    pub insertions: usize,
    /// Deletions introduced.
    pub deletions: usize,
}

struct Rng([u64; 4]);

impl Rng {
    fn new(seed: u64) -> Self {
        // SplitMix64 to fill the state from one word.
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        };
        Self([next(), next(), next(), next()])
    }

    fn next_u64(&mut self) -> u64 {
        let s = &mut self.0;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// A uniform draw in `[0, 1)` from the top 53 bits.
    #[allow(clippy::cast_precision_loss)] // 53 bits is exactly an f64 mantissa
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn base(&mut self) -> u8 {
        b"ACGT"[(self.next_u64() >> 62) as usize]
    }

    fn other_base(&mut self, not: u8) -> u8 {
        loop {
            let b = self.base();
            if b != not {
                return b;
            }
        }
    }
}

/// Runs `strands` through `channel`, returning every read and what was done.
#[must_use]
pub fn sequence(strands: &[Vec<u8>], channel: &Channel) -> (Vec<Vec<u8>>, Tally) {
    let mut rng = Rng::new(channel.seed);
    let mut tally = Tally {
        strands: strands.len(),
        ..Tally::default()
    };
    let mut reads = Vec::with_capacity(strands.len() * channel.coverage);
    for strand in strands {
        if rng.unit() < channel.dropout {
            tally.dropped += 1;
            continue;
        }
        for _ in 0..channel.coverage {
            reads.push(read_once(strand, channel, &mut rng, &mut tally));
        }
    }
    tally.reads = reads.len();
    (reads, tally)
}

fn read_once(strand: &[u8], channel: &Channel, rng: &mut Rng, tally: &mut Tally) -> Vec<u8> {
    let mut read = Vec::with_capacity(strand.len() + 4);
    for &base in strand {
        let roll = rng.unit();
        if roll < channel.deletion {
            tally.deletions += 1;
        } else if roll < channel.deletion + channel.insertion {
            tally.insertions += 1;
            read.push(rng.base());
            read.push(base);
        } else if roll < channel.deletion + channel.insertion + channel.substitution {
            tally.substitutions += 1;
            read.push(rng.other_base(base));
        } else {
            read.push(base);
        }
    }
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_replays_from_its_seed_and_honours_its_rates() {
        let strands = vec![b"ACGT".repeat(55); 2_000];
        let channel = Channel::fifteen_percent_loss(42);
        let (reads, tally) = sequence(&strands, &channel);
        assert_eq!(sequence(&strands, &channel).0, reads, "replayable");
        // 15% of 2,000 strands, within five standard deviations (~80).
        assert!((tally.dropped as i64 - 300).abs() < 80, "{tally:?}");
        assert_eq!(
            tally.reads,
            (tally.strands - tally.dropped) * channel.coverage
        );
        // 0.5% of ~1.87M read bases is ~9,350 substitutions.
        assert!((8_500..10_200).contains(&tally.substitutions), "{tally:?}");
        assert!(SIMULATED && BANNER.starts_with("SIM"));
    }
}
