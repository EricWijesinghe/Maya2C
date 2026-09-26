//! Genesis ceremony: root keys, treasury allocation, and a locked state root.
//!
//! Where [`genesis`](../genesis/index.html) turns a set of allocations into a
//! `genesis.json`, this drives the whole launch step: it generates the root
//! signing keys, funds the DAO treasury, writes the genesis file, and prints a
//! commitment sheet that every operator can diff.
//!
//! ```text
//! genesis-ceremony --chain-id maya-genesis-rc1 \
//!                  --timestamp 1767225600 \
//!                  --difficulty-bits 22 \
//!                  --roots 3 \
//!                  --treasury-share-bps 2000 \
//!                  --supply 21000000000 \
//!                  --out-dir ./ceremony
//! ```
//!
//! # This will not produce a mainnet genesis, and that is deliberate
//!
//! `docs/mainnet-readiness.md §1`: the shielded pool's circuit has had no
//! independent audit. One missing constraint in the joinsplit AIR lets anyone
//! mint shielded value that no supply audit would reveal.
//!
//! Three other places already refuse a value-bearing chain id — the node at
//! startup, both terraform module sets, and a test pinning
//! `CIRCUIT_IS_AUDITED = false`. This is the fourth, and it refuses for the same
//! reason: a genesis file is the one artefact that cannot be revised after the
//! fact, so the tool that mints it is the wrong place to be permissive.
//!
//! Lifting the block needs an independent audit of the joinsplit AIR, its
//! findings fixed, and only then the flag flipped. That is calendar time and
//! external reviewers. It is not a flag on this binary.
//!
//! # Secret material never reaches stdout
//!
//! Root secret keys are written to files, one per root, and the terminal sees
//! only their public halves and a commitment digest. A key echoed to a
//! terminal is a key in a scrollback buffer, a terminal multiplexer's capture
//! file, and whatever CI collected the job log — and a root key that leaked at
//! generation time cannot be rotated, because it is the thing the chain's
//! identity is built on.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::crypto::hybrid;
use custom_l1_node::genesis::{
    Allocation, BPS_DENOMINATOR, GenesisConfig, MAX_TREASURY_SHARE_BPS, TreasuryGenesis,
};

/// Chain ids this binary refuses to mint a genesis for.
///
/// The same list `bins/maya2c-node/src/main.rs` refuses to start on. Duplicated rather than
/// shared because the two crates answer different questions — "may I run?" and
/// "may I create?" — and a single list would invite someone relaxing one to
/// relax both.
const VALUE_BEARING_CHAINS: &[&str] = &["maya-mainnet", "mainnet"];

/// Default number of root keys.
///
/// Three, not one. A single root key is a single machine, a single operator,
/// and a single compromise; three is the smallest number where losing one is
/// survivable and the ceremony has witnesses.
const DEFAULT_ROOTS: usize = 3;

/// Largest number of root keys the ceremony will generate.
///
/// Each root is a signing key somebody has to physically protect, and a
/// ceremony that produces more custody obligations than it has custodians has
/// produced liabilities rather than security.
const MAX_ROOTS: usize = 16;

/// Which of the three ceremony modes is being run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Everything on one host. Fine for a testnet, not for a launch.
    SingleParty,
    /// One participant generating their own root key.
    Contribute,
    /// The coordinator, over collected public halves only.
    Assemble,
}

struct Args {
    mode: Mode,
    label: String,
    contributions: PathBuf,
    treasury_public: Option<PathBuf>,
    chain_id: String,
    timestamp: Option<u64>,
    difficulty_bits: u32,
    pow_limit_bits: Option<u32>,
    roots: usize,
    supply: u64,
    treasury_share_bps: u16,
    out_dir: PathBuf,
    force: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            mode: Mode::SingleParty,
            label: String::new(),
            contributions: PathBuf::from("./contributions"),
            treasury_public: None,
            chain_id: String::new(),
            timestamp: None,
            difficulty_bits: 22,
            pow_limit_bits: None,
            roots: DEFAULT_ROOTS,
            supply: 0,
            treasury_share_bps: 0,
            out_dir: PathBuf::from("./ceremony"),
            force: false,
        }
    }
}

