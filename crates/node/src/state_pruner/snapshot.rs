//! State snapshots, and bootstrapping a pruned node from one.
//!
//! # What a new pruned node downloads
//!
//! 1. **Every header**, genesis to tip. 144 bytes each, fully validated: the
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
//! by every full node since the tx_root fix. A server that lies about one
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
//! [`Snapshots::take`] makes a RocksDB checkpoint (hard links, cheap) at the
//! current tip. [`Snapshots::serveable`] serves the newest one at least K deep,
//! so what a new node adopts is below the depth a pruned node treats as final.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rocksdb::WriteBatch;

use crate::consensus::{Chain, ChainConfig};
use crate::core::codec::ByteReader;
use crate::core::{Block, BlockHeader};
use crate::error::{NodeError, Result};
use crate::state::commitments::{committed_prefixes, is_committed};
use crate::state::db::StateDB;

/// Entries per snapshot chunk.
pub const CHUNK_ENTRIES: usize = 4_096;

/// Headers asked for per request during bootstrap.
pub const HEADER_BATCH: u64 = 2_000;

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
        let key_len = reader.read_collection_len(1)?;
        let key = reader.read_slice(key_len)?.to_vec();
        let value_len = reader.read_collection_len(1)?;
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
pub trait BootstrapSource {
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
        ids.extend(chain.import_headers(&headers)?);
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
    if !state.scan_prefix(b"")?.is_empty() {
        return Err(NodeError::Storage(
            "a pruned bootstrap needs an empty database".to_string(),
        ));
    }
    let chain = Chain::open(Arc::clone(&state), genesis, config)?;
    match bootstrap_into(chain, &state, source, min_depth) {
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
) -> Result<Chain> {
    let tip = source.tip_height()?;
    let ids = sync_headers(&mut chain, source, tip)?;

    let manifest = source.snapshot_manifest()?;
    let height = manifest.height;
    if height == 0 || height.saturating_add(min_depth) > tip {
        return Err(NodeError::Network(format!(
            "snapshot at height {height} is not {min_depth} blocks below tip {tip}"
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
    for body_height in height + 1..=tip {
        chain.insert_block(source.block(body_height)?)?;
    }
    Ok(chain)
}

/// Snapshots at rest, one RocksDB checkpoint per height.
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
        let chunks: Vec<Vec<u8>> = entries.chunks(CHUNK_ENTRIES).map(encode_chunk).collect();
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
                    balance: 9,
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
}
