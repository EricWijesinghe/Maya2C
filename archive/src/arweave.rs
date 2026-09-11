//! Arweave, read through a public gateway.
//!
//! Read-only on purpose. Uploading to Arweave signs a transaction with a
//! wallet and pays AR for every byte, forever. That is a spending decision and
//! a key-custody decision, and it is not made on an operator's behalf by a
//! background task. Until one is made, `put` refuses, and an archive
//! uploaded some other way (a bundler, the operator's own tooling) is still
//! fetchable here and still verified by CID like any other copy.

use std::time::Duration;

use cid::Cid;
use reqwest::blocking::Client;

use crate::error::{ArchiveError, Result};
use crate::store::{ArchiveStore, Locator};
use crate::{Archive, decompress, is_zstd, read_capped};

/// How long one gateway fetch may take.
const TIMEOUT: Duration = Duration::from_secs(300);

/// An Arweave gateway such as `https://arweave.net`.
#[derive(Clone, Debug)]
pub struct ArweaveGateway {
    base: String,
    client: Client,
}

impl ArweaveGateway {
    /// A gateway at `base`.
    ///
    /// # Errors
    ///
    /// Returns [`ArchiveError::Http`] if the HTTP client cannot be built.
    pub fn new(base: impl Into<String>) -> Result<Self> {
        Ok(Self {
            base: base.into().trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(TIMEOUT)
                .build()
                .map_err(|e| ArchiveError::Http(e.to_string()))?,
        })
    }
}

fn is_transaction_id(reference: &str) -> bool {
    // 43 characters of base64url: an Arweave transaction id. Checked so a
    // receipt cannot steer the request to another path on the gateway.
    reference.len() == 43
        && reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl ArchiveStore for ArweaveGateway {
    fn kind(&self) -> &'static str {
        "arweave"
    }

    fn put(&self, _archive: &Archive) -> Result<Locator> {
        Err(ArchiveError::Unsupported(
            "Arweave upload needs a funded wallet and pays AR per byte; it is not done \
             automatically. Upload the .car.zst with your own tooling and record the \
             transaction id"
                .into(),
        ))
    }

    fn get(&self, locator: &Locator, _root: &Cid) -> Result<Vec<u8>> {
        if locator.kind != self.kind() || !is_transaction_id(&locator.reference) {
            return Err(ArchiveError::Unsupported(format!(
                "not an Arweave transaction locator: {}",
                locator.encode()
            )));
        }
        let response = self
            .client
            .get(format!("{}/{}", self.base, locator.reference))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|e| ArchiveError::Http(e.to_string()))?;
        // Capped while reading: a gateway is a third party, and a check after
        // a full read would already have buffered whatever it sent.
        let bytes = read_capped(response)?;
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

    #[test]
    fn upload_is_refused_rather_than_spending_on_the_operators_behalf() {
        let gateway = ArweaveGateway::new("https://arweave.net").expect("client");
        let archive = crate::build_archive(
            "maya-test",
            &[crate::ArchivedBlock {
                height: 1,
                id: [1; 32],
                bytes: vec![1],
            }],
        )
        .expect("build");
        assert!(matches!(
            gateway.put(&archive),
            Err(ArchiveError::Unsupported(_))
        ));
    }

    #[test]
    fn only_a_transaction_id_is_fetched() {
        assert!(is_transaction_id(
            "bNbA3TEQVL60xlgCcqdz4ZPHFZ711cZ3hmkpGttDt_U"
        ));
        for bad in [
            "",
            "../../admin",
            "bNbA3TEQVL60xlgCcqdz4ZPHFZ711cZ3hmkpGttDt_",
            "a/b",
        ] {
            assert!(!is_transaction_id(bad), "{bad}");
        }
    }
}
