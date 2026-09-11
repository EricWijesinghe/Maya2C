//! Content-addressed archives of historical block batches.
//!
//! A node that prunes old block bodies (`custom-l1-node`'s `state_pruner`)
//! first writes them here: a batch of blocks becomes one CAR v1 file whose
//! root is a manifest linking every block by CID. That file can live in a
//! local directory, on an IPFS node, or behind an Arweave gateway, and it
//! reads back the same everywhere, because a CID names the bytes and not
//! where they are.
//!
//! # Trust
//!
//! None of those places is trusted. [`open_archive`] checks every section
//! against its CID and the manifest against the sections, so a store can
//! refuse to answer but cannot substitute bytes. What this crate cannot check
//! is that the bytes are the *right block*: it does not know what a block is.
//! The node does that by requiring each decoded block's header id to match the
//! header it kept, and its `tx_root` to match its transactions.
//!
//! # Compression
//!
//! Files at rest are zstd-compressed CAR (`.car.zst`). IPFS imports plain CAR,
//! so [`kubo::KuboStore`] sends it uncompressed. Every CID is over the
//! uncompressed block bytes, so an archive has one root CID on every backend.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod arweave;
pub mod car;
pub mod error;
pub mod kubo;
pub mod manifest;
pub mod store;

use std::io::Read;

/// Re-exported so a caller can parse a stored root without depending on `cid`.
pub use cid::Cid;

pub use crate::error::{ArchiveError, Result};
pub use crate::manifest::{Manifest, ManifestEntry};
pub use crate::store::{ArchiveStore, LocalDirStore, Locator};

use crate::car::{DAG_CBOR_CODEC, RAW_CODEC, cid_of, read_car, write_car};

/// Largest uncompressed archive this crate will produce or accept.
///
/// A bound on a hostile archive, not a target: at the default batch of 1,000
/// blocks it leaves room for about 500 KiB per block.
pub const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;

/// zstd level for archives at rest. Written once and read rarely, so a higher
/// level than a stream would use.
const COMPRESSION_LEVEL: i32 = 12;

/// One block going into an archive, or coming out of one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchivedBlock {
    /// Height of the block.
    pub height: u64,
    /// The block's id.
    pub id: [u8; 32],
    /// The block's wire encoding.
    pub bytes: Vec<u8>,
}

/// A built archive: the CAR bytes, uncompressed, and what they hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive {
    /// CID of the manifest, and the CAR's single root.
    pub root: Cid,
    /// What the archive holds.
    pub manifest: Manifest,
    /// The CAR v1 file, uncompressed.
    pub car: Vec<u8>,
}

/// Builds an archive of `blocks`, which must be contiguous in height.
///
/// # Errors
///
/// Returns [`ArchiveError::Manifest`] for an empty batch or a gap in heights,
/// and [`ArchiveError::TooLarge`] if the result would exceed
/// [`MAX_ARCHIVE_BYTES`].
pub fn build_archive(chain_id: &str, blocks: &[ArchivedBlock]) -> Result<Archive> {
    let first_height = blocks
        .first()
        .map(|block| block.height)
        .ok_or_else(|| ArchiveError::Manifest("an archive needs at least one block".into()))?;
    let entries: Vec<ManifestEntry> = blocks
        .iter()
        .map(|block| ManifestEntry {
            height: block.height,
            id: block.id,
            cid: cid_of(RAW_CODEC, &block.bytes),
        })
        .collect();
    let manifest = Manifest {
        chain_id: chain_id.to_string(),
        first_height,
        entries,
    };
    let manifest_bytes = manifest.encode();
    // Decoding what was just encoded is the contiguity check, in one place.
    Manifest::decode(&manifest_bytes)?;

    let root = cid_of(DAG_CBOR_CODEC, &manifest_bytes);
    let mut sections: Vec<(Cid, &[u8])> = vec![(root, &manifest_bytes)];
    sections.extend(
        manifest
            .entries
            .iter()
            .zip(blocks)
            .map(|(entry, block)| (entry.cid, block.bytes.as_slice())),
    );
    let car = write_car(&root, &sections);
    if car.len() > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::TooLarge {
            limit: MAX_ARCHIVE_BYTES,
        });
    }
    Ok(Archive {
        root,
        manifest,
        car,
    })
}

/// Opens an uncompressed CAR and returns its blocks in height order, each one
/// verified by CID.
///
/// Checks, in order:
/// - every section hashes to its CID;
/// - the root is a manifest for `expected_root`;
/// - every block the manifest lists is present;
/// - nothing else is present.
///
/// A section the manifest does not link is refused: this crate never wrote
/// one, so the archive came from somewhere else.
///
/// # Errors
///
/// Any [`ArchiveError`]; each one means the archive is not usable.
pub fn open_archive(car: &[u8], expected_root: &Cid) -> Result<Vec<ArchivedBlock>> {
    let mut parsed = read_car(car)?;
    if parsed.root != *expected_root {
        return Err(ArchiveError::WrongRoot {
            expected: expected_root.to_string(),
            actual: parsed.root.to_string(),
        });
    }
    let manifest_bytes = parsed
        .sections
        .remove(&parsed.root)
        .ok_or_else(|| ArchiveError::Manifest("the root section is missing".into()))?;
    if parsed.root.codec() != DAG_CBOR_CODEC {
        return Err(ArchiveError::Manifest("the root is not DAG-CBOR".into()));
    }
    let manifest = Manifest::decode(&manifest_bytes)?;

    let mut blocks = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let bytes = parsed.sections.remove(&entry.cid).ok_or_else(|| {
            ArchiveError::Manifest(format!("block at height {} is missing", entry.height))
        })?;
        blocks.push(ArchivedBlock {
            height: entry.height,
            id: entry.id,
            bytes,
        });
    }
    if let Some(extra) = parsed.sections.keys().next() {
        return Err(ArchiveError::Manifest(format!(
            "section {extra} is not linked by the manifest"
        )));
    }
    Ok(blocks)
}

