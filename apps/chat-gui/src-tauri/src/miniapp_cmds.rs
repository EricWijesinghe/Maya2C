//! The mini-app runner (ADR-138).
//!
//! An installed app is a folder `miniapps/<id>/` in the app data directory
//! holding `manifest.cbor` and `index.html`. The manifest is verified and the
//! HTML re-hashed every time the app is listed, opened or served. The HTML is
//! served on the `miniapp:` scheme with its own CSP: no network, no frames,
//! no forms. The window loads it into an `<iframe sandbox="allow-scripts">`,
//! an opaque origin with no Tauri API, and relays its `postMessage` calls to
//! [`miniapp_call`] with the token [`miniapp_open`] gave that frame.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use maya_chat::miniapp::{Manifest, Verified};
use maya_chat::miniapp_bridge::{Bridge, Call, Reply, Token};
use serde::Serialize;
use tauri::http::Response;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::App;
use crate::pay_cmds::NETWORK;

/// The scheme the runner serves on.
pub const SCHEME: &str = "miniapp";
/// Largest entry file, bytes. A policy bound: a mini-app is one page.
const MAX_ENTRY_BYTES: u64 = 2 * 1024 * 1024;
/// The served page's policy. Inline script and style only, since the app is
/// one file; nothing it can fetch, frame, submit or navigate a base to.
pub const APP_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
     img-src data:; font-src data:; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'";

struct Running {
    bridge: Bridge,
    html: Vec<u8>,
}

/// The one running mini-app, if any.
#[derive(Default)]
pub(crate) struct Runner(Mutex<Option<Running>>);

/// One installed app, as the picker shows it.
#[derive(Serialize)]
pub(crate) struct Listed {
    id: String,
    name: Option<String>,
    permissions: Vec<&'static str>,
    publisher: Option<String>,
    /// Why the app cannot be opened, if it cannot.
    error: Option<String>,
}

fn is_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn base<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("miniapps"))
}

fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > cap {
        return Err("file is too large".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

/// Verifies an installed app and returns it with its entry HTML.
fn load(dir: &Path, id: &str) -> Result<(Verified, Vec<u8>), String> {
    if !is_id(id) {
        return Err("not an app id".into());
    }
    let folder = dir.join(id);
    let cap = u64::try_from(maya_chat::miniapp::MAX_MANIFEST_BYTES).unwrap_or(u64::MAX);
    let manifest = Manifest::from_bytes(&read_capped(&folder.join("manifest.cbor"), cap)?)
        .map_err(|e| e.to_string())?;
    let verified = manifest.verify().map_err(|e| e.to_string())?;
    let html = read_capped(&folder.join("index.html"), MAX_ENTRY_BYTES)?;
    if !verified.entry_matches(&html) {
        return Err("index.html is not the file the publisher signed".into());
    }
    Ok((verified, html))
}

fn token(hex_text: &str) -> Option<Token> {
    hex::decode(hex_text).ok()?.try_into().ok()
}

#[tauri::command]
pub(crate) fn miniapps(app: AppHandle) -> Result<Vec<Listed>, String> {
    let dir = base(&app)?;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new()); // nothing installed yet
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        out.push(match load(&dir, &id) {
            Ok((v, _)) => Listed {
                id,
                name: Some(v.name.clone()),
                permissions: v.permissions.iter().map(|p| p.name()).collect(),
                publisher: Some(hex::encode(v.publisher)),
                error: None,
            },
            Err(e) => Listed {
                id,
                name: None,
                permissions: Vec::new(),
                publisher: None,
                error: Some(e),
            },
        });
    }
    Ok(out)
}

/// Starts app `id`, replacing any running one. Returns the frame token (hex).
#[tauri::command]
pub(crate) async fn miniapp_open(
    app: AppHandle,
    state: State<'_, Arc<App>>,
    runner: State<'_, Runner>,
    id: String,
) -> Result<String, String> {
    let (verified, html) = load(&base(&app)?, &id)?;
    let me = {
        let guard = state.session.lock().await;
        guard
            .as_ref()
            .ok_or("create an identity first")?
            .client
            .identity()
            .address()
    };
    let bridge = Bridge::new(verified, NETWORK, me).map_err(|e| e.to_string())?;
    let token = hex::encode(bridge.token());
    *runner.0.lock().map_err(|_| "runner lock poisoned")? = Some(Running { bridge, html });
    Ok(token)
}

/// A call the window relayed from the frame. `None`: dropped.
#[tauri::command]
pub(crate) fn miniapp_call(
    runner: State<'_, Runner>,
    token: String,
    call: Call,
) -> Result<Option<Reply>, String> {
    let mut guard = runner.0.lock().map_err(|_| "runner lock poisoned")?;
    let (Some(running), Some(from)) = (guard.as_mut(), self::token(&token)) else {
        return Ok(None);
    };
    Ok(running.bridge.handle(&from, call))
}

/// The user's answer to a prompt. `None`: dropped.
#[tauri::command]
pub(crate) fn miniapp_answer(
    runner: State<'_, Runner>,
    token: String,
    prompt: u64,
    yes: bool,
) -> Result<Option<Reply>, String> {
    let mut guard = runner.0.lock().map_err(|_| "runner lock poisoned")?;
    let (Some(running), Some(from)) = (guard.as_mut(), self::token(&token)) else {
        return Ok(None);
    };
    Ok(running.bridge.answer(&from, prompt, yes))
}

#[tauri::command]
pub(crate) fn miniapp_close(runner: State<'_, Runner>) -> Result<(), String> {
    *runner.0.lock().map_err(|_| "runner lock poisoned")? = None;
    Ok(())
}

/// The `miniapp:` scheme: the running app's entry file, re-hashed, under
/// [`APP_CSP`]. Anything else is a 404.
pub(crate) fn serve<R: Runtime>(app: &AppHandle<R>) -> Response<Vec<u8>> {
    let runner = app.state::<Runner>();
    let page = runner.0.lock().ok().and_then(|g| {
        g.as_ref()
            .filter(|r| r.bridge.app().entry_matches(&r.html))
            .map(|r| r.html.clone())
    });
    let built = match page {
        Some(html) => Response::builder()
            .status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", APP_CSP)
            .header("X-Content-Type-Options", "nosniff")
            .header("Cache-Control", "no-store")
            .header("Referrer-Policy", "no-referrer")
            .body(html),
        None => Response::builder().status(404).body(Vec::new()),
    };
    built.unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_cannot_leave_the_folder() {
        for bad in [
            "",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "A",
            "x".repeat(33).as_str(),
        ] {
            assert!(!is_id(bad), "{bad:?}");
        }
        assert!(is_id("split-bill-2"));
    }

    #[test]
    fn the_app_policy_allows_no_network() {
        assert!(APP_CSP.contains("default-src 'none'"));
        assert!(APP_CSP.contains("connect-src 'none'"));
        assert!(!APP_CSP.contains("http"));
    }
}
