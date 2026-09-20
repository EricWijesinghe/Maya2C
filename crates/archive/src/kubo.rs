//! An IPFS node's HTTP RPC API (kubo), as an archive store.
//!
//! - `put` is `POST /api/v0/dag/import?pin-roots=true` with the plain CAR as a
//!   multipart file. kubo imports every section and pins the root. Because the
//!   manifest links every block, pinning the root keeps the batch from being
//!   garbage-collected.
//! - `get` is `POST /api/v0/dag/export?arg=<root>`, which streams the pinned
//!   DAG back as a CAR.
//!
//! The kubo RPC API is an administrative interface. It belongs on loopback or
//! behind the operator's own authentication, never on a public address. Point
//! this at `http://127.0.0.1:5001`, not at a public gateway.

use std::time::Duration;

use cid::Cid;
use reqwest::blocking::{Client, multipart};

use crate::error::{ArchiveError, Result};
use crate::store::{ArchiveStore, Locator};
use crate::{Archive, MAX_ARCHIVE_BYTES, read_capped};

/// How long one import or export may take before the store gives up. Generous:
/// a batch is up to hundreds of megabytes.
const TIMEOUT: Duration = Duration::from_secs(600);

/// A kubo node reachable over its RPC API.
#[derive(Clone, Debug)]
pub struct KuboStore {
    api: String,
    client: Client,
}

fn http(error: reqwest::Error) -> ArchiveError {
    ArchiveError::Http(error.to_string())
}

impl KuboStore {
    /// A store talking to the RPC API at `api`, e.g. `http://127.0.0.1:5001`.
    ///
    /// # Errors
    ///
    /// Returns [`ArchiveError::Http`] if the HTTP client cannot be built.
    pub fn new(api: impl Into<String>) -> Result<Self> {
        Ok(Self {
            api: api.into().trim_end_matches('/').to_string(),
            client: Client::builder().timeout(TIMEOUT).build().map_err(http)?,
        })
    }

    /// The root CID in a `dag/import` response.
    ///
    /// The body is newline-delimited JSON; the line that matters is
    /// `{"Root": {"Cid": {"/": "<cid>"}, "PinErrorMsg": ""}}`. A non-empty
    /// `PinErrorMsg` means the data landed but will not be kept, which for an
    /// archive is the same as not landing.
    fn imported_root(body: &str) -> Result<Cid> {
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            let value: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| ArchiveError::Http(format!("kubo sent non-JSON: {e}")))?;
            let Some(root) = value.get("Root") else {
                continue;
            };
            if let Some(error) = root.get("PinErrorMsg").and_then(|e| e.as_str())
                && !error.is_empty()
            {
                return Err(ArchiveError::Http(format!("kubo did not pin: {error}")));
            }
            let text = root
                .get("Cid")
                .and_then(|cid| cid.get("/"))
                .and_then(|cid| cid.as_str())
                .ok_or_else(|| ArchiveError::Http("kubo named no root CID".into()))?;
            return Cid::try_from(text).map_err(|e| ArchiveError::Cid(e.to_string()));
        }
        Err(ArchiveError::Http("kubo reported no imported root".into()))
    }
}

