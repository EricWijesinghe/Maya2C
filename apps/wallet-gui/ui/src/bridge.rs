//! The typed edge of the Tauri IPC bridge.
//!
//! Every call here crosses out of the WASM sandbox into the Rust core. Keeping
//! them in one module makes the trust boundary something you can read in a
//! single file rather than something scattered through the UI.
//!
//! Nothing in this module can obtain key material: the core does not expose a
//! command that returns one. The recovery phrase arrives exactly once, from
//! [`create_wallet`], and is never requestable again.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
unsafe extern "C" {
    /// Tauri's IPC entry point, injected into the webview by the host.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// An account as the UI sees it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    /// Index within the wallet.
    pub index: u32,
    /// Derivation path.
    pub path: String,
    /// Hex-encoded address.
    pub address: String,
}

/// A parsed payment request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PaymentRequest {
    /// Hex-encoded recipient.
    pub recipient: String,
    /// Requested amount, if any.
    pub amount: Option<u64>,
    /// Requester-supplied label. Untrusted text.
    pub label: Option<String>,
}

/// A signed, broadcastable transaction.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedTransfer {
    /// Hex-encoded transaction id.
    pub txid: String,
    /// Hex-encoded sender.
    pub sender: String,
    /// Hex-encoded recipient.
    pub recipient: String,
    /// Amount sent.
    pub amount: u64,
    /// Nonce used.
    pub nonce: u64,
    /// Fee burned.
    pub fee: u64,
    /// Raw transaction hex.
    pub raw_hex: String,
}

/// A transfer and where it has got to.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingTransfer {
    /// The signed transaction.
    pub transfer: SignedTransfer,
    /// `Signed`, `Broadcast`, `Confirmed`, or `Failed`.
    pub status: String,
    /// Failure detail, when there is one.
    pub detail: Option<String>,
}

/// A newly created wallet, including its one-time recovery phrase.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatedWallet {
    /// The 24-word phrase. Shown once; never retrievable.
    pub mnemonic: String,
    /// The first derived account.
    pub account: Account,
}

/// A confirmation summary.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferPreview {
    /// Hex-encoded sender.
    pub sender: String,
    /// Hex-encoded recipient.
    pub recipient: String,
    /// Amount to the recipient.
    pub amount: u64,
    /// Fee.
    pub fee: u64,
    /// Amount plus fee.
    pub total: u64,
    /// Nonce.
    pub nonce: u64,
}

/// A fee preset.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeeOption {
    /// Display name.
    pub label: String,
    /// Suggested fee.
    pub fee: u64,
}

/// The fee presets for the next transfer, and where the fee must go.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FeeTerms {
    /// The collector to pay on a fee-market chain; `None` where fees are off.
    pub collector: Option<String>,
    /// Base fee per byte; 0 where fees are off.
    pub base_fee: u64,
    /// Economy, Standard and Priority.
    pub options: Vec<FeeOption>,
}

/// A node's view of an account.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AccountState {
    /// Spendable balance.
    pub balance: u64,
    /// Next required nonce.
    pub nonce: u64,
}

/// Calls a command and deserializes its reply.
async fn call<A: Serialize, R: for<'de> Deserialize<'de>>(
    command: &str,
    args: &A,
) -> Result<R, String> {
    let payload = serde_wasm_bindgen::to_value(args)
        .map_err(|e| format!("could not encode arguments: {e}"))?;

    let value = tauri_invoke(command, payload).await.map_err(|error| {
        // Tauri rejects with a string carrying the core's error message.
        error
            .as_string()
            .unwrap_or_else(|| format!("{command} failed"))
    })?;

    serde_wasm_bindgen::from_value(value)
        .map_err(|e| format!("could not decode the reply to {command}: {e}"))
}

#[derive(Serialize)]
struct NoArgs {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NameAndPassphrase<'a> {
    name: &'a str,
    passphrase: &'a str,
}

/// Creates a wallet and returns its one-time recovery phrase.
pub async fn create_wallet(name: &str, passphrase: &str) -> Result<CreatedWallet, String> {
    call("create_wallet", &NameAndPassphrase { name, passphrase }).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoverArgs<'a> {
    name: &'a str,
    phrase: &'a str,
    passphrase: &'a str,
}

/// Restores a wallet from a recovery phrase.
pub async fn recover_wallet(name: &str, phrase: &str, passphrase: &str) -> Result<Account, String> {
    call(
        "recover_wallet",
        &RecoverArgs {
            name,
            phrase,
            passphrase,
        },
    )
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PhraseArgs<'a> {
    phrase: &'a str,
}

