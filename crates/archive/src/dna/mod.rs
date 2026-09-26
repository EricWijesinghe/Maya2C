//! A DNA archive codec: bytes to synthesisable oligonucleotides and back,
//! through the errors synthesis, storage and sequencing introduce.
//!
//! **The codec is REAL; the lab is SIM.** Everything here produces sequences a
//! synthesis vendor would accept (FASTA, [`fasta`]) and decodes reads a
//! sequencer would return (FASTQ). What cannot be done here is the chemistry,
//! so [`sim`] stands in for synthesis, storage and sequencing, and says so.
//!
//! # Two codes, because there are two kinds of damage
//!
//! - **Inside a strand**, synthesis and sequencing substitute bases. The inner
//!   Reed–Solomon code ([`strand`]) corrects up to four corrupted bytes per
//!   strand. An insertion or deletion changes the read's length, which the
//!   decoder sees and discards the read for -- another read of the same strand
//!   (sequencing coverage) usually survives.
//! - **Across strands**, some never make it: dropout in synthesis, loss in
//!   storage, strands no read recovered. The outer code ([`outer`]) is an
//!   erasure code over blocks of [`BLOCK_DATA`] strands with
//!   [`BLOCK_PARITY_PERCENT`]% parity, so a block survives losing a third of
//!   its strands.
//!
//! # Nothing decodes silently wrong
//!
//! The archive is framed as `length ‖ BLAKE3(data) ‖ data`. A read the inner
//! code *mis*corrects is outvoted by the other reads of its strand; if one
//! still got through, the digest check fails and [`decode`] returns an error
//! instead of the wrong bytes.

pub mod fasta;
pub mod outer;
pub mod primer;
pub mod sim;
pub mod strand;

use std::collections::BTreeMap;

use primer::PrimerPair;
use strand::{PAYLOAD, Strand};

/// Data strands per outer-code block.
pub const BLOCK_DATA: usize = 100;
/// Parity strands per block, as a percentage of its data strands.
pub const BLOCK_PARITY_PERCENT: usize = 50;
/// `u64` length and a 32-byte BLAKE3 digest.
const FRAME: usize = 8 + 32;

/// What can go wrong.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DnaError {
    /// No whitening seed met the constraints (see [`strand`]).
    #[error("strand {index} of block {block}: no whitening seed meets the constraints")]
    Unscreenable {
        /// Block.
        block: u16,
        /// Index in the block.
        index: u16,
    },
    /// The archive is too large for 16-bit block numbers.
    #[error("{0} bytes is more than one DNA archive holds")]
    TooLarge(usize),
    /// A block lost more strands than its parity replaces.
    #[error("block {block} recovered {have} of the {need} strands it needs")]
    BlockLost {
        /// Block.
        block: u16,
        /// Strands recovered.
        have: usize,
        /// Strands needed.
        need: usize,
    },
    /// The outer code refused its inputs.
    #[error("outer code: {0}")]
    Outer(String),
    /// The recovered bytes are not the archive that was encoded.
    #[error("the decoded archive does not match its own digest")]
    DigestMismatch,
}

fn parity_for(data: usize) -> usize {
    (data * BLOCK_PARITY_PERCENT).div_ceil(100).max(1)
}

fn frame(data: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(FRAME + data.len());
    framed.extend_from_slice(&(data.len() as u64).to_le_bytes());
    framed.extend_from_slice(blake3::hash(data).as_bytes());
    framed.extend_from_slice(data);
    framed.resize(framed.len().div_ceil(PAYLOAD) * PAYLOAD, 0);
    framed
}

/// Encodes `data` as the strands of archive `archive`, in synthesis order.
///
/// # Errors
///
/// [`DnaError::TooLarge`] past 65,535 blocks, or [`DnaError::Unscreenable`].
pub fn encode(data: &[u8], archive: u32) -> Result<Vec<Vec<u8>>, DnaError> {
    let framed = frame(data);
    let chunks: Vec<[u8; PAYLOAD]> = framed
        .chunks(PAYLOAD)
        .map(|c| c.try_into().expect("padded to PAYLOAD"))
        .collect();
    let blocks: Vec<&[[u8; PAYLOAD]]> = chunks.chunks(BLOCK_DATA).collect();
    if blocks.len() > usize::from(u16::MAX) {
        return Err(DnaError::TooLarge(data.len()));
    }
    let primers = PrimerPair::for_archive(archive);
    let mut sequences = Vec::new();
    for (b, block) in blocks.iter().enumerate() {
        let block_no = u16::try_from(b).expect("checked above");
        let parity = outer::encode(block, parity_for(block.len()))?;
        for (i, payload) in block.iter().chain(&parity).enumerate() {
            let strand = Strand {
                block: block_no,
                index: u16::try_from(i).expect("< 2^16"),
                payload: *payload,
            };
            sequences.push(strand.encode(archive, &primers)?);
        }
    }
    Ok(sequences)
}

/// Decodes archive `archive` from sequenced reads, in any order, with any
/// number of reads per strand and any number of strands missing up to what
/// the outer code replaces.
///
/// # Errors
///
/// [`DnaError::BlockLost`], [`DnaError::Outer`], or
/// [`DnaError::DigestMismatch`].
pub fn decode<'a>(
    reads: impl IntoIterator<Item = &'a [u8]>,
    archive: u32,
) -> Result<Vec<u8>, DnaError> {
    let primers = PrimerPair::for_archive(archive);
    let strands = vote(
        reads
            .into_iter()
            .filter_map(|r| Strand::decode(r, archive, &primers)),
    );
    let frame_block = strands.get(&(0, 0)).ok_or(DnaError::BlockLost {
        block: 0,
        have: 0,
        need: 1,
    })?;
    let length = u64::from_le_bytes(frame_block[..8].try_into().expect("8 bytes"));
    let length = usize::try_from(length).map_err(|_| DnaError::TooLarge(usize::MAX))?;
    let total_chunks = (FRAME + length).div_ceil(PAYLOAD);
    let mut framed = Vec::with_capacity(total_chunks * PAYLOAD);
    for (b, first) in (0..total_chunks).step_by(BLOCK_DATA).enumerate() {
        let data = (total_chunks - first).min(BLOCK_DATA);
        let block = u16::try_from(b).map_err(|_| DnaError::TooLarge(length))?;
        framed.extend(
            outer::decode(&strands, block, data, parity_for(data))?
                .into_iter()
                .flatten(),
        );
    }
    let body = framed
        .get(FRAME..FRAME + length)
        .ok_or(DnaError::DigestMismatch)?;
    if blake3::hash(body).as_bytes()[..] != framed[8..FRAME] {
        return Err(DnaError::DigestMismatch);
    }
    Ok(body.to_vec())
}

/// For each `(block, index)`, the payload most of its reads agree on.
fn vote(decoded: impl Iterator<Item = Strand>) -> BTreeMap<(u16, u16), [u8; PAYLOAD]> {
    let mut tallies: BTreeMap<(u16, u16), BTreeMap<[u8; PAYLOAD], usize>> = BTreeMap::new();
    for strand in decoded {
        *tallies
            .entry((strand.block, strand.index))
            .or_default()
            .entry(strand.payload)
            .or_default() += 1;
    }
    tallies
        .into_iter()
        .filter_map(|(key, counts)| {
            let (payload, _) = counts.into_iter().max_by_key(|&(_, n)| n)?;
            Some((key, payload))
        })
        .collect()
}
