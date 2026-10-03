//! Tauri command surface: the bridge between the WASM frontend and the wallet
//! core.
//!
//! ## Why this is a submodule
//!
//! `#[tauri::command]` on a `pub` function emits a `#[macro_export]` macro,
//! which lands at the crate root, *and* a `pub use` of that macro in the
//! defining module. At the crate root those are the same name in the same
//! namespace, so defining the commands in `lib.rs` fails to compile with
//! E0255. Keeping them one module down separates the two.
//!
//! ## What may cross this boundary
//!
//! Every function here is a deliberate hole in the trust boundary, so the rule
//! is explicit: **no key material leaves the core**. Commands return addresses,
//! payload summaries, and signed hex. They never return a seed, a private key,
//! or a chain code.
//!
//! The one exception is [`create_wallet`], which returns the recovery phrase
//! exactly once so the user can write it down. There is no command to retrieve
//! it afterwards — a wallet that can re-display its own phrase on demand is one
//! compromised frontend away from leaking it.
//!
//! ## Session state
//!
//! An unlocked [`Wallet`] lives in Tauri's managed state behind a mutex. Locking
//! drops it, which zeroizes the seed.
use std::sync::Mutex;

use maya_wallet_core::hd::seed_from_mnemonic;
use maya_wallet_core::payment::{FeeTier, PaymentRequest, SignedTransfer};
use maya_wallet_core::vault::{OsKeychain, Vault};
use maya_wallet_core::wallet::{
    Account, ChainTag, PendingTransfer, SignTerms, TransferStatus, Wallet,
};
use maya_wallet_core::{generate_mnemonic, payment, validate_mnemonic, wallet};
use serde::{Deserialize, Serialize};

/// The unlocked session, if any.
#[derive(Default)]
pub struct Session {
    wallet: Mutex<Option<Wallet>>,
}

impl Session {
    /// Runs `body` against the unlocked wallet.
    fn with<T>(&self, body: impl FnOnce(&mut Wallet) -> Result<T, String>) -> Result<T, String> {
        let mut guard = self
            .wallet
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.as_mut() {
            Some(wallet) => body(wallet),
            None => Err("the wallet is locked".to_string()),
        }
    }
}

/// A newly created wallet.
///
/// Carries the recovery phrase, and is the only response that ever does.
#[derive(Serialize)]
pub struct CreatedWallet {
    /// The 24-word recovery phrase. Displayed once, never stored in the UI.
    pub mnemonic: String,
    /// The first account, so the UI has something to show immediately.
    pub account: Account,
}

/// A summary of what a signature would authorize.
///
/// Shown for confirmation *before* signing. Users approve a rendered summary,
/// so the summary must describe exactly what gets signed and nothing else.
#[derive(Serialize)]
pub struct TransferPreview {
    /// Hex-encoded sender.
    pub sender: String,
    /// Hex-encoded recipient.
    pub recipient: String,
    /// Amount to the recipient.
    pub amount: u64,
    /// Fee, burned.
    pub fee: u64,
    /// Amount plus fee.
    pub total: u64,
    /// Nonce that will be used.
    pub nonce: u64,
}

/// A selectable fee preset.
#[derive(Serialize)]
pub struct FeeOption {
    /// Display name.
    pub label: String,
    /// Suggested fee.
    pub fee: u64,
}

/// The fee presets for the next transfer, and where the fee must go.
#[derive(Serialize)]
pub struct FeeTerms {
    /// The collector to pay on a fee-market chain; `None` where fees are off.
    pub collector: Option<String>,
    /// Base fee per byte; 0 where fees are off.
    pub base_fee: u64,
    /// Economy, Standard and Priority.
    pub options: Vec<FeeOption>,
    /// The node's genesis id, hex: what `sign_transfer` signs for (ADR-036).
    pub genesis: String,
}

/// Generates a wallet and stores its sealed seed in the OS keychain.
///
/// Returns the recovery phrase once. It is never retrievable afterwards.
#[tauri::command]
pub fn create_wallet(
    name: String,
    passphrase: String,
    session: tauri::State<'_, Session>,
) -> Result<CreatedWallet, String> {
    let mnemonic = generate_mnemonic().map_err(|e| e.to_string())?;

    // An empty BIP-39 passphrase here is deliberate: the wallet passphrase
    // protects the keychain envelope. Using it as the BIP-39 passphrase too
    // would mean changing it silently changed every derived address.
    let seed = seed_from_mnemonic(&mnemonic, "").map_err(|e| e.to_string())?;

    Vault::new(OsKeychain::new())
        .store(&name, &seed, &passphrase)
        .map_err(|e| e.to_string())?;

    let mut wallet = Wallet::from_seed(seed);
    let account = wallet.account(0).map_err(|e| e.to_string())?;

    *session
        .wallet
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(wallet);

    Ok(CreatedWallet {
        mnemonic: mnemonic.to_string(),
        account,
    })
}

