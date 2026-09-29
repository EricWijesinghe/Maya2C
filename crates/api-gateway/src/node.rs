//! The gateway's view of a node: a JSON-RPC client behind a trait.
//!
//! The trait exists so the REST and GraphQL layers can be tested against a
//! recorded node without a running chain. That is not a convenience — an
//! end-to-end test that needs RocksDB, a genesis file, and a mined block in
//! order to check that a 404 is a 404 is a test nobody runs.

use std::time::Duration;

use async_trait::async_trait;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;

use crate::allowlist;
use crate::error::GatewayError;

/// How long a node call may take before the gateway gives up.
///
/// A gateway that waits forever on a wedged node holds a connection per waiting
/// client and turns one stuck backend into an exhausted frontend.
pub const NODE_TIMEOUT: Duration = Duration::from_secs(10);

/// An account's balance and nonce, as the node reports them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct Balance {
    /// Spendable amount in base units.
    pub balance: u64,
    /// Next valid nonce.
    pub nonce: u64,
}

/// Circulating and total supply.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct Supply {
    /// Units in circulation.
    pub circulating: u64,
    /// Units issued in total.
    pub total: u64,
}

/// What the chain charges, and where the fee goes (ADR-029).
///
/// A wallet cannot build an acceptable transfer without it: on a fee-market
/// chain the fee is an output to `collector` of at least `base_fee` per byte.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct FeeInfo {
    /// Whether this chain charges fees at all.
    pub active: bool,
    /// Base fee per serialized byte for the next block.
    pub base_fee: u64,
    /// Hex address a transaction pays its fee to.
    pub collector: String,
}

/// What the gateway needs from a node.
///
/// Deliberately narrow: one method per allowlisted RPC call and nothing else.
/// A trait that mirrored the node's whole interface would make the allowlist a
/// suggestion rather than a boundary.
#[async_trait]
pub trait NodeClient: Send + Sync + 'static {
    /// Balance and nonce for a hex-encoded address.
    async fn get_balance(&self, address: &str) -> Result<Balance, GatewayError>;

    /// A block by height, as the node's own JSON shape.
    async fn get_block_by_height(&self, height: u64) -> Result<serde_json::Value, GatewayError>;

    /// Circulating and total supply.
    async fn get_supply(&self) -> Result<Supply, GatewayError>;

    /// Submits a hex-encoded signed transaction, returning its hash.
    async fn send_raw_transaction(&self, raw: &str) -> Result<String, GatewayError>;

    /// The chain's current fee terms.
    async fn get_fee_info(&self) -> Result<FeeInfo, GatewayError>;
}

/// A [`NodeClient`] backed by a real node's JSON-RPC port.
pub struct RpcNodeClient {
    client: HttpClient,
}

impl RpcNodeClient {
    /// Connects to a node's JSON-RPC endpoint.
    ///
    /// # Errors
    ///
    /// [`GatewayError::Upstream`] if the URL is unusable.
    pub fn connect(url: &str) -> Result<Self, GatewayError> {
        let client = HttpClientBuilder::default()
            .request_timeout(NODE_TIMEOUT)
            .build(url)
            .map_err(|e| GatewayError::Upstream(format!("connecting to node at {url}: {e}")))?;
        Ok(Self { client })
    }

    /// Forwards a call, refusing anything outside the allowlist.
    ///
    /// The allowlist is checked **here**, at the single point every outbound
    /// call passes through, rather than at each route. A route that forgot the
    /// check would otherwise be a hole, and the compiler cannot see a missing
    /// call to a free function.
    async fn call<R>(
        &self,
        method: &'static str,
        params: jsonrpsee::core::params::ArrayParams,
    ) -> Result<R, GatewayError>
    where
        R: serde::de::DeserializeOwned,
    {
        if !allowlist::is_allowed(method) {
            return Err(GatewayError::MethodNotAllowed(method.to_string()));
        }
        self.client
            .request(method, params)
            .await
            .map_err(|e| classify(method, &e))
    }
}

/// JSON-RPC "invalid params", and the node's own refusal code.
const REFUSAL_CODES: [i32; 2] = [-32_602, -32_000];

/// A call error the node chose to return (bad params, a refused transaction)
/// is the caller's fault; anything else — transport, timeout, an internal
/// error — is the gateway's or the node's. `cargo xtask sdk-e2e` found the
/// first kind being reported as the second.
fn classify(method: &str, error: &jsonrpsee::core::ClientError) -> GatewayError {
    match error {
        jsonrpsee::core::ClientError::Call(call) if REFUSAL_CODES.contains(&call.code()) => {
            GatewayError::Rejected(format!("{method}: {}", call.message()))
        }
        _ => GatewayError::Upstream(format!("{method}: {error}")),
    }
}

#[async_trait]
impl NodeClient for RpcNodeClient {
    async fn get_balance(&self, address: &str) -> Result<Balance, GatewayError> {
        self.call("get_balance", rpc_params![address]).await
    }

    async fn get_block_by_height(&self, height: u64) -> Result<serde_json::Value, GatewayError> {
        self.call("get_block_by_height", rpc_params![height]).await
    }

    async fn get_supply(&self) -> Result<Supply, GatewayError> {
        self.call("get_supply", rpc_params![]).await
    }

    async fn send_raw_transaction(&self, raw: &str) -> Result<String, GatewayError> {
        // The node answers `{txid, accepted}`, not a bare hash. Until
        // `cargo xtask sdk-e2e` ran against a real node, only the recorded
        // node in the tests had spoken to this, and it answered a string.
        #[derive(serde::Deserialize)]
        struct Submitted {
            txid: String,
        }
        let submitted: Submitted = self.call("send_raw_transaction", rpc_params![raw]).await?;
        Ok(submitted.txid)
    }

    async fn get_fee_info(&self) -> Result<FeeInfo, GatewayError> {
        self.call("get_fee_info", rpc_params![]).await
    }
}
