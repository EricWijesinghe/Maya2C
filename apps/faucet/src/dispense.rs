//! Turning a granted request into a signed, submitted transfer.
//!
//! # Why this is a trait
//!
//! So the HTTP layer and the load test can run without a chain. A test that
//! needed RocksDB, a genesis file, and a funded account in order to check that
//! the second request from an IP gets a 429 is a test nobody runs — the same
//! reasoning behind `api-gateway`'s `NodeClient`.
//!
//! # The nonce is tracked here, not read per request
//!
//! Every grant is a transfer from one account, so every grant needs the next
//! nonce for that account. Reading it from the node per request would be wrong
//! twice over: the read is a round trip taken while a request is waiting, and
//! two grants issued before the first is mined would both read the same nonce
//! and produce two transactions of which the chain accepts one. So the faucet
//! seeds the nonce from the node once at startup and advances it itself.
//!
//! The cost is that a restart must re-read it, and that a submission the node
//! rejects leaves a gap. Both are recoverable by restarting; a duplicate nonce
//! is not — it silently drops a user's funding on the floor and the faucet
//! reports success.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::HybridSigningKey;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;

/// How long a node call may take before the faucet gives up.
///
/// A faucet blocked on a wedged node holds a connection per waiting client and
/// turns one stuck backend into an exhausted frontend.
pub const NODE_TIMEOUT: Duration = Duration::from_secs(10);

/// Address fees are paid to.
///
/// All zeros, and unspendable: no private key produces the identity point as a
/// public key. The same sink the wallet uses — fees are burned, because the
/// chain has no coinbase mechanism to credit a miner.
pub const FEE_SINK: [u8; 32] = [0u8; 32];

/// Fee the faucet pays on its own transfers.
pub const FAUCET_FEE: u64 = 1;

/// Something went wrong sending funds.
#[derive(Debug, thiserror::Error)]
pub enum DispenseError {
    /// The node was unreachable, slow, or refused the transaction.
    #[error("node: {0}")]
    Node(String),

    /// The transaction could not be signed.
    #[error("signing: {0}")]
    Signing(String),

    /// The faucet account cannot cover the grant.
    #[error("the faucet account is out of funds and needs topping up")]
    Underfunded,
}

/// What the faucet needs in order to hand out funds.
#[async_trait]
pub trait Dispenser: Send + Sync + 'static {
    /// Sends `amount` to `recipient`, returning the transaction id.
    ///
    /// # Errors
    ///
    /// [`DispenseError`] if signing or submission failed.
    async fn send(&self, recipient: &[u8; 32], amount: u64) -> Result<String, DispenseError>;
}

/// A [`Dispenser`] that signs with a hot key and submits over JSON-RPC.
pub struct NodeDispenser {
    key: HybridSigningKey,
    client: HttpClient,
    /// Seeded from the node at startup, advanced locally thereafter.
    next_nonce: AtomicU64,
}

impl NodeDispenser {
    /// Connects to a node and reads the faucet account's next nonce.
    ///
    /// # Errors
    ///
    /// [`DispenseError::Node`] if the node is unreachable or the account
    /// cannot be read. Failing here rather than on the first request is
    /// deliberate: a faucet that cannot reach its node should not accept
    /// traffic and hand out 500s.
    pub async fn connect(url: &str, key: HybridSigningKey) -> Result<Self, DispenseError> {
        let client = HttpClientBuilder::default()
            .request_timeout(NODE_TIMEOUT)
            .build(url)
            .map_err(|e| DispenseError::Node(format!("connecting to {url}: {e}")))?;

        let address = hex::encode(key.address());
        let info: serde_json::Value = client
            .request("get_balance", rpc_params![address.clone()])
            .await
            .map_err(|e| DispenseError::Node(format!("reading account {address}: {e}")))?;

        let nonce = info
            .get("nonce")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| DispenseError::Node("account response has no nonce".to_string()))?;