fn print_usage() {
    println!(
        "genesis-ceremony — mint a launch genesis\n\n\
         USAGE:\n  \
         genesis-ceremony contribute --label <NAME> [--out-dir <PATH>]\n  \
         genesis-ceremony assemble --chain-id <ID> --supply <UNITS> \
--contributions <DIR> [OPTIONS]\n  \
         genesis-ceremony --chain-id <ID> --supply <UNITS> [OPTIONS]   (single-party)\n\n\
         MODES:\n  \
         contribute   one participant, on their own machine: generates a root key,\n               \
         keeps the secret locally, emits only the public half\n  \
         assemble     the coordinator, over collected public halves. Handles no\n               \
         secret key at any point\n  \
         (neither)    everything on one host. Fine for a testnet; for a launch the\n               \
         two-step form distributes custody instead of concentrating it\n\n\
         OPTIONS:\n  \
         --label <NAME>               contribute: participant label, [A-Za-z0-9_-]\n  \
         --contributions <DIR>        assemble: directory of collected *.public files\n  \
         --treasury-public <PATH>     assemble: contributed public key for the treasury,\n                               \
         required when --treasury-share-bps is non-zero\n
         OPTIONS:\n  \
         --chain-id <ID>              network identifier; value-bearing ids are refused\n  \
         --supply <UNITS>             total genesis supply, treasury included\n  \
         --timestamp <UNIX>           genesis timestamp; pin it for a real launch\n  \
         --difficulty-bits <N>        starting difficulty (default 22)\n  \
         --pow-limit-bits <N>         difficulty floor (default: same as starting)\n  \
         --roots <N>                  root keys to generate (default 3, max 16)\n  \
         --treasury-share-bps <BPS>   DAO treasury share, max {MAX_TREASURY_SHARE_BPS}\n  \
         --out-dir <PATH>             output directory (default ./ceremony)\n  \
         --force                      overwrite an existing output directory\n  \
         -h, --help                   show this message"
    );
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = Args::default();
    let mut argv = std::env::args().skip(1).peekable();

    // A leading bare word selects the mode. Checked before the flag loop so
    // that `contribute` is a subcommand rather than a flag value.
    match argv.peek().map(String::as_str) {
        Some("contribute") => {
            args.mode = Mode::Contribute;
            argv.next();
        }
        Some("assemble") => {
            args.mode = Mode::Assemble;
            argv.next();
        }
        _ => {}
    }

    while let Some(flag) = argv.next() {
        let mut value = || -> Result<String, Box<dyn Error>> {
            argv.next()
                .ok_or_else(|| format!("{flag} requires a value").into())
        };
        match flag.as_str() {
            "--label" => args.label = value()?,
            "--contributions" => args.contributions = PathBuf::from(value()?),
            "--treasury-public" => args.treasury_public = Some(PathBuf::from(value()?)),
            "--chain-id" => args.chain_id = value()?,
            "--supply" => args.supply = value()?.parse()?,
            "--timestamp" => args.timestamp = Some(value()?.parse()?),
            "--difficulty-bits" => args.difficulty_bits = value()?.parse()?,
            "--pow-limit-bits" => args.pow_limit_bits = Some(value()?.parse()?),
            "--roots" => args.roots = value()?.parse()?,
            "--treasury-share-bps" => args.treasury_share_bps = value()?.parse()?,
            "--out-dir" => args.out_dir = PathBuf::from(value()?),
            "--force" => args.force = true,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag {other}").into()),
        }
    }

    // `contribute` mints nothing, so it has neither a chain to name nor a
    // supply to divide. A root key is chain-agnostic until an assemble step
    // allocates to it — which is what lets a participant contribute before the
    // launch parameters are settled.
    if args.mode == Mode::Contribute {
        if args.label.is_empty() {
            return Err("--label is required for contribute".into());
        }
        return Ok(args);
    }

    if args.chain_id.is_empty() {
        return Err("--chain-id is required".into());
    }
    if args.supply == 0 {
        return Err("--supply is required and must be non-zero".into());
    }
    Ok(args)
}

