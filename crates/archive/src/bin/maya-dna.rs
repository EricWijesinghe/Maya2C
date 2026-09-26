//! `maya-dna`: the DNA archive codec from the command line.
//!
//! ```text
//! maya-dna encode   <input> <archive-id> <strands.fasta>   bytes -> synthesis order
//! maya-dna decode   <reads.fastq|fasta> <archive-id> <output>
//! maya-dna simulate <strands.fasta> <reads.fastq> [seed]   SIM: the lab, simulated
//! maya-dna report   <bytes> [seed]                          measure the whole pipeline
//! ```
//!
//! `encode` and `decode` are the real codec: the FASTA is what a synthesis
//! vendor takes, and `decode` accepts a sequencer's FASTQ. `simulate` and
//! `report` run the **SIM** channel and print its banner first, every time.

use std::process::ExitCode;
use std::time::Instant;

use maya_archive::dna::{self, fasta, sim};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("encode") if args.len() == 4 => encode(&args[1], &args[2], &args[3]),
        Some("decode") if args.len() == 4 => decode(&args[1], &args[2], &args[3]),
        Some("simulate") if (3..=4).contains(&args.len()) => {
            simulate(&args[1], &args[2], args.get(3))
        }
        Some("report") if (2..=3).contains(&args.len()) => report(&args[1], args.get(2)),
        _ => Err(usage()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("maya-dna: {message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    "usage: maya-dna encode <input> <archive-id> <out.fasta> | decode <reads> <archive-id> <out> \
     | simulate <in.fasta> <out.fastq> [seed] | report <bytes> [seed]"
        .to_string()
}

fn number<T: core::str::FromStr>(text: &str, what: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{what} must be a number, got {text:?}"))
}

fn encode(input: &str, archive: &str, out: &str) -> Result<(), String> {
    let archive: u32 = number(archive, "archive-id")?;
    let data = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let strands = dna::encode(&data, archive).map_err(|e| e.to_string())?;
    std::fs::write(out, fasta::write_fasta(archive, &strands))
        .map_err(|e| format!("{out}: {e}"))?;
    println!(
        "{} bytes -> {} strands of {} nt -> {out}",
        data.len(),
        strands.len(),
        strands.first().map_or(0, Vec::len)
    );
    Ok(())
}

fn decode(reads: &str, archive: &str, out: &str) -> Result<(), String> {
    let archive: u32 = number(archive, "archive-id")?;
    let text = std::fs::read_to_string(reads).map_err(|e| format!("{reads}: {e}"))?;
    let sequences = fasta::read_sequences(&text);
    let data =
        dna::decode(sequences.iter().map(Vec::as_slice), archive).map_err(|e| e.to_string())?;
    std::fs::write(out, &data).map_err(|e| format!("{out}: {e}"))?;
    println!("{} reads -> {} bytes -> {out}", sequences.len(), data.len());
    Ok(())
}

fn simulate(input: &str, out: &str, seed: Option<&String>) -> Result<(), String> {
    println!("{}", sim::BANNER);
    let seed = seed.map_or(Ok(1), |s| number(s, "seed"))?;
    let text = std::fs::read_to_string(input).map_err(|e| format!("{input}: {e}"))?;
    let strands = fasta::read_sequences(&text);
    let channel = sim::Channel::fifteen_percent_loss(seed);
    let (reads, tally) = sim::sequence(&strands, &channel);
    std::fs::write(out, fasta::write_fastq(&reads)).map_err(|e| format!("{out}: {e}"))?;
    println!("{tally:?} -> {out}");
    Ok(())
}

#[allow(clippy::cast_precision_loss)] // report arithmetic on counts
fn report(bytes: &str, seed: Option<&String>) -> Result<(), String> {
    println!("{}", sim::BANNER);
    let len: usize = number(bytes, "bytes")?;
    let seed: u64 = seed.map_or(Ok(7), |s| number(s, "seed"))?;
    let mut data = vec![0u8; len];
    blake3::Hasher::new()
        .update(&seed.to_le_bytes())
        .finalize_xof()
        .fill(&mut data);

    let started = Instant::now();
    let strands = dna::encode(&data, 1).map_err(|e| e.to_string())?;
    let encoded = started.elapsed();
    let nucleotides: usize = strands.iter().map(Vec::len).sum();
    let channel = sim::Channel::fifteen_percent_loss(seed);
    let (reads, tally) = sim::sequence(&strands, &channel);
    let started = Instant::now();
    let restored = dna::decode(reads.iter().map(Vec::as_slice), 1).map_err(|e| e.to_string())?;
    let decoded = started.elapsed();

    println!("channel        {channel:?}");
    println!("input          {len} bytes");
    println!(
        "strands        {} ({} nt each, {nucleotides} nt in all)",
        strands.len(),
        strands[0].len()
    );
    println!(
        "net density    {:.3} bits per synthesised nt (primers, seed, inner and outer parity included)",
        (len * 8) as f64 / nucleotides as f64
    );
    println!(
        "dropped        {} of {} strands ({:.1}%)",
        tally.dropped,
        tally.strands,
        tally.dropped as f64 * 100.0 / tally.strands as f64
    );
    println!(
        "reads          {} ({} substitutions, {} insertions, {} deletions)",
        tally.reads, tally.substitutions, tally.insertions, tally.deletions
    );
    println!(
        "encode         {:.2?} ({:.1} MB/s)",
        encoded,
        len as f64 / encoded.as_secs_f64() / 1e6
    );
    println!(
        "decode         {:.2?} ({:.1} MB/s)",
        decoded,
        len as f64 / decoded.as_secs_f64() / 1e6
    );
    println!(
        "restored       {}",
        if restored == data {
            "byte-for-byte"
        } else {
            "MISMATCH"
        }
    );
    if restored == data {
        Ok(())
    } else {
        Err("decoded bytes differ from the input".to_string())
    }
}
