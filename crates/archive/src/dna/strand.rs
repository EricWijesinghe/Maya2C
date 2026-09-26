//! One strand: bytes ↔ nucleotides, with the biochemical constraints enforced
//! by construction and an inner Reed–Solomon code against substitutions.
//!
//! # Layout
//!
//! ```text
//! forward primer (20 nt)
//! seed (1 B) ‖ whiten(block u16 ‖ index u16 ‖ payload) ‖ inner RS parity
//! reverse primer (20 nt)
//! ```
//!
//! at exactly two bits per base (`00 A`, `01 C`, `10 G`, `11 T`, most
//! significant pair first).
//!
//! # Constraints without giving up two bits per base
//!
//! Synthesis and sequencing both fail on long homopolymer runs and on strands
//! far from 50% GC. A rotation code avoids runs by construction, but at
//! log2(3) = 1.58 bits per base, and the brief asks for two. So this is the
//! screening approach of DNA Fountain (Erlich & Zielinski, *Science* 2017):
//! whiten the strand with a keystream chosen by a one-byte seed, and try seeds
//! until the *whole* strand -- primers, seed, parity and all -- has no run
//! longer than [`MAX_RUN`] and GC content inside [`GC_RANGE`]. The seed is
//! stored in the clear (it has to be read before de-whitening) and the inner
//! code covers it. About one seed in six passes for a 220 nt strand, so 256
//! seeds fail with probability near 10^-21; [`Strand::encode`] reports the
//! failure rather than emitting a strand that breaks a constraint.

use reed_solomon::{Decoder, Encoder};
use sha3::Shake128;
use sha3::digest::{ExtendableOutput, Update, XofReader};

use super::DnaError;
use super::primer::PrimerPair;

/// The longest homopolymer run a strand may contain.
pub const MAX_RUN: usize = 3;
/// GC content every strand is held inside, in percent.
pub const GC_RANGE: core::ops::RangeInclusive<usize> = 40..=60;
/// Payload bytes per strand.
pub const PAYLOAD: usize = 32;
/// Inner Reed–Solomon parity bytes: corrects up to four byte errors.
pub const INNER_PARITY: usize = 8;
/// `block ‖ index`.
const HEADER: usize = 4;
/// Bytes a strand stores between its primers.
pub const STRAND_BYTES: usize = 1 + HEADER + PAYLOAD + INNER_PARITY;
/// Nucleotides between the primers.
pub const BODY_NT: usize = STRAND_BYTES * 4;

const WHITEN_DOMAIN: &[u8] = b"maya2c.archive.dna.whiten.v1";
const BASES: [u8; 4] = *b"ACGT";

/// A strand's decoded contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strand {
    /// Which outer-code block it belongs to.
    pub block: u16,
    /// Its position in that block (data strands first, then parity).
    pub index: u16,
    /// The bytes it carries.
    pub payload: [u8; PAYLOAD],
}

fn keystream(archive: u32, seed: u8) -> [u8; HEADER + PAYLOAD] {
    let mut xof = Shake128::default();
    xof.update(WHITEN_DOMAIN);
    xof.update(&archive.to_le_bytes());
    xof.update(&[seed]);
    let mut out = [0u8; HEADER + PAYLOAD];
    xof.finalize_xof().read(&mut out);
    out
}

/// Two bits per base.
#[must_use]
pub fn to_bases(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .flat_map(|&b| [6u8, 4, 2, 0].map(|shift| BASES[usize::from((b >> shift) & 3)]))
        .collect()
}

/// The inverse of [`to_bases`]; `None` for a symbol that is not `ACGT`.
#[must_use]
pub fn from_bases(bases: &[u8]) -> Option<Vec<u8>> {
    bases
        .chunks(4)
        .map(|quad| {
            quad.iter().try_fold(0u8, |acc, &base| {
                let value = BASES.iter().position(|&b| b == base)?;
                Some((acc << 2) | u8::try_from(value).ok()?)
            })
        })
        .collect()
}

/// Whether `seq` meets both constraints.
#[must_use]
pub fn satisfies_constraints(seq: &[u8]) -> bool {
    let longest_run = seq
        .chunk_by(|a, b| a == b)
        .map(<[u8]>::len)
        .max()
        .unwrap_or(0);
    let gc = seq.iter().filter(|&&b| b == b'G' || b == b'C').count();
    longest_run <= MAX_RUN && !seq.is_empty() && GC_RANGE.contains(&(gc * 100 / seq.len()))
}

