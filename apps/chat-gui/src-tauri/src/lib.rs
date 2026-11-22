//! Maya Chat desktop: the `maya-chat` library behind a Tauri window.
//!
//! Every privileged step (the key, the relay, the files) happens here in
//! Rust. The webview only calls these commands and draws what they return,
//! and its capability set grants it nothing else.
//!
//! RESEARCH (ADR-031): the protocol is unaudited. The window says so.

mod deeplink;
mod delivery;
mod starter;
mod sticker_cmds;
mod stickers;
mod store;

use std::sync::Arc;

use libp2p::Multiaddr;
use maya_chat::client::Client;
use maya_chat::identity::Identity;
use maya_chat::{ChatError, courier, now};
use serde::Serialize;
use store::{Contact, Files, Line, Profile};
use tauri::{Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

/// How long a published prekey stays valid, seconds (the CLI's figure).
const PREKEY_TTL: u64 = 30 * 24 * 3_600;
/// maya-testnet-1's public relay, over WebSocket through the tunnel, so a
/// fresh install can chat with no setup. The peer id is the relay's
/// persistent key; Settings and `MAYA_CHAT_RELAY` override it.
const DEFAULT_RELAY: &str =
    "/dns4/chat.maya2c.dev/tcp/443/wss/p2p/12D3KooWE2G9to26wVuq5znjrE4BQkFrH7YjLypjudtsW8Myi86T";
const HISTORY_LIMIT: usize = 5_000;
/// Text bytes the history may hold. The line count alone let long messages
/// grow the profile far past what the window can load.
const HISTORY_BYTES: usize = 16 << 20;
/// Text bytes of one message, sent or received.
const MESSAGE_BYTES: usize = 16 << 10;
/// The welcome contact on that relay (`maya-chat bot`), written to during
/// first run so a new user sees a sealed reply before they know anyone.
/// Empty while no bot runs, which skips that step; `MAYA_CHAT_WELCOME`
/// overrides it.
const WELCOME_BOT: &str = "";

struct Session {
    seed: Zeroizing<[u8; 32]>,
    client: Client,
    profile: Profile,
    /// Sticker packs arriving, and pack requests answered (in memory only).
    incoming: sticker_cmds::Incoming,
}

struct App {
    files: Files,
    session: Mutex<Option<Session>>,
}

/// What the window needs to draw its first screen.
#[derive(Serialize)]
struct Status {
    ready: bool,
    address: Option<String>,
    relay: String,
    contacts: Vec<Contact>,
    welcome_bot: Option<String>,
}

/// The welcome bot's address, if one is configured and well formed.
fn welcome_bot() -> Option<String> {
    let configured = std::env::var("MAYA_CHAT_WELCOME").unwrap_or_else(|_| WELCOME_BOT.into());
    address(configured.trim()).ok().map(hex::encode)
}

fn relay_addr(profile: &Profile) -> Result<Multiaddr, String> {
    if profile.relay.is_empty() {
        return Err("no relay yet: set one in Settings".into());
    }
    profile
        .relay
        .parse()
        .map_err(|e| format!("the relay address is not valid: {e}"))
}

fn address(hex_text: &str) -> Result<[u8; 32], String> {
    hex::decode(hex_text.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| "an address is 64 hex characters".to_string())
}

fn why(e: &ChatError) -> String {
    match e {
        ChatError::NoSession => {
            "they have not published a key on this relay yet; ask them to open Maya Chat once"
                .into()
        }
        other => other.to_string(),
    }
}

impl App {
    fn open(files: Files) -> Result<Self, String> {
        let session = match store::load_seed()? {
            None => None,
            Some(seed) => Some(Self::session(&files, seed)?),
        };
        Ok(Self {
            files,
            session: Mutex::new(session),
        })
    }

    fn session(files: &Files, seed: Zeroizing<[u8; 32]>) -> Result<Session, String> {
        let identity = Identity::from_seed(*seed);
        let client = match files.sessions() {
            Some(saved) => Client::restore(identity, &saved).map_err(|e| e.to_string())?,
            None => Client::new(identity),
        };
        let mut profile = files.profile(&seed)?;
        if profile.relay.is_empty() {
            profile.relay =
                std::env::var("MAYA_CHAT_RELAY").unwrap_or_else(|_| DEFAULT_RELAY.into());
        }
        let mut session = Session {
            seed,
            client,
            profile,
            incoming: sticker_cmds::Incoming::default(),
        };
        sticker_cmds::ensure_starter(&mut session);
        Ok(session)
    }

    fn save(&self, s: &Session) -> Result<(), String> {
        self.files
            .save_sessions(&s.client.save().map_err(|e| e.to_string())?)?;
        self.files.save_profile(&s.seed, &s.profile)
    }
}

#[tauri::command]
async fn status(app: State<'_, Arc<App>>) -> Result<Status, String> {
    let guard = app.session.lock().await;
    Ok(match guard.as_ref() {
        None => Status {
            ready: false,
            address: None,
            relay: String::new(),
            contacts: Vec::new(),
            welcome_bot: welcome_bot(),
        },
        Some(s) => Status {
            ready: true,
            address: Some(hex::encode(s.client.identity().address())),
            relay: s.profile.relay.clone(),
            contacts: s.profile.contacts.clone(),
            welcome_bot: welcome_bot(),
        },
    })
}

#[tauri::command]
async fn create_identity(app: State<'_, Arc<App>>) -> Result<String, String> {
    let mut guard = app.session.lock().await;
    if guard.is_some() {
        return Err("an identity already exists on this computer".into());
    }
    let identity = Identity::generate().map_err(|e| e.to_string())?;
    let seed = Zeroizing::new(*identity.seed());
    store::store_seed(&seed)?;
    let session = App::session(&app.files, seed)?;
    let addr = hex::encode(session.client.identity().address());
    app.save(&session)?;
    *guard = Some(session);
    Ok(addr)
}

#[tauri::command]
async fn set_relay(app: State<'_, Arc<App>>, relay: String) -> Result<(), String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let relay = relay.trim().to_owned();
    relay
        .parse::<Multiaddr>()
        .map_err(|e| format!("not a relay address: {e}"))?;
    s.profile.relay = relay;
    app.save(s)
}

/// Publishes this identity's prekey so others can start a conversation.
#[tauri::command]
async fn publish(app: State<'_, Arc<App>>) -> Result<(), String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    let relay = relay_addr(&s.profile)?;
    let bundle = s
        .client
        .identity()
        .prekey_bundle(1, now() + PREKEY_TTL)
        .map_err(|e| e.to_string())?;
    courier::publish(&relay, bundle).await.map_err(|e| why(&e))
}

#[tauri::command]
async fn add_contact(
    app: State<'_, Arc<App>>,
    name: String,
    addr: String,
) -> Result<Vec<Contact>, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let addr = hex::encode(address(&addr)?);
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err("give the contact a name".into());
    }
    s.profile.contacts.retain(|c| c.address != addr);
    s.profile.contacts.push(Contact {
        name,
        address: addr,
    });
    app.save(s)?;
    Ok(s.profile.contacts.clone())
}