/// Refuses a value-bearing chain id.
///
/// # Errors
///
/// Returns the reason, naming the document that explains the block, so an
/// operator who hits this is pointed at the ceremony they actually need rather
/// than at this binary's source.
fn refuse_value_bearing(chain_id: &str) -> Result<(), Box<dyn Error>> {
    if VALUE_BEARING_CHAINS.contains(&chain_id) {
        return Err(format!(
            "refusing to mint a genesis for value-bearing chain id '{chain_id}'.\n\
             \n\
             The shielded pool's circuit has had no independent audit: one missing\n\
             constraint lets anyone mint shielded value that no supply audit reveals.\n\
             See docs/mainnet-readiness.md section 1.\n\
             \n\
             Three other guards refuse the same ids — the node at startup, both\n\
             terraform module sets, and a test pinning CIRCUIT_IS_AUDITED = false.\n\
             A genesis file cannot be revised after the fact, so this one refuses\n\
             too.\n\
             \n\
             Use a non-value-bearing id such as 'maya-genesis-rc1'."
        )
        .into());
    }
    Ok(())
}

/// Splits `supply` into a treasury balance and the remainder.
///
/// # Errors
///
/// Returns a message if the split does not land exactly on `share_bps`.
/// Rejecting rather than rounding: a treasury off by a rounding step is a
/// treasury whose declared share is a lie, and the file is immutable.
fn split_supply(supply: u64, share_bps: u16) -> Result<(u64, u64), Box<dyn Error>> {
    if share_bps > MAX_TREASURY_SHARE_BPS {
        return Err(format!(
            "treasury share of {share_bps} bps exceeds the {MAX_TREASURY_SHARE_BPS} bps ceiling"
        )
        .into());
    }

    let treasury = (u128::from(supply) * u128::from(share_bps)) / u128::from(BPS_DENOMINATOR);
    let treasury = u64::try_from(treasury).map_err(|_| "treasury balance overflows u64")?;

    let implied = (u128::from(treasury) * u128::from(BPS_DENOMINATOR)) / u128::from(supply);
    if implied != u128::from(share_bps) {
        return Err(format!(
            "a supply of {supply} cannot be split exactly at {share_bps} bps \
             (nearest is {implied} bps); choose a supply divisible by \
             {BPS_DENOMINATOR}"
        )
        .into());
    }

    let remainder = supply
        .checked_sub(treasury)
        .ok_or("treasury exceeds total supply")?;
    Ok((treasury, remainder))
}

/// Writes `bytes` to `path` with an owner-only mode where the platform has one.
fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    fs::write(path, bytes)?;

    // Unix only. Windows inherits the directory ACL, which is why the ceremony
    // directory itself is created fresh rather than reused — an existing
    // directory may already be readable by someone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Stack for the worker thread `main` hands off to.
