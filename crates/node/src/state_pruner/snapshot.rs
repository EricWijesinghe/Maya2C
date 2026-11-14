//! State snapshots, and bootstrapping a pruned node from one.
//!
//! # What a new pruned node downloads
//!
//! 1. **Every header**, genesis to tip. 145 bytes each, fully validated: the
//!    exact retarget rule, proof of work, cumulative work.
//! 2. **The state at a height `H` at least K below the tip**, as a snapshot:
//!    every key/value under a committed prefix, in chunks.
//! 3. **The bodies above `H`**, applied with full validation.
//!
//! Nothing below `H` but headers. That is the whole saving.
//!
//! # Why the snapshot can be believed
//!
//! Each chunk is checked against the manifest's chunk hashes. That only proves
//! the server was consistent. The snapshot is then imported, the state root
//! recomputed, and the result required to equal `header(H).state_root`. That
//! header's proof of work was checked in step 1, and its state root is checked
//! by every full node since the `tx_root` fix. A server that lies about one
//! balance, drops one nullifier or changes one contract slot produces a
//! different root, and the import is refused and wiped. The root covers every
//! committed prefix (see `state::commitments`), and a key under any other
//! prefix is refused on sight.
//!
//! What this cannot rule out is the SPV limitation: a source that shows only
//! a lower-work fork of headers. That is the same eclipse problem a light
//! client has, and the answer is the same: ask more than one peer.
//!
//! # Snapshots at rest
//!
//! [`Snapshots::take`] makes a `RocksDB` checkpoint (hard links, cheap) at the
//! current tip. [`Snapshots::serveable`] serves the newest one at least K deep,
//! so what a new node adopts is below the depth a pruned node treats as final.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rocksdb::WriteBatch;

use crate::consensus::{Chain, ChainConfig};
use crate::core::codec::{ByteReader, MAX_RECORD_BYTES};
use crate::core::{Block, BlockHeader};
use crate::error::{NodeError, Result};
use crate::rpc::server::RPC_RESPONSE_LIMIT;
use crate::state::commitments::{committed_prefixes, is_committed};
use crate::state::db::StateDB;

/// Most entries per snapshot chunk. A chunk also closes early at
/// [`MAX_CHUNK_BYTES`]; see [`encode_chunks`].
pub const CHUNK_ENTRIES: usize = 4_096;

/// Longest key a chunk entry may carry.
const MAX_ENTRY_KEY: usize = 64 * 1024;

/// Longest value a chunk entry may carry. The old 65,536 ceiling (the
/// collection-count cap, reused by mistake) made a busy chain's snapshot export
/// something it could not import.
const MAX_ENTRY_VALUE: usize = MAX_RECORD_BYTES;

/// Headers asked for per request during bootstrap.
pub const HEADER_BATCH: u64 = 2_000;

/// Room left in one JSON-RPC response for everything around a chunk's hex:
/// the envelope (`{"jsonrpc":"2.0","id":…,"result":"…"}`) is under 100 bytes.
const RPC_ENVELOPE_BYTES: usize = 64 * 1024;

/// Most bytes one snapshot chunk may be, served or received.
///
/// `get_snapshot_chunk` sends a chunk hex-encoded (two characters a byte) in
/// one JSON-RPC response capped at [`RPC_RESPONSE_LIMIT`]. A chunk past half
/// that cap could be built but never delivered, and every new node's
/// bootstrap would stop at it for good: the old bound, 64 MiB with chunks cut
/// by entry count alone, let about 33 maximal contracts do exactly that. The
/// importer refuses the same bound, so a peer cannot make it hash and decode
/// an unbounded body.
pub const MAX_CHUNK_BYTES: usize = RPC_RESPONSE_LIMIT as usize / 2 - RPC_ENVELOPE_BYTES;

const _: () = assert!(
    2 * MAX_CHUNK_BYTES + RPC_ENVELOPE_BYTES <= RPC_RESPONSE_LIMIT as usize,
    "a maximal chunk, hex-encoded with its envelope, must fit one RPC response"
);

/// Bytes [`encode_chunk`] spends on the entry count.
const CHUNK_HEADER_BYTES: usize = 8;

/// Bytes [`encode_chunk`] spends per entry besides its key and value.
const ENTRY_OVERHEAD_BYTES: usize = 16;

