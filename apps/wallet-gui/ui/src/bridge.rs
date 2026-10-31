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
    /// Requested amount, if any: a decimal string, as the wallet core sends
    /// every 256-bit amount (ADR-046).
    pub amount: Option<String>,
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
    /// Amount sent, a decimal string (256-bit, ADR-046).
    pub amount: String,
    /// Nonce used.
    pub nonce: u64,
    /// Fee, a decimal string.
    pub fee: String,
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

/// What the confirm screen shows. Computed by the wallet core from the exact
/// transaction that would be signed; the UI renders it and adds nothing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Confirmation {
    /// Hash of the signing bytes. Handed back to `sign_transfer`.
    pub signing_hash: String,
    /// One sentence from `describe`.
    pub description: String,
    /// `describe`'s detail lines, in full.
    pub details: Vec<(String, String)>,
    /// Genesis id the signature commits to, hex.
    pub chain_id: String,
    /// Sender address, hex.
    pub sender: String,
    /// Nonce.
    pub nonce: u64,
    /// Recipient, checksummed hex (transfers only).
    pub recipient: Option<String>,
    /// Amount in display units (transfers only).
    pub amount: Option<String>,
    /// Fee in display units (transfers only).
    pub fee: Option<String>,
    /// Fee above the amount: shown as a warning.
    pub fee_exceeds_amount: bool,
}

/// A fee preset.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeeOption {
    /// Display name.
    pub label: String,
    /// Suggested fee, base units as a decimal string.
    pub fee: String,
}

/// The fee presets for the next transfer, and where the fee must go.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FeeTerms {
    /// The collector to pay on a fee-market chain; `None` where fees are off.
    pub collector: Option<String>,
    /// Base fee per byte, a decimal string; "0" where fees are off.
    pub base_fee: String,
    /// Economy, Standard and Priority.
    pub options: Vec<FeeOption>,
    /// The node's genesis id, hex: the one chain a signature is valid on
    /// (ADR-036). Empty until the node has answered.
    pub genesis: String,
}

/// A node's view of an account.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AccountState {
    /// Spendable balance, base units as a decimal string.
    pub balance: String,
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
struct PreviewArgs<'a> {
    index: u32,
    recipient: &'a str,
    amount: &'a str,
    fee: &'a str,
    fee_to: Option<&'a str>,
    nonce: u64,
    genesis: &'a str,
}

/// Computes the confirm screen for a transfer. `amount` and `fee` are
/// base-unit decimal strings: the IPC never carries a JS number. Signs nothing.
pub async fn preview_transfer(
    index: u32,
    recipient: &str,
    amount: &str,
    fee: &str,
    nonce: u64,
    terms: &FeeTerms,
) -> Result<Confirmation, String> {
    call(
        "preview_transfer",
        &PreviewArgs {
            index,
            recipient,
            amount,
            fee,
            fee_to: terms.collector.as_deref(),
            nonce,
            genesis: &terms.genesis,
        },
    )
    .await
}

/// Signs a transfer. No network access.
///
/// `terms.collector` is the fee collector (`None` burns the fee, which only a
/// chain without a fee market accepts) and `terms.genesis` the chain the
/// signature is valid on; both come from [`fee_options`].
pub async fn sign_transfer(
    index: u32,
    recipient: &str,
    amount: &str,
    fee: &str,
    nonce: u64,
    terms: &FeeTerms,
    confirmed_hash: &str,
) -> Result<SignedTransfer, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        index: u32,
        recipient: &'a str,
        amount: &'a str,
        fee: &'a str,
        fee_to: Option<&'a str>,
        nonce: u64,
        genesis: &'a str,
        confirmed_hash: &'a str,
    }
    call(
        "sign_transfer",
        &Args {
            index,
            recipient,
            amount,
            fee,
            fee_to: terms.collector.as_deref(),
            nonce,
            genesis: &terms.genesis,
            confirmed_hash,
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

/// The network the wallet is talking to.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NetworkStatus {
    /// The network's name (`maya-testnet-1`), if the node gave one.
    pub network: Option<String>,
    /// Genesis id, hex: the chain this wallet's signatures are for.
    pub genesis: String,
    /// The node's tip height.
    pub height: u64,
    /// `mainnet`, `test` or `impostor`, decided by the wallet core from the
    /// genesis id against the pinned mainnet genesis, never from the name.
    /// Missing reads as `test`: the safe direction to be wrong in.
    #[serde(default)]
    pub class: String,
    /// The genesis id's two ends, to compare with the published one.
    #[serde(default)]
    pub fingerprint: String,
}

impl NetworkStatus {
    /// Whether this is a test network, whose coins have no value. Only a
    /// pinned mainnet genesis is mainnet: a node's name is a claim, and a
    /// wallet that mistook a testnet for mainnet is the dangerous direction
    /// to be wrong in.
    #[must_use]
    pub fn is_test_network(&self) -> bool {
        self.class != "mainnet"
    }

    /// A node that claims to be mainnet with a genesis that is not pinned.
    #[must_use]
    pub fn is_impostor(&self) -> bool {
        self.class == "impostor"
    }
}

/// Reads the network's name, genesis and tip height.
pub async fn network_status(node_url: &str) -> Result<NetworkStatus, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        node_url: &'a str,
    }
    call("network_status", &Args { node_url }).await
}

/// What the Receive screen shows.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiveRequest {
    /// The account's address, hex.
    pub address: String,
    /// The `maya:` URI the code encodes.
    pub uri: String,
    /// The QR code as an SVG document, drawn by the wallet core.
    pub qr_svg: String,
}

/// Builds the Receive screen's request for account `index`.
pub async fn receive_request(index: u32, amount: Option<String>) -> Result<ReceiveRequest, String> {
    #[derive(Serialize)]
    struct Args {
        index: u32,
        amount: Option<String>,
    }
    call("receive_request", &Args { index, amount }).await
}

#[wasm_bindgen]
unsafe extern "C" {
    /// The webview's clipboard. Writing needs no permission prompt inside a
    /// user gesture, which is the only place this is called from.
    #[wasm_bindgen(js_namespace = ["navigator", "clipboard"], js_name = writeText, catch)]
    async fn clipboard_write_text(text: &str) -> Result<JsValue, JsValue>;
}

/// Copies `text` to the clipboard.
pub async fn copy_to_clipboard(text: &str) -> Result<(), String> {
    clipboard_write_text(text).await.map(|_| ()).map_err(|e| {
        e.as_string()
            .unwrap_or_else(|| "the clipboard refused".to_string())
    })
}