/// Restores a wallet from a recovery phrase.
#[tauri::command]
pub fn recover_wallet(
    name: String,
    phrase: String,
    passphrase: String,
    session: tauri::State<'_, Session>,
) -> Result<Account, String> {
    // Validate before touching the keychain, so a typo does not create a
    // half-configured wallet.
    validate_mnemonic(&phrase).map_err(|e| e.to_string())?;

    let seed = seed_from_mnemonic(&phrase, "").map_err(|e| e.to_string())?;

    Vault::new(OsKeychain::new())
        .replace(&name, &seed, &passphrase)
        .map_err(|e| e.to_string())?;

    let mut wallet = Wallet::from_seed(seed);
    let account = wallet.account(0).map_err(|e| e.to_string())?;

    *session
        .wallet
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(wallet);

    Ok(account)
}

/// Checks a phrase without storing anything.
///
/// Lets the UI validate as the user types, so a bad word is caught before it
/// becomes a wallet nobody can recover.
#[tauri::command]
pub fn check_phrase(phrase: String) -> Result<bool, String> {
    Ok(validate_mnemonic(&phrase).is_ok())
}

/// Unlocks a stored wallet.
#[tauri::command]
pub fn unlock(
    name: String,
    passphrase: String,
    accounts: u32,
    session: tauri::State<'_, Session>,
) -> Result<Vec<Account>, String> {
    let seed = Vault::new(OsKeychain::new())
        .unlock(&name, &passphrase)
        .map_err(|e| e.to_string())?;

    let mut wallet = Wallet::from_seed(seed);
    for index in 0..accounts.max(1) {
        wallet.account(index).map_err(|e| e.to_string())?;
    }
    let derived = wallet.accounts();

    *session
        .wallet
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(wallet);

    Ok(derived)
}

/// Locks the wallet, dropping and zeroizing the seed.
#[tauri::command]
pub fn lock(session: tauri::State<'_, Session>) -> Result<(), String> {
    *session
        .wallet
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    Ok(())
}

/// Whether a wallet is currently unlocked.
#[tauri::command]
pub fn is_unlocked(session: tauri::State<'_, Session>) -> Result<bool, String> {
    Ok(session
        .wallet
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some())
}

/// Whether a wallet exists under `name`.
#[tauri::command]
pub fn wallet_exists(name: String) -> Result<bool, String> {
    Vault::new(OsKeychain::new())
        .exists(&name)
        .map_err(|e| e.to_string())
}

/// Derives an additional account.
#[tauri::command]
pub fn derive_account(index: u32, session: tauri::State<'_, Session>) -> Result<Account, String> {
    session.with(|wallet| wallet.account(index).map_err(|e| e.to_string()))
}

/// Accounts derived so far.
#[tauri::command]
pub fn list_accounts(session: tauri::State<'_, Session>) -> Result<Vec<Account>, String> {
    session.with(|wallet| Ok(wallet.accounts()))
}

/// Parses a scanned QR image.
///
/// Takes the raw PNG as base64 from the camera. Decoding and parsing both
/// happen in Rust: a QR payload is attacker-controlled, and parsing it in the
/// frontend would put that string inside the trust boundary.
#[tauri::command]
pub fn scan_qr(png_base64: String) -> Result<PaymentRequest, String> {
    use base64::Engine;

    let png = base64::engine::general_purpose::STANDARD
        .decode(png_base64.as_bytes())
        .map_err(|e| format!("image is not valid base64: {e}"))?;

    payment::scan_payment_request(&png).map_err(|e| e.to_string())
}

/// Parses a pasted address or `maya:` URI.
#[tauri::command]
pub fn parse_payment_uri(input: String) -> Result<PaymentRequest, String> {
    PaymentRequest::parse(&input).map_err(|e| e.to_string())
}

/// Summarises a transfer for confirmation, without signing it.
#[tauri::command]
pub fn preview_transfer(
    index: u32,
    recipient: String,
    amount: u64,
    fee: u64,
    nonce: u64,
    session: tauri::State<'_, Session>,
) -> Result<TransferPreview, String> {
    let recipient = payment::normalize_address(&recipient).map_err(|e| e.to_string())?;
    let total = amount
        .checked_add(fee)
        .ok_or_else(|| "amount and fee overflow".to_string())?;

    session.with(|wallet| {
        let account = wallet.account(index).map_err(|e| e.to_string())?;
        Ok(TransferPreview {
            sender: account.address,
            recipient: recipient.clone(),
            amount,
            fee,
            total,
            nonce,
        })
    })
}