///
/// Windows gives the main thread 1 MiB. SLH-DSA-SHA2-128s key generation builds
/// a hypertree whose working set does not comfortably fit in that, and the
/// symptom is not an error — it is `thread 'main' has overflowed its stack`,
/// after which the process is gone and no genesis was written.
///
/// The single-party path happened to survive on 1 MiB and `contribute` did not,
/// which is the useful detail: the margin was already gone and nobody knew,
/// so the next edit to either path could have taken it. 16 MiB is far past
/// what either needs and costs nothing on a tool that runs once.
const CEREMONY_STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() {
    // Not `fn main() -> Result<..>`: that path Debug-prints the error, which
    // renders a multi-line refusal as a single line of escaped newlines. The
    // refusal here is something an operator has to actually read.
    let worker = std::thread::Builder::new()
        .name("ceremony".to_string())
        .stack_size(CEREMONY_STACK_BYTES)
        // Rendered to a String inside the thread: `Box<dyn Error>` is not
        // `Send`, and the message is the only part of it this binary ever
        // uses.
        .spawn(|| run().map_err(|error| error.to_string()))
        .expect("spawning the ceremony thread");

    match worker.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
        // The thread panicked. Its message has already gone to stderr; this
        // only makes the exit status say so, because a ceremony that failed
        // must not look like one that succeeded.
        Err(_) => std::process::exit(1),
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;

    // `contribute` mints no genesis, so it has no chain id to refuse and needs
    // none: a root key is chain-agnostic until an assemble step allocates to
    // it. The refusal belongs on the two paths that write a genesis file.
    if args.mode == Mode::Contribute {
        return contribute(&args.label, &args.out_dir, args.force);
    }

    refuse_value_bearing(&args.chain_id)?;

    if args.mode == Mode::Assemble {
        return assemble(&args);
    }

    if args.roots == 0 || args.roots > MAX_ROOTS {
        return Err(format!("--roots must be between 1 and {MAX_ROOTS}").into());
    }

    if args.out_dir.exists() && !args.force {
        return Err(format!(
            "{} already exists; refusing to overwrite a ceremony directory without --force",
            args.out_dir.display()
        )
        .into());
    }
    fs::create_dir_all(&args.out_dir)?;

    let (treasury_balance, distributable) = split_supply(args.supply, args.treasury_share_bps)?;

    // --- root keys ---------------------------------------------------------
    //
    // Each root gets an equal share of what the treasury did not take. Equal,
    // not weighted: an unequal split at genesis is a governance decision, and
    // a ceremony binary is the wrong place to encode one silently.
    let per_root = distributable / args.roots as u64;
    let dust = distributable - per_root * args.roots as u64;

    let mut allocations = Vec::with_capacity(args.roots);
    let mut public_lines = Vec::with_capacity(args.roots);

    for index in 0..args.roots {
        let key = hybrid::generate_signing_key()?;
        let address = hex::encode(key.address());

        // The secret half goes to a file and nowhere else.
        let secret_path = args.out_dir.join(format!("root-{index}.secret"));
        write_secret(&secret_path, key.to_bytes().as_slice())?;

        let mut public = Vec::new();
        key.public_key().encode_into(&mut public);
        fs::write(
            args.out_dir.join(format!("root-{index}.public")),
            hex::encode(&public),
        )?;

        // The first root absorbs the remainder, so the allocations sum to the
        // supply exactly. Handing dust to whichever root sorts first would
        // make the distribution depend on key generation.
        let balance = if index == 0 {
            per_root + dust
        } else {
            per_root
        };

        public_lines.push(format!("  root-{index}  {address}  {balance}"));
        allocations.push(Allocation {
            address: address.clone(),
            balance,
        });
    }

    // --- treasury ----------------------------------------------------------
    let treasury = if args.treasury_share_bps > 0 {
        let key = hybrid::generate_signing_key()?;
        let address = hex::encode(key.address());
        write_secret(
            &args.out_dir.join("treasury.secret"),
            key.to_bytes().as_slice(),
        )?;
        let mut public = Vec::new();
        key.public_key().encode_into(&mut public);
        fs::write(args.out_dir.join("treasury.public"), hex::encode(&public))?;

        Some(TreasuryGenesis {
            address,
            balance: treasury_balance,
            share_bps: args.treasury_share_bps,
        })
    } else {
        None
    };

    // --- genesis -----------------------------------------------------------
    let config = GenesisConfig {
        chain_id: args.chain_id.clone(),
        timestamp: args.timestamp.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is before 1970")
                .as_secs()
        }),
        difficulty_bits: args.difficulty_bits,
        pow_limit_bits: args.pow_limit_bits.unwrap_or(args.difficulty_bits),
        allocations,
        oracle: None,
        sealed: None,
        treasury,
        protocol_upgrades: Vec::new(),
    };

    config.validate()?;

    let state_root = config.state_root()?;
    let block = config.genesis_block()?;
    let total = config.total_supply()?;

    let genesis_path = args.out_dir.join("genesis.json");
    fs::write(&genesis_path, config.to_json()?)?;

    // --- commitment sheet --------------------------------------------------
    //
    // Everything an operator needs to confirm their copy matches, and nothing
    // that would compromise the chain if it were pasted into a ticket.
    let sheet = format!(
        "Maya2C genesis ceremony\n\
         =======================\n\
         \n\
         chain id       {}\n\
         timestamp      {}\n\
         difficulty     {} bits (floor {} bits)\n\
         roots          {}\n\
         total supply   {total}\n\
         treasury       {} units ({} bps)\n\
         \n\
         state root     {}\n\
         genesis block  {}\n\
         \n\
         Root addresses and balances:\n{}\n\
         \n\
         Every operator must see identical values above. A differing state root\n\
         or genesis block id means the fleet is not on one network.\n\
         \n\
         Secret keys are in this directory as *.secret and were never printed.\n\
         Move them to their custodians and remove them from this host.\n",
        config.chain_id,
        config.timestamp,
        config.difficulty_bits,
        config.pow_limit_bits,
        args.roots,
        treasury_balance,
        args.treasury_share_bps,
        hex::encode(state_root),
        hex::encode(block.header.id()),
        public_lines.join("\n"),
    );

    fs::write(args.out_dir.join("COMMITMENT.txt"), &sheet)?;
    print!("{sheet}");
    println!("Wrote {}", genesis_path.display());

    Ok(())
}

