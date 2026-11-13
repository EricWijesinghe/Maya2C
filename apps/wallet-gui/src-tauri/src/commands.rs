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
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use maya_clear_sign::Context;
use maya_wallet_core::address_book::{AddressBook, NameCheck, PasteCheck, Source};
use maya_wallet_core::networks;

use maya_wallet_core::confirm::{Confirmation, approval_message};
use maya_wallet_core::hd::seed_from_mnemonic;
use maya_wallet_core::payment::{FeeTier, PaymentRequest, SignedTransfer};
use maya_wallet_core::vault::{OsKeychain, Vault};
use maya_wallet_core::wallet::{
    Account, ChainTag, PendingTransfer, SignTerms, TransferStatus, Wallet,
};
use maya_wallet_core::{generate_mnemonic, payment, validate_mnemonic, wallet};
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// The unlocked session, if any.
#[derive(Default)]
pub struct Session {
    wallet: Mutex<Option<Wallet>>,
    /// The unlocked wallet's name: where its address book is stored.
    name: Mutex<Option<String>>,
    /// True while a sign_transfer call holds the one native confirm screen.
    dialog_open: AtomicBool,
}

/// Holds the single confirm-screen slot; releases it on every return path.
pub(crate) struct DialogSlot<'a>(&'a AtomicBool);

impl<'a> DialogSlot<'a> {
    pub(crate) fn take(flag: &'a AtomicBool) -> Result<Self, String> {
        if flag.swap(true, Ordering::AcqRel) {
            return Err("a confirm screen is already open; nothing was signed".to_string());
        }
        Ok(Self(flag))
    }
}

impl Drop for DialogSlot<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Session {
    /// Runs `body` against the unlocked wallet.
    pub(crate) fn with<T>(
        &self,
        body: impl FnOnce(&mut Wallet) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = self
            .wallet
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.as_mut() {
            Some(wallet) => body(wallet),
            None => Err("the wallet is locked".to_string()),
        }
    }

    /// The one native confirm screen's slot, shared by every signing command.
    pub(crate) fn dialog_open(&self) -> &AtomicBool {
        &self.dialog_open
    }

    /// Opens a session over `wallet`, stored under `name`.
    fn open(&self, name: &str, wallet: Wallet) {
        *self.wallet.lock().unwrap_or_else(|p| p.into_inner()) = Some(wallet);
        *self.name.lock().unwrap_or_else(|p| p.into_inner()) = Some(name.to_string());
    }

    /// Runs `body` against the unlocked wallet's address book and the
    /// addresses paid this session; saves the book afterwards when `save`.
    fn with_book<T>(
        &self,
        app: &tauri::AppHandle,
        save: bool,
        body: impl FnOnce(&mut AddressBook, &BTreeSet<[u8; 32]>) -> Result<T, String>,
    ) -> Result<T, String> {
        let name = self
            .name
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or("the wallet is locked")?;
        let (key, paid) = self.with(|wallet| {
            let paid = wallet
                .history()
                .iter()
                .filter_map(|p| payment::decode_address(&p.transfer.recipient).ok())
                .collect();
            Ok((wallet.address_book_key(), paid))
        })?;
        let path = book_path(app, &name)?;
        let mut book = AddressBook::load_file(&path, &key).map_err(|e| e.to_string())?;
        let out = body(&mut book, &paid)?;
        if save {
            book.save_file(&path, &key).map_err(|e| e.to_string())?;
        }
        Ok(out)
    }

    /// The review context for a confirm screen: the saved contacts.
    ///
    /// shortcut: no balances, so `DrainsBalance` is not raised in the GUI yet;
    /// pass the node-read balance in once the core reads it itself.
    pub(crate) fn review_context(&self, app: &tauri::AppHandle) -> Result<Context, String> {
        self.with_book(app, false, |book, _| {
            Ok(Context {
                contacts: book.review_contacts(),
                ..Context::default()
            })
        })
    }
}

/// Where wallet `name`'s sealed address book lives: a file in the app's data
/// directory, because the Windows keychain caps a secret at 2,560 bytes. The
/// name came from the webview, so it is held to a plain file stem: no
/// separators, no `..`, nothing that could point the write elsewhere.
fn book_path(app: &tauri::AppHandle, name: &str) -> Result<std::path::PathBuf, String> {
    const MAX_NAME: usize = 64;
    if name.is_empty()
        || name.len() > MAX_NAME
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("a wallet name is letters, digits, '-' and '_' only".to_string());
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no data directory for the address book: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir.join(format!("{name}.contacts")))
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