#[tauri::command]
async fn history(app: State<'_, Arc<App>>, peer: String) -> Result<Vec<Line>, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    Ok(s.profile
        .history
        .iter()
        .filter(|l| l.peer == peer)
        .cloned()
        .collect())
}

fn remember(profile: &mut Profile, line: Line) {
    profile.history.push(line);
    let over = profile.history.len().saturating_sub(HISTORY_LIMIT);
    profile.history.drain(..over);
    // The newest line always stays, even alone over the budget.
    let mut bytes: usize = profile.history.iter().map(|l| l.text.len()).sum();
    let mut stale = 0;
    while bytes > HISTORY_BYTES && stale + 1 < profile.history.len() {
        bytes -= profile.history[stale].text.len();
        stale += 1;
    }
    profile.history.drain(..stale);
}

#[tauri::command]
async fn send(app: State<'_, Arc<App>>, peer: String, text: String) -> Result<Line, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let to = address(&peer)?;
    // A NUL would let typed text pose as a sticker or pack message.
    if text.contains('\0') {
        return Err("a message cannot contain a NUL character".into());
    }
    if text.len() > MESSAGE_BYTES {
        return Err(format!("a message is at most {} KB", MESSAGE_BYTES >> 10));
    }
    let at = now();
    let sent = sticker_cmds::deliver(&app, s, &peer, text.as_bytes(), at).await?;
    let line = Line {
        peer: hex::encode(to),
        mine: true,
        text,
        at,
        sticker: None,
        outbox: Some(hex::encode(sent)),
    };
    remember(&mut s.profile, line.clone());
    app.save(s)?;
    Ok(line)
}