// ===========================================================================
// Multi-party ceremony
// ===========================================================================
//
// # What "multi-party" fixes
//
// The single-party path below calls `generate_signing_key()` in a loop on one
// host, so at one moment every root secret for the chain exists on one machine.
// That machine is then the whole of the chain's key custody, and whoever
// controls it controls every root. The ceremony has witnesses but no
// distribution of trust.
//
// The two subcommands here split it:
//
//   contribute  — run by each participant, on their own machine. Generates
//                 their key, keeps the secret locally, emits only the public
//                 half.
//   assemble    — run by the coordinator over the collected public halves.
//                 Never opens a secret, because it is never given one.
//
// The coordinator ends up holding no key material at all. That is the property
// worth having, and it is what makes the resulting genesis something a
// participant can verify rather than trust.

/// A participant's published contribution.
///
/// The file name carries the label so a coordinator collecting a directory of
/// them can tell whose is whose without opening any.
const PUBLIC_SUFFIX: &str = ".public";

/// Longest participant label.
///
/// It becomes a file name and a line on the commitment sheet everyone diffs.
const MAX_LABEL: usize = 32;

/// Generates one participant's key pair.
///
/// The secret is written to a file with an owner-only mode and is never
/// printed; the public half and the address go to stdout so the participant can
/// read them back to the coordinator over any channel, including a phone call.
fn contribute(label: &str, out_dir: &Path, force: bool) -> Result<(), Box<dyn Error>> {
    check_label(label)?;

    if out_dir.exists() && !force {
        return Err(format!(
            "{} already exists; refusing to overwrite a contribution without --force",
            out_dir.display()
        )
        .into());
    }
    fs::create_dir_all(out_dir)?;

    let key = hybrid::generate_signing_key()?;
    let address = hex::encode(key.address());

    let secret_path = out_dir.join(format!("{label}.secret"));
    write_secret(&secret_path, key.to_bytes().as_slice())?;

    let mut public = Vec::new();
    key.public_key().encode_into(&mut public);
    let public_hex = hex::encode(&public);
    let public_path = out_dir.join(format!("{label}{PUBLIC_SUFFIX}"));
    fs::write(&public_path, &public_hex)?;

    // A digest of the public key, not of the key itself: short enough to read
    // aloud, and it is what the coordinator's commitment sheet will repeat, so
    // a participant can confirm their contribution went in unaltered without
    // comparing 1,984 bytes of hex.
    let digest = blake3::hash(&public);

    println!(
        "Contribution written\n\
         ====================\n\
         \n\
         label          {label}\n\
         address        {address}\n\
         commitment     {}\n\
         \n\
         Send ONLY {} to the coordinator.\n\
         \n\
         {} is your root secret key. It was not printed and must not be.\n\
         Nothing can reissue it: a root key is what the chain's identity is\n\
         built on, so a lost one cannot be rotated and a leaked one cannot be\n\
         revoked.\n",
        &digest.to_hex()[..32],
        public_path.display(),
        secret_path.display(),
    );

    Ok(())
}

/// One participant's contribution, as the coordinator sees it.
struct Contribution {
    label: String,
    address: String,
    /// First 32 hex characters of the BLAKE3 digest of the encoded public key.
    ///
    /// Repeated on the commitment sheet so each participant can confirm the
    /// value they were shown by their own `contribute` run.
    commitment: String,
}

