//! Maya2C zero-trust wallet.
//!
//! The application is split so that key material has exactly one home. The
//! `maya-wallet-core` crate owns seeds, derivation, and signing; this crate
//! exposes a narrow command surface over it; and the Leptos frontend holds no
//! secrets at all, only what the commands hand back.
//!
//! See [`commands`] for the rules governing what is allowed to cross that
//! boundary.

pub mod commands;

use commands::Session;

/// Builds and runs the application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Session::default())
        .invoke_handler(tauri::generate_handler![
            commands::create_wallet,
            commands::recover_wallet,
            commands::check_phrase,
            commands::unlock,
            commands::lock,
            commands::is_unlocked,
            commands::wallet_exists,
            commands::derive_account,
            commands::list_accounts,
            commands::scan_qr,
            commands::parse_payment_uri,
            commands::preview_transfer,
            commands::sign_transfer,
            commands::broadcast,
            commands::record_outcome,
            commands::history,
            commands::fetch_account,
            commands::fee_options,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the wallet");
}
