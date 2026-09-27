//! `maya2c vault` — a vault's policy and requests, with what it does and does
//! not protect (ADR-030; Master Prompt 28).

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;

/// Printed with every vault, because the protection has edges (ADR-030).
pub const RISK_LABEL: &str = "A vault protects against theft of your own key: withdrawals above the limit wait out the delay, and any guardian can cancel them. It does NOT protect against a thief who also holds a guardian key, or against transfers within the per-window limit, which are instant. An asset bridged into a vault is only as safe as the weakest of its source chain, the bridge route and Maya2C; if the source chain is broken, the bridged representation is affected too.";

/// The node's view of `address`'s vault, or `None` if it has none.
///
/// # Errors
///
/// An unreachable node or a malformed answer.
pub async fn status(rpc: &str, address: &str) -> anyhow::Result<Option<serde_json::Value>> {
    let client = HttpClientBuilder::default().build(rpc)?;
    let vault: serde_json::Value = client.request("vault_get", rpc_params![address]).await?;
    Ok((!vault.is_null()).then_some(vault))
}
