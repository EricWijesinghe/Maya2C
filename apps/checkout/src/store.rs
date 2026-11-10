//! The ledger: invoices, the scan position, the payment claims and the recent
//! block ids, in one JSON file written atomically and durably.
//!
//! Claims must survive a restart. If they did not, a restart would forget that
//! an output already paid an invoice, and the same output could settle a second
//! one. So the claims and the scan position are committed together.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CheckoutError, Result};
use crate::invoice::{Invoice, Status, UnmatchedPayment};

/// How many recent block ids are kept for reorg checks. A reorg deeper than
/// this is not detected: shortcut: depth-limited reorg check, upgrade by
/// keeping ids back to the finalized checkpoint.
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
    /// The merchant receiving address, hex. Payments to it are recorded even
    /// before the first invoice exists, so none goes unseen.
    #[serde(default)]
    pub merchant: Option<String>,
    /// Invoices in creation order.
    pub invoices: Vec<Invoice>,
    /// `"txid:index"` to what the output was used for.
    pub claims: BTreeMap<String, Claim>,
    /// Payments to the merchant that settled nothing, in scan order.
    pub unmatched: Vec<UnmatchedPayment>,
    /// Block id per recent height, for reorg detection.
    pub block_ids: BTreeMap<u64, String>,
}

/// A ledger bound to the file it lives in.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    ledger: Ledger,
    /// The bytes last written, so an unchanged ledger is not rewritten.
    last_written: Option<Vec<u8>>,
    /// How many times the file was actually written.
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
        let (ledger, last_written) = match std::fs::read(path) {
            Ok(bytes) => {
                let ledger = serde_json::from_slice(&bytes)
                    .map_err(|e| CheckoutError::Store(format!("ledger is corrupt: {e}")))?;
                (ledger, Some(bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Ledger::default(), None),
            Err(e) => return Err(CheckoutError::Store(format!("read ledger: {e}"))),
        };
        Ok(Self { path: path.to_path_buf(), ledger, last_written, writes: 0 })
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

    /// Adds a pending invoice and commits.
    ///
    /// Refuses an amount that another live invoice (pending or detected) to the
    /// same recipient is waiting for. A payment carries no memo on chain, so
    /// the exact amount is the only thing that ties it to an invoice; two live
    /// invoices with one amount would make a payment ambiguous.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Conflict`] on an amount clash, [`CheckoutError::Store`]
    /// if the commit fails (the invoice is then not kept in memory either).
    pub fn insert(&mut self, invoice: Invoice) -> Result<()> {
        let clash = self.ledger.invoices.iter().any(|open| {
            matches!(open.status, Status::Pending | Status::Detected)
                && open.recipient == invoice.recipient
                && open.amount == invoice.amount
        });
        if clash {
            return Err(CheckoutError::Conflict(
                "another open invoice is waiting for this exact amount; change it by one base unit".into(),
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
    file.write_all(bytes).map_err(|e| CheckoutError::Store(format!("write: {e}")))?;
    file.sync_all().map_err(|e| CheckoutError::Store(format!("sync: {e}")))?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|e| CheckoutError::Store(format!("rename: {e}")))?;
    #[cfg(unix)]
    sync_parent(path)?;
    Ok(())
}

#[cfg(unix)]
fn private_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn private_file(path: &Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)
}

/// Flushes the rename itself to disk. Unix only: Windows has no directory
/// fsync, and its rename is already written through.
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
    use crate::invoice::NewInvoice;
    use primitive_types::U256;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maya-checkout-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn invoice(id: &str, amount: u64) -> Invoice {
        let req = NewInvoice {
            amount: U256::from(amount),
            memo: "o".into(),
            expires_in_blocks: 10,
            webhook_url: "https://shop.example/hook".into(),
        };
        Invoice::open(id.into(), [1; 32], &req, 0).unwrap()
    }

    #[test]
    fn claims_survive_a_reopen() {
        let path = temp_path("survive.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.ledger_mut().claims.insert(claim_key("ab", 0), Claim::Invoice("inv-1".into()));
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
    fn second_open_invoice_with_same_amount_is_refused() {
        let path = temp_path("clash.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.insert(invoice("a", 500)).unwrap();
        assert!(matches!(store.insert(invoice("b", 500)), Err(CheckoutError::Conflict(_))));
        assert!(store.insert(invoice("c", 501)).is_ok());
    }

    #[test]
    fn a_detected_invoice_still_holds_its_amount() {
        let path = temp_path("detected-clash.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.insert(invoice("a", 500)).unwrap();
        store.ledger_mut().invoices[0].status = Status::Detected;
        assert!(matches!(store.insert(invoice("b", 500)), Err(CheckoutError::Conflict(_))));
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
