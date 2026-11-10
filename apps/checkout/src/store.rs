//! The ledger: invoices, the scan position and the payment claims, in one JSON
//! file written atomically.
//!
//! Claims must survive a restart. If they did not, a restart would forget that
//! an output already paid an invoice, and the same output could settle a second
//! one. So the claims and the scan position are committed together.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CheckoutError, Result};
use crate::invoice::{Invoice, Status};

/// Everything the service remembers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// Highest block height whose outputs have been examined.
    pub scanned_to: Option<u64>,
    /// Invoices in creation order. Matching walks this order, so the result
    /// is deterministic.
    pub invoices: Vec<Invoice>,
    /// `"txid:index"` to the invoice id it settled. An output appears here at
    /// most once.
    pub claims: BTreeMap<String, String>,
}

/// A ledger bound to the file it lives in.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    ledger: Ledger,
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
        let ledger = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| CheckoutError::Store(format!("ledger is corrupt: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
            Err(e) => return Err(CheckoutError::Store(format!("read ledger: {e}"))),
        };
        Ok(Self { path: path.to_path_buf(), ledger })
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

    /// Writes the ledger to a temporary file and renames it over the old one,
    /// so a crash leaves either the old ledger or the new one, never half of each.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Store`] on any I/O failure.
    pub fn commit(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.ledger)
            .map_err(|e| CheckoutError::Store(format!("encode ledger: {e}")))?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| CheckoutError::Store(format!("write: {e}")))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| CheckoutError::Store(format!("rename: {e}")))
    }

    /// Adds a pending invoice and commits.
    ///
    /// Refuses an amount that another open invoice to the same recipient is
    /// waiting for. A payment carries no memo on chain, so the amount is the
    /// only thing that ties it to an invoice; two open invoices with one amount
    /// would make a payment ambiguous.
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Conflict`] on an amount clash, [`CheckoutError::Store`]
    /// if the commit fails (the invoice is then not kept in memory either).
    pub fn insert(&mut self, invoice: Invoice) -> Result<()> {
        let clash = self.ledger.invoices.iter().any(|open| {
            open.status == Status::Pending
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
        store.ledger_mut().claims.insert(claim_key("ab", 0), "inv-1".into());
        store.commit().unwrap();
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.ledger().claims.get("ab:0").map(String::as_str), Some("inv-1"));
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
}