/// Domain of a chunk's hash in the manifest.
const CHUNK_CONTEXT: &str = "maya snapshot chunk v1";

/// What a snapshot holds: the height it was taken at, the block it followed,
/// and a hash per chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotManifest {
    /// Height of the block whose post-state this is.
    pub height: u64,
    /// That block's id.
    pub block_id: [u8; 32],
    /// BLAKE3 of each chunk's encoding, in order.
    pub chunks: Vec<[u8; 32]>,
}

impl SnapshotManifest {
    /// Encodes the manifest.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + 32 + 8 + self.chunks.len() * 32);
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.block_id);
        buf.extend_from_slice(&(self.chunks.len() as u64).to_le_bytes());
        for chunk in &self.chunks {
            buf.extend_from_slice(chunk);
        }
        buf
    }

    /// Decodes a manifest.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if it is malformed.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let height = reader.read_u64()?;
        let block_id = reader.read_array::<32>()?;
        let count = reader.read_collection_len(32)?;
        let mut chunks = Vec::with_capacity(count);
        for _ in 0..count {
            chunks.push(reader.read_array::<32>()?);
        }
        reader.finish()?;
        Ok(Self {
            height,
            block_id,
            chunks,
        })
    }
}

/// The hash a chunk is listed under.
#[must_use]
pub fn chunk_hash(chunk: &[u8]) -> [u8; 32] {
    blake3::derive_key(CHUNK_CONTEXT, chunk)
}

/// Encodes one chunk of key/value pairs.
#[must_use]
pub fn encode_chunk(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (key, value) in entries {
        buf.extend_from_slice(&(key.len() as u64).to_le_bytes());
        buf.extend_from_slice(key);
        buf.extend_from_slice(&(value.len() as u64).to_le_bytes());
        buf.extend_from_slice(value);
    }
    buf
}

/// Splits `entries` into encoded chunks, each closed at whichever comes first:
/// [`CHUNK_ENTRIES`] entries or [`MAX_CHUNK_BYTES`] bytes. One entry too large
/// for a chunk on its own is refused rather than served as a chunk no peer
/// can download.
///
/// # Errors
///
/// Returns [`NodeError::Storage`] for an entry whose encoding alone exceeds
/// [`MAX_CHUNK_BYTES`].
pub fn encode_chunks(entries: &[(Vec<u8>, Vec<u8>)]) -> Result<Vec<Vec<u8>>> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut bytes = CHUNK_HEADER_BYTES;
    for (index, (key, value)) in entries.iter().enumerate() {
        let size = ENTRY_OVERHEAD_BYTES + key.len() + value.len();
        if CHUNK_HEADER_BYTES + size > MAX_CHUNK_BYTES {
            return Err(NodeError::Storage(format!(
                "snapshot entry {} is {size} bytes encoded, over the \
                 {MAX_CHUNK_BYTES}-byte chunk cap: it cannot be served over RPC",
                hex::encode(&key[..key.len().min(64)])
            )));
        }
        if index - start == CHUNK_ENTRIES || bytes + size > MAX_CHUNK_BYTES {
            chunks.push(encode_chunk(&entries[start..index]));
            start = index;
            bytes = CHUNK_HEADER_BYTES;
        }
        bytes += size;
    }
    if start < entries.len() {
        chunks.push(encode_chunk(&entries[start..]));
    }
    Ok(chunks)
}

/// Decodes one chunk.
///
/// # Errors
///
/// Returns [`NodeError::Decode`] if it is malformed.
pub fn decode_chunk(bytes: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut reader = ByteReader::new(bytes);
    let count = reader.read_collection_len(16)?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let key_len = reader.read_byte_len(MAX_ENTRY_KEY)?;
        let key = reader.read_slice(key_len)?.to_vec();
        let value_len = reader.read_byte_len(MAX_ENTRY_VALUE)?;
        let value = reader.read_slice(value_len)?.to_vec();
        entries.push((key, value));
    }
    reader.finish()?;
    Ok(entries)
}