        Ok(Self {
            key,
            client,
            next_nonce: AtomicU64::new(nonce),
        })
    }

    /// The faucet's own address, for a status page or a top-up.
    #[must_use]
    pub fn address(&self) -> [u8; 32] {
        self.key.address()
    }

    /// The nonce the next transfer will carry.
    #[must_use]
    pub fn next_nonce(&self) -> u64 {
        self.next_nonce.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Dispenser for NodeDispenser {
    async fn send(&self, recipient: &[u8; 32], amount: u64) -> Result<String, DispenseError> {
        let raw = sign_transfer(
            &self.key,
            recipient,
            amount,
            FAUCET_FEE,
            self.next_nonce.fetch_add(1, Ordering::SeqCst),
        )?;

        let result: serde_json::Value = self
            .client
            .request("send_raw_transaction", rpc_params![raw])
            .await
            .map_err(|e| DispenseError::Node(format!("submitting: {e}")))?;

        result
            .get("txid")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| DispenseError::Node("submission response has no txid".to_string()))
    }
}

/// Signs a transfer and returns the hex a node accepts.
///
/// Shaped like the wallet's `sign_transfer`, but built here rather than
/// imported: the faucet is a service, and depending on the GUI wallet's core to
/// move value would put a keyring, an image decoder, and a QR parser in the
/// process that terminates public HTTP.
fn sign_transfer(
    key: &HybridSigningKey,
    recipient: &[u8; 32],
    amount: u64,
    fee: u64,
    nonce: u64,
) -> Result<String, DispenseError> {
    // Checked, because a wrapped total would sign a transfer nobody intended.
    amount.checked_add(fee).ok_or(DispenseError::Underfunded)?;

    let mut outputs = vec![TxOutput {
        amount,
        recipient: *recipient,
    }];
    if fee > 0 {
        outputs.push(TxOutput {
            amount: fee,
            recipient: FEE_SINK,
        });
    }

    let mut tx = Transaction::new(vec![], outputs, nonce);
    tx.sign(key)
        .map_err(|e| DispenseError::Signing(e.to_string()))?;

    Ok(hex::encode(tx.to_bytes()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use custom_l1_node::crypto::hybrid::generate_signing_key;

    #[test]
    fn a_signed_transfer_carries_the_fee_sink_output() {
        let key = generate_signing_key().expect("keygen");
        let raw = sign_transfer(&key, &[7u8; 32], 100, FAUCET_FEE, 0).expect("signed");

        let bytes = hex::decode(&raw).expect("the faucet emits valid hex");
        let tx = Transaction::from_bytes(&bytes).expect("a node must be able to parse it");

        assert_eq!(tx.outputs.len(), 2, "the recipient and the fee sink");
        assert_eq!(tx.outputs[0].recipient, [7u8; 32]);
        assert_eq!(tx.outputs[0].amount, 100);
        assert_eq!(tx.outputs[1].recipient, FEE_SINK);
        assert_eq!(tx.outputs[1].amount, FAUCET_FEE);
    }

    #[test]
    fn a_signed_transfer_verifies() {
        // The faucet's signature must satisfy the same check a block applies.
        let key = generate_signing_key().expect("keygen");
        let raw = sign_transfer(&key, &[3u8; 32], 500, 1, 9).expect("signed");
        let bytes = hex::decode(&raw).expect("hex");
        let tx = Transaction::from_bytes(&bytes).expect("parse");

        tx.verify()
            .expect("a node would reject a transfer that does not verify");
        assert_eq!(tx.nonce, 9);
        assert_eq!(tx.sender(), key.address());
    }

    #[test]
    fn an_amount_that_would_wrap_with_the_fee_is_refused() {
        // Not reachable through the HTTP layer, which caps the dispense — but
        // the check belongs where the addition is, not where today's callers
        // happen to be.
        let key = generate_signing_key().expect("keygen");
        assert!(matches!(
            sign_transfer(&key, &[1u8; 32], u64::MAX, 1, 0),
            Err(DispenseError::Underfunded)
        ));
    }
}
