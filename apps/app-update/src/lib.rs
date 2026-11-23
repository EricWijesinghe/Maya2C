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
//! fingerprint ([`verify::fingerprint`]) with **Download** or **Later**;
//! nothing downloads without that click. The fingerprint is of the key this
//! app already trusts, so it cannot reveal a stolen key. The download must
//! then verify against that key ([`verify::verify_confirm_install`]), and a
//! second dialog shows its SHA-256 to compare with the release's
//! `SHA256SUMS`, the one check independent of the update key. Only
//! **Install now** there installs it, and the app restarts.
//!
//! A build without an updater section in its Tauri config (every local and
//! unsigned build) has no updater at all: release CI adds the section only
//! when the signing key exists.

pub mod verify;

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogBuilder, MessageDialogButtons, MessageDialogKind,
};
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

/// What the app must do before an update replaces it, such as stopping the
/// processes it started. Installing ends the process without closing its
/// windows (`std::process::exit` on Windows, [`AppHandle::restart`]
/// elsewhere), so window-close handlers never run. If the install then fails,
/// the hook is not undone: the error the user sees says so.
pub type BeforeExit<R> = fn(&AppHandle<R>);

struct BeforeExitHook<R: Runtime>(BeforeExit<R>);

/// Call from `setup`: registers the updater plugin in a signed build, and
/// `before_exit`, which runs after the user's last yes and before the update
/// is handed to the installer. It does not check for updates; see the crate
/// docs for why.
///
/// # Errors
///
/// Only if the updater plugin cannot be registered (a malformed config).
pub fn start<R: Runtime>(
    app: &tauri::App<R>,
    before_exit: Option<BeforeExit<R>>,
) -> tauri::Result<()> {
    if let Some(hook) = before_exit {
        app.manage(BeforeExitHook(hook));
    }
    if pubkey(app).is_some() {
        // Set in code, so it holds even when the config sets `allowDowngrades`.
        let builder = tauri_plugin_updater::Builder::new()
            .default_version_comparator(|current, release| is_newer(&release.version, &current));
        app.handle().plugin(builder.build())?;
    }
    Ok(())
}

/// Whether an offered release is strictly newer than the running one. Equal
/// and older releases are refused, so a manifest cannot roll an app back.
fn is_newer<V: PartialOrd>(offered: &V, current: &V) -> bool {
    offered > current
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

/// Set while a check runs, so a double click or a webview script calling the
/// command in a loop cannot open two dialogs and start two installs at once.
static CHECKING: AtomicBool = AtomicBool::new(false);

/// Holds [`CHECKING`] until dropped, on every return path.
struct Running;

impl Running {
    fn begin() -> Option<Self> {
        CHECKING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        CHECKING.store(false, Ordering::SeqCst);
    }
}

fn question<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    text: String,
    yes: &str,
    no: &str,
) -> MessageDialogBuilder<R> {
    app.dialog()
        .message(text)
        .title(title)
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(yes.into(), no.into()))
}

async fn check_and_offer<R: Runtime>(app: &AppHandle<R>) -> Result<String, BoxError> {
    let Some(_running) = Running::begin() else {
        return Ok("An update check is already running.".into());
    };
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
    let offer = question(
        app,
        "Update available",
        format!(
            "{} {} is available (you have {}).\n\n\
             Publisher key (SHA-256):\n{key}\n\n\
             This is the key this app has trusted since you installed it. A \
             match with docs/verify-release.md does not prove this update is \
             genuine if that key was stolen. If that page lists no \
             fingerprint, do not install.\n\n\
             Download it? Nothing is installed until you have seen the \
             download's SHA-256 and pressed Install now.",
            app.package_info().name,
            update.version,
            update.current_version
        ),
        "Download",
        "Later",
    );
    // A blocking dialog must not hold an async worker thread.
    if !tauri::async_runtime::spawn_blocking(move || offer.blocking_show()).await? {
        return Ok(format!("Update to {} not downloaded.", update.version));
    }
    install_offered(app, pubkey, update).await
}

/// Downloads the update, then hands it to [`verify::verify_confirm_install`]:
/// signature, then the user's yes on the download's SHA-256, then install.
async fn install_offered<R: Runtime>(
    app: &AppHandle<R>,
    pubkey: String,
    update: tauri_plugin_updater::Update,
) -> Result<String, BoxError> {
    // The plugin (2.14.1, the minimum Cargo.toml allows) also checks the
    // signature here and in install(); the gate below is the stricter second
    // check (no legacy signatures, and the signed version must be the
    // announced one).
    let bytes = update.download(|_, _| {}, || {}).await?;
    let version = update.version.clone();
    let shown = version.clone();
    let handle = app.clone();
    let installed = tauri::async_runtime::spawn_blocking(move || {
        verify::verify_confirm_install(
            &pubkey,
            &update.signature,
            &update.version,
            bytes,
            |digest| {
                question(
                    &handle,
                    "Check the download",
                    format!(
                        "Version {shown} downloaded; its signature matches the \
                         publisher key.\n\nSHA-256 of the download:\n{digest}\n\n\
                         Find this value in SHA256SUMS of the release that ships \
                         {shown}. That file is signed by the release workflow \
                         (cosign), not by the update key, so it is the one check \
                         that holds if the update key was stolen. Install only if \
                         they match."
                    ),
                    "Install now",
                    "Cancel",
                )
                .blocking_show()
            },
            |bytes| {
                let hook = handle.try_state::<BeforeExitHook<R>>();
                if let Some(hook) = &hook {
                    (hook.0)(&handle);
                }
                update.install(bytes).map_err(|error| -> BoxError {
                    // The hook already ran (the Hub's stops its miners) and
                    // nothing undoes it, so say so rather than fail silently.
                    if hook.is_some() {
                        format!("{error}. Work stopped for the update was not restarted; start it again.").into()
                    } else {
                        error.into()
                    }
                })
            },
        )
    })
    .await??;
    if !installed {
        return Ok(format!("Update to {version} not installed."));
    }
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::{Running, is_newer};

    /// `start` runs on every launch, so it must never reach the network or
    /// the install path. Source-level: a check needs a running app.
    #[test]
    fn start_never_checks_downloads_or_installs() {
        let source = include_str!("lib.rs");
        let body = source
            .split("pub fn start<")
            .nth(1)
            .and_then(|rest| rest.split("\n}").next())
            .expect("start() exists");
        for call in ["spawn", ".check(", "download", "install", "updater()", "offer"] {
            assert!(!body.contains(call), "start() reaches `{call}`");
        }
    }

    #[test]
    fn a_second_check_is_refused_while_one_runs() {
        let first = Running::begin().expect("no check is running");
        assert!(Running::begin().is_none());
        drop(first);
        assert!(Running::begin().is_some());
    }

    // Tuples stand in for semver here: their order matches the numeric core.

    #[test]
    fn updater_refuses_older_version() {
        assert!(!is_newer(&(0, 1, 4), &(0, 1, 5)));
        assert!(!is_newer(&(0, 1, 5), &(0, 1, 5)));
    }

    #[test]
    fn updater_accepts_newer_version_compared_numerically() {
        assert!(is_newer(&(0, 1, 10), &(0, 1, 9)));
        assert!(!is_newer(&(0, 1, 9), &(0, 1, 10)));
    }
}
