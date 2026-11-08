//! The live transport: the node's own JSON-RPC over HTTP.

use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::params::ArrayParams;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use serde_json::Value;

use crate::obs::Transport;

/// Per-call deadline. A node that cannot answer in this long is treated as
/// unreachable for this poll.
pub const REQUEST_TIMEOUT_SECS: u64 = 5;

/// Builds an HTTP JSON-RPC client for `url`.
///
/// # Errors
///
/// Returns the builder's error text for a URL it cannot use.
pub fn client(url: &str) -> Result<HttpClient, String> {
    HttpClientBuilder::default()
        .request_timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build(url)
        .map_err(|e| format!("{url}: {e}"))
}

impl Transport for HttpClient {
    async fn call(&self, method: &'static str, args: Vec<u64>) -> Result<Value, String> {
        let mut params = ArrayParams::new();
        for a in &args {
            params.insert(*a).map_err(|e| e.to_string())?;
        }
        ClientT::request::<Value, _>(self, method, params)
            .await
            .map_err(|e| e.to_string())
    }
}
