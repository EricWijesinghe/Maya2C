//! The ledger: invoices, the scan position, the payment claims and the recent
//! block ids, in one JSON file written atomically and durably.
//!
//! Claims must survive a restart. If they did not, a restart would forget that
//! an output already paid an invoice, and the same output could settle a second
//! one. So the claims and the scan position are committed together.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CheckoutError, Result};

use crate::invoice::{Invoice, UnmatchedPayment};

/// How many recent block ids are kept for reorg and checkpoint checks. A
/// checkpoint more than this far below the scan position cannot be matched to
/// a stored id, so nothing confirms until finality catches up (fail closed).
pub const RETAIN_BLOCKS: u64 = 256;

/// What an output was used for. An output appears in the claims at most once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Claim {
    /// The output settled this invoice (by id).
    Invoice(String),
    /// The output paid the merchant but matched no invoice. Kept for the
    /// merchant to handle by hand.
    Unmatched,
}

/// Everything the service remembers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// Highest block height whose outputs have been examined.
    pub scanned_to: Option<u64>,
    /// The merchant's deposit addresses, hex: derived in the merchant's wallet
    /// and loaded at startup (public addresses only, no keys). Each invoice is
    /// given one that no invoice ever had, and it is never handed out again.
    #[serde(default)]
    pub addresses: BTreeSet<String>,
    /// Invoices in creation order.
    pub invoices: Vec<Invoice>,
    /// `"txid:index"` to what the output was used for.
    pub claims: BTreeMap<String, Claim>,
    /// Payments to the merchant that settled nothing, in scan order.
    pub unmatched: Vec<UnmatchedPayment>,
    /// Block id per recent height, for reorg detection. Kept in its own small
    /// file (see [`Store`]), because it changes every block and must not force
    /// a rewrite of the ledger.
    #[serde(skip)]
    pub block_ids: BTreeMap<u64, String>,
}

/// A ledger bound to the file it lives in, and to a sidecar file holding the
/// recent block ids.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    blocks_path: PathBuf,
    ledger: Ledger,
    /// The bytes last written, so an unchanged ledger is not rewritten.
    last_written: Option<Vec<u8>>,
    /// The block-id bytes last written, for the same reason.
    last_blocks: Option<Vec<u8>>,
    /// How many times the ledger file was actually written.
    writes: u64,
}

/// The claim key for one transaction output.
#[must_use]
pub fn claim_key(txid: &str, index: u32) -> String {
    format!("{txid}:{index}")
}