/// Fetches and opens whatever the relay holds for this identity.
#[tauri::command]
async fn poll(app: State<'_, Arc<App>>) -> Result<Vec<Line>, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let relay = relay_addr(&s.profile)?;
    let results = courier::collect(&mut s.client, &relay, now())
        .await
        .map_err(|e| why(&e))?;
    let mut lines = Vec::new();
    let mut incoming = std::mem::take(&mut s.incoming);
    for r in results.into_iter().filter_map(Result::ok) {
        let line = if stickers::is_structured(&r.text) {
            sticker_cmds::receive(&app, s, &mut incoming, r.from, &r.text).await
        } else if r.text.len() > MESSAGE_BYTES {
            // Too long to keep, so dropped, as a malformed sticker is.
            None
        } else {
            Some(Line {
                peer: hex::encode(r.from),
                mine: false,
                text: String::from_utf8_lossy(&r.text).into_owned(),
                at: now(),
                sticker: None,
                outbox: None,
            })
        };
        lines.extend(line);
    }
    s.incoming = incoming;
    for line in &lines {
        remember(&mut s.profile, line.clone());
    }
    // Retries the outbox; a failure leaves entries queued for the next poll.
    let _retry = courier::flush(&mut s.client, &relay, now()).await;
    // Saved even with no line to show: a pack request answered moved the
    // sending chain, and a pack item filled the cache.
    app.save(s)?;
    Ok(lines)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch hands its argv (and any link) to the
        // running app and exits, instead of opening a second chat window.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            deeplink::focus(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(maya_app_update::dialog())
        .setup(|app| {
            let handle = app.handle().clone();
            app.deep_link()
                .on_open_url(move |event| deeplink::handle(&handle, &event.urls()));
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                deeplink::handle(app.handle(), &urls);
            }
            let dir = app.path().app_data_dir()?;
            let state = App::open(Files::new(dir)?)?;
            app.manage(Arc::new(state));
            maya_app_update::start(app, None)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            create_identity,
            set_relay,
            publish,
            add_contact,
            history,
            send,
            poll,
            delivery::deliveries,
            delivery::dismiss_delivery,
            sticker_cmds::sticker_packs,
            sticker_cmds::sticker_file,
            sticker_cmds::import_sticker,
            sticker_cmds::create_pack,
            sticker_cmds::remove_pack,
            sticker_cmds::send_sticker,
            sticker_cmds::request_pack,
            maya_app_update::command::check_for_updates
        ])
        .run(tauri::generate_context!())
        .expect("error while running Maya Chat");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line {
        Line {
            peer: "p".into(),
            mine: false,
            text: text.into(),
            at: 1,
            sticker: None,
            outbox: None,
        }
    }

    #[test]
    fn history_keeps_the_newest_lines_within_its_byte_budget() {
        let mut profile = Profile::default();
        let big = "x".repeat(MESSAGE_BYTES);
        for _ in 0..(HISTORY_BYTES / MESSAGE_BYTES + 8) {
            remember(&mut profile, line(&big));
        }
        let kept: usize = profile.history.iter().map(|l| l.text.len()).sum();
        assert!(kept <= HISTORY_BYTES, "{kept} bytes kept");
        assert!(
            profile.history.last().is_some_and(|l| l.text == big),
            "the newest line stays"
        );
    }
}
