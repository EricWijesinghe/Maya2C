//! Standalone ArgonBlake miner.
//!
//! Fetches a candidate header, searches for a satisfying nonce across several
//! threads, and submits the solved block back to the chain.
//!
//! ## On "fetches from the node"
//!
//! The candidate source is abstracted behind [`CandidateSource`], but only the
//! in-process [`LocalSource`] is implemented today. A miner in a *separate*
//! process cannot share the node's storage: RocksDB takes an exclusive lock on
//! its directory, so two processes cannot open the same database. Connecting a
//! remote miner therefore requires an RPC endpoint (`getblocktemplate` /
//! `submitblock`), which this node does not yet expose. The trait is where that
//! implementation slots in; nothing else here changes.
//!
//! ## Usage
//!
//! ```text
//! miner --state ./chain-data --blocks 5 --bits 10 --threads 4
//! ```
//!
//! `--bits` sets the genesis difficulty as a count of required leading zero
//! bits. Each attempt costs a 32 MiB Argon2id pass (tens of milliseconds), so
//! expected time is roughly `2^bits × 30ms ÷ threads`. Values above ~16 are not
//! practical on one machine.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use custom_l1_node::consensus::difficulty::TARGET_BLOCK_TIME;
use custom_l1_node::consensus::{
    Chain, ChainConfig, InsertOutcome, PowMode, mine_header_with, suggested_threads,
};
use custom_l1_node::core::{Block, BlockHeader};
use custom_l1_node::crypto::dag::registry::CacheRegistry;
use custom_l1_node::crypto::pow::{leading_zero_bits, target_from_leading_zero_bits};
use custom_l1_node::state::StateDB;

/// Where the miner gets work and where it sends results.
trait CandidateSource {
    /// Returns a header to mine, already carrying the correct target.
    fn candidate(&self) -> Result<BlockHeader, Box<dyn Error>>;

    /// Submits a solved block, returning what the chain did with it.
    fn submit(&mut self, block: Block) -> Result<InsertOutcome, Box<dyn Error>>;

    /// Current chain height.
    fn height(&self) -> u64;

    /// The epoch caches to mine against.
    ///
    /// Comes from the same place the validator's does, so a solution is
    /// searched for against exactly the dataset the chain will check it with.
    fn dag(&self) -> &Arc<CacheRegistry>;
}

/// Candidate source backed by a chain owned by this process.
struct LocalSource {
    chain: Chain,
}

impl LocalSource {
    /// The genesis target doubles as this network's difficulty floor. The
    /// default mainnet floor would reject a low-difficulty demo chain outright.
    fn new(state: Arc<StateDB>, genesis: Block) -> Self {
        let config = ChainConfig::with_pow_limit(genesis.header.difficulty_target);
        Self {
            chain: Chain::new(state, genesis, config),
        }
    }
}

impl CandidateSource for LocalSource {
    fn candidate(&self) -> Result<BlockHeader, Box<dyn Error>> {
        Ok(self.chain.candidate_header(unix_now())?)
    }

    fn submit(&mut self, block: Block) -> Result<InsertOutcome, Box<dyn Error>> {
        Ok(self.chain.insert_block(block)?)
    }

    fn height(&self) -> u64 {
        self.chain.height()
    }

    fn dag(&self) -> &Arc<CacheRegistry> {
        self.chain.dag()
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parsed command line.
struct Args {
    state_path: String,
    blocks: u64,
    bits: u32,
    threads: usize,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            state_path: "./chain-data".to_string(),
            blocks: 1,
            bits: 8,
            threads: suggested_threads(),
        }
    }
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = Args::default();
    let mut argv = std::env::args().skip(1);

    while let Some(flag) = argv.next() {
        let mut value = || {
            argv.next()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--state" => args.state_path = value()?,
            "--blocks" => args.blocks = value()?.parse()?,
            "--bits" => args.bits = value()?.parse()?,
            "--threads" => args.threads = value()?.parse()?,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    if args.threads == 0 {
        return Err("--threads must be at least 1".into());
    }

    Ok(args)
}

fn print_usage() {
    println!(
        "ArgonBlake miner\n\n\
         USAGE:\n  \
         miner [OPTIONS]\n\n\
         OPTIONS:\n  \
         --state <PATH>     chain state directory (default ./chain-data)\n  \
         --blocks <N>       number of blocks to mine (default 1)\n  \
         --bits <N>         genesis difficulty in leading zero bits (default 8)\n  \
         --threads <N>      hashing threads (default: available parallelism, max 8)\n  \
         -h, --help         show this message"
    );
}

/// Builds the genesis block for a fresh chain at the requested difficulty.
fn genesis_block(bits: u32) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: unix_now(),
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(bits),
            tx_root: [0u8; 32],
        },
        Vec::new(),
    )
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;

    println!(
        "opening state at {} | difficulty {} bits | {} thread(s)",
        args.state_path, args.bits, args.threads
    );

    let state = Arc::new(StateDB::open(&args.state_path)?);
    let mut source = LocalSource::new(Arc::clone(&state), genesis_block(args.bits));

    // Never set here; the hook a signal handler would use to stop mining.
    let cancel = AtomicBool::new(false);

    for round in 1..=args.blocks {
        let header = source.candidate()?;
        let target_height = source.height() + 1;
        println!(
            "[{round}/{}] mining height {} against target {}",
            args.blocks,
            target_height,
            hex_prefix(&header.difficulty_target)
        );

        // Light mining: this binary is a reference and a demo, so it recomputes
        // pages from the 64 MiB cache rather than allocating a 4 GiB dataset on
        // whatever machine happens to run it. Roughly a hundredfold slower than
        // a real miner, which is the honest position for a CPU here anyway.
        let cache = if source.dag().is_active(target_height) {
            Some(source.dag().cache_for_height(target_height)?)
        } else {
            None
        };
        let mode = cache.as_deref().map_or(PowMode::Argon, PowMode::DagLight);

        let started = Instant::now();
        let solution = mine_header_with(&header, args.threads, &cancel, None, &mode)?;

        let Some(result) = solution else {
            println!("  no solution found (cancelled or exhausted)");
            break;
        };

        let elapsed = started.elapsed();
        let rate = result.attempts as f64 / elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
        println!(
            "  solved: nonce {} in {:.2}s | {} attempts | {:.1} H/s | {} leading zero bits",
            result.header.nonce,
            elapsed.as_secs_f64(),
            result.attempts,
            rate,
            leading_zero_bits(&result.hash)
        );

        // A real miner would drain the mempool here; this node has no path from
        // the gossip mempool into block assembly yet, so blocks are empty.
        let block = Block::new(result.header, Vec::new());
        match source.submit(block)? {
            InsertOutcome::Extended { tip } => {
                println!("  accepted, tip now {}", hex_prefix(&tip));
            }
            InsertOutcome::Reorganized {
                tip,
                reverted,
                applied,
            } => {
                println!(
                    "  accepted via reorg, tip {} ({} reverted, {} applied)",
                    hex_prefix(&tip),
                    reverted.len(),
                    applied.len()
                );
            }
            InsertOutcome::SideBranch { id } => {
                println!("  stored on a side branch: {}", hex_prefix(&id));
            }
            InsertOutcome::Duplicate { id } => {
                println!("  already known: {}", hex_prefix(&id));
            }
        }
    }

    println!(
        "final height {} | target block time {}s",
        source.height(),
        TARGET_BLOCK_TIME
    );
    Ok(())
}

fn hex_prefix(bytes: &[u8; 32]) -> String {
    hex::encode(&bytes[..8])
}