impl Store {
    /// Opens the ledger at `path`. A missing file is an empty ledger.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Store`] if the file exists but cannot be read or parsed.
    /// A corrupt ledger is refused, never silently reset: resetting would drop
    /// the claims.
    pub fn open(path: &Path) -> Result<Self> {
        let (mut ledger, last_written): (Ledger, Option<Vec<u8>>) = match std::fs::read(path) {
            Ok(bytes) => {
                let ledger = serde_json::from_slice(&bytes)
                    .map_err(|e| CheckoutError::Store(format!("ledger is corrupt: {e}")))?;
                (ledger, Some(bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Ledger::default(), None),
            Err(e) => return Err(CheckoutError::Store(format!("read ledger: {e}"))),
        };
        let blocks_path = blocks_path_for(path);
        let last_blocks = match std::fs::read(&blocks_path) {
            Ok(bytes) => {
                ledger.block_ids = serde_json::from_slice(&bytes)
                    .map_err(|e| CheckoutError::Store(format!("block ids are corrupt: {e}")))?;
                Some(bytes)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(CheckoutError::Store(format!("read block ids: {e}"))),
        };
        Ok(Self {
            path: path.to_path_buf(),
            blocks_path,
            ledger,
            last_written,
            last_blocks,
            writes: 0,
        })
    }

    /// The current ledger.
    #[must_use]
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// Mutable ledger. Changes are durable only after [`Store::commit`].
    pub fn ledger_mut(&mut self) -> &mut Ledger {
        &mut self.ledger
    }

    /// How many times the ledger file has been written by this process.
    #[must_use]
    pub fn writes(&self) -> u64 {
        self.writes
    }

    /// Writes the ledger if it changed since the last write. The write goes to
    /// a temporary file, is flushed to disk, and then renamed over the old
    /// one, so a crash leaves either the old ledger or the new one.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Store`] on any I/O failure.
    pub fn commit(&mut self) -> Result<()> {
        // Block ids first: a crash between the two writes leaves ids that are
        // at worst ahead of the scan position, which is harmless.
        let blocks = serde_json::to_vec(&self.ledger.block_ids)
            .map_err(|e| CheckoutError::Store(format!("encode block ids: {e}")))?;
        if self.last_blocks.as_deref() != Some(blocks.as_slice()) {
            write_durably(&self.blocks_path, &blocks)?;
            self.last_blocks = Some(blocks);
        }
        let bytes = serde_json::to_vec(&self.ledger)
            .map_err(|e| CheckoutError::Store(format!("encode ledger: {e}")))?;
        if self.last_written.as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        write_durably(&self.path, &bytes)?;
        self.last_written = Some(bytes);
        self.writes += 1;
        Ok(())
    }

    /// A pool address no invoice has ever been given.
    ///
    /// A transaction carries no memo, so the deposit address is what binds a
    /// payment to one invoice. Addresses are never reused, not even after an
    /// invoice expires: a late payment then still names the invoice it was for.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Conflict`] when every pool address has been used: the
    /// merchant must load more.
    pub fn free_address(&self) -> Result<[u8; 32]> {
        let used: BTreeSet<String> = self
            .ledger
            .invoices
            .iter()
            .map(|inv| hex::encode(inv.recipient))
            .collect();
        let free = self
            .ledger
            .addresses
            .iter()
            .find(|a| !used.contains(*a))
            .ok_or_else(|| {
                CheckoutError::Conflict("no unused deposit address left; load more".into())
            })?;
        hex::decode(free)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .ok_or_else(|| CheckoutError::Store("pool address is not 32 bytes of hex".into()))
    }

    /// Adds an invoice and commits.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Conflict`] when its address is not an unused pool
    /// address, [`CheckoutError::Store`] if the commit fails (the invoice is
    /// then not kept in memory either).
    pub fn insert(&mut self, invoice: Invoice) -> Result<()> {
        let address = hex::encode(invoice.recipient);
        let shared = self
            .ledger
            .invoices
            .iter()
            .any(|inv| inv.recipient == invoice.recipient);
        if shared || !self.ledger.addresses.contains(&address) {
            return Err(CheckoutError::Conflict(
                "the deposit address is not an unused pool address".into(),
            ));
        }
        self.ledger.invoices.push(invoice);
        if let Err(e) = self.commit() {
            self.ledger.invoices.pop();
            return Err(e);
        }
        Ok(())
    }

    /// The invoice with this id.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&Invoice> {
        self.ledger.invoices.iter().find(|inv| inv.id == id)
    }
}

/// Writes `bytes` to `path` atomically and durably. Private to the owner on
/// Unix, because the file names invoices and webhook URLs.
fn write_durably(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    // A leftover temporary file keeps its old permissions if reused, so start clean.
    let _ = std::fs::remove_file(&tmp);
    let mut file = private_file(&tmp).map_err(|e| CheckoutError::Store(format!("create: {e}")))?;
    file.write_all(bytes)
        .map_err(|e| CheckoutError::Store(format!("write: {e}")))?;
    file.sync_all()
        .map_err(|e| CheckoutError::Store(format!("sync: {e}")))?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|e| CheckoutError::Store(format!("rename: {e}")))?;
    #[cfg(unix)]
    sync_parent(path)?;
    Ok(())
}

#[cfg(unix)]
fn private_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn private_file(path: &Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// The sidecar file that holds the recent block ids: `<ledger path>.blocks`.
fn blocks_path_for(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".blocks");
    PathBuf::from(name)
}

/// Flushes the rename itself to disk. Unix only. On Windows the directory is
/// not fsynced; whether the rename is durable there is NOT VERIFIED on this host.
#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<()> {
    match path.parent() {
        Some(dir) => File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|e| CheckoutError::Store(format!("sync directory: {e}"))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::invoice::{NewInvoice, Status};
    use primitive_types::U256;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maya-checkout-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn invoice(id: &str, recipient: [u8; 32]) -> Invoice {
        let req = NewInvoice {
            amount: U256::from(500u64),
            memo: "o".into(),
            expires_in_blocks: 10,
            webhook_url: "https://shop.example/hook".into(),
        };
        Invoice::open(id.into(), recipient, &req, 0).unwrap()
    }

    fn pooled(name: &str, pool: &[[u8; 32]]) -> Store {
        let path = temp_path(name);
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        for a in pool {
            store.ledger_mut().addresses.insert(hex::encode(a));
        }
        store
    }

    #[test]
    fn claims_survive_a_reopen() {
        let path = temp_path("survive.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store
            .ledger_mut()
            .claims
            .insert(claim_key("ab", 0), Claim::Invoice("inv-1".into()));
        store.commit().unwrap();
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.ledger().claims.get("ab:0"),
            Some(&Claim::Invoice("inv-1".into()))
        );
    }

    #[test]
    fn corrupt_ledger_is_refused_not_reset() {
        let path = temp_path("corrupt.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(matches!(Store::open(&path), Err(CheckoutError::Store(_))));
    }

    #[test]
    fn each_invoice_takes_a_fresh_pool_address() {
        let mut store = pooled("fresh.json", &[[1; 32], [2; 32]]);
        let first = store.free_address().unwrap();
        store.insert(invoice("a", first)).unwrap();
        let second = store.free_address().unwrap();
        assert_ne!(first, second);
        store.insert(invoice("b", second)).unwrap();
        assert!(matches!(
            store.free_address(),
            Err(CheckoutError::Conflict(_))
        ));
    }

    #[test]
    fn an_address_is_never_given_to_a_second_invoice() {
        let mut store = pooled("shared.json", &[[1; 32], [2; 32]]);
        store.insert(invoice("a", [1; 32])).unwrap();
        // Not even once the first invoice is over: a late payment to it must
        // still name invoice `a`.
        store.ledger_mut().invoices[0].status = Status::Expired;
        assert!(matches!(
            store.insert(invoice("b", [1; 32])),
            Err(CheckoutError::Conflict(_))
        ));
        assert_eq!(store.free_address().unwrap(), [2; 32]);
    }

    #[test]
    fn an_address_outside_the_pool_is_refused() {
        let mut store = pooled("outside.json", &[[1; 32]]);
        assert!(matches!(
            store.insert(invoice("a", [3; 32])),
            Err(CheckoutError::Conflict(_))
        ));
        assert!(store.ledger().invoices.is_empty());
    }

    #[test]
    fn block_id_changes_do_not_rewrite_the_ledger() {
        // N3: block ids change every block. They go to their own small file, so
        // the ledger file (invoices, claims) is not rewritten for them.
        let path = temp_path("blocks-side.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.commit().unwrap();
        let ledger_writes = store.writes();
        store.ledger_mut().block_ids.insert(9, "b9".into());
        store.commit().unwrap();
        assert_eq!(
            store.writes(),
            ledger_writes,
            "a block id must not rewrite the ledger"
        );
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.ledger().block_ids.get(&9).map(String::as_str),
            Some("b9")
        );
    }

    #[test]
    fn an_unchanged_ledger_is_not_rewritten() {
        let path = temp_path("unchanged.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.ledger_mut().scanned_to = Some(7);
        store.commit().unwrap();
        assert_eq!(store.writes(), 1);
        store.commit().unwrap();
        assert_eq!(store.writes(), 1, "a no-op commit must not touch the disk");
        store.ledger_mut().scanned_to = Some(8);
        store.commit().unwrap();
        assert_eq!(store.writes(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn the_ledger_file_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("private.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.commit().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