/// Signs a transfer. No network access.
///
/// `fee_to` is the collector from [`fee_options`]; without it the fee is
/// burned, which only a chain without a fee market accepts. `genesis` is the
/// hex genesis id from [`fee_options`]: the one chain the signature is valid on.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri maps each JS argument to one parameter.
pub fn sign_transfer(
    index: u32,
    recipient: String,
    amount: u64,
    fee: u64,
    fee_to: Option<String>,
    nonce: u64,
    genesis: String,
    session: tauri::State<'_, Session>,
) -> Result<SignedTransfer, String> {
    let chain = parse_genesis(&genesis)?;
    let terms = SignTerms {
        fee,
        fee_to: fee_to.as_deref(),
        nonce,
        chain: &chain,
    };
    session.with(|wallet| {
        wallet
            .sign(index, &recipient, amount, &terms)
            .map_err(|e| e.to_string())
    })
}

/// A genesis id as `fee_options` returned it: 64 hex characters.
fn parse_genesis(genesis: &str) -> Result<ChainTag, String> {
    hex::decode(genesis)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .map(ChainTag::from_genesis)
        .ok_or_else(|| "the node's genesis id is missing or malformed; reload fees".to_string())
}

/// Broadcasts raw transaction hex to a node.
///
/// Takes hex rather than reaching into the session, so a transaction signed on
/// an air-gapped device can be broadcast from a machine that never held a key.
#[tauri::command]
pub async fn broadcast(node_url: String, raw_hex: String) -> Result<String, String> {
    wallet::broadcast(&node_url, &raw_hex)
        .await
        .map_err(|e| e.to_string())
}

/// Records the outcome of a broadcast in the session history.
#[tauri::command]
pub fn record_outcome(
    txid: String,
    broadcast_ok: bool,
    detail: Option<String>,
    session: tauri::State<'_, Session>,
) -> Result<(), String> {
    let status = if broadcast_ok {
        TransferStatus::Broadcast
    } else {
        TransferStatus::Failed
    };
    session.with(|wallet| {
        wallet.record_outcome(&txid, status, detail.clone());
        Ok(())
    })
}

/// Transfers signed this session, newest first.
#[tauri::command]
pub fn history(session: tauri::State<'_, Session>) -> Result<Vec<PendingTransfer>, String> {
    session.with(|wallet| Ok(wallet.history()))
}

/// Reads an account's balance and next nonce from a node.
#[tauri::command]
pub async fn fetch_account(node_url: String, address: String) -> Result<AccountState, String> {
    let (balance, nonce) = wallet::fetch_account(&node_url, &address)
        .await
        .map_err(|e| e.to_string())?;
    Ok(AccountState { balance, nonce })
}

/// A node's view of an account.
#[derive(Serialize, Deserialize)]
pub struct AccountState {
    /// Spendable balance.
    pub balance: u64,
    /// Nonce the next transaction must carry.
    pub nonce: u64,
}

/// The fee presets for a transfer from account `index`, priced by the node.
///
/// On a fee-market chain (ADR-029) each tier is a multiple of
/// `base_fee x size`, and the fee must be paid to the collector the node
/// names; a flat guess would be refused as underpaid. Where fees are off,
/// the fixed tiers stand and nothing is required.
#[tauri::command]
pub async fn fee_options(
    node_url: String,
    index: u32,
    nonce: u64,
    session: tauri::State<'_, Session>,
) -> Result<FeeTerms, String> {
    let quote = wallet::fee_quote(&node_url)
        .await
        .map_err(|e| e.to_string())?;
    let chain = wallet::chain_info(&node_url)
        .await
        .map_err(|e| e.to_string())?;
    let genesis = chain.to_string();
    let tiers = FeeTier::all();
    if !quote.active {
        return Ok(FeeTerms {
            collector: None,
            base_fee: 0,
            genesis,
            options: tiers
                .iter()
                .map(|tier| FeeOption {
                    label: tier.label().to_string(),
                    fee: tier.suggested_fee(),
                })
                .collect(),
        });
    }
    let size = session.with(|wallet| {
        wallet
            .transfer_size(index, &quote.collector, nonce, &chain)
            .map_err(|e| e.to_string())
    })?;
    let fees = payment::priced_fee_tiers(quote.base_fee, size).map_err(|e| e.to_string())?;
    Ok(FeeTerms {
        collector: Some(quote.collector),
        base_fee: quote.base_fee,
        genesis,
        options: tiers
            .iter()
            .zip(fees)
            .map(|(tier, fee)| FeeOption {
                label: tier.label().to_string(),
                fee,
            })
            .collect(),
    })
}
