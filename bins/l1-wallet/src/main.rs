//! `l1-wallet` — command line wallet for the custom L1 node.
//!
//! ```text
//! l1-wallet generate                          create and encrypt a new key
//! l1-wallet address                           print the address of a keystore
//! l1-wallet balance                           query account state over RPC
//! l1-wallet send --to <ADDR> --amount <VAL>   sign and broadcast a transfer
//!               [--no-broadcast]              ... or only print the signed hex
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

use l1_wallet::keystore;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use custom_l1_node::core::{ChainTag, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};

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
    /// Generate a new Ed25519 keypair and save it encrypted.
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

        /// Amount to transfer, in base units.
        #[arg(long)]
        amount: u64,

        /// Sender nonce. Fetched from the node when omitted.
        #[arg(long)]
        nonce: Option<u64>,

        /// Fee paid to the collector. Defaults to twice the base fee on the
        /// transaction's size, the excess being the validators' tip, where
        /// the chain charges fees; nothing where it does not.
        #[arg(long)]
        fee: Option<u64>,

        /// Sign and print the raw transaction hex instead of broadcasting it,
        /// for a different client (an SDK, a gateway) to submit.
        #[arg(long)]
        no_broadcast: bool,
    },
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
    amount: u64,
    nonce: Option<u64>,
    fee: Option<u64>,
    broadcast: bool,
) -> Result<()> {
    let recipient = decode_address(to)?;
    let key = load_key(path)?;
    let sender_hex = hex::encode(key.address());

    let client = NodeClient::connect(rpc_url)?;

    // Fetching the nonce keeps the caller from having to track it, and a stale
    // value is the most common cause of a rejected transfer.
    let nonce = match nonce {
        Some(value) => value,
        None => client.get_balance(&sender_hex).await?.nonce,
    };

    // ADR-036: the signature commits to this node's genesis, so the transfer
    // is worthless on any other Maya2C chain.
    let chain = client.get_chain_info().await?;
    let outputs = with_fee(
        &client,
        &key,
        vec![TxOutput { amount, recipient }],
        nonce,
        fee,
        &chain,
    )
    .await?;
    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(&key, &chain).context("signing the transaction")?;

    let raw = hex::encode(tx.to_bytes());
    if !broadcast {
        println!("raw:      {raw}");
        println!("txid:     {}", hex::encode(tx.txid()));
        return Ok(());
    }
    let result = client.send_raw_transaction(&raw).await?;

    println!("txid:     {}", result.txid);
    println!("from:     {sender_hex}");
    println!("to:       {}", hex::encode(recipient));
    println!("amount:   {amount}");
    println!("nonce:    {nonce}");
    if result.accepted {
        println!("status:   accepted into the mempool");
    } else {
        println!("status:   already known to the node");
    }
    Ok(())
}

/// Appends the fee output where the chain charges fees (ADR-029).
///
/// The size is measured on a signed probe that already carries the fee output
/// — an output's encoding does not depend on its amount — so the fee covers
/// exactly the bytes that will be sent.
async fn with_fee(
    client: &NodeClient,
    key: &HybridSigningKey,
    mut outputs: Vec<TxOutput>,
    nonce: u64,
    fee: Option<u64>,
    chain: &ChainTag,
) -> Result<Vec<TxOutput>> {
    let info = match client.get_fee_info().await {
        Ok(info) if info.active => info,
        _ => return Ok(outputs),
    };
    let collector = decode_address(&info.collector)?;
    outputs.push(TxOutput {
        amount: 0,
        recipient: collector,
    });
    let amount = if let Some(fee) = fee {
        fee
    } else {
        let mut probe = Transaction::new(vec![], outputs.clone(), nonce);
        probe.sign(key, chain).context("signing the fee probe")?;
        let size = u64::try_from(probe.to_bytes().len())?;
        info.base_fee.saturating_mul(size).saturating_mul(2)
    };
    if let Some(last) = outputs.last_mut() {
        last.amount = amount;
    }
    Ok(outputs)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Generate => command_generate(&cli.keystore),
        Command::Address => command_address(&cli.keystore),
        Command::Balance { address } => command_balance(&cli.keystore, &cli.rpc_url, address).await,
        Command::Block { height } => command_block(&cli.rpc_url, height).await,
        Command::Send {
            to,
            amount,
            nonce,
            fee,
            no_broadcast,
        } => {
            Box::pin(command_send(
                &cli.keystore,
                &cli.rpc_url,
                &to,
                amount,
                nonce,
                fee,
                !no_broadcast,
            ))
            .await
        }
    }
}
