//! `l1-wallet` — command line wallet for the custom L1 node.
//!
//! ```text
//! l1-wallet generate                          create and encrypt a new key
//! l1-wallet address                           print the address of a keystore
//! l1-wallet balance                           query account state over RPC
//! l1-wallet send --to <ADDR> --amount <VAL>   confirm, then sign and broadcast
//!               [--genesis ID] [--max-fee F]  pin the chain, cap the fee
//!               [--yes] [--no-broadcast]      script mode, or only print the signed hex
//! ```
//!
//! ## Password handling
//!
//! There is deliberately **no `--password` flag**. A password on the command
//! line lands in shell history and is visible to every user on the machine via
//! the process list. Instead the wallet prompts on a terminal, and falls back
//! to `L1_WALLET_PASSWORD` for scripted use — which is still imperfect, but an
//! environment variable is not persisted to disk by default and is not visible
//! in `ps` output on modern systems.

use l1_wallet::guard::{self, GenesisTrust};
use l1_wallet::keystore;
use primitive_types::U256;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use custom_l1_node::core::council_payload::CouncilAction;
use custom_l1_node::core::staking_payload::StakingAction;
use custom_l1_node::core::{ChainTag, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::rpc::ChainInfo;

use l1_wallet::client::NodeClient;

/// Environment variable consulted when no terminal is available.
const PASSWORD_ENV: &str = "L1_WALLET_PASSWORD";

/// Command line wallet for the custom L1 chain.
#[derive(Parser, Debug)]
#[command(name = "l1-wallet", version, about, long_about = None)]
struct Cli {
    /// Path to the encrypted keystore.
    #[arg(
        long,
        global = true,
        default_value = "wallet.key",
        env = "L1_WALLET_KEYSTORE"
    )]
    keystore: PathBuf,

    /// Node JSON-RPC endpoint.
    #[arg(
        long,
        global = true,
        default_value = "http://127.0.0.1:8545",
        env = "L1_RPC_URL"
    )]
    rpc_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate a new hybrid ML-DSA-65 + SLH-DSA keypair and save it encrypted.
    Generate,

    /// Print the address held in the keystore.
    Address,

    /// Query the account balance over RPC.
    Balance {
        /// Address to query. Defaults to the keystore's own address.
        #[arg(long)]
        address: Option<String>,
    },

    /// Show a block from the active chain.
    Block {
        /// Height to fetch. Genesis is height 0.
        #[arg(long)]
        height: u64,
    },

    /// Sign and broadcast a transfer.
    Send {
        /// Recipient address, 32 bytes hex encoded.
        #[arg(long)]
        to: String,

        /// Amount to transfer, in base units (decimal, up to 2^256 - 1).
        #[arg(long, value_parser = parse_amount)]
        amount: U256,

        #[command(flatten)]
        tx: TxOptions,
    },

    /// Register a validator key and bond to it from this wallet (ADR-028).
    /// The committee is recomputed from stake at each epoch boundary.
    RegisterValidator {
        /// Validator key file written by `maya2c-node --generate-validator-key`.
        #[arg(long)]
        validator_key: PathBuf,

        /// Self bond, in base units (decimal, up to 2^256 - 1).
        #[arg(long, value_parser = parse_amount)]
        bond: U256,

        /// Commission on delegators' rewards, in basis points (500 = 5 %).
        #[arg(long, default_value_t = 0)]
        commission_bps: u16,

        #[command(flatten)]
        tx: TxOptions,
    },

    /// Delegate stake from this wallet to a registered validator.
    Delegate {
        /// Validator id, 64 hex characters (`register-validator` prints it).
        #[arg(long)]
        validator: String,

        /// Amount to delegate, in base units (decimal, up to 2^256 - 1).
        #[arg(long, value_parser = parse_amount)]
        amount: U256,

        #[command(flatten)]
        tx: TxOptions,
    },

    /// File double-sign evidence a validator saved under its data dir
    /// (`evidence/equivocation-*.hex`). Anyone may file it; the chain checks
    /// both signatures and slashes the offender.
    ReportEquivocation {
        /// The evidence file.
        #[arg(long)]
        evidence: PathBuf,

        #[command(flatten)]
        tx: TxOptions,
    },

    /// Carry a security-council action (ADR-050) from this wallet. The
    /// authority is the approvals inside it, never this wallet: the file is
    /// what `genesis-ceremony council combine` wrote.
    CouncilAction {
        /// Hex file from `genesis-ceremony council combine`.
        #[arg(long)]
        action: PathBuf,

        #[command(flatten)]
        tx: TxOptions,
    },
}