/// Reads every `*.public` in `dir`, rejecting anything that is not one.
///
/// # Why it refuses a directory containing secrets
///
/// A coordinator who has been sent a `.secret` file has a problem that is not
/// solved by ignoring the file: the key is already somewhere it should not be,
/// and continuing would produce a genesis whose custody story is quietly
/// false. Better to stop and make somebody rotate that root before launch,
/// while rotating is still free.
fn collect_contributions(dir: &Path) -> Result<Vec<Contribution>, Box<dyn Error>> {
    let mut found = Vec::new();

    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if name.ends_with(".secret") {
            return Err(format!(
                "{} contains {name}.\n\
                 \n\
                 A coordinator must never hold a participant's secret key, and a\n\
                 secret that reached this directory has already left the machine\n\
                 that generated it. Rotate that root -- run `contribute` again on\n\
                 the participant's own host -- and assemble from public halves\n\
                 only.",
                dir.display()
            )
            .into());
        }

        let Some(label) = name.strip_suffix(PUBLIC_SUFFIX) else {
            continue;
        };
        check_label(label)?;

        let hex_text = fs::read_to_string(&path)?;
        let bytes = hex::decode(hex_text.trim())
            .map_err(|e| format!("{}: not hex: {e}", path.display()))?;

        // Decoded rather than trusted. A truncated or hand-edited public key
        // that still parsed as hex would otherwise become an address nobody
        // holds the key for, and an allocation to it is supply burned at
        // genesis with no way to tell.
        let mut reader = ByteReader::new(&bytes);
        let public = hybrid::HybridPublicKey::decode(&mut reader)
            .map_err(|e| format!("{}: not a hybrid public key: {e}", path.display()))?;

        found.push(Contribution {
            label: label.to_string(),
            address: hex::encode(public.address()),
            commitment: blake3::hash(&bytes).to_hex()[..32].to_string(),
        });
    }

    if found.is_empty() {
        return Err(format!(
            "no {PUBLIC_SUFFIX} files in {}; each participant runs `contribute` and sends theirs",
            dir.display()
        )
        .into());
    }

    // Sorted by label, so the allocation order — and therefore which root
    // absorbs the rounding dust — depends on the participants rather than on
    // the order the filesystem happened to return.
    found.sort_by(|a, b| a.label.cmp(&b.label));

    // Two participants who generated the same address is either a collision,
    // which does not happen, or the same contribution submitted twice, which
    // does. Silently merging them would halve one participant's allocation.
    for pair in found.windows(2) {
        if pair[0].address == pair[1].address {
            return Err(format!(
                "{} and {} are the same address; a contribution was submitted twice",
                pair[0].label, pair[1].label
            )
            .into());
        }
    }

    Ok(found)
}

fn check_label(label: &str) -> Result<(), Box<dyn Error>> {
    let ok = (1..=MAX_LABEL).contains(&label.len())
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(
            format!("label must be 1-{MAX_LABEL} characters of [A-Za-z0-9_-]; got {label:?}")
                .into(),
        )
    }
}