/// A selectable fee preset.
#[derive(Serialize)]
pub struct FeeOption {
    /// Display name.
    pub label: String,
    /// Suggested fee, base units as a decimal string.
    pub fee: String,
}

/// The fee presets for the next transfer, and where the fee must go.
#[derive(Serialize)]
pub struct FeeTerms {
    /// The collector to pay on a fee-market chain; `None` where fees are off.
    pub collector: Option<String>,
    /// Base fee per byte, a decimal string; "0" where fees are off.
    pub base_fee: String,
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

    session.open(&name, wallet);

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

    session.open(&name, wallet);

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

    session.open(&name, wallet);

    Ok(derived)
}

/// Locks the wallet, dropping and zeroizing the seed.
#[tauri::command]
pub fn lock(session: tauri::State<'_, Session>) -> Result<(), String> {
    *session
        .wallet
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *session.name.lock().unwrap_or_else(|p| p.into_inner()) = None;
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
pub fn scan_qr(png_base64: String, genesis: Option<String>) -> Result<PaymentRequest, String> {
    use base64::Engine;

    let png = base64::engine::general_purpose::STANDARD
        .decode(png_base64.as_bytes())
        .map_err(|e| format!("image is not valid base64: {e}"))?;

    let request = payment::scan_payment_request(&png).map_err(|e| e.to_string())?;
    checked_request(request, genesis.as_deref())
}

/// This machine's clock in Unix seconds: wallet wall-clock, never consensus.
fn unix_now() -> Result<u64, String> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "this computer's clock is before 1970; fix it to read payment requests")?
        .as_secs())
}

/// Refuses a request for another network or past its `exp`, before the send
/// form opens (ADR-086 §7). `exp` is this machine's clock, not consensus.
pub(crate) fn checked_request(
    request: PaymentRequest,
    genesis: Option<&str>,
) -> Result<PaymentRequest, String> {
    let now = unix_now()?;
    request
        .check(networks::address_network(genesis.unwrap_or_default()), now)
        .map_err(|e| e.to_string())?;
    Ok(request)
}

/// Parses a pasted address or `maya:` URI.
#[tauri::command]
pub fn parse_payment_uri(input: String, genesis: Option<String>) -> Result<PaymentRequest, String> {
    let request = PaymentRequest::parse(&input).map_err(|e| e.to_string())?;
    checked_request(request, genesis.as_deref())
}

// ---------------------------------------------------------------------------
// address book (ADR-086 §7)
// ---------------------------------------------------------------------------

/// One saved address as the Contacts screen shows it.
#[derive(Serialize)]
pub struct ContactView {
    /// Bech32m, ungrouped: what Copy and "Pay" use.
    pub address: String,
    /// Bech32m in groups of four.
    pub grouped: String,
    /// The user's name for it.
    pub name: String,
    /// The user's note.
    pub note: String,
}

/// The saved contacts, in the order added.
#[tauri::command]
pub fn contacts(
    app: tauri::AppHandle,
    session: tauri::State<'_, Session>,
) -> Result<Vec<ContactView>, String> {
    session.with_book(&app, false, |book, _| {
        Ok(book
            .contacts()
            .iter()
            .map(|c| {
                // `open` refuses an unknown prefix, so this default is never taken.
                let network = maya_address::Network::from_hrp(&c.network)
                    .unwrap_or(maya_address::Network::Devnet);
                ContactView {
                    address: maya_address::encode(network, &c.address),
                    grouped: maya_clear_sign::grouped(network, &c.address),
                    name: c.name.clone(),
                    note: c.note.clone(),
                }
            })
            .collect())
    })
}

