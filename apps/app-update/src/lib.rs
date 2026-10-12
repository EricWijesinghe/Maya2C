//! In-app updates for the Maya2C desktop apps (Hub, Wallet, Chat).
//!
//! On start, a release build asks `maya2c.dev/updates/<app>.json` whether a
//! newer version exists. If one does, a native dialog says so and offers
//! **Install now** or **Later**; nothing downloads without that click. The
//! download is checked against the update-signing public key compiled into
//! the app before it is installed, and the app restarts into the new version.
//!
//! A build without an updater section in its Tauri config (every local and
//! unsigned build) skips all of this: release CI adds the section only when
//! the signing key exists, so an app can never be offered an update it cannot
//! verify.

use std::error::Error;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

/// The dialog plugin, for [`start`]'s question. Register it on the builder:
/// `.plugin(maya_app_update::dialog())`.
#[must_use]
pub fn dialog<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_dialog::init()
}

/// Whether this build carries an updater config (a signed release build).
fn enabled<R: Runtime, M: Manager<R>>(app: &M) -> bool {
    app.config().plugins.0.contains_key("updater")
}

/// Call from `setup`: registers the updater and checks once in the
/// background. Never fails the app's start: a check that cannot reach the
/// server is reported to stderr and retried on the next start.
///
/// # Errors
///
/// Only if the updater plugin cannot be registered (a malformed config).
pub fn start<R: Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    if !enabled(app) {
        return Ok(());
    }
    app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = check_and_offer(&handle).await {
            eprintln!("update check failed: {error}");
        }
    });
    Ok(())
}

async fn check_and_offer<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(());
    };
    let name = app.package_info().name.clone();
    let question = app
        .dialog()
        .message(format!(
            "A new version of {name} is ready: {} (you have {}).\n\n\
             Install it now? The app restarts when it is done.",
            update.version, update.current_version
        ))
        .title("Update available")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install now".into(),
            "Later".into(),
        ));
    // A blocking dialog must not hold an async worker thread.
    let install = tauri::async_runtime::spawn_blocking(move || question.blocking_show()).await?;
    if !install {
        return Ok(());
    }
    update.download_and_install(|_, _| {}, || {}).await?;
    app.restart();
}
