//! Maya Chat desktop: the `maya-chat` library behind a Tauri window.
//!
//! Every privileged step (the key, the relay, the files) happens here in
//! Rust. The webview only calls these commands and draws what they return,
//! and its capability set grants it nothing else.
//!
//! RESEARCH (ADR-031): the protocol is unaudited. The window says so.

mod deeplink;
mod delivery;
mod expiry;
mod profile_cmds;
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
use maya_chat::frame::{self, Content, Mark};
use store::{Contact, Files, Line, Profile, ReplyRef, Shown};
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
    /// Disappearing-message time, which never runs backwards.
    clock: expiry::Clock,
}

/// A chat's disappearing-message setting, as the window shows it.
#[derive(Serialize)]
struct TimerView {
    secs: u64,
    options: Vec<(u64, &'static str)>,
    /// The peer's app has not shown that it reads timers.
    may_not_support: Option<&'static str>,
    /// The one-time suggestion, in the third chat.
    prompt: Option<&'static str>,
    /// The plain limit, shown wherever the timer is on.
    limit: &'static str,
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
        let clock = expiry::Clock::start(now(), profile.clock_floor);
        let mut session = Session {
            seed,
            client,
            profile,
            incoming: sticker_cmds::Incoming::default(),
            clock,
        };
        sticker_cmds::ensure_starter(&mut session);
        // Whatever reached its deadline while the app was closed goes first.
        Self::sweep(files, &mut session)?;
        Ok(session)
    }

    fn save(&self, s: &Session) -> Result<(), String> {
        self.files
            .save_sessions(&s.client.save().map_err(|e| e.to_string())?)?;
        self.files.save_profile(&s.seed, &s.profile)
    }