/// Nonce, fee, confirmation and broadcast choices shared by every signed transaction.
#[derive(clap::Args, Debug)]
struct TxOptions {
    /// Sender nonce. Fetched from the node when omitted.
    #[arg(long)]
    nonce: Option<u64>,

    /// Fee paid to the collector. Defaults to twice the base fee on the
    /// transaction's size, the excess being the validators' tip, where
    /// the chain charges fees; nothing where it does not.
    #[arg(long, value_parser = parse_amount)]
    fee: Option<U256>,

    /// Largest fee to pay, in base units. Without it the cap is
    /// `guard::DEFAULT_MAX_FEE` (one MAYA), for every kind of transaction.
    #[arg(long, value_parser = parse_amount)]
    max_fee: Option<U256>,

    /// The genesis id (hex) to sign for. Required for a network this build does
    /// not pin; it must equal the id the node reports.
    #[arg(long)]
    genesis: Option<String>,

    /// Sign without the typed confirmation, for scripts. The description is still printed.
    #[arg(long)]
    yes: bool,

    /// Sign and print the raw transaction hex instead of broadcasting it,
    /// for a different client (an SDK, a gateway) to submit.
    #[arg(long)]
    no_broadcast: bool,
}

/// Reads a password, preferring an interactive prompt.
fn read_password(confirm: bool) -> Result<String> {
    if let Ok(from_env) = std::env::var(PASSWORD_ENV)
        && !from_env.is_empty()
    {
        return Ok(from_env);
    }

    let password = rpassword::prompt_password("keystore password: ")
        .context("reading password; set L1_WALLET_PASSWORD for non-interactive use")?;

    if confirm {
        let again = rpassword::prompt_password("confirm password: ")
            .context("reading password confirmation")?;
        if again != password {
            bail!("passwords do not match");
        }
    }

    if password.is_empty() {
        bail!("password must not be empty");
    }

    Ok(password)
}

/// Loads and decrypts the keystore at `path`.
fn load_key(path: &Path) -> Result<HybridSigningKey> {
    if !path.exists() {
        bail!(
            "no keystore at {} — run `l1-wallet generate` first",
            path.display()
        );
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let password = read_password(false)?;
    keystore::decrypt(&bytes, &password)
}

fn decode_address(value: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(value.trim_start_matches("0x")).context("address is not valid hex")?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| anyhow::anyhow!("address must be 32 bytes, got {}", bytes.len()))
}

fn command_generate(path: &Path) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists — refusing to overwrite an existing key",
            path.display()
        );
    }

    let password = read_password(true)?;
    let signing_key = generate_signing_key()?;
    let address = signing_key.address();

    let blob = keystore::encrypt(&signing_key, &password)?;
    keystore::save(path, &blob)?;

    println!("keystore written to {}", path.display());
    println!("address: {}", hex::encode(address));

    if !keystore::permissions_are_restricted() {
        eprintln!(
            "warning: file permissions were not restricted on this platform; \
             the keystore inherits its directory's access rules"
        );
    }
    println!("keep this file and its password safe — neither can be recovered");
    Ok(())
}

fn command_address(path: &Path) -> Result<()> {
    let key = load_key(path)?;
    println!("{}", hex::encode(key.address()));
    Ok(())
}

async fn command_balance(path: &Path, rpc_url: &str, address: Option<String>) -> Result<()> {
    // Only unlock the keystore when the address was not supplied: querying
    // someone else's balance should not require a password.
    let address_hex = match address {
        Some(value) => {
            decode_address(&value)?;
            value
        }
        None => hex::encode(load_key(path)?.address()),
    };

    let client = NodeClient::connect(rpc_url)?;
    let account = client.get_balance(&address_hex).await?;

    println!("address: {}", account.address);
    println!("balance: {}", account.balance);
    println!("nonce:   {}", account.nonce);
    Ok(())
}