impl Strand {
    /// The full synthesisable sequence for this strand of archive `archive`.
    ///
    /// # Errors
    ///
    /// [`DnaError::Unscreenable`] if no whitening seed yields a sequence that
    /// meets the constraints -- which the module docs put near 10^-21.
    pub fn encode(&self, archive: u32, primers: &PrimerPair) -> Result<Vec<u8>, DnaError> {
        let encoder = Encoder::new(INNER_PARITY);
        let mut plain = [0u8; HEADER + PAYLOAD];
        plain[..2].copy_from_slice(&self.block.to_le_bytes());
        plain[2..4].copy_from_slice(&self.index.to_le_bytes());
        plain[HEADER..].copy_from_slice(&self.payload);
        for seed in 0..=u8::MAX {
            let mut stored = Vec::with_capacity(STRAND_BYTES);
            stored.push(seed);
            stored.extend(
                plain
                    .iter()
                    .zip(keystream(archive, seed))
                    .map(|(p, k)| p ^ k),
            );
            let codeword = encoder.encode(&stored);
            let sequence = primers.wrap(&to_bases(&codeword));
            if satisfies_constraints(&sequence) {
                return Ok(sequence);
            }
        }
        Err(DnaError::Unscreenable {
            block: self.block,
            index: self.index,
        })
    }

    /// Reads one strand back from a sequenced read, correcting up to
    /// [`INNER_PARITY`]` / 2` byte errors. `None` for a read of the wrong
    /// length (an insertion or deletion), foreign primers, or more errors than
    /// the inner code corrects: the outer code treats every one of those as an
    /// erasure.
    #[must_use]
    pub fn decode(read: &[u8], archive: u32, primers: &PrimerPair) -> Option<Self> {
        let body = primers.unwrap(read)?;
        let bytes = from_bases(body)?;
        let corrected = Decoder::new(INNER_PARITY).correct(&bytes, None).ok()?;
        let data = corrected.data();
        let seed = data[0];
        let mut plain = [0u8; HEADER + PAYLOAD];
        for ((out, &c), k) in plain
            .iter_mut()
            .zip(&data[1..])
            .zip(keystream(archive, seed))
        {
            *out = c ^ k;
        }
        let mut payload = [0u8; PAYLOAD];
        payload.copy_from_slice(&plain[HEADER..]);
        Some(Self {
            block: u16::from_le_bytes([plain[0], plain[1]]),
            index: u16::from_le_bytes([plain[2], plain[3]]),
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_bits_per_base_round_trip() {
        let bytes: Vec<u8> = (0..=255).collect();
        let bases = to_bases(&bytes);
        assert_eq!(bases.len(), 4 * 256);
        assert_eq!(&bases[..8], b"AAAAAAAC");
        assert_eq!(from_bases(&bases), Some(bytes));
        assert_eq!(from_bases(b"ACGN"), None);
    }

    #[test]
    fn the_constraints_mean_what_they_say() {
        assert!(satisfies_constraints(b"ACGTACGTAAAC"));
        assert!(!satisfies_constraints(b"ACGTAAAACGTC"), "a run of four");
        assert!(!satisfies_constraints(b"ATATATATATAT"), "0% GC");
    }

    #[test]
    fn a_strand_round_trips_through_four_substitutions() {
        let primers = PrimerPair::for_archive(7);
        let strand = Strand {
            block: 3,
            index: 41,
            payload: [0u8; PAYLOAD],
        };
        let mut sequence = strand.encode(7, &primers).expect("screened");
        assert!(satisfies_constraints(&sequence));
        assert_eq!(
            sequence.len(),
            BODY_NT + 2 * super::super::primer::PRIMER_NT
        );
        // One substitution in each of four different bytes of the body.
        for position in [25, 60, 100, 170] {
            sequence[position] = if sequence[position] == b'A' {
                b'C'
            } else {
                b'A'
            };
        }
        assert_eq!(Strand::decode(&sequence, 7, &primers), Some(strand));
    }

    #[test]
    fn an_indel_or_a_foreign_archive_is_an_erasure_not_a_wrong_answer() {
        let primers = PrimerPair::for_archive(7);
        let strand = Strand {
            block: 0,
            index: 0,
            payload: [9u8; PAYLOAD],
        };
        let sequence = strand.encode(7, &primers).expect("screened");
        let mut short = sequence.clone();
        short.remove(80);
        assert_eq!(Strand::decode(&short, 7, &primers), None);
        assert_eq!(
            Strand::decode(&sequence, 8, &PrimerPair::for_archive(8)),
            None
        );
    }
}