/// Saves `address` under the name the user typed, after a native OK/Cancel
/// dialog whose words the core wrote: a compromised webview cannot save an
/// attacker's address as a trusted contact without the person seeing it.
/// `scanned` marks one read from a QR code. The name never comes from a
/// payment request's label.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri maps each JS argument to one parameter.
pub async fn add_contact(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    address: String,
    name: String,
    note: String,
    scanned: bool,
    genesis: String,
    session: tauri::State<'_, Session>,
) -> Result<NameCheck, String> {
    let network = networks::address_network(&genesis);
    let id = payment::decode_address_for(&address, network).map_err(|e| e.to_string())?;
    let source = if scanned {
        Source::Scanned
    } else {
        Source::Manual
    };
    let preview = session.with_book(&app, false, |book, _| {
        book.check_add(&id, &name).map_err(|e| e.to_string())
    })?;
    let mut lines: Vec<String> = preview
        .warnings
        .iter()
        .map(|w| format!("Warning: {w}"))
        .collect();
    lines.push(format!("Save as: {}", preview.name));
    lines.push(format!(
        "Address: {}",
        maya_clear_sign::grouped(network, &id)
    ));
    if !native_ok(&app, &window, "Save contact", lines.join("\n"), "Save").await? {
        return Err("cancelled on the native dialog; nothing was saved".to_string());
    }
    session.with_book(&app, true, |book, _| {
        book.add(id, network, &name, &note, source, 0)
            .map_err(|e| e.to_string())
    })
}

/// Deletes a saved address after a native OK/Cancel dialog, so a compromised
/// webview cannot quietly remove the real contact a look-alike imitates.
/// Returns whether it was deleted.
#[tauri::command]
pub async fn remove_contact(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    address: String,
    session: tauri::State<'_, Session>,
) -> Result<bool, String> {
    let id = payment::decode_address(&address).map_err(|e| e.to_string())?;
    let Some((name, hrp)) = session.with_book(&app, false, |book, _| {
        Ok(book.find(&id).map(|c| (c.name.clone(), c.network.clone())))
    })?
    else {
        return Ok(false);
    };
    let network = maya_address::Network::from_hrp(&hrp).unwrap_or(maya_address::Network::Devnet);
    let message = format!(
        "Delete the contact '{name}'?\nAddress: {}",
        maya_clear_sign::grouped(network, &id)
    );
    if !native_ok(&app, &window, "Delete contact", message, "Delete").await? {
        return Ok(false);
    }
    session.with_book(&app, true, |book, _| Ok(book.remove(&id)))
}

/// The address book sealed for export, base64. Opens only in a wallet
/// restored from the same recovery phrase.
#[tauri::command]
pub fn export_contacts(
    app: tauri::AppHandle,
    session: tauri::State<'_, Session>,
) -> Result<String, String> {
    use base64::Engine;
    let key = session.with(|wallet| Ok(wallet.address_book_key()))?;
    session.with_book(&app, false, |book, _| {
        let sealed = book.seal(&key).map_err(|e| e.to_string())?;
        Ok(base64::engine::general_purpose::STANDARD.encode(sealed))
    })
}

/// Adds every entry of an export that is not already saved. Returns the count.
#[tauri::command]
pub fn import_contacts(
    app: tauri::AppHandle,
    data: String,
    session: tauri::State<'_, Session>,
) -> Result<usize, String> {
    use base64::Engine;
    let sealed = base64::engine::general_purpose::STANDARD
        .decode(data.trim().as_bytes())
        .map_err(|e| format!("not an address-book export: {e}"))?;
    let key = session.with(|wallet| Ok(wallet.address_book_key()))?;
    session.with_book(&app, true, |book, _| {
        book.import(&sealed, &key).map_err(|e| e.to_string())
    })
}

/// Checks a pasted recipient before it fills the send form: saved, a
/// look-alike of a saved one, or new (the interstitial is then mandatory).
#[tauri::command]
pub fn check_recipient(
    app: tauri::AppHandle,
    text: String,
    genesis: String,
    session: tauri::State<'_, Session>,
) -> Result<PasteCheck, String> {
    let network = networks::address_network(&genesis);
    session.with_book(&app, false, |book, paid| {
        book.check_paste(&text, network, paid)
            .map_err(|e| e.to_string())
    })
}

/// The confirm screen's content for a transfer, computed by the wallet core
/// from the transaction that would be signed. Signs nothing.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri maps each JS argument to one parameter.
pub fn preview_transfer(
    app: tauri::AppHandle,
    index: u32,
    recipient: String,
    amount: String,
    fee: String,
    fee_to: Option<String>,
    nonce: u64,
    genesis: String,
    session: tauri::State<'_, Session>,
) -> Result<Confirmation, String> {
    let chain = parse_genesis(&genesis)?;
    let amount = parse_amount_arg("amount", &amount)?;
    let terms = SignTerms {
        fee: parse_amount_arg("fee", &fee)?,
        fee_to: fee_to.as_deref(),
        nonce,
        chain: &chain,
        allow_high_fee: false,
    };
    let ctx = session.review_context(&app)?;
    session.with(|wallet| {
        wallet
            .confirm_with(index, &recipient, amount, &terms, &ctx)
            .map_err(|e| e.to_string())
    })
}

