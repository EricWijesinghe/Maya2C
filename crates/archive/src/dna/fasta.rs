//! FASTA for what goes to a synthesis vendor, FASTQ for what comes back from
//! a sequencer. The decoder trusts neither file's headers -- a strand says
//! which block and index it is from inside its error-corrected body -- so a
//! reordered, relabelled or merged file decodes the same.

use core::fmt::Write as _;

/// One `>` record per strand, named by archive and position in synthesis
/// order.
#[must_use]
pub fn write_fasta(archive: u32, strands: &[Vec<u8>]) -> String {
    let mut out = String::new();
    for (i, strand) in strands.iter().enumerate() {
        let _ = writeln!(
            out,
            ">maya2c archive={archive} strand={i} length={}",
            strand.len()
        );
        out.push_str(&String::from_utf8_lossy(strand));
        out.push('\n');
    }
    out
}

/// One `@` record per read. The simulator has no per-base quality, so every
/// base gets the same Phred score (`I`, Q40) -- the decoder does not use it.
#[must_use]
pub fn write_fastq(reads: &[Vec<u8>]) -> String {
    let mut out = String::new();
    for (i, read) in reads.iter().enumerate() {
        let _ = writeln!(out, "@read{i}");
        out.push_str(&String::from_utf8_lossy(read));
        out.push_str("\n+\n");
        out.push_str(&"I".repeat(read.len()));
        out.push('\n');
    }
    out
}

/// The sequences in a FASTA or FASTQ file, in order. Headers, `+` lines and
/// quality lines are skipped; a FASTA record's sequence may span lines.
#[must_use]
pub fn read_sequences(text: &str) -> Vec<Vec<u8>> {
    let mut sequences = Vec::new();
    let mut lines = text.lines();
    let mut current: Option<Vec<u8>> = None;
    while let Some(line) = lines.next() {
        match line.as_bytes().first() {
            Some(b'>') => {
                sequences.extend(current.take());
                current = Some(Vec::new());
            }
            Some(b'@') => {
                sequences.extend(current.take());
                if let Some(seq) = lines.next() {
                    sequences.push(seq.as_bytes().to_vec());
                }
                // The `+` separator and the quality line.
                lines.next();
                lines.next();
            }
            Some(_) => {
                if let Some(seq) = current.as_mut() {
                    seq.extend_from_slice(line.trim().as_bytes());
                }
            }
            None => {}
        }
    }
    sequences.extend(current);
    sequences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_formats_round_trip_their_sequences() {
        let strands = vec![b"ACGTTGCA".to_vec(), b"GGCATT".to_vec()];
        assert_eq!(read_sequences(&write_fasta(3, &strands)), strands);
        assert_eq!(read_sequences(&write_fastq(&strands)), strands);
        // A FASTA sequence wrapped at a fixed width is one sequence.
        assert_eq!(
            read_sequences(">x\nACGT\nTGCA\n>y\nGG\n"),
            vec![b"ACGTTGCA".to_vec(), b"GG".to_vec()]
        );
    }
}