impl ArchiveStore for KuboStore {
    fn kind(&self) -> &'static str {
        "ipfs"
    }

    fn put(&self, archive: &Archive) -> Result<Locator> {
        let part = multipart::Part::bytes(archive.car.clone())
            .file_name(format!("{}.car", archive.root))
            .mime_str("application/vnd.ipld.car")
            .map_err(http)?;
        let response = self
            .client
            .post(format!("{}/api/v0/dag/import?pin-roots=true", self.api))
            .multipart(multipart::Form::new().part("file", part))
            .send()
            .map_err(http)?
            .error_for_status()
            .map_err(http)?;
        let root = Self::imported_root(&response.text().map_err(http)?)?;
        if root != archive.root {
            return Err(ArchiveError::WrongRoot {
                expected: archive.root.to_string(),
                actual: root.to_string(),
            });
        }
        Ok(Locator {
            kind: self.kind().to_string(),
            reference: root.to_string(),
        })
    }

    fn get(&self, locator: &Locator, root: &Cid) -> Result<Vec<u8>> {
        if locator.kind != self.kind() {
            return Err(ArchiveError::Unsupported(format!(
                "an IPFS store cannot read a {} locator",
                locator.kind
            )));
        }
        // Asked for by the root the receipt recorded, not the locator's text:
        // the CID is the thing that will be verified.
        let response = self
            .client
            .post(format!("{}/api/v0/dag/export?arg={root}", self.api))
            .send()
            .map_err(http)?
            .error_for_status()
            .map_err(http)?;
        // Refused early when the length is declared, and capped while reading
        // when it is not: a chunked body has no length to check.
        if response
            .content_length()
            .is_some_and(|len| len > MAX_ARCHIVE_BYTES as u64)
        {
            return Err(ArchiveError::TooLarge {
                limit: MAX_ARCHIVE_BYTES,
            });
        }
        read_capped(response)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::{Multipart, Query, State};
    use axum::routing::post;

    use crate::car::read_car;
    use crate::{ArchivedBlock, build_archive, open_archive};

    type Pinned = Arc<Mutex<HashMap<String, Vec<u8>>>>;

    /// A stand-in for kubo's two endpoints: it stores each imported CAR under
    /// the root the CAR itself names, and exports it back verbatim.
    async fn import(State(pinned): State<Pinned>, mut form: Multipart) -> String {
        let field = form
            .next_field()
            .await
            .expect("field")
            .expect("a file part");
        let car = field.bytes().await.expect("bytes").to_vec();
        let root = read_car(&car).expect("the client sent a valid CAR").root;
        pinned.lock().expect("lock").insert(root.to_string(), car);
        format!("{{\"Root\":{{\"Cid\":{{\"/\":\"{root}\"}},\"PinErrorMsg\":\"\"}}}}\n")
    }

    async fn export(
        State(pinned): State<Pinned>,
        Query(query): Query<HashMap<String, String>>,
    ) -> Vec<u8> {
        let root = query.get("arg").expect("arg");
        pinned
            .lock()
            .expect("lock")
            .get(root)
            .cloned()
            .unwrap_or_default()
    }

    fn mock_kubo() -> (String, Pinned) {
        let pinned: Pinned = Arc::default();
        let app = Router::new()
            .route("/api/v0/dag/import", post(import))
            .route("/api/v0/dag/export", post(export))
            .with_state(Arc::clone(&pinned));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                tx.send(listener.local_addr().expect("addr")).expect("send");
                axum::serve(listener, app).await.expect("serve");
            });
        });
        let addr = rx.recv().expect("address");
        (format!("http://{addr}"), pinned)
    }

    fn archive() -> Archive {
        let blocks: Vec<ArchivedBlock> = (7..10)
            .map(|height| ArchivedBlock {
                height,
                id: [height as u8; 32],
                bytes: vec![height as u8; 100],
            })
            .collect();
        build_archive("maya-test", &blocks).expect("build")
    }

    #[test]
    fn an_archive_imports_pinned_and_exports_back_verified() {
        let (api, pinned) = mock_kubo();
        let store = KuboStore::new(api).expect("store");
        let archive = archive();

        let locator = store.put(&archive).expect("import");
        assert_eq!(locator.reference, archive.root.to_string());
        assert!(
            pinned
                .lock()
                .expect("lock")
                .contains_key(&locator.reference)
        );

        let car = store.get(&locator, &archive.root).expect("export");
        let blocks = open_archive(&car, &archive.root).expect("verifies");
        assert_eq!(blocks.len(), 3);
    }

    #[test]
    fn a_node_that_reports_a_pin_failure_is_an_upload_failure() {
        let body = "{\"Root\":{\"Cid\":{\"/\":\"bafkqaaa\"},\"PinErrorMsg\":\"out of space\"}}";
        assert!(KuboStore::imported_root(body).is_err());
        assert!(KuboStore::imported_root("").is_err());
        assert!(KuboStore::imported_root("not json").is_err());
    }
}
