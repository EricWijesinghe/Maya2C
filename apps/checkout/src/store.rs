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
use primitive_types::U256;

use crate::invoice::{Delivery, Invoice, Status, UnmatchedPayment};

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

/// Whether an invoice holds its amount: no other invoice may be quoted it.
/// Pending and detected invoices hold it. A confirmed invoice holds it until
/// its webhook is delivered, because a reorg can still revert it to pending.
#[must_use]
pub fn holds_amount(inv: &Invoice) -> bool {
    match inv.status {
        Status::Pending | Status::Detected => true,
        Status::Confirmed => inv.delivery != Delivery::Delivered,
        Status::Expired => false,
    }
}

/// How far above the requested amount a quote may go to find a free amount.
/// Bounded, so a flood of open invoices cannot push quotes without limit.
pub const MAX_AMOUNT_OFFSET: u64 = 1_000;

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

    /// Adds an invoice and commits. Returns the amount it is quoted at.
    ///
    /// A payment carries no memo on chain, so the exact amount is the only thing
    /// that ties it to an invoice. Two invoices holding one amount would make a
    /// payment ambiguous, so when the requested amount is held, the invoice is
    /// quoted the next free base unit above it (at most [`MAX_AMOUNT_OFFSET`]
    /// above). The shop shows the quoted amount to the customer.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Conflict`] when no free amount is left within the bound,
    /// [`CheckoutError::Store`] if the commit fails (the invoice is then not kept
    /// in memory either).
    pub fn insert(&mut self, mut invoice: Invoice) -> Result<U256> {
        let requested = invoice.requested_amount;
        let mut quoted = None;
        for offset in 0..=MAX_AMOUNT_OFFSET {
            let candidate = requested.saturating_add(U256::from(offset));
            let taken = self.ledger.invoices.iter().any(|open| {
                holds_amount(open) && open.recipient == invoice.recipient && open.amount == candidate
            });
            if !taken {
                quoted = Some(candidate);
                break;
            }
        }
        let quoted = quoted.ok_or_else(|| {
            CheckoutError::Conflict("too many open invoices at this amount; try again later".into())
        })?;
        invoice.amount = quoted;
        self.ledger.invoices.push(invoice);
        if let Err(e) = self.commit() {
            self.ledger.invoices.pop();
            return Err(e);
        }
        Ok(quoted)
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
    use crate::invoice::{Delivery, NewInvoice};
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
    fn second_open_invoice_with_same_amount_is_quoted_a_unit_higher() {
        // N4: no 409. The second invoice is quoted the next free base unit.
        let path = temp_path("clash.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.insert(invoice("a", 500)).unwrap(), U256::from(500u64));
        assert_eq!(store.insert(invoice("b", 500)).unwrap(), U256::from(501u64));
        assert_eq!(store.insert(invoice("c", 500)).unwrap(), U256::from(502u64));
        assert_eq!(store.find("b").unwrap().amount, U256::from(501u64));
        assert_eq!(store.find("b").unwrap().requested_amount, U256::from(500u64));
    }

    #[test]
    fn a_detected_invoice_still_holds_its_amount() {
        let path = temp_path("detected-clash.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.insert(invoice("a", 500)).unwrap();
        store.ledger_mut().invoices[0].status = Status::Detected;
        assert_eq!(store.insert(invoice("b", 500)).unwrap(), U256::from(501u64));
    }

    #[test]
    fn a_confirmed_invoice_that_can_still_revert_holds_its_amount() {
        // A reorg can revert an undelivered confirmed invoice to pending, so its
        // amount must stay reserved, or two pending invoices could share it.
        let path = temp_path("confirmed-holds.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.insert(invoice("a", 500)).unwrap();
        store.ledger_mut().invoices[0].status = Status::Confirmed;
        store.ledger_mut().invoices[0].delivery = Delivery::Due;
        assert_eq!(store.insert(invoice("b", 500)).unwrap(), U256::from(501u64));
    }

    #[test]
    fn a_delivered_invoice_releases_its_amount() {
        // Delivered invoices never revert, so their amount is free again.
        let path = temp_path("delivered-free.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path).unwrap();
        store.insert(invoice("a", 500)).unwrap();
        store.ledger_mut().invoices[0].status = Status::Confirmed;
        store.ledger_mut().invoices[0].delivery = Delivery::Delivered;
        assert_eq!(store.insert(invoice("b", 500)).unwrap(), U256::from(500u64));
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
        assert_eq!(store.writes(), ledger_writes, "a block id must not rewrite the ledger");
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.ledger().block_ids.get(&9).map(String::as_str), Some("b9"));
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