async fn command_block(rpc_url: &str, height: u64) -> Result<()> {
    let client = NodeClient::connect(rpc_url)?;
    let block = client.get_block_by_height(height).await?;

    println!("node:      {}", client.url());
    println!("height:    {}", block.height);
    println!("id:        {}", block.header.id);
    println!("prev:      {}", block.header.prev_hash);
    println!("state root:{}", block.header.state_root);
    println!("timestamp: {}", block.header.timestamp);
    println!("nonce:     {}", block.header.nonce);
    println!("target:    {}", block.header.difficulty_target);
    println!("txs:       {}", block.transactions.len());
    for tx in &block.transactions {
        println!(
            "  {} nonce {} ({} output(s))",
            tx.txid,
            tx.nonce,
            tx.outputs.len()
        );
    }
    Ok(())
}

async fn command_send(
    path: &Path,
    rpc_url: &str,
    to: &str,
    amount: U256,
    opts: &TxOptions,
) -> Result<()> {
    let recipient = decode_address(to)?;
    let key = load_key(path)?;
    let payload = Payload {
        kind: TxKind::Transfer,
        outputs: vec![TxOutput { amount, recipient }],
    };
    if submit(&key, rpc_url, payload, opts).await? {
        println!("to:       {}", hex::encode(recipient));
        println!("amount:   {amount}");
    }
    Ok(())
}

async fn command_register_validator(
    path: &Path,
    rpc_url: &str,
    validator_key: &Path,
    bond: U256,
    commission_bps: u16,
    opts: &TxOptions,
) -> Result<()> {
    let validator = l1_wallet::staking::load_validator_key(validator_key)?;
    let key = load_key(path)?;
    let action = l1_wallet::staking::register(&validator, &key.address(), bond, commission_bps)?;
    let payload = Payload {
        kind: TxKind::Staking(Box::new(action)),
        outputs: vec![],
    };
    if submit(&key, rpc_url, payload, opts).await? {
        println!(
            "validator: {}",
            hex::encode(l1_wallet::staking::validator_id(&validator))
        );
        println!("bond:     {bond}");
        println!("joins the committee at the next epoch boundary if its stake ranks");
    }
    Ok(())
}

async fn command_delegate(
    path: &Path,
    rpc_url: &str,
    validator: &str,
    amount: U256,
    opts: &TxOptions,
) -> Result<()> {
    let validator = l1_wallet::staking::parse_validator_id(validator)?;
    let key = load_key(path)?;
    let payload = Payload {
        kind: TxKind::Staking(Box::new(StakingAction::Delegate { validator, amount })),
        outputs: vec![],
    };
    if submit(&key, rpc_url, payload, opts).await? {
        println!("validator: {}", hex::encode(validator));
        println!("amount:   {amount}");
    }
    Ok(())
}

async fn command_report_equivocation(
    path: &Path,
    rpc_url: &str,
    evidence: &Path,
    opts: &TxOptions,
) -> Result<()> {
    let text = std::fs::read_to_string(evidence)
        .with_context(|| format!("reading {}", evidence.display()))?;
    let action = custom_l1_node::consensus::bft::evidence::from_text(&text)?;
    let key = load_key(path)?;
    let payload = Payload {
        kind: TxKind::Staking(Box::new(action)),
        outputs: vec![],
    };
    submit(&key, rpc_url, payload, opts).await?;
    Ok(())
}

async fn command_council_action(
    path: &Path,
    rpc_url: &str,
    action: &Path,
    opts: &TxOptions,
) -> Result<()> {
    let text =
        std::fs::read_to_string(action).with_context(|| format!("reading {}", action.display()))?;
    let bytes = hex::decode(text.trim()).context("the council action file is not hex")?;
    let mut reader = custom_l1_node::core::codec::ByteReader::new(&bytes);
    let decoded = CouncilAction::decode(&mut reader).context("not a council action")?;
    reader
        .finish()
        .context("trailing bytes after the council action")?;
    let key = load_key(path)?;
    let summary = format!("{:?} nonce {}", decoded.kind, decoded.nonce);
    let payload = Payload {
        kind: TxKind::Council(Box::new(decoded)),
        outputs: vec![],
    };
    if submit(&key, rpc_url, payload, opts).await? {
        println!("council:  {summary}");
    }
    Ok(())
}

