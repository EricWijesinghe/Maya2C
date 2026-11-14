//! `maya2c notary`: prove that a file existed at a time, without the file.
//!
//! The file is hashed here (BLAKE3, streamed; plain, so `b3sum` reproduces
//! it). The hash rides in an `AnchorAttestation` transaction, an existing
//! payload (tag 34) that carries a 32-byte root under a schema label. Nothing
//! but the hash reaches the node. The block that holds the transaction fixes
//! the time; the newest committee checkpoint says whether that block is final.
//!
//! Each stamp overwrites the node's state record for its schema, so the
//! transaction in its block is the proof, not that record.
//!
//! shortcut: verify scans blocks from `--from` in 32-block RPC calls, because
//! the node has no transaction-lookup RPC. A `get_transaction` RPC makes verify
//! one call; that is a post-genesis node change.
//! shortcut: the checkpoint signatures are not verified here, so finality is
//! the node's word. Verifying the committee signatures is the next step.
//! shortcut: the stamp's signature is checked against the chain tag the node
//! reports (`get_chain_info`), so the chain identity is the node's word, as
//! finality is. Pinning the genesis on verify (as `stamp` does) closes it.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use custom_l1_node::core::identity_payload::{AnchorAttestation, RegisterDid};
use custom_l1_node::core::{Block, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::HybridSigningKey;
use custom_l1_node::rpc::types::{BlockInfo, ChainInfo};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use l1_wallet::client::NodeClient;
use l1_wallet::guard;
use primitive_types::U256;
use serde::{Deserialize, Serialize};

/// The schema label every notary stamp uses. Part of the receipt format.
pub const SCHEMA: &str = "notary-v1";
/// Read size for hashing. A stream, so memory does not grow with the file.
const READ_BYTES: usize = 64 * 1024;
/// The node's cap on one `get_blocks_by_height_range` call.
const SCAN_BATCH: u64 = 32;

/// Whether a stamped block is covered by a committee checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finality {
    /// A checkpoint at or above the block height exists.
    Final,
    /// No checkpoint covers the block yet.
    Pending,
}

/// Where the anchoring transaction sits on chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Height of the block that holds the transaction.
    pub height: u64,
    /// Id of that block.
    pub block_hash: [u8; 32],
    /// Unix seconds, from that block's header.
    pub timestamp: u64,
}

/// The proof a verifier prints. All hashes are lowercase hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// BLAKE3 of the file.
    pub hash: String,
    /// The anchoring transaction.
    pub txid: String,
    /// The sender's address, i.e. the DID that signed the stamp (signature
    /// checked by `verify`).
    pub sender: String,
    /// Schema label; always [`SCHEMA`] for a receipt.
    pub schema: String,
    /// Block height.
    pub height: u64,
    /// Block id.
    pub block_hash: String,
    /// Block time, unix seconds.
    pub timestamp: u64,
    /// Whether a checkpoint covers the block.
    pub finality: Finality,
}

/// BLAKE3 of everything `reader` yields, read in [`READ_BYTES`] chunks.
///
/// # Errors
///
/// Returns the read error, if one occurs.
pub fn hash_reader(mut reader: impl Read) -> io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; READ_BYTES];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(*hasher.finalize().as_bytes())
}

/// The BLAKE3 hash of the file at `path`.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or read.
pub fn hash_file(path: &Path) -> Result<[u8; 32]> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    hash_reader(file).with_context(|| format!("reading {}", path.display()))
}

/// The root a notary stamp anchors, or `None` for any other transaction.
#[must_use]
pub fn anchored_hash(tx: &Transaction) -> Option<[u8; 32]> {
    match &tx.kind {
        TxKind::AnchorAttestation(anchor) if anchor.schema == SCHEMA => Some(anchor.root),
        _ => None,
    }
}

/// Refuses unless the file's hash is the anchored one.
///
/// # Errors
///
/// Returns an error for any difference, so one changed byte is enough.
pub fn check_match(file_hash: &[u8; 32], anchored: &[u8; 32]) -> Result<()> {
    if file_hash == anchored {
        Ok(())
    } else {
        bail!(
            "the file does not match the stamped hash: it changed after stamping, or it is a different file"
        )
    }
}

/// [`Finality::Final`] when a checkpoint is at or above `height`.
#[must_use]
pub fn finality(height: u64, checkpoint_height: Option<u64>) -> Finality {
    match checkpoint_height {
        Some(checkpoint) if checkpoint >= height => Finality::Final,
        _ => Finality::Pending,
    }
}

