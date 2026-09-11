//! Where archives live.
//!
//! A store is untrusted. It may lose an archive or refuse to answer, but it
//! cannot substitute bytes without failing [`crate::open_archive`]. So the
//! uploader writes to every configured store and reads each copy back before
//! the node deletes anything, and the fetcher tries stores in turn until one
//! returns an archive that verifies.

use std::fs;
use std::path::{Path, PathBuf};

use cid::Cid;

use crate::error::{ArchiveError, Result};
use crate::{Archive, compress, decompress, is_zstd};

/// Where one copy of an archive can be fetched from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locator {
    /// Which kind of store: `local`, `ipfs` or `arweave`.
    pub kind: String,
    /// The store's own name for it: a file name, a CID, a transaction id.
    pub reference: String,
}

impl Locator {
    /// `kind:reference`, the form a receipt stores.
    #[must_use]
    pub fn encode(&self) -> String {
        format!("{}:{}", self.kind, self.reference)
    }

    /// Parses [`Locator::encode`]'s output.
    ///
    /// # Errors
    ///
    /// Returns [`ArchiveError::Unsupported`] if there is no `:`.
    pub fn decode(text: &str) -> Result<Self> {
        let (kind, reference) = text
            .split_once(':')
            .ok_or_else(|| ArchiveError::Unsupported(format!("locator {text:?} has no kind")))?;
        Ok(Self {
            kind: kind.to_string(),
            reference: reference.to_string(),
        })
    }
}

/// Somewhere archives can be written to and read from.
pub trait ArchiveStore: Send + Sync {
    /// The locator kind this store produces and reads.
    fn kind(&self) -> &'static str;

    /// Writes `archive` and returns where to find it.
    ///
    /// # Errors
    ///
    /// Any [`ArchiveError`] the backend reports.
    fn put(&self, archive: &Archive) -> Result<Locator>;

    /// Reads back the uncompressed CAR that `locator` names.
    ///
    /// The bytes are not verified here; pass them to [`crate::open_archive`].
    ///
    /// # Errors
    ///
    /// Any [`ArchiveError`] the backend reports, or
    /// [`ArchiveError::Unsupported`] for a locator of another kind.
    fn get(&self, locator: &Locator, root: &Cid) -> Result<Vec<u8>>;
}

/// Archives as `<root CID>.car.zst` files in one directory.
///
/// The operator's own copy, and what the node falls back to first when a
/// pruned block is asked for: a local read, and no third party to reach.
#[derive(Clone, Debug)]
pub struct LocalDirStore {
    dir: PathBuf,
}

impl LocalDirStore {
    /// A store rooted at `dir`, created if absent.
    ///
    /// # Errors
    ///
    /// Returns [`ArchiveError::Io`] if the directory cannot be created.
    pub fn new(dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(dir.as_ref())?;
        Ok(Self {
            dir: dir.as_ref().to_path_buf(),
        })
    }

    fn path_for(&self, reference: &str) -> Result<PathBuf> {
        // The reference comes from a receipt, so it is checked to be a bare
        // file name: a receipt naming `../elsewhere` must not reach outside
        // the directory.
        let name = Path::new(reference);
        if name.components().count() != 1 || name.file_name().is_none() {
            return Err(ArchiveError::Unsupported(format!(
                "local locator {reference:?} is not a bare file name"
            )));
        }
        Ok(self.dir.join(name))
    }
}

impl ArchiveStore for LocalDirStore {
    fn kind(&self) -> &'static str {
        "local"
    }

    fn put(&self, archive: &Archive) -> Result<Locator> {
        let reference = format!("{}.car.zst", archive.root);
        let path = self.path_for(&reference)?;
        // Written beside the target and renamed into place, so a crash leaves
        // either the whole archive or none of it, never a truncated file that
        // fails every later read.
        let partial = path.with_extension("zst.partial");
        fs::write(&partial, compress(&archive.car)?)?;
        fs::rename(&partial, &path)?;
        Ok(Locator {
            kind: self.kind().to_string(),
            reference,
        })
    }

    fn get(&self, locator: &Locator, _root: &Cid) -> Result<Vec<u8>> {
        if locator.kind != self.kind() {
            return Err(ArchiveError::Unsupported(format!(
                "a local store cannot read a {} locator",
                locator.kind
            )));
        }
        let bytes = fs::read(self.path_for(&locator.reference)?)?;
        if is_zstd(&bytes) {
            decompress(&bytes)
        } else {
            Ok(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArchivedBlock, build_archive, open_archive};

    fn archive() -> Archive {
        let blocks: Vec<ArchivedBlock> = (1..4)
            .map(|height| ArchivedBlock {
                height,
                id: [height as u8; 32],
                bytes: vec![height as u8; 64],
            })
            .collect();
        build_archive("maya-test", &blocks).expect("build")
    }

    #[test]
    fn a_local_store_round_trips_a_compressed_archive() {
        let dir = tempfile::tempdir().expect("dir");
        let store = LocalDirStore::new(dir.path()).expect("store");
        let archive = archive();

        let locator = store.put(&archive).expect("put");
        assert!(locator.reference.ends_with(".car.zst"));
        let on_disk = fs::read(dir.path().join(&locator.reference)).expect("read");
        assert!(is_zstd(&on_disk), "stored compressed");

        let car = store.get(&locator, &archive.root).expect("get");
        assert_eq!(car, archive.car);
        assert!(open_archive(&car, &archive.root).is_ok());
    }

    #[test]
    fn a_locator_cannot_escape_the_directory() {
        let dir = tempfile::tempdir().expect("dir");
        let store = LocalDirStore::new(dir.path()).expect("store");
        for reference in ["../outside.car.zst", "a/b.car.zst", ""] {
            let locator = Locator {
                kind: "local".into(),
                reference: reference.into(),
            };
            assert!(store.get(&locator, &archive().root).is_err(), "{reference}");
        }
    }

    #[test]
    fn a_locator_round_trips_its_text_form() {
        let locator = Locator {
            kind: "ipfs".into(),
            reference: "bafy...".into(),
        };
        assert_eq!(Locator::decode(&locator.encode()).expect("decode"), locator);
        assert!(Locator::decode("no-kind").is_err());
    }
}