/// What a transaction does, before nonce, fee and signature.
struct Payload {
    kind: TxKind,
    outputs: Vec<TxOutput>,
}

/// Prints what `tx` will do, in full, before anything is signed: the chain it
/// is for, every output with its full address, and the fee.
fn print_description(tx: &Transaction, chain: &ChainInfo, trust: GenesisTrust, fee: U256) {
    let described = custom_l1_node::core::describe::describe(tx);
    println!(
        "chain:    {} ({})",
        chain.chain_id.as_deref().unwrap_or("unnamed"),
        trust.label()
    );
    println!("genesis:  {}", chain.genesis);
    println!("signing:  {}", described.summary);
    for (label, value) in &described.details {
        println!("          {label}: {value}");
    }
    println!("fee:      {fee} base units, paid to the protocol collector");
    if described.review == custom_l1_node::core::describe::Review::Careful {
        println!("          check every line above; this moves value or changes control");
    }
}

/// The value a payload moves out of the sender's balance, for the fee warning.
/// Staking actions carry their amounts outside the outputs, so they are read here.
fn value_moved(payload: &Payload) -> U256 {
    let outputs = payload
        .outputs
        .iter()
        .fold(U256::zero(), |sum, o| sum.saturating_add(o.amount));
    let staked = match &payload.kind {
        TxKind::Staking(action) => match action.as_ref() {
            StakingAction::Register { bond, .. } => *bond,
            StakingAction::BondMore { amount, .. }
            | StakingAction::Unbond { amount, .. }
            | StakingAction::Delegate { amount, .. }
            | StakingAction::Undelegate { amount, .. } => *amount,
            _ => U256::zero(),
        },
        _ => U256::zero(),
    };
    outputs.saturating_add(staked)
}

/// Signs `payload` with the wallet key and broadcasts it, or prints it raw.
/// Returns whether it was broadcast.
async fn submit(
    key: &HybridSigningKey,
    rpc_url: &str,
    payload: Payload,
    opts: &TxOptions,
) -> Result<bool> {
    let sender_hex = hex::encode(key.address());
    let client = NodeClient::connect(rpc_url)?;

    // Fetching the nonce keeps the caller from having to track it, and a stale
    // value is the most common cause of a rejected transaction.
    let nonce = match opts.nonce {
        Some(value) => value,
        None => client.get_balance(&sender_hex).await?.nonce,
    };

    // ADR-036: the signature commits to this node's genesis, so the
    // transaction is worthless on any other Maya2C chain. The node's word for
    // which chain that is is checked before anything else is trusted.
    let info = client.get_network().await?;
    let chain = info.chain_tag().context("get_chain_info")?;
    let trust = guard::check_genesis(
        &info.genesis,
        info.chain_id.as_deref(),
        opts.genesis.as_deref(),
        guard::MAINNET_GENESIS,
        guard::TESTNET_GENESIS,
    )?;

    let (outputs, fee) = with_fee(&client, key, &payload, nonce, opts, &chain).await?;
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.kind = payload.kind;
    // Shown from the exact transaction that is signed, fee output included.
    print_description(&tx, &info, trust, fee);
    let mut stdin = std::io::stdin().lock();
    guard::require_confirmation(
        opts.yes,
        std::io::stdin().is_terminal(),
        &mut stdin,
        &mut std::io::stderr(),
    )?;
    tx.sign(key, &chain).context("signing the transaction")?;

    let raw = hex::encode(tx.to_bytes());
    if opts.no_broadcast {
        println!("raw:      {raw}");
        println!("txid:     {}", hex::encode(tx.txid()));
        return Ok(false);
    }
    let result = client.send_raw_transaction(&raw).await?;

    println!("txid:     {}", result.txid);
    println!("from:     {sender_hex}");
    println!("nonce:    {nonce}");
    if result.accepted {
        println!("status:   accepted into the mempool");
    } else {
        println!("status:   already known to the node");
    }
    Ok(true)
}