/// Builds the receipt for `tx`, refusing unless it anchors `file_hash`.
///
/// # Errors
///
/// Returns an error if `tx` is not a notary stamp, or anchors another hash.
pub fn build_receipt(
    file_hash: [u8; 32],
    tx: &Transaction,
    place: Placement,
    checkpoint_height: Option<u64>,
) -> Result<Receipt> {
    let anchored = anchored_hash(tx)
        .context("the transaction is not a notary stamp (no anchor under the notary schema)")?;
    check_match(&file_hash, &anchored)?;
    Ok(Receipt {
        hash: hex::encode(file_hash),
        txid: hex::encode(tx.txid()),
        sender: hex::encode(tx.sender()),
        schema: SCHEMA.to_string(),
        height: place.height,
        block_hash: hex::encode(place.block_hash),
        timestamp: place.timestamp,
        finality: finality(place.height, checkpoint_height),
    })
}

/// Hashes `file`, anchors the hash on chain, and returns the transaction id.
///
/// The keystore's DID must already exist; see [`setup`].
///
/// # Errors
///
/// Returns an error if the file cannot be read, the keystore cannot be opened
/// with `password`, the chain is not one the genesis rules trust, or the node
/// refuses the transaction.
pub async fn stamp(
    keystore: &Path,
    password: &str,
    rpc: &str,
    genesis: Option<&str>,
    file: &Path,
) -> Result<String> {
    let key = open_key(keystore, password)?;
    let root = hash_file(file)?;
    let kind = TxKind::AnchorAttestation(Box::new(AnchorAttestation {
        schema: SCHEMA.to_string(),
        root,
        expires_at: 0,
    }));
    submit(&key, rpc, genesis, kind).await
}

/// Creates the keystore's DID (`register_did`), which a stamp needs once.
///
/// # Errors
///
/// As for [`stamp`].
pub async fn setup(
    keystore: &Path,
    password: &str,
    rpc: &str,
    genesis: Option<&str>,
) -> Result<String> {
    let key = open_key(keystore, password)?;
    let mut public_key = Vec::new();
    key.public_key().encode_into(&mut public_key);
    let kind = TxKind::RegisterDid(Box::new(RegisterDid {
        public_key,
        endpoints: vec![],
    }));
    submit(&key, rpc, genesis, kind).await
}

/// Finds the transaction `txid` in the chain at `rpc` and checks it against `file`.
///
/// Scans from `from` to the tip, so the cost grows with chain length.
///
/// # Errors
///
/// Returns an error if the file cannot be read, the transaction is not found,
/// the node serves a block whose bytes contradict its header, the transaction is
/// not a notary stamp, or the file does not match the stamped hash.
pub async fn verify(file: &Path, txid_hex: &str, rpc: &str, from: u64) -> Result<Receipt> {
    let file_hash = hash_file(file)?;
    let txid = parse_hash(txid_hex)?;
    let client = HttpClientBuilder::default().build(rpc)?;
    let tip: ChainInfo = client.request("get_chain_info", rpc_params![]).await?;

    let chain = tip.chain_tag().context("get_chain_info")?;

    let mut next = from.max(1);
    while next <= tip.height {
        let count = SCAN_BATCH.min(tip.height - next + 1);
        let infos: Vec<BlockInfo> = client
            .request("get_blocks_by_height_range", rpc_params![next, count])
            .await?;
        let after = next_height(&infos, next)?;
        for info in &infos {
            let block = decode_block(info)?;
            if let Some(tx) = block.transactions.iter().find(|t| t.txid() == txid) {
                tx.verify(&chain)
                    .map_err(|e| anyhow!("the stamp's signature does not verify: {e}"))?;
                return finish(&client, file_hash, tx, &block, info.height).await;
            }
        }
        next = after;
    }
    bail!(
        "transaction {txid_hex} is not in blocks {from}..={}",
        tip.height
    )
}

/// The height after a served batch, refusing unless the batch is the
/// non-empty, contiguous run of heights starting at `next`. Without this a
/// node serving the same block forever keeps verify looping.
///
/// # Errors
///
/// Returns an error for an empty batch or any height out of sequence.
pub fn next_height(infos: &[BlockInfo], next: u64) -> Result<u64> {
    if infos.is_empty() {
        bail!("the node stopped serving blocks at height {next}");
    }
    let mut expected = next;
    for info in infos {
        if info.height != expected {
            bail!(
                "the node served block {} where block {expected} belongs",
                info.height
            );
        }
        expected = expected.checked_add(1).context("block height overflow")?;
    }
    Ok(expected)
}