impl StateDB {
    /// Every key/value pair under a committed prefix, in key order: exactly
    /// what a snapshot carries.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on an iteration failure.
    pub fn committed_entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut entries = Vec::new();
        for prefix in committed_prefixes() {
            entries.extend(self.scan_prefix(prefix)?);
        }
        entries.sort();
        Ok(entries)
    }

    /// Clears a pruned bootstrap that was interrupted before it adopted its
    /// snapshot, and says whether it did.
    ///
    /// [`bootstrap_pruned`] writes the chain's metadata (tip = genesis) before
    /// it downloads anything, and undoes itself on an *error*, but not when the
    /// process is killed: a PC rebooted or switched off during a first sync.
    /// That database then looks like a chain at height 0, a restart skipped
    /// the bootstrap ("the database already holds a chain"), and catch-up from
    /// epoch 0 failed on every restart, forever (seen 2026-10-12). Tip still
    /// at genesis with nothing pruned means no snapshot was adopted, so there
    /// is nothing to lose; any database past that is left alone.
    ///
    /// # Errors
    ///
    /// Storage failures reading the metadata or deleting the keys.
    pub fn discard_interrupted_bootstrap(&self) -> Result<bool> {
        let interrupted = self
            .chain_meta()?
            .is_some_and(|meta| meta.tip == meta.genesis && meta.prune_horizon == 0);
        if interrupted {
            self.purge_all()?;
        }
        Ok(interrupted)
    }

    /// Deletes every key. What a failed bootstrap, which started from an
    /// empty database, is undone with.
    fn purge_all(&self) -> Result<()> {
        let mut batch = WriteBatch::default();
        for (key, _) in self.scan_prefix(b"")? {
            batch.delete(key);
        }
        self.write_batch(batch)
    }
}

/// Where a pruned node gets its headers, its snapshot and its recent bodies.
///
/// Every call is untrusted: the bootstrap validates whatever comes back.
/// `Sync` because block bodies are fetched with several requests in flight
/// ([`super::prefetch`]).
pub trait BootstrapSource: Sync {
    /// Height of the source's tip.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn tip_height(&self) -> Result<u64>;

    /// Headers at heights `from..=to` on the source's active chain.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn headers(&self, from: u64, to: u64) -> Result<Vec<BlockHeader>>;

    /// The snapshot the source is serving.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source, or no snapshot deep enough.
    fn snapshot_manifest(&self) -> Result<SnapshotManifest>;

    /// One chunk of that snapshot.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn snapshot_chunk(&self, index: usize) -> Result<Vec<u8>>;

    /// The active-chain block at `height`.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn block(&self, height: u64) -> Result<Block>;
}

/// Downloads every header and validates it into `chain`, returning the ids by
/// height (index 0 is genesis).
fn sync_headers(
    chain: &mut Chain,
    source: &dyn BootstrapSource,
    tip: u64,
) -> Result<Vec<[u8; 32]>> {
    let mut ids = vec![chain.genesis()];
    let mut from = 1;
    while from <= tip {
        let to = tip.min(from + HEADER_BATCH - 1);
        let headers = source.headers(from, to)?;
        if headers.len() as u64 != to - from + 1 {
            return Err(NodeError::Network(format!(
                "asked for headers {from}..={to}, got {}",
                headers.len()
            )));
        }
        let imported = chain.import_headers(&headers)?;
        // import_headers only needs each parent to be known, so a source could
        // repeat or skip headers and still hand back the right count. Heights
        // are positions in this vector, so it has to be a chain by position.
        let mut parent = *ids.last().unwrap_or(&[0; 32]);
        for (offset, (header, id)) in headers.iter().zip(&imported).enumerate() {
            if header.prev_hash != parent {
                return Err(NodeError::Network(format!(
                    "headers do not chain: the header served for height {} does not extend                      the one before it",
                    from + offset as u64
                )));
            }
            parent = *id;
        }
        ids.extend(imported);
        from = to + 1;
    }
    Ok(ids)
}