/// Appends the fee output where the chain charges fees (ADR-029).
///
/// The size is measured on a signed probe that already carries the fee output
/// — an output's encoding does not depend on its amount — so the fee covers
/// exactly the bytes that will be sent.
async fn with_fee(
    client: &NodeClient,
    key: &HybridSigningKey,
    payload: &Payload,
    nonce: u64,
    opts: &TxOptions,
    chain: &ChainTag,
) -> Result<(Vec<TxOutput>, U256)> {
    let mut outputs = payload.outputs.clone();
    let info = match client.get_fee_info().await {
        Ok(info) if info.active => info,
        _ => return Ok((outputs, U256::zero())),
    };
    // The collector comes from the node, so it is checked against the protocol's before any fee is paid to it.
    guard::check_collector(&info.collector)?;
    let collector = decode_address(&info.collector)?;
    outputs.push(TxOutput {
        amount: U256::zero(),
        recipient: collector,
    });
    // The fee's size is measured on a signed probe that already carries the fee output.
    let mut probe = Transaction::new(vec![], outputs.clone(), nonce);
    probe.kind = payload.kind.clone();
    probe.sign(key, chain).context("signing the fee probe")?;
    let size = u64::try_from(probe.to_bytes().len())?;
    let fee = guard::quote_fee(&info, size, opts.fee, opts.max_fee)?;
    if matches!(payload.kind, TxKind::Transfer) {
        if let Some(warning) = guard::fee_warning(fee, value_moved(payload)) {
            eprintln!("{warning}");
        }
    }
    if let Some(last) = outputs.last_mut() {
        last.amount = fee;
    }
    Ok((outputs, fee))
}

/// Stack for the thread that runs every command. ML-DSA keys and 256-bit
/// amounts are large stack values, and `generate` overflowed Windows' 1 MiB
/// main thread in a debug build (attacknet setup, 2026-10-29), as
/// `genesis-ceremony` did before it. 16 MiB costs nothing on a CLI.
const WALLET_STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() -> Result<()> {
    std::thread::Builder::new()
        .name("l1-wallet".to_string())
        .stack_size(WALLET_STACK_BYTES)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(WALLET_STACK_BYTES)
                .build()
                .context("starting the async runtime")?
                .block_on(run())
        })
        .context("spawning the wallet thread")?
        .join()
        .map_err(|_| anyhow::anyhow!("the wallet thread panicked"))?
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Generate => command_generate(&cli.keystore),
        Command::Address => command_address(&cli.keystore),
        Command::Balance { address } => command_balance(&cli.keystore, &cli.rpc_url, address).await,
        Command::Block { height } => command_block(&cli.rpc_url, height).await,
        Command::Send { to, amount, tx } => {
            Box::pin(command_send(&cli.keystore, &cli.rpc_url, &to, amount, &tx)).await
        }
        Command::RegisterValidator {
            validator_key,
            bond,
            commission_bps,
            tx,
        } => {
            Box::pin(command_register_validator(
                &cli.keystore,
                &cli.rpc_url,
                &validator_key,
                bond,
                commission_bps,
                &tx,
            ))
            .await
        }
        Command::Delegate {
            validator,
            amount,
            tx,
        } => {
            Box::pin(command_delegate(
                &cli.keystore,
                &cli.rpc_url,
                &validator,
                amount,
                &tx,
            ))
            .await
        }
        Command::ReportEquivocation { evidence, tx } => {
            Box::pin(command_report_equivocation(
                &cli.keystore,
                &cli.rpc_url,
                &evidence,
                &tx,
            ))
            .await
        }
        Command::CouncilAction { action, tx } => {
            Box::pin(command_council_action(
                &cli.keystore,
                &cli.rpc_url,
                &action,
                &tx,
            ))
            .await
        }
    }
}

/// A decimal amount in base units. `U256::from_str` reads hex, so the CLI
/// parses decimal explicitly: `--amount 10` must mean ten, not sixteen.
fn parse_amount(text: &str) -> std::result::Result<U256, String> {
    U256::from_dec_str(text).map_err(|_| format!("{text:?} is not a decimal amount below 2^256"))
}
