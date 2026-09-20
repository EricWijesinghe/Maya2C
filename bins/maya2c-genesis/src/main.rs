//! Genesis file generator.
//!
//! Produces the `genesis.json` every node on a network must share. The output
//! is deterministic: the same inputs always yield the same file, the same
//! genesis block id, and the same state root, so operators can diff files
//! across regions to confirm the fleet agrees.
//!
//! ```text
//! genesis --chain-id l1-testnet-1 \
//!         --timestamp 1756252800 \
//!         --difficulty-bits 18 \
//!         --alloc <ADDR>:1000000 \
//!         --alloc <ADDR>:500000 \
//!         --out genesis.json
//! ```
//!
//! Timestamp defaults to now, but pin it explicitly for a real launch —
//! otherwise every operator who regenerates the file gets a different chain.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use custom_l1_node::genesis::{Allocation, GenesisConfig};

/// Default starting difficulty, in required leading zero bits.
///
/// Each attempt is a 32 MiB Argon2id pass at roughly 25 ms, so 18 bits is
/// about 262k attempts — a few minutes of a small fleet's combined hashrate,
/// which is a sane testnet starting point.
const DEFAULT_DIFFICULTY_BITS: u32 = 18;

struct Args {
    /// Path to an existing genesis file to recompute and report, rather than a
    /// new one to mint.
    verify: Option<PathBuf>,
    /// State root the verified file must produce.
    expect_state_root: Option<String>,
    chain_id: String,
    timestamp: Option<u64>,
    difficulty_bits: u32,
    pow_limit_bits: Option<u32>,
    allocations: Vec<Allocation>,
    out: PathBuf,
    force: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            verify: None,
            expect_state_root: None,
            chain_id: "l1-testnet-1".to_string(),
            timestamp: None,
            difficulty_bits: DEFAULT_DIFFICULTY_BITS,
            pow_limit_bits: None,
            allocations: Vec::new(),
            out: PathBuf::from("genesis.json"),
            force: false,
        }
    }
}

fn print_usage() {
    println!(
        "genesis — generate a genesis.json for an L1 network\n\n\
         USAGE:\n  \
         genesis [OPTIONS]                       mint a genesis file\n  \
         genesis --verify <PATH> [--expect-state-root <HEX>]\n\n\
         OPTIONS:\n  \
         --verify <PATH>          recompute an existing genesis file's state root and\n                           \
         block id and print them; mints nothing\n  \
         --expect-state-root <HEX>  with --verify, exit non-zero unless the recomputed\n                           \
         root matches. This is the check that tells an operator\n                           \
         whether they joined the network they meant to\n  \
         --chain-id <ID>          network identifier (default l1-testnet-1)\n  \
         --timestamp <UNIX>       genesis timestamp (default: now; pin for real launches)\n  \
         --difficulty-bits <N>    starting difficulty in leading zero bits (default {DEFAULT_DIFFICULTY_BITS})\n  \
         --pow-limit-bits <N>     difficulty floor (default: same as --difficulty-bits)\n  \
         --alloc <ADDR>:<AMOUNT>  premined balance; repeatable\n  \
         --out <PATH>             output path (default genesis.json)\n  \
         --force                  overwrite an existing file\n  \
         -h, --help               show this message"
    );
}

fn parse_allocation(spec: &str) -> Result<Allocation, Box<dyn Error>> {
    let (address, amount) = spec
        .rsplit_once(':')
        .ok_or_else(|| format!("--alloc expects <ADDR>:<AMOUNT>, got {spec:?}"))?;

    // Validate the address here so a typo fails at generation rather than at
    // every node's startup.
    custom_l1_node::genesis::decode_address(address)?;

    Ok(Allocation {
        address: address.trim_start_matches("0x").to_lowercase(),
        balance: amount
            .parse()
            .map_err(|e| format!("allocation amount {amount:?} is not a u64: {e}"))?,
    })
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
            "--verify" => args.verify = Some(PathBuf::from(value()?)),
            "--expect-state-root" => args.expect_state_root = Some(value()?),
            "--chain-id" => args.chain_id = value()?,
            "--timestamp" => args.timestamp = Some(value()?.parse()?),
            "--difficulty-bits" => args.difficulty_bits = value()?.parse()?,
            "--pow-limit-bits" => args.pow_limit_bits = Some(value()?.parse()?),
            "--alloc" => args.allocations.push(parse_allocation(&value()?)?),
            "--out" => args.out = PathBuf::from(value()?),
            "--force" => args.force = true,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(args)
}

