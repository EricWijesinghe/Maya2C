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
pub mod deeplink;

use commands::Session;
use tauri_plugin_deep_link::DeepLinkExt;

/// wry's own WebView2 arguments (the mini menu and SmartScreen off), kept
/// when automation adds its own.
const WRY_DEFAULT_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

/// Honours `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. WebView2 reads it, but
/// wry always passes arguments of its own, which win, so a WebDriver session
/// (`msedgedriver` sets the variable to open a debugging port) could never
/// attach — `apps/wallet-gui/e2e`. Unset in normal use, when nothing
/// changes; anything that can set this process's environment could already
/// control WebView2.
fn with_automation_args<R: tauri::Runtime>(mut context: tauri::Context<R>) -> tauri::Context<R> {
    if let Ok(extra) = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS") {
        for window in &mut context.config_mut().app.windows {
            window.additional_browser_args = Some(format!("{WRY_DEFAULT_ARGS} {extra}"));
        }
    }
    context
}

/// Builds and runs the application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch (with a link) hands its argv to the
        // running app and exits, instead of opening a second wallet.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            deeplink::focus(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(maya_app_update::dialog())
        .setup(|app| {
            let handle = app.handle().clone();
            app.deep_link()
                .on_open_url(move |event| deeplink::handle(&handle, &event.urls()));
            // Windows and Linux pass a startup link as argv, not as an event.
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                deeplink::handle(app.handle(), &urls);
            }
            Ok(maya_app_update::start(app)?)
        })
        .manage(Session::default())
        .manage(deeplink::PendingPay::default())
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
            commands::network_status,
            commands::receive_request,
            deeplink::take_pending_pay,
            commands::contacts,
            commands::add_contact,
            commands::remove_contact,
            commands::export_contacts,
            commands::import_contacts,
            commands::check_recipient,
            commands::hold_transfer,
            commands::release_held,
            commands::cancel_held,
        ])
        .run(with_automation_args(tauri::generate_context!()))
        .expect("error while running the wallet");
}
