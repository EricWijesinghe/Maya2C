//! Primer addressing: every strand of one archive is flanked by the same
//! forward and reverse primer, derived from the archive's id.
//!
//! In a pooled synthesis many archives share one tube; PCR with an archive's
//! primer pair amplifies only its strands, which is how one file is retrieved
//! from a pool without sequencing all of it. Here the pair also filters reads:
//! a read whose flanks are too far from this archive's primers belongs to
//! something else and is dropped.

use sha3::Shake128;
use sha3::digest::{ExtendableOutput, Update, XofReader};

use super::strand::satisfies_constraints;

/// Primer length, nucleotides.
pub const PRIMER_NT: usize = 20;
/// Substitutions a primer may show and still be recognised.
pub const PRIMER_TOLERANCE: usize = 4;

const PRIMER_DOMAIN: &[u8] = b"maya2c.archive.dna.primer.v1";

/// One archive's forward and reverse primer, in the orientation they appear
/// on the stored strand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrimerPair {
    forward: [u8; PRIMER_NT],
    reverse: [u8; PRIMER_NT],
}

fn hamming(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

impl PrimerPair {
    /// The pair for archive `archive`: the first candidates from a keyed XOF
    /// that meet the strand constraints on their own and differ from each
    /// other in at least half their positions.
    #[must_use]
    pub fn for_archive(archive: u32) -> Self {
        let mut xof = Shake128::default();
        xof.update(PRIMER_DOMAIN);
        xof.update(&archive.to_le_bytes());
        let mut reader = xof.finalize_xof();
        let mut candidate = || loop {
            let mut raw = [0u8; PRIMER_NT];
            reader.read(&mut raw);
            let primer = raw.map(|b| b"ACGT"[usize::from(b & 3)]);
            if satisfies_constraints(&primer) {
                break primer;
            }
        };
        let forward = candidate();
        let reverse = loop {
            let r = candidate();
            if hamming(&r, &forward) >= PRIMER_NT / 2 {
                break r;
            }
        };
        Self { forward, reverse }
    }

    /// `forward ‖ body ‖ reverse`.
    #[must_use]
    pub fn wrap(&self, body: &[u8]) -> Vec<u8> {
        [&self.forward[..], body, &self.reverse[..]].concat()
    }

    /// The body of a read, if its length is exactly one strand's and both
    /// flanks are within [`PRIMER_TOLERANCE`] substitutions of this pair.
    #[must_use]
    pub fn unwrap<'a>(&self, read: &'a [u8]) -> Option<&'a [u8]> {
        if read.len() != super::strand::BODY_NT + 2 * PRIMER_NT {
            return None;
        }
        let (head, rest) = read.split_at(PRIMER_NT);
        let (body, tail) = rest.split_at(rest.len() - PRIMER_NT);
        let close = |got: &[u8], want: &[u8]| hamming(got, want) <= PRIMER_TOLERANCE;
        (close(head, &self.forward) && close(tail, &self.reverse)).then_some(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_get_distinct_well_formed_primers() {
        let a = PrimerPair::for_archive(1);
        let b = PrimerPair::for_archive(2);
        assert_ne!(a, b);
        for p in [&a, &b] {
            assert!(satisfies_constraints(&p.forward));
            assert!(satisfies_constraints(&p.reverse));
            assert!(hamming(&p.forward, &p.reverse) >= PRIMER_NT / 2);
        }
        assert_eq!(PrimerPair::for_archive(1), a, "deterministic");
    }
}
