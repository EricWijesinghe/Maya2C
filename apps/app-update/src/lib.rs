//! In-app updates for the Maya2C desktop apps (Hub, Wallet, Chat).
//!
//! **Manual only.** Nothing checks, downloads or installs on start: one
//! person's key signs the update manifests, and a stolen key would reach
//! every install at once (ADR-080, amendment 2026-11-14). The apps check only
//! when the user presses **Check for updates**, which calls
//! [`command::check_for_updates`]. Automatic checks stay off until the manifests are
//! k-of-n signed.
//!
//! A check asks `maya2c.dev/updates/<app>.json` whether a newer version
//! exists. If one does, a native dialog names it and shows the publisher key
//! fingerprint ([`verify::fingerprint`]) with **Install now** or **Later**;
//! nothing downloads without that click. The download must verify against the
//! public key compiled into the app ([`verify::verify`]) before it is
//! installed, and the app restarts into the new version.
//!
//! A build without an updater section in its Tauri config (every local and
//! unsigned build) has no updater at all: release CI adds the section only
//! when the signing key exists.

pub mod verify;

use std::error::Error;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

type BoxError = Box<dyn Error + Send + Sync>;

/// The dialog plugin, for [`command::check_for_updates`]'s question. Register it on
/// the builder: `.plugin(maya_app_update::dialog())`.
#[must_use]
pub fn dialog<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_dialog::init()
}

/// The update-signing public key of this build, when it carries an updater
/// config (a signed release build).
fn pubkey<R: Runtime, M: Manager<R>>(app: &M) -> Option<String> {
    let updater = app.config().plugins.0.get("updater")?;
    Some(updater.get("pubkey")?.as_str()?.to_owned())
}

/// Call from `setup`: registers the updater plugin in a signed build. It does
/// not check for updates; see the crate docs for why.
///
/// # Errors
///
/// Only if the updater plugin cannot be registered (a malformed config).
pub fn start<R: Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    if pubkey(app).is_some() {
        app.handle()
            .plugin(tauri_plugin_updater::Builder::new().build())?;
    }
    Ok(())
}

/// The app command. A module because a `pub` Tauri command at a crate root
/// collides with the macros `#[tauri::command]` exports there.
pub mod command {
    use tauri::{AppHandle, Runtime};

    /// The **Check for updates** command. Register it in the app's
    /// `generate_handler!` as `maya_app_update::command::check_for_updates`.
    /// Returns a line for the app to show; when an update is offered and
    /// accepted, the app restarts instead of returning.
    ///
    /// # Errors
    ///
    /// A message saying why the check or the install failed, including a
    /// refused signature.
    #[tauri::command]
    pub async fn check_for_updates<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
        super::check_and_offer(&app)
            .await
            .map_err(|error| error.to_string())
    }
}

async fn check_and_offer<R: Runtime>(app: &AppHandle<R>) -> Result<String, BoxError> {
    let Some(pubkey) = pubkey(app) else {
        return Ok(
            "Updates are off in this build: it has no update-signing key. \
                   Install new versions from the release page."
                .into(),
        );
    };
    let key = verify::fingerprint(&pubkey)?;
    let Some(update) = app.updater()?.check().await? else {
        return Ok(format!(
            "Up to date ({}). Publisher key {key}",
            app.package_info().version
        ));
    };
    let name = app.package_info().name.clone();
    let question = app
        .dialog()
        .message(format!(
            "{name} {} is available (you have {}).\n\n\
             Publisher key (SHA-256):\n{key}\n\n\
             Compare it with the key in docs/verify-release.md before you \
             install. The download is checked against this key, then \
             installed, and the app restarts.",
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
        return Ok(format!("Update to {} not installed.", update.version));
    }
    let bytes = update.download(|_, _| {}, || {}).await?;
    // The plugin verified this download already; this repeats the check with
    // the gate this crate's tests pin, so no plugin change can relax it.
    verify::verify(&pubkey, &update.signature, &bytes)?;
    update.install(bytes)?;
    app.restart();
}