fn main() {
    // Not `fn main() -> Result<..>`, for the reason `genesis-ceremony.rs` gives
    // at its own `main`: that path Debug-prints the error, so a multi-line
    // refusal arrives as one line of escaped newline characters. The
    // state-root mismatch below is several lines and is the message an
    // operator most needs to read.
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;

    if let Some(path) = &args.verify {
        return verify(path, args.expect_state_root.as_deref());
    }

    if args.out.exists() && !args.force {
        return Err(format!(
            "{} already exists — pass --force to overwrite. Replacing a genesis \
             file changes the chain identity and orphans every existing node.",
            args.out.display()
        )
        .into());
    }

    let timestamp = match args.timestamp {
        Some(value) => value,
        None => {
            eprintln!(
                "warning: no --timestamp given, using the current clock; \
                 regenerating this file later will produce a different chain"
            );
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }
    };

    let config = GenesisConfig {
        chain_id: args.chain_id,
        timestamp,
        difficulty_bits: args.difficulty_bits,
        pow_limit_bits: args.pow_limit_bits.unwrap_or(args.difficulty_bits),
        allocations: args.allocations,
        // Absent, and this tool offers no flag to populate it. An oracle
        // authority set is the chain's only trusted party, and picking one is a
        // governance decision written into the genesis file by hand — not
        // something a command-line default should be able to do quietly.
        oracle: None,
        // Likewise absent, and likewise no flag. A sealed-mempool committee is
        // a set of parties who can read the mempool a block before anyone
        // else, and its generation is a trusted setup. Neither belongs in a
        // command-line default.
        sealed: None,
        // Also absent, also no flag. Funding a DAO treasury is a launch
        // decision that needs root keys, a declared share, and a commitment
        // sheet operators can diff — see the `genesis-ceremony` binary. This
        // tool produces plain allocations.
        treasury: None,
    };

    // Fails loudly on a bad address, duplicate allocation, or a floor harder
    // than the starting difficulty.
    config.validate()?;

    let block = config.genesis_block()?;
    let json = config.to_json()?;

    if let Some(parent) = args.out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, &json)?;

    let total: u64 = config.allocations.iter().map(|a| a.balance).sum();
    println!("wrote {}", args.out.display());
    println!("  chain id:     {}", config.chain_id);
    println!("  timestamp:    {}", config.timestamp);
    println!(
        "  difficulty:   {} leading zero bits",
        config.difficulty_bits
    );
    println!(
        "  pow limit:    {} leading zero bits",
        config.pow_limit_bits
    );
    println!(
        "  allocations:  {} ({total} total)",
        config.allocations.len()
    );
    println!("  state root:   {}", hex::encode(config.state_root()?));
    println!("  genesis id:   {}", hex::encode(block.header.id()));
    println!("\nevery node on this network must use a byte-identical genesis.json");

    Ok(())
}

/// Recomputes an existing genesis file's commitments and reports them.
///
/// # Why this exists
///
/// A node that fetched the wrong `genesis.json` does not error. It starts,
/// syncs nothing, and looks like a node that is merely slow to find peers --
/// so the check that catches it has to happen before the node runs, and it has
/// to be a check somebody can automate.
///
/// `infra/deploy/deploy_bootstrap.sh` and `infra/ansible/roles/maya_node` both call
/// this. The bash script previously degraded to "could not compute the state
/// root locally ... verify it by hand", which made the most important step of
/// a bootstrap a no-op whenever nobody did it by hand.
///
/// # Errors
///
/// A file that cannot be read, cannot be parsed, or fails
/// [`GenesisConfig::validate`]. With `expect_state_root`, also a mismatch --
/// which is the point of passing it.
fn verify(path: &Path, expect_state_root: Option<&str>) -> Result<(), Box<dyn Error>> {
    let json = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config = GenesisConfig::from_json(&json).map_err(|e| format!("{}: {e}", path.display()))?;

    // Validated, not merely parsed. A file whose allocations overflow the
    // supply parses fine and produces a state root nobody else will agree with.
    config.validate()?;

    let state_root = hex::encode(config.state_root()?);
    let block_id = hex::encode(config.genesis_block()?.header.id());

    println!("chain id       {}", config.chain_id);
    println!("timestamp      {}", config.timestamp);
    println!("total supply   {}", config.total_supply()?);
    println!("state root     {state_root}");
    println!("genesis block  {block_id}");

    if let Some(expected) = expect_state_root {
        // Case-insensitive, because a root copied out of COMMITMENT.txt and
        // one pasted from a terminal that upper-cased it are the same root, and
        // failing on that would train operators to skip the check.
        if !expected.eq_ignore_ascii_case(&state_root) {
            return Err(format!(
                "state root mismatch.\n  expected  {expected}\n  actual    {state_root}\n\n\
                 This genesis file is not the one the ceremony produced. A fleet\n\
                 split across two genesis files does not report an error -- it\n\
                 looks like a network that is slow to converge."
            )
            .into());
        }
        println!("\nstate root matches the expected commitment.");
    }

    Ok(())
}