    /// Deletes every line past its deadline and the sticker files only they
    /// used, and saves the profile if anything went. Returns the removed ids.
    fn sweep(files: &Files, s: &mut Session) -> Result<Vec<String>, String> {
        let now = s.clock.tick(&mut s.profile);
        let swept = expiry::sweep(&mut s.profile, now);
        if swept.ids.is_empty() {
            return Ok(swept.ids);
        }
        // The profile first: a line must not outlive its deadline because a
        // cache file could not be removed.
        files.save_profile(&s.seed, &s.profile)?;
        for id in &swept.stickers {
            files.remove_sticker(id)?;
        }
        Ok(swept.ids)
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
async fn history(app: State<'_, Arc<App>>, peer: String) -> Result<Vec<Shown>, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    App::sweep(&app.files, s)?;
    // The chat is on screen now: received timed lines start counting.
    let now = s.clock.tick(&mut s.profile);
    if expiry::start_clocks(&mut s.profile.history, &peer, now) {
        app.save(s)?;
    }
    let all = &s.profile.history;
    Ok(all.iter().filter(|l| l.peer == peer).map(|l| store::show(all, l)).collect())
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
    let to = address(&peer)?;
    // A NUL would let typed text pose as a sticker or pack message.
    if text.contains('\0') {
        return Err("a message cannot contain a NUL character".into());
    }
    // `:shortcode:` becomes `:<id>:`, the name every app reads the same way,
    // and a custom emoji pack goes to this peer before its first use.
    let (text, used) = maya_chat::emoji::expand_codes(&text, &s.profile.packs);
    sticker_cmds::share_emoji(&app, s, &hex::encode(to), used).await;
    let at = now();
    let timer = expiry::timer_of(&s.profile, &hex::encode(to));
    // Untimed text stays unframed, so older apps still read it.
    let plain = Zeroizing::new(if timer == 0 { text.as_bytes().to_vec() } else { frame::encode_timed(&Content::Text, text.as_bytes(), timer)? });
    let sent = sticker_cmds::deliver_for(&app, s, &peer, &plain, at, timer).await?;
    let line = Line {
        id: hex::encode(sent),
        peer: hex::encode(to),
        mine: true,
        text,
        at,
        outbox: Some(hex::encode(sent)),
        timer,
        deadline: expiry::deadline(timer, s.clock.tick(&mut s.profile)),
        ..Line::default()
    };
    remember(&mut s.profile, line.clone());
    app.save(s)?;
    Ok(line)
}

/// Replies to the line `target` in this chat. The frame carries the hash of
/// the text replied to, so the other side shows its own copy or no quote.
#[tauri::command]
async fn reply(app: State<'_, Arc<App>>, peer: String, target: String, text: String) -> Result<Shown, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let peer = hex::encode(address(&peer)?);
    let quoted = store::find(&s.profile.history, &peer, &target).ok_or("that message is not in this chat")?;
    let quote = frame::quote_hash(&quoted.text);
    let content = Content::Reply { target: target.clone(), quote: quote.clone() };
    let at = now();
    let timer = expiry::timer_of(&s.profile, &peer);
    let plain = Zeroizing::new(frame::encode_timed(&content, text.as_bytes(), timer)?);
    let sent = sticker_cmds::deliver_for(&app, s, &peer, &plain, at, timer).await?;
    let line = Line {
        id: hex::encode(sent),
        peer,
        mine: true,
        text,
        at,
        outbox: Some(hex::encode(sent)),
        reply: Some(ReplyRef { target, quote }),
        timer,
        deadline: expiry::deadline(timer, s.clock.tick(&mut s.profile)),
        ..Line::default()
    };
    remember(&mut s.profile, line.clone());
    app.save(s)?;
    Ok(store::show(&s.profile.history, &line))
}

/// Sets this user's `emoji` on the line `target` on or off, here and on the
/// other side. Returns the line as it now reads.
#[tauri::command]
async fn react(app: State<'_, Arc<App>>, peer: String, target: String, emoji: String, on: bool) -> Result<Shown, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let peer = hex::encode(address(&peer)?);
    frame::check_emoji(&emoji)?;
    store::find(&s.profile.history, &peer, &target).ok_or("that message is not in this chat")?;
    if on && let Some(pack) = maya_chat::emoji::pack_of(&emoji, &s.profile.packs) {
        sticker_cmds::share_emoji(&app, s, &peer, std::collections::BTreeSet::from([pack])).await;
    }
    let at = now();
    let content = Content::Reaction { target: target.clone(), emoji: emoji.clone(), on, at };
    sticker_cmds::deliver(&app, s, &peer, &frame::encode(&content, &[])?, at).await?;
    let mark = Mark { mine: true, emoji, on, at };
    let i = store::react(&mut s.profile.history, &peer, &target, mark).ok_or("that message is not in this chat")?;
    app.save(s)?;
    Ok(store::show(&s.profile.history, &s.profile.history[i]))
}

/// Fetches and opens whatever the relay holds for this identity. Returns new
/// lines, and lines a reaction changed (the window redraws those in place).
/// `open` is the chat on screen: its timed lines start counting now.
#[tauri::command]
async fn poll(app: State<'_, Arc<App>>, open: Option<String>) -> Result<Vec<Shown>, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let relay = relay_addr(&s.profile)?;
    let results = courier::collect(&mut s.client, &relay, now())
        .await
        .map_err(|e| why(&e))?;
    let mut lines = Vec::new();
    let mut touched = Vec::new();
    let mut incoming = std::mem::take(&mut s.incoming);
    for r in results.into_iter().filter_map(Result::ok) {
        let got = if stickers::is_structured(&r.text) {
            sticker_cmds::receive(&app, s, &mut incoming, r.from, r.id, &r.text).await
        } else {
            Some(sticker_cmds::Got::Line(Line {
                id: hex::encode(r.id),
                peer: hex::encode(r.from),
                text: String::from_utf8_lossy(&r.text).into_owned(),
                at: now(),
                ..Line::default()
            }))
        };
        match got {
            Some(sticker_cmds::Got::Line(line)) => lines.push(line),
            Some(sticker_cmds::Got::Touched(i)) => touched.extend(s.profile.history.get(i).map(|l| l.id.clone())),
            None => {}
        }
    }
    s.incoming = incoming;
    if let Some(peer) = &open {
        let now = s.clock.tick(&mut s.profile);
        expiry::start_clocks(&mut lines, peer, now);
    }
    for line in &lines {
        remember(&mut s.profile, line.clone());
    }
    // Resolved after `remember`, so a reply that arrived with its original
    // in the same pass shows its quote. Ids, not indexes: `remember` trims.
    let all = &s.profile.history;
    let mut shown: Vec<Shown> = lines.iter().map(|l| store::show(all, l)).collect();
    shown.extend(touched.iter().filter_map(|id| all.iter().find(|l| &l.id == id)).map(|l| store::show(all, l)));
    // Retries the outbox; a failure leaves entries queued for the next poll.
    let _retry = courier::flush(&mut s.client, &relay, now()).await;
    // Saved even with no line to show: a pack request answered moved the
    // sending chain, and a pack item filled the cache.
    app.save(s)?;
    Ok(shown)
}

/// Deletes what has reached its deadline; returns the ids the window removes.
/// The window calls this every second while it is open.
#[tauri::command]
async fn expire(app: State<'_, Arc<App>>) -> Result<Vec<String>, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    App::sweep(&app.files, s)
}

/// The chat's disappearing-message setting, the plain limit, and whether to
/// warn or suggest.
#[tauri::command]
async fn chat_timer(app: State<'_, Arc<App>>, peer: String) -> Result<TimerView, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    let peer = hex::encode(address(&peer)?);
    Ok(TimerView {
        secs: expiry::timer_of(&s.profile, &peer),
        options: frame::TIMER_OPTIONS.to_vec(),
        may_not_support: expiry::may_not_support(&s.profile, &peer).then_some(expiry::MAY_NOT_SUPPORT),
        prompt: expiry::should_prompt(&s.profile).then_some(expiry::PROMPT),
        limit: expiry::LIMIT,
    })
}

/// Sets the chat's timer for messages sent from now on, here and, through a
/// `Timer` frame, on the other side. Older messages keep theirs.
#[tauri::command]
async fn set_timer(app: State<'_, Arc<App>>, peer: String, secs: u64) -> Result<Shown, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let peer = hex::encode(address(&peer)?);
    frame::check_timer(secs)?;
    sticker_cmds::deliver(&app, s, &peer, &frame::encode(&Content::Timer { secs }, &[])?, now()).await?;
    s.profile.timers.insert(peer.clone(), secs);
    s.profile.timer_prompted = true;
    let line = Line { peer, mine: true, text: expiry::notice(secs, "you"), at: now(), notice: true, ..Line::default() };
    remember(&mut s.profile, line.clone());
    app.save(s)?;
    Ok(store::show(&s.profile.history, &line))
}

/// The one-time suggestion was dismissed without choosing a timer.
#[tauri::command]
async fn dismiss_timer_prompt(app: State<'_, Arc<App>>) -> Result<(), String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    s.profile.timer_prompted = true;
    app.save(s)
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
            maya_app_update::start(app)?;
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
            reply,
            react,
            poll,
            expire,
            chat_timer,
            set_timer,
            dismiss_timer_prompt,
            delivery::deliveries,
            delivery::dismiss_delivery,
            sticker_cmds::sticker_packs,
            sticker_cmds::sticker_file,
            sticker_cmds::import_sticker,
            sticker_cmds::create_pack,
            sticker_cmds::remove_pack,
            sticker_cmds::send_sticker,
            sticker_cmds::request_pack,
            profile_cmds::my_cards,
            profile_cmds::save_card,
            profile_cmds::remove_card,
            profile_cmds::set_default_card,
            profile_cmds::set_card_visibility,
            profile_cmds::send_card,
            profile_cmds::peer_card,
            profile_cmds::looks,
            profile_cmds::chat_look,
            profile_cmds::set_look
        ])
        .run(tauri::generate_context!())
        .expect("error while running Maya Chat");
}