/// Compresses a CAR for storage at rest.
///
/// # Errors
///
/// Returns [`ArchiveError::Io`] if the encoder fails.
pub fn compress(car: &[u8]) -> Result<Vec<u8>> {
    Ok(zstd::encode_all(car, COMPRESSION_LEVEL)?)
}

/// Decompresses an archive, refusing to produce more than
/// [`MAX_ARCHIVE_BYTES`].
///
/// Bounded because the input is untrusted: a few kilobytes of zstd can claim
/// gigabytes.
///
/// # Errors
///
/// Returns [`ArchiveError::TooLarge`] past the bound, and
/// [`ArchiveError::Io`] for a stream that is not zstd.
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    read_capped(zstd::stream::read::Decoder::new(bytes)?)
}

/// Reads `reader` to the end, refusing to hold more than
/// [`MAX_ARCHIVE_BYTES`].
///
/// Every untrusted body goes through this, whether a zstd stream or an HTTP
/// response, so the cap binds while reading. A check after a full read would
/// already have buffered whatever a hostile store chose to send.
pub(crate) fn read_capped(reader: impl Read) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    reader
        .take(MAX_ARCHIVE_BYTES as u64 + 1)
        .read_to_end(&mut out)?;
    if out.len() > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::TooLarge {
            limit: MAX_ARCHIVE_BYTES,
        });
    }
    Ok(out)
}

/// Whether `bytes` start with the zstd frame magic.
#[must_use]
pub fn is_zstd(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x28, 0xB5, 0x2F, 0xFD])
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn batch(first: u64, count: u64) -> Vec<ArchivedBlock> {
        (first..first + count)
            .map(|height| ArchivedBlock {
                height,
                id: [height as u8; 32],
                bytes: format!("block {height}").repeat(10).into_bytes(),
            })
            .collect()
    }

    #[test]
    fn an_archive_opens_to_exactly_the_blocks_it_was_built_from() {
        let blocks = batch(100, 5);
        let archive = build_archive("maya-test", &blocks).expect("build");
        assert_eq!(
            open_archive(&archive.car, &archive.root).expect("open"),
            blocks
        );
    }

    #[test]
    fn the_same_batch_always_has_the_same_root() {
        let a = build_archive("maya-test", &batch(1, 3)).expect("build");
        let b = build_archive("maya-test", &batch(1, 3)).expect("build");
        assert_eq!(a.root, b.root);
        assert_eq!(a.car, b.car);
    }

    #[test]
    fn an_archive_opened_against_another_root_is_refused() {
        let archive = build_archive("maya-test", &batch(1, 2)).expect("build");
        let other = build_archive("maya-test", &batch(5, 2)).expect("build");
        assert!(matches!(
            open_archive(&archive.car, &other.root),
            Err(ArchiveError::WrongRoot { .. })
        ));
    }

    #[test]
    fn a_missing_block_or_an_unlinked_section_is_refused() {
        let blocks = batch(1, 3);
        let archive = build_archive("maya-test", &blocks).expect("build");
        let manifest_bytes = archive.manifest.encode();

        // Drop the last block.
        let short = write_car(
            &archive.root,
            &[
                (archive.root, manifest_bytes.as_slice()),
                (archive.manifest.entries[0].cid, blocks[0].bytes.as_slice()),
                (archive.manifest.entries[1].cid, blocks[1].bytes.as_slice()),
            ],
        );
        assert!(open_archive(&short, &archive.root).is_err());

        // Add a section nobody linked.
        let stray = cid_of(RAW_CODEC, b"stray");
        let mut sections: Vec<(Cid, &[u8])> = vec![(archive.root, &manifest_bytes)];
        for (entry, block) in archive.manifest.entries.iter().zip(&blocks) {
            sections.push((entry.cid, &block.bytes));
        }
        sections.push((stray, b"stray"));
        let padded = write_car(&archive.root, &sections);
        assert!(open_archive(&padded, &archive.root).is_err());
    }

    #[test]
    fn an_empty_or_gapped_batch_is_refused() {
        assert!(build_archive("maya-test", &[]).is_err());
        let mut gapped = batch(1, 3);
        gapped[2].height = 9;
        assert!(build_archive("maya-test", &gapped).is_err());
    }

    #[test]
    fn compression_round_trips_and_is_recognised() {
        let archive = build_archive("maya-test", &batch(1, 50)).expect("build");
        let packed = compress(&archive.car).expect("compress");
        assert!(is_zstd(&packed));
        assert!(!is_zstd(&archive.car));
        assert!(packed.len() < archive.car.len());
        assert_eq!(decompress(&packed).expect("decompress"), archive.car);
        assert!(decompress(b"definitely not zstd").is_err());
    }
}