/// Decodes a served block, refusing unless its body matches the `tx_root`
/// its id commits to, and that id is the one the node reported.
///
/// The decoded header's own `tx_root` is the one checked: `Block::from_bytes`
/// keeps the value it arrived with, and `header.id()` hashes it, so a swapped
/// body under an honest header shows here. The JSON `tx_root` is a separate
/// claim and proves nothing.
///
/// # Errors
///
/// Returns an error if the bytes are not a block, the body disagrees with the
/// header, or the header's id is not `info.header.id`.
pub fn decode_block(info: &BlockInfo) -> Result<Block> {
    let bytes = hex::decode(&info.raw).context("the node sent a block that is not hex")?;
    let block =
        Block::from_bytes(&bytes).map_err(|e| anyhow!("decoding block {}: {e}", info.height))?;
    if block.tx_root() != block.header.tx_root {
        bail!(
            "block {} transactions do not match its header's tx root",
            info.height
        );
    }
    if hex::encode(block.header.id()) != info.header.id {
        bail!(
            "block {} bytes are not the block the node named",
            info.height
        );
    }
    Ok(block)
}

async fn finish(
    client: &jsonrpsee::http_client::HttpClient,
    file_hash: [u8; 32],
    tx: &Transaction,
    block: &Block,
    height: u64,
) -> Result<Receipt> {
    let checkpoint: Option<CheckpointHeight> =
        client.request("get_checkpoint", rpc_params![]).await?;
    let place = Placement {
        height,
        block_hash: block.header.id(),
        timestamp: block.header.timestamp,
    };
    build_receipt(file_hash, tx, place, checkpoint.map(|c| c.height))
}

/// The one field of a checkpoint that finality needs.
#[derive(Deserialize)]
struct CheckpointHeight {
    height: u64,
}

fn open_key(keystore: &Path, password: &str) -> Result<HybridSigningKey> {
    let bytes =
        std::fs::read(keystore).with_context(|| format!("reading {}", keystore.display()))?;
    l1_wallet::keystore::decrypt(&bytes, password).context("opening the keystore")
}

/// Signs `kind` with the wallet key and broadcasts it. Returns the txid.
async fn submit(
    key: &HybridSigningKey,
    rpc: &str,
    genesis: Option<&str>,
    kind: TxKind,
) -> Result<String> {
    let client = NodeClient::connect(rpc)?;
    let sender = hex::encode(key.address());
    let nonce = client.get_balance(&sender).await?.nonce;
    let info = client.get_network().await?;
    guard::check_genesis(
        &info.genesis,
        info.chain_id.as_deref(),
        genesis,
        guard::MAINNET_GENESIS,
        guard::TESTNET_GENESIS,
    )?;
    let chain = info.chain_tag().context("get_chain_info")?;
    let outputs = fee_outputs(&client, key, &kind, nonce, &chain).await?;

    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.kind = kind;
    tx.sign(key, &chain).context("signing the transaction")?;
    client
        .send_raw_transaction(&hex::encode(tx.to_bytes()))
        .await
        .context("the node refused the transaction")?;
    Ok(hex::encode(tx.txid()))
}

/// The fee output, sized on a signed probe, as the wallet does (ADR-029).
async fn fee_outputs(
    client: &NodeClient,
    key: &HybridSigningKey,
    kind: &TxKind,
    nonce: u64,
    chain: &custom_l1_node::core::ChainTag,
) -> Result<Vec<TxOutput>> {
    let fee = client
        .get_fee_info()
        .await
        .context("querying the node's fee rules; refusing to sign without them")?;
    if !fee.active {
        return Ok(vec![]);
    }
    guard::check_collector(&fee.collector)?;
    let mut outputs = vec![TxOutput {
        amount: U256::zero(),
        recipient: parse_hash(&fee.collector)?,
    }];
    let mut probe = Transaction::new(vec![], outputs.clone(), nonce);
    probe.kind = kind.clone();
    probe.sign(key, chain).context("signing the fee probe")?;
    let size = u64::try_from(probe.to_bytes().len())?;
    outputs[0].amount = guard::quote_fee(&fee, size, None, None)?;
    Ok(outputs)
}

/// A 32-byte value from hex, with or without a `0x` prefix.
fn parse_hash(text: &str) -> Result<[u8; 32]> {
    let raw = text.trim().strip_prefix("0x").unwrap_or(text.trim());
    let bytes = hex::decode(raw).with_context(|| format!("{text:?} is not hex"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("{text:?} is not 32 bytes"))
}