/// Downloads, checks and imports the snapshot, then requires its root to be
/// the one the header at its height committed to.
fn import_snapshot(
    state: &StateDB,
    source: &dyn BootstrapSource,
    manifest: &SnapshotManifest,
    expected_root: &[u8; 32],
) -> Result<()> {
    let mut previous: Option<Vec<u8>> = None;
    for (index, expected_hash) in manifest.chunks.iter().enumerate() {
        let chunk = source.snapshot_chunk(index)?;
        if chunk.len() > MAX_CHUNK_BYTES {
            return Err(NodeError::Network(format!(
                "snapshot chunk {index} is {} bytes, over the {MAX_CHUNK_BYTES}-byte cap",
                chunk.len()
            )));
        }
        if chunk_hash(&chunk) != *expected_hash {
            return Err(NodeError::Network(format!(
                "snapshot chunk {index} does not match its manifest hash"
            )));
        }
        let mut batch = WriteBatch::default();
        for (key, value) in decode_chunk(&chunk)? {
            // Only state the root covers. A key under anything else would go
            // unverified, and a key out of order could overwrite one already
            // imported.
            if !is_committed(&key) {
                return Err(NodeError::Network(format!(
                    "snapshot key {} is outside the state root",
                    hex::encode(&key)
                )));
            }
            if previous.as_ref().is_some_and(|last| key <= *last) {
                return Err(NodeError::Network(
                    "snapshot keys are not strictly increasing".to_string(),
                ));
            }
            batch.put(&key, &value);
            previous = Some(key);
        }
        state.write_batch(batch)?;
    }

    let actual = state.state_root()?;
    if actual != *expected_root {
        return Err(NodeError::StateRootMismatch {
            expected: hex::encode(expected_root),
            actual: hex::encode(actual),
        });
    }
    Ok(())
}

/// Bootstraps a pruned node into the empty database `state`.
///
/// Downloads and validates every header, imports a snapshot at least
/// `min_depth` below the source's tip and checks it against its header's
/// state root, then fetches and fully validates only the bodies above it.
/// The resulting chain's prune horizon is the snapshot height: it holds no
/// body, and no undo journal, below it.
///
/// # Errors
///
/// - [`NodeError::Storage`] if `state` is not empty.
/// - [`NodeError::Network`] for a snapshot that is too shallow, off the best
///   header chain, or inconsistent with its manifest.
/// - [`NodeError::StateRootMismatch`] for a snapshot that does not reproduce
///   its header's root. Everything it imported is deleted first.
/// - Anything header or block validation refuses.
pub fn bootstrap_pruned(
    state: Arc<StateDB>,
    genesis: Block,
    config: ChainConfig,
    source: &dyn BootstrapSource,
    min_depth: u64,
) -> Result<Chain> {
    bootstrap_pruned_anchored(state, genesis, config, source, min_depth, None)
}

/// A block the operator vouches for, learned from somewhere other than the
/// bootstrap source (a second node, an explorer, a person).
///
/// Why it exists: a bootstrap checks that the source's headers link and meet
/// the proof-of-work target, and that the snapshot reproduces a header's state
/// root. On a DAG-BFT chain the target is near zero, so nothing in that costs
/// a lying source anything: it can serve a self-consistent chain that no
/// committee ever signed. Checking one block id against an independent source
/// is the weak-subjectivity checkpoint proof-of-stake chains use for the same
/// reason; it turns "trust the source for everything" into "trust the source
/// for nothing the operator already knows".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustedBlock {
    /// Height of the block.
    pub height: u64,
    /// Its id (the header hash).
    pub id: [u8; 32],
}

/// As [`bootstrap_pruned`], additionally refusing a source whose best chain
/// does not contain `anchor` (when given).
///
/// # Errors
///
/// As [`bootstrap_pruned`]; plus [`NodeError::Network`] when the trusted block
/// is above the source's tip or is not the source's block at that height. The
/// database is purged in that case, like any failed bootstrap.
pub fn bootstrap_pruned_anchored(
    state: Arc<StateDB>,
    genesis: Block,
    config: ChainConfig,
    source: &dyn BootstrapSource,
    min_depth: u64,
    anchor: Option<TrustedBlock>,
) -> Result<Chain> {
    if !state.scan_prefix(b"")?.is_empty() {
        return Err(NodeError::Storage(
            "a pruned bootstrap needs an empty database".to_string(),
        ));
    }
    let chain = Chain::open(Arc::clone(&state), genesis, config)?;
    match bootstrap_into(chain, &state, source, min_depth, anchor) {
        Ok(chain) => Ok(chain),
        Err(error) => {
            // The database was empty when this started, so wiping it is exact:
            // an operator retries with the same directory, and a half-imported
            // snapshot is never mistaken for a chain.
            state.purge_all()?;
            Err(error)
        }
    }
}