/// Validates a phrase as the user types, so a bad word is caught early.
pub async fn check_phrase(phrase: &str) -> Result<bool, String> {
    call("check_phrase", &PhraseArgs { phrase }).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnlockArgs<'a> {
    name: &'a str,
    passphrase: &'a str,
    accounts: u32,
}

/// Unlocks a stored wallet and derives `accounts` addresses.
pub async fn unlock(name: &str, passphrase: &str, accounts: u32) -> Result<Vec<Account>, String> {
    call(
        "unlock",
        &UnlockArgs {
            name,
            passphrase,
            accounts,
        },
    )
    .await
}

/// Locks the wallet, zeroizing the seed.
pub async fn lock() -> Result<(), String> {
    call("lock", &NoArgs {}).await
}

/// Whether a wallet exists under `name`.
pub async fn wallet_exists(name: &str) -> Result<bool, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        name: &'a str,
    }
    call("wallet_exists", &Args { name }).await
}

/// Derives an additional account at `index`.
///
/// Deriving is idempotent: the same index always yields the same address, so
/// re-deriving an existing account is harmless.
pub async fn derive_account(index: u32) -> Result<Account, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        index: u32,
    }
    call("derive_account", &Args { index }).await
}

/// Accounts derived so far.
pub async fn list_accounts() -> Result<Vec<Account>, String> {
    call("list_accounts", &NoArgs {}).await
}

/// Parses a scanned QR frame. Decoding happens in Rust, not here.
pub async fn scan_qr(png_base64: &str) -> Result<PaymentRequest, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        png_base64: &'a str,
    }
    call("scan_qr", &Args { png_base64 }).await
}

/// Parses a pasted address or `maya:` URI.
pub async fn parse_payment_uri(input: &str) -> Result<PaymentRequest, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        input: &'a str,
    }
    call("parse_payment_uri", &Args { input }).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferArgs<'a> {
    index: u32,
    recipient: &'a str,
    amount: u64,
    fee: u64,
    nonce: u64,
}

/// Summarises a transfer for confirmation, without signing it.
pub async fn preview_transfer(
    index: u32,
    recipient: &str,
    amount: u64,
    fee: u64,
    nonce: u64,
) -> Result<TransferPreview, String> {
    call(
        "preview_transfer",
        &TransferArgs {
            index,
            recipient,
            amount,
            fee,
            nonce,
        },
    )
    .await
}

/// Signs a transfer. No network access.
///
/// `fee_to` is the collector from [`fee_options`]; `None` burns the fee,
/// which only a chain without a fee market accepts.
pub async fn sign_transfer(
    index: u32,
    recipient: &str,
    amount: u64,
    fee: u64,
    fee_to: Option<&str>,
    nonce: u64,
) -> Result<SignedTransfer, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        index: u32,
        recipient: &'a str,
        amount: u64,
        fee: u64,
        fee_to: Option<&'a str>,
        nonce: u64,
    }
    call(
        "sign_transfer",
        &Args {
            index,
            recipient,
            amount,
            fee,
            fee_to,
            nonce,
        },
    )
    .await
}

/// Broadcasts raw hex to a node.
pub async fn broadcast(node_url: &str, raw_hex: &str) -> Result<String, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        node_url: &'a str,
        raw_hex: &'a str,
    }
    call("broadcast", &Args { node_url, raw_hex }).await
}

/// Records a broadcast outcome in the session history.
pub async fn record_outcome(
    txid: &str,
    broadcast_ok: bool,
    detail: Option<String>,
) -> Result<(), String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        txid: &'a str,
        broadcast_ok: bool,
        detail: Option<String>,
    }
    call(
        "record_outcome",
        &Args {
            txid,
            broadcast_ok,
            detail,
        },
    )
    .await
}

/// Transfers signed this session.
pub async fn history() -> Result<Vec<PendingTransfer>, String> {
    call("history", &NoArgs {}).await
}

/// Reads balance and nonce from a node.
pub async fn fetch_account(node_url: &str, address: &str) -> Result<AccountState, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        node_url: &'a str,
        address: &'a str,
    }
    call("fetch_account", &Args { node_url, address }).await
}

/// Fee presets for account `index`, priced by the node at `node_url`.
pub async fn fee_options(node_url: &str, index: u32, nonce: u64) -> Result<FeeTerms, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        node_url: &'a str,
        index: u32,
        nonce: u64,
    }
    call(
        "fee_options",
        &Args {
            node_url,
            index,
            nonce,
        },
    )
    .await
}