/// Builds the genesis from collected public halves.
///
/// # It handles no secret key
///
/// That is the whole point of the two-step form, and it is worth being able to
/// state plainly: nothing this function opens, reads, or writes is secret
/// material. The treasury is a contribution like any other rather than a key
/// minted here, because a coordinator who generates the treasury key is a
/// coordinator holding a key — which is the thing the split was for.
fn assemble(args: &Args) -> Result<(), Box<dyn Error>> {
    let contributions = collect_contributions(&args.contributions)?;

    if contributions.len() > MAX_ROOTS {
        return Err(format!(
            "{} contributions, more than the {MAX_ROOTS} this ceremony accepts. \
             Each root is a custody obligation somebody has to meet.",
            contributions.len()
        )
        .into());
    }

    if args.out_dir.exists() && !args.force {
        return Err(format!(
            "{} already exists; refusing to overwrite a ceremony directory without --force",
            args.out_dir.display()
        )
        .into());
    }
    fs::create_dir_all(&args.out_dir)?;

    let (treasury_balance, distributable) = split_supply(args.supply, args.treasury_share_bps)?;

    // Equal shares, as in the single-party path: an unequal split is a
    // governance decision and a ceremony binary is the wrong place to encode
    // one silently.
    let count = contributions.len() as u64;
    let per_root = distributable / count;
    let dust = distributable - per_root * count;

    let mut allocations = Vec::with_capacity(contributions.len());
    let mut public_lines = Vec::with_capacity(contributions.len());

    for (index, contribution) in contributions.iter().enumerate() {
        // The first by label absorbs the remainder. Sorted by label rather
        // than by address, so which participant it is was decided by their
        // name and not by the bits their key happened to hash to.
        let balance = if index == 0 {
            per_root + dust
        } else {
            per_root
        };

        public_lines.push(format!(
            "  {:<16} {}  {balance}\n    commitment     {}",
            contribution.label, contribution.address, contribution.commitment
        ));
        allocations.push(Allocation {
            address: contribution.address.clone(),
            balance,
        });
    }

    // --- treasury ----------------------------------------------------------
    let treasury = if args.treasury_share_bps > 0 {
        let path = args.treasury_public.as_ref().ok_or(
            "--treasury-share-bps is non-zero, so --treasury-public is required.\n\
             \n\
             The treasury key is a contribution like any other: somebody runs\n\
             `contribute` for it and sends the public half. Minting it here would\n\
             put a key on the coordinator's host, which is what assembling from\n\
             public halves exists to avoid.",
        )?;

        let bytes = hex::decode(fs::read_to_string(path)?.trim())
            .map_err(|e| format!("{}: not hex: {e}", path.display()))?;
        let mut reader = ByteReader::new(&bytes);
        let public = hybrid::HybridPublicKey::decode(&mut reader)
            .map_err(|e| format!("{}: not a hybrid public key: {e}", path.display()))?;
        let address = hex::encode(public.address());

        // A treasury sharing an address with a root would concentrate two
        // allocations on one key while the sheet showed them as separate
        // parties.
        if allocations.iter().any(|a| a.address == address) {
            return Err(
                "the treasury public key is also a root contribution; they must be \
                 different keys held by different people"
                    .into(),
            );
        }

        Some(TreasuryGenesis {
            address,
            balance: treasury_balance,
            share_bps: args.treasury_share_bps,
        })
    } else {
        None
    };

    let config = GenesisConfig {
        chain_id: args.chain_id.clone(),
        timestamp: args.timestamp.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is before 1970")
                .as_secs()
        }),
        difficulty_bits: args.difficulty_bits,
        pow_limit_bits: args.pow_limit_bits.unwrap_or(args.difficulty_bits),
        allocations,
        oracle: None,
        sealed: None,
        treasury,
        protocol_upgrades: Vec::new(),
    };

    config.validate()?;

    let state_root = config.state_root()?;
    let block = config.genesis_block()?;
    let total = config.total_supply()?;

    let genesis_path = args.out_dir.join("genesis.json");
    fs::write(&genesis_path, config.to_json()?)?;

    let sheet = format!(
        "Maya2C genesis ceremony (multi-party)\n\
         =====================================\n\
         \n\
         chain id       {}\n\
         timestamp      {}\n\
         difficulty     {} bits (floor {} bits)\n\
         contributions  {}\n\
         total supply   {total}\n\
         treasury       {} units ({} bps)\n\
         \n\
         state root     {}\n\
         genesis block  {}\n\
         \n\
         Contributions:\n{}\n\
         \n\
         Every operator must see identical values above. A differing state root\n\
         or genesis block id means the fleet is not on one network.\n\
         \n\
         Each participant should confirm the commitment beside their label\n\
         matches what their own `contribute` run printed. A mismatch means the\n\
         public half that reached the coordinator is not the one they sent.\n\
         \n\
         No secret key was read, written, or held by this step.\n",
        config.chain_id,
        config.timestamp,
        config.difficulty_bits,
        config.pow_limit_bits,
        contributions.len(),
        treasury_balance,
        args.treasury_share_bps,
        hex::encode(state_root),
        hex::encode(block.header.id()),
        public_lines.join("\n"),
    );

    fs::write(args.out_dir.join("COMMITMENT.txt"), &sheet)?;
    print!("{sheet}");
    println!("Wrote {}", genesis_path.display());

    Ok(())
}