/// The body of [`bootstrap_pruned`], past the point where it has written.
fn bootstrap_into(
    mut chain: Chain,
    state: &StateDB,
    source: &dyn BootstrapSource,
    min_depth: u64,
    anchor: Option<TrustedBlock>,
) -> Result<Chain> {
    let tip = source.tip_height()?;
    let ids = sync_headers(&mut chain, source, tip)?;
    if let Some(trusted) = anchor {
        // Height 0 is the genesis this node already has: it proves nothing
        // about the source.
        let theirs = usize::try_from(trusted.height)
            .ok()
            .filter(|h| *h > 0)
            .and_then(|h| ids.get(h));
        if theirs != Some(&trusted.id) {
            return Err(NodeError::Network(format!(
                "the source's chain does not contain the trusted block {} at height {}                  (its tip is {tip}; height 0 and heights above the tip never match):                  refusing to bootstrap from it",
                hex::encode(trusted.id),
                trusted.height
            )));
        }
    }

    let manifest = source.snapshot_manifest()?;
    let height = manifest.height;
    if height == 0 || height.saturating_add(min_depth) > tip {
        return Err(NodeError::Network(format!(
            "snapshot at height {height} is not {min_depth} blocks below tip {tip}"
        )));
    }
    // Block ids commit to their ancestors, so the anchor vouches only for the
    // chain up to its own height. A snapshot above it sits on headers the
    // anchor says nothing about.
    if let Some(trusted) = anchor
        && trusted.height < height
    {
        return Err(NodeError::Network(format!(
            "the trusted block is at height {}, below the snapshot at {height}: it cannot              vouch for the chain the snapshot sits on (give a newer trusted block)",
            trusted.height
        )));
    }
    let snapshot_id = ids[height as usize];
    if manifest.block_id != snapshot_id {
        return Err(NodeError::Network(format!(
            "snapshot follows {}, which is not the best chain's block at height {height}",
            hex::encode(manifest.block_id)
        )));
    }
    let expected_root = chain
        .get(&snapshot_id)
        .map(|record| record.header.state_root)
        .ok_or_else(|| NodeError::Network("snapshot header missing".to_string()))?;

    import_snapshot(state, source, &manifest, &expected_root)?;

    chain.adopt_snapshot(&ids[..=height as usize])?;
    // Fetched a window at a time with several requests in flight, inserted
    // strictly in order: every body is still fully validated.
    let mut next = height + 1;
    super::prefetch::for_each_prefetched(height + 1, tip, &|h| source.block(h), |block| {
        // Each body must extend the header chain already verified, so a source
        // cannot swap in a different fork above the snapshot; the tip's own id
        // is checked once (computing an id costs a proof-of-work hash).
        let verified_parent = ids[next as usize - 1];
        if block.header.prev_hash != verified_parent
            || (next == tip && block.header.id() != ids[next as usize])
        {
            return Err(NodeError::Network(format!(
                "the body served for height {next} is not the block its header chain names"
            )));
        }
        next += 1;
        chain.insert_block(block).map(|_| ())
    })?;
    Ok(chain)
}

/// Snapshots at rest, one `RocksDB` checkpoint per height.
pub struct Snapshots {
    dir: PathBuf,
}

/// Name of the file recording which block a checkpoint follows.
const MARKER: &str = "SNAPSHOT_BLOCK";

impl Snapshots {
    /// Snapshots kept under `dir`, created if absent.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the directory cannot be created.
    pub fn new(dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(dir.as_ref()).map_err(|e| NodeError::Storage(e.to_string()))?;
        Ok(Self {
            dir: dir.as_ref().to_path_buf(),
        })
    }

    /// Checkpoints `state` as the post-state of block `block_id` at `height`.
    ///
    /// The caller holds the chain lock, so the checkpoint and the tip it is
    /// labelled with cannot disagree.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the checkpoint cannot be written.
    pub fn take(&self, state: &StateDB, height: u64, block_id: &[u8; 32]) -> Result<()> {
        let path = self.dir.join(height.to_string());
        if path.exists() {
            return Ok(());
        }
        state.create_checkpoint(&path)?;
        fs::write(path.join(MARKER), hex::encode(block_id))
            .map_err(|e| NodeError::Storage(e.to_string()))
    }

