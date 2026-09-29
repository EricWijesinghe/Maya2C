//! Maya Chat desktop: the `maya-chat` library behind a Tauri window.
//!
//! Every privileged step (the key, the relay, the files) happens here in
//! Rust. The webview only calls these commands and draws what they return,
//! and its capability set grants it nothing else.
//!
//! RESEARCH (ADR-031): the protocol is unaudited. The window says so.

mod store;

use std::sync::Arc;

use libp2p::Multiaddr;
use maya_chat::client::Client;
use maya_chat::identity::Identity;
use maya_chat::{ChatError, courier, now};
use serde::Serialize;
use store::{Contact, Files, Line, Profile};
use tauri::{Manager, State};
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

struct Session {
    seed: Zeroizing<[u8; 32]>,
    client: Client,
    profile: Profile,
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
            "they have not published a key on this relay yet; ask them to open Maya Chat once".into()
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
            profile.relay = std::env::var("MAYA_CHAT_RELAY").unwrap_or_else(|_| DEFAULT_RELAY.into());
        }
        Ok(Session {
            seed,
            client,
            profile,
        })
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
        },
        Some(s) => Status {
            ready: true,
            address: Some(hex::encode(s.client.identity().address())),
            relay: s.profile.relay.clone(),
            contacts: s.profile.contacts.clone(),
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
async fn add_contact(app: State<'_, Arc<App>>, name: String, addr: String) -> Result<Vec<Contact>, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let addr = hex::encode(address(&addr)?);
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err("give the contact a name".into());
    }
    s.profile.contacts.retain(|c| c.address != addr);
    s.profile.contacts.push(Contact { name, address: addr });
    app.save(s)?;
    Ok(s.profile.contacts.clone())
}

#[tauri::command]
async fn history(app: State<'_, Arc<App>>, peer: String) -> Result<Vec<Line>, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    Ok(s.profile.history.iter().filter(|l| l.peer == peer).cloned().collect())
}

fn remember(profile: &mut Profile, line: Line) {
    profile.history.push(line);
    let over = profile.history.len().saturating_sub(HISTORY_LIMIT);
    profile.history.drain(..over);
}

#[tauri::command]
async fn send(app: State<'_, Arc<App>>, peer: String, text: String) -> Result<Line, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let relay = relay_addr(&s.profile)?;
    let to = address(&peer)?;
    if let Err(e) = courier::deliver(&mut s.client, &relay, to, text.as_bytes(), now()).await {
        // The sending chain may have moved past a message the relay never
        // took; the saved state has not.
        *s = App::session(&app.files, s.seed.clone())?;
        return Err(why(&e));
    }
    let line = Line {
        peer: hex::encode(to),
        mine: true,
        text,
        at: now(),
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
    let lines: Vec<Line> = results
        .into_iter()
        .filter_map(Result::ok)
        .map(|r| Line {
            peer: hex::encode(r.from),
            mine: false,
            text: String::from_utf8_lossy(&r.text).into_owned(),
            at: now(),
        })
        .collect();
    for line in &lines {
        remember(&mut s.profile, line.clone());
    }
    if !lines.is_empty() {
        app.save(s)?;
    }
    Ok(lines)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let state = App::open(Files::new(dir)?)?;
            app.manage(Arc::new(state));
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
            poll
        ])
        .run(tauri::generate_context!())
        .expect("error while running Maya Chat");
}
