//! The DNA codec end to end, against the brief's test: a 1 MB snapshot
//! encoded to strands, put through substitutions, insertions, deletions and
//! 15% strand loss, and restored without a byte wrong.
//!
//! The channel is [`sim`] -- **SIM**, no chemistry. What these tests establish
//! is that the codec survives the damage model DNA-storage work tests
//! against, and that when damage exceeds what it corrects it says so rather
//! than returning wrong bytes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_archive::dna::primer::PRIMER_NT;
use maya_archive::dna::sim::{self, Channel};
use maya_archive::dna::strand::{BODY_NT, satisfies_constraints};
use maya_archive::dna::{DnaError, decode, encode, fasta};

const MEGABYTE: usize = 1 << 20;

/// Deterministic incompressible bytes: a snapshot's worth of BLAKE3 XOF.
fn snapshot(len: usize, seed: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; len];
    blake3::Hasher::new()
        .update(seed)
        .finalize_xof()
        .fill(&mut out);
    out
}

fn reads_of(data: &[u8], archive: u32, channel: &Channel) -> (Vec<Vec<u8>>, sim::Tally) {
    let strands = encode(data, archive).expect("encode");
    sim::sequence(&strands, channel)
}

#[test]
fn a_megabyte_survives_substitutions_indels_and_fifteen_percent_strand_loss() {
    let data = snapshot(MEGABYTE, b"maya2c dna 1 MiB snapshot");
    let strands = encode(&data, 1).expect("encode");
    for strand in &strands {
        assert_eq!(strand.len(), BODY_NT + 2 * PRIMER_NT);
        assert!(
            satisfies_constraints(strand),
            "every strand meets both constraints"
        );
    }
    let channel = Channel::fifteen_percent_loss(7);
    let (reads, tally) = sim::sequence(&strands, &channel);
    assert!(
        tally.dropped * 100 >= tally.strands * 14,
        "the channel really lost ~15%: {tally:?}"
    );
    assert!(
        tally.substitutions > 0 && tally.insertions > 0 && tally.deletions > 0,
        "{tally:?}"
    );
    // Through the file format a sequencer would hand back.
    let fastq = fasta::write_fastq(&reads);
    let parsed = fasta::read_sequences(&fastq);
    assert_eq!(parsed.len(), reads.len());
    let restored = decode(parsed.iter().map(Vec::as_slice), 1).expect("decode");
    assert!(restored == data, "lossless");
}

#[test]
fn low_entropy_data_still_meets_the_constraints() {
    // All zeros is the worst case for homopolymers before whitening.
    let strands = encode(&vec![0u8; 64 * 1024], 2).expect("encode");
    assert!(strands.iter().all(|s| satisfies_constraints(s)));
    let (reads, _) = sim::sequence(&strands, &Channel::fifteen_percent_loss(3));
    assert_eq!(
        decode(reads.iter().map(Vec::as_slice), 2).expect("decode"),
        vec![0u8; 64 * 1024]
    );
}

#[test]
fn losing_more_than_the_outer_code_replaces_is_an_error_not_wrong_bytes() {
    let data = snapshot(32 * 1024, b"loss");
    let channel = Channel {
        dropout: 0.6,
        ..Channel::fifteen_percent_loss(11)
    };
    let (reads, _) = reads_of(&data, 3, &channel);
    assert!(matches!(
        decode(reads.iter().map(Vec::as_slice), 3),
        Err(DnaError::BlockLost { .. })
    ));
}

#[test]
fn another_archives_reads_decode_to_nothing() {
    let data = snapshot(8 * 1024, b"other");
    let (reads, _) = reads_of(&data, 4, &Channel::fifteen_percent_loss(5));
    // Archive 5's primers match none of archive 4's reads.
    assert!(decode(reads.iter().map(Vec::as_slice), 5).is_err());
}

#[test]
fn the_empty_archive_and_the_fasta_file_both_round_trip() {
    let strands = encode(&[], 6).expect("encode");
    let text = fasta::write_fasta(6, &strands);
    assert!(text.starts_with(">maya2c archive=6"));
    let back = fasta::read_sequences(&text);
    assert_eq!(
        decode(back.iter().map(Vec::as_slice), 6).expect("decode"),
        Vec::<u8>::new()
    );
}