    /// Heights with a checkpoint, ascending.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the directory cannot be read.
    pub fn heights(&self) -> Result<Vec<u64>> {
        let mut heights: Vec<u64> = fs::read_dir(&self.dir)
            .map_err(|e| NodeError::Storage(e.to_string()))?
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .filter(|height: &u64| self.dir.join(height.to_string()).join(MARKER).exists())
            .collect();
        heights.sort_unstable();
        Ok(heights)
    }

    /// Deletes every checkpoint but the newest `keep`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if a directory cannot be removed.
    pub fn retain_newest(&self, keep: usize) -> Result<()> {
        let heights = self.heights()?;
        let excess = heights.len().saturating_sub(keep);
        for height in &heights[..excess] {
            fs::remove_dir_all(self.dir.join(height.to_string()))
                .map_err(|e| NodeError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    /// The newest checkpoint at least `min_depth` below `tip`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the directory cannot be read.
    pub fn serveable(&self, tip: u64, min_depth: u64) -> Result<Option<u64>> {
        Ok(self
            .heights()?
            .into_iter()
            .rev()
            .find(|height| height.saturating_add(min_depth) <= tip))
    }

    /// The manifest and chunks of the checkpoint at `height`.
    ///
    /// Opens the checkpoint as its own database, so the live one is not
    /// touched and the served state cannot move while it is read.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the checkpoint cannot be opened.
    pub fn load(&self, height: u64) -> Result<(SnapshotManifest, Vec<Vec<u8>>)> {
        let path = self.dir.join(height.to_string());
        let marker =
            fs::read_to_string(path.join(MARKER)).map_err(|e| NodeError::Storage(e.to_string()))?;
        let block_id: [u8; 32] = hex::decode(marker.trim())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| NodeError::Storage(format!("bad marker in snapshot {height}")))?;

        let checkpoint = StateDB::open(&path)?;
        let entries = checkpoint.committed_entries()?;
        let chunks = encode_chunks(&entries)?;
        let manifest = SnapshotManifest {
            height,
            block_id,
            chunks: chunks.iter().map(|chunk| chunk_hash(chunk)).collect(),
        };
        Ok((manifest, chunks))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::state::account::Account;
    use crate::state::db::nullifier_key_bytes;

    /// A source serving fixed chunks, for driving `import_snapshot` directly.
    struct Chunks(Vec<Vec<u8>>);

    impl BootstrapSource for Chunks {
        fn tip_height(&self) -> Result<u64> {
            unreachable!("not used by import_snapshot")
        }
        fn headers(&self, _: u64, _: u64) -> Result<Vec<BlockHeader>> {
            unreachable!("not used by import_snapshot")
        }
        fn snapshot_manifest(&self) -> Result<SnapshotManifest> {
            unreachable!("not used by import_snapshot")
        }
        fn snapshot_chunk(&self, index: usize) -> Result<Vec<u8>> {
            Ok(self.0[index].clone())
        }
        fn block(&self, _: u64) -> Result<Block> {
            unreachable!("not used by import_snapshot")
        }
    }

    fn import(
        entries: &[(Vec<u8>, Vec<u8>)],
        root: &[u8; 32],
    ) -> (Result<()>, StateDB, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("dir");
        let target = StateDB::open(dir.path()).expect("open");
        let chunks: Vec<Vec<u8>> = entries.chunks(2).map(encode_chunk).collect();
        let manifest = SnapshotManifest {
            height: 1,
            block_id: [0; 32],
            chunks: chunks.iter().map(|c| chunk_hash(c)).collect(),
        };
        let result = import_snapshot(&target, &Chunks(chunks), &manifest, root);
        (result, target, dir)
    }

    #[test]
    fn a_snapshot_that_drops_a_nullifier_is_refused() {
        // The double-spend a pruned node would otherwise accept: every chunk
        // consistent with its manifest, one spent note left out. Before the
        // nullifier layer existed this imported cleanly.
        let dir = tempfile::tempdir().expect("dir");
        let source = StateDB::open(dir.path()).expect("open");
        source
            .put_account(
                &[1; 32],
                &Account {
                    balance: primitive_types::U256::from(9),
                    nonce: 0,
                },
            )
            .expect("put");
        source
            .raw_put(&nullifier_key_bytes(&[5; 32]), &[])
            .expect("put");
        let root = source.state_root().expect("root");
        let honest = source.committed_entries().expect("entries");

        let (result, _, _dir) = import(&honest, &root);
        assert!(result.is_ok(), "the honest snapshot must import");

        let dropped: Vec<_> = honest
            .into_iter()
            .filter(|(key, _)| !key.starts_with(b"null:"))
            .collect();
        let (result, _, _dir) = import(&dropped, &root);
        assert!(matches!(result, Err(NodeError::StateRootMismatch { .. })));
    }

    #[test]
    fn a_key_outside_the_root_or_out_of_order_is_refused() {
        let dir = tempfile::tempdir().expect("dir");
        let source = StateDB::open(dir.path()).expect("open");
        let root = source.state_root().expect("root");

        let (result, _, _dir) = import(&[(b"undo:x".to_vec(), vec![1])], &root);
        assert!(
            matches!(result, Err(NodeError::Network(_))),
            "a local-only key was imported"
        );

        let unordered = vec![
            (b"acct:b".to_vec(), vec![0; 16]),
            (b"acct:a".to_vec(), vec![0; 16]),
        ];
        let (result, _, _dir) = import(&unordered, &root);
        assert!(
            matches!(result, Err(NodeError::Network(_))),
            "out-of-order keys were imported"
        );
    }

    #[test]
    fn a_chunk_over_the_byte_cap_is_refused_before_it_is_decoded() {
        // Every entry is valid and the manifest hash matches, so only the cap
        // can refuse it: two 9 MiB values come to 18 MiB, over the ~16 MiB cap.
        let big = vec![(b"acct:a".to_vec(), vec![0xab; 9 * 1024 * 1024]); 2];
        let chunk = encode_chunk(&big);
        assert!(chunk.len() > MAX_CHUNK_BYTES);
        let manifest = SnapshotManifest {
            height: 1,
            block_id: [0; 32],
            chunks: vec![chunk_hash(&chunk)],
        };
        let dir = tempfile::tempdir().expect("dir");
        let target = StateDB::open(dir.path()).expect("open");
        let result = import_snapshot(&target, &Chunks(vec![chunk]), &manifest, &[0; 32]);
        assert!(
            matches!(&result, Err(NodeError::Network(msg)) if msg.contains("cap")),
            "an oversized chunk was not refused by the cap: {result:?}"
        );
    }

    #[test]
    fn a_manifest_and_a_chunk_round_trip() {
        let manifest = SnapshotManifest {
            height: 42,
            block_id: [7; 32],
            chunks: vec![[1; 32], [2; 32]],
        };
        assert_eq!(SnapshotManifest::decode(&manifest.encode()), Ok(manifest));

        let entries = vec![(b"acct:a".to_vec(), vec![1, 2]), (b"d:x".to_vec(), vec![])];
        assert_eq!(decode_chunk(&encode_chunk(&entries)), Ok(entries));
        assert!(decode_chunk(&[0xff; 9]).is_err());
    }

    #[test]
    fn a_value_past_the_old_collection_cap_still_round_trips() {
        // The staking record is one value; a busy chain's passes 65,536 bytes.
        let entries = vec![(b"stk".to_vec(), vec![0xaa; 200_000])];
        assert_eq!(decode_chunk(&encode_chunk(&entries)), Ok(entries));
    }

    #[test]
    fn chunks_close_at_the_byte_cap_and_an_entry_that_fits_alone_is_served_alone() {
        // 6 MiB values: two fit a chunk, a third would not.
        let value = vec![0x5a; 6 * 1024 * 1024];
        let entries: Vec<_> = (0u8..5)
            .map(|i| (vec![b'c', b'o', b'd', b'e', b':', i], value.clone()))
            .collect();
        let chunks = encode_chunks(&entries).expect("chunks");
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() <= MAX_CHUNK_BYTES));
        let back: Vec<_> = chunks
            .iter()
            .flat_map(|c| decode_chunk(c).expect("decode"))
            .collect();
        assert_eq!(back, entries);

        // The largest single entry a chunk can hold is still served, alone.
        let fits = MAX_CHUNK_BYTES - CHUNK_HEADER_BYTES - ENTRY_OVERHEAD_BYTES - 6;
        let edge = vec![
            (b"code:a".to_vec(), vec![1]),
            (b"code:b".to_vec(), vec![2; fits]),
        ];
        let chunks = encode_chunks(&edge).expect("an entry that fits alone");
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].len(), MAX_CHUNK_BYTES);