/// Parses an amount argument from the frontend: plain base-unit digits only.
/// The name goes into the error so the user sees which field was refused.
pub(crate) fn parse_amount_arg(name: &str, text: &str) -> Result<primitive_types::U256, String> {
    maya_wallet_core::amount::parse_base_units(text).map_err(|e| format!("{name}: {e}"))
}

/// Signs a transfer, but only after the person answers a native OK/Cancel
/// dialog. No network access.
///
/// The dialog text is built by the core from the transaction it would sign, so
/// a compromised webview cannot change what the signer reads. `fee_to` is the
/// collector from [`fee_options`]; without it the fee is burned, which only a
/// chain without a fee market accepts. `genesis` is the hex genesis id from
/// [`fee_options`]. `confirmed_hash` must be the `signing_hash`
/// [`preview_transfer`] returned for these same arguments.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri maps each JS argument to one parameter.
pub async fn sign_transfer(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    index: u32,
    recipient: String,
    amount: String,
    fee: String,
    fee_to: Option<String>,
    nonce: u64,
    genesis: String,
    confirmed_hash: String,
    expires_at: Option<u64>,
    session: tauri::State<'_, Session>,
) -> Result<SignedTransfer, String> {
    // Taken first: a refused concurrent call must not touch the reviewed slot.
    let _slot = DialogSlot::take(&session.dialog_open)?;
    // The request may have expired while the form sat open (ADR-086 §7).
    if expires_at.is_some_and(|exp| unix_now().is_ok_and(|now| now > exp)) {
        return Err("the payment request has expired; nothing was signed".to_string());
    }
    let chain = parse_genesis(&genesis)?;
    let amount = parse_amount_arg("amount", &amount)?;
    let fee = parse_amount_arg("fee", &fee)?;
    // Refused before the dialog, so a person is never asked to approve a fee the core would not sign.
    wallet::check_fee_cap(fee, amount, false).map_err(|e| e.to_string())?;
    let terms = SignTerms {
        fee,
        fee_to: fee_to.as_deref(),
        nonce,
        chain: &chain,
        allow_high_fee: false,
    };
    // Rust computes the confirmation; the reviewed slot is set here by `confirm`.
    let ctx = session.review_context(&app)?;
    let shown = session.with(|wallet| {
        wallet
            .confirm_with(index, &recipient, amount, &terms, &ctx)
            .map_err(|e| e.to_string())
    })?;
    if shown.signing_hash != confirmed_hash {
        return Err(
            "the transaction differs from the one on the confirm screen; nothing was signed"
                .to_string(),
        );
    }
    if !approved_on_screen(&app, &window, &shown).await? {
        return Err("cancelled on the native confirm screen; nothing was signed".to_string());
    }
    session.with(|wallet| {
        wallet
            .sign(index, &recipient, amount, &terms, &shown.signing_hash)
            .map_err(|e| e.to_string())
    })
}

/// The native OK/Cancel screen for a signature. Its words come from the core's
/// [`Confirmation`], never from the webview.
async fn approved_on_screen(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    shown: &Confirmation,
) -> Result<bool, String> {
    native_ok(
        app,
        window,
        "Confirm transfer",
        approval_message(shown),
        "Sign",
    )
    .await
}

/// A native OK/Cancel dialog with words the core wrote; `ok` labels the
/// button that proceeds.
pub(crate) async fn native_ok(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    title: &str,
    message: String,
    ok: &str,
) -> Result<bool, String> {
    let question = app
        .dialog()
        .message(message)
        .parent(window)
        .title(title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            ok.into(),
            "Cancel".into(),
        ));
    // A blocking dialog must not hold an async worker thread (as in app-update).
    tauri::async_runtime::spawn_blocking(move || question.blocking_show())
        .await
        .map_err(|e| e.to_string())
}