        // One byte more cannot be served by anyone: refused, not shipped.
        let over = vec![(b"code:b".to_vec(), vec![2; fits + 1])];
        assert!(
            matches!(encode_chunks(&over), Err(NodeError::Storage(m)) if m.contains("chunk cap"))
        );
    }

    #[test]
    fn chunks_still_close_at_the_entry_count() {
        let entries: Vec<_> = (0..=CHUNK_ENTRIES)
            .map(|i| (i.to_be_bytes().to_vec(), Vec::new()))
            .collect();
        let chunks = encode_chunks(&entries).expect("chunks");
        assert_eq!(chunks.len(), 2);
        assert_eq!(decode_chunk(&chunks[1]).expect("decode").len(), 1);
        assert!(encode_chunks(&[]).expect("empty").is_empty());
    }

    /// A sparse state whose root cache is built, after `blocks` commits.
    fn sparse_source(blocks: u8) -> (StateDB, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("dir");
        let source = StateDB::open(dir.path()).expect("open");
        source
            .raw_put(crate::state::stateless::SPARSE_ACCOUNTS_KEY, &[1])
            .expect("mark");
        for block in 0..blocks {
            commit_accounts(&source, block);
        }
        (source, dir)
    }

    /// Commits a block of four accounts derived from `block`, through the
    /// preview and the journaled commit a chain uses.
    fn commit_accounts(state: &StateDB, block: u8) {
        let mut overlay = crate::state::db::Overlay::new();
        for i in 0..4_u8 {
            overlay.accounts.insert(
                *blake3::hash(&[block, i]).as_bytes(),
                Account {
                    balance: primitive_types::U256::from(u64::from(block) + 1),
                    nonce: u64::from(i),
                },
            );
        }
        state.root_with_overlay(&mut overlay).expect("preview");
        state
            .commit_journaled(&overlay, &[block; 32], |_| ())
            .expect("commit");
    }

    #[test]
    fn a_snapshot_carries_no_root_cache_and_the_importer_builds_its_own() {
        let (source, _source_dir) = sparse_source(6);
        assert!(
            crate::state::root_cache::marker(source.raw_db())
                .unwrap()
                .is_some()
        );
        let entries = source.committed_entries().expect("entries");
        assert!(
            entries
                .iter()
                .all(|(key, _)| !key.starts_with(crate::state::root_cache::ROOT_CACHE_PREFIX)),
            "the cache is local-only (invariant 25)"
        );

        let (result, target, _dir) = import(&entries, &source.state_root().expect("root"));
        result.expect("import");
        assert!(
            crate::state::root_cache::marker(target.raw_db())
                .unwrap()
                .is_none()
        );

        // The importer's next block builds its cache from what it imported,
        // and the cache then walks like any other.
        for block in 6..9 {
            commit_accounts(&source, block);
            commit_accounts(&target, block);
            assert_eq!(target.state_root().unwrap(), source.state_root().unwrap());
            assert!(target.verify_root_cache().expect("verify"));
        }
        let stats = target.root_cache_stats();
        assert_eq!((stats.built, stats.walked, stats.repaired), (1, 2, 0));
    }

    #[test]
    fn purging_a_failed_bootstrap_clears_the_root_cache() {
        let (state, _dir) = sparse_source(3);
        assert!(
            !state
                .scan_prefix(crate::state::root_cache::ROOT_CACHE_PREFIX)
                .unwrap()
                .is_empty()
        );
        state.purge_all().expect("purge");
        assert!(state.scan_prefix(b"").unwrap().is_empty());
        assert!(
            crate::state::root_cache::marker(state.raw_db())
                .unwrap()
                .is_none()
        );
    }
}