/// A genesis id as `fee_options` returned it: 64 hex characters.
pub(crate) fn parse_genesis(genesis: &str) -> Result<ChainTag, String> {
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
    Ok(AccountState {
        balance: balance.to_string(),
        nonce,
    })
}

/// The network the wallet is talking to: its name, genesis and tip height.
#[tauri::command]
pub async fn network_status(node_url: String) -> Result<wallet::NetworkStatus, String> {
    wallet::network_status(&node_url)
        .await
        .map_err(|e| e.to_string())
}

/// What the Receive screen shows for account `index`: the address, the
/// `maya:` URI and that URI as a QR code. No network access.
#[derive(Serialize, Deserialize)]
pub struct ReceiveRequest {
    /// The account's address, Bech32m: what Copy puts on the clipboard.
    pub address: String,
    /// The same address in groups of four, for reading aloud or comparing.
    pub grouped: String,
    /// The payment URI the QR code encodes.
    pub uri: String,
    /// The QR code, as an SVG document.
    pub qr_svg: String,
}

/// Builds the Receive screen's request for account `index`, optionally
/// asking for `amount`.
#[tauri::command]
pub fn receive_request(
    index: u32,
    amount: Option<String>,
    genesis: Option<String>,
    session: tauri::State<'_, Session>,
) -> Result<ReceiveRequest, String> {
    let network = networks::address_network(genesis.as_deref().unwrap_or_default());
    let amount = amount
        .map(|text| parse_amount_arg("amount", &text))
        .transpose()?;
    let address = session.with(|wallet| {
        wallet
            .account(index)
            .map(|account| account.address)
            .map_err(|e| e.to_string())
    })?;
    let id = payment::decode_address(&address).map_err(|e| e.to_string())?;
    let request = PaymentRequest {
        recipient: address,
        amount,
        ..PaymentRequest::default()
    };
    Ok(ReceiveRequest {
        uri: request.to_uri(network).map_err(|e| e.to_string())?,
        qr_svg: payment::payment_qr_svg(&request, network).map_err(|e| e.to_string())?,
        address: maya_address::encode(network, &id),
        grouped: maya_clear_sign::grouped(network, &id),
    })
}

/// A node's view of an account.
#[derive(Serialize, Deserialize)]
pub struct AccountState {
    /// Spendable balance, base units as a decimal string.
    pub balance: String,
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
    // The genesis the transfer will be signed for, refused when the node
    // claims to be mainnet without the pinned genesis (networks.rs).
    let status = wallet::network_status(&node_url)
        .await
        .map_err(|e| e.to_string())?;
    if status.class == maya_wallet_core::networks::NetworkClass::Impostor {
        return Err(format!(
            "this node says it is mainnet, but its genesis ({}) is not the mainnet genesis; not signing for it",
            status.fingerprint
        ));
    }
    let chain = parse_genesis(&status.genesis)?;
    let genesis = status.genesis;
    let tiers = FeeTier::all();
    if !quote.active {
        return Ok(FeeTerms {
            collector: None,
            base_fee: "0".to_string(),
            genesis,
            options: tiers
                .iter()
                .map(|tier| FeeOption {
                    label: tier.label().to_string(),
                    fee: tier.suggested_fee().to_string(),
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
        base_fee: quote.base_fee.to_string(),
        genesis,
        options: tiers
            .iter()
            .zip(fees)
            .map(|(tier, fee)| FeeOption {
                label: tier.label().to_string(),
                fee: fee.to_string(),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_amount_arg;
    use primitive_types::U256;

    #[test]
    fn ipc_amounts_above_u64_arrive_exactly() {
        let max = U256::MAX.to_string();
        assert_eq!(parse_amount_arg("amount", &max), Ok(U256::MAX));
        assert_eq!(
            parse_amount_arg("amount", "18446744073709551616"),
            Ok(U256::from(u64::MAX) + 1)
        );
    }

    #[test]
    fn malformed_ipc_amounts_are_refused_naming_the_field() {
        for text in ["", "-1", "+1", "1e18", "1.5", "0x10", "1,000"] {
            let error = parse_amount_arg("fee", text).expect_err(text);
            assert!(error.starts_with("fee: "), "{error}");
        }
        let two_pow_256 =
            "115792089237316195423570985008687907853269984665640564039457584007913129639936";
        assert!(parse_amount_arg("amount", two_pow_256).is_err());
    }
}
