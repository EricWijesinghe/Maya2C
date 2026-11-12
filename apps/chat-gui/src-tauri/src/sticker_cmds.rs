//! The window's sticker commands, and what `poll` does with sticker messages.
//!
//! Files cross the IPC boundary as raw bytes (`tauri::ipc::Response` and a raw
//! request body), never as JSON arrays: a 120 KB sticker as a JSON number
//! array is about a megabyte.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use maya_chat::{courier, now};
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::{InvokeBody, Request, Response};

use crate::stickers::{self, Content, Pack, PackItem};
use crate::store::{self, Line, RECENT_LIMIT, ReplyRef, StickerRef};
use maya_chat::frame::{self, Mark};
use crate::{App, Session, address, expiry, relay_addr, remember, why};

/// The picker's contents.
#[derive(Serialize)]
pub struct Packs {
    packs: Vec<Pack>,
    recent: Vec<String>,
}

/// One sticker the window wants in a new pack.
#[derive(Deserialize)]
pub struct NewItem {
    id: String,
    emoji: String,
}

/// A sticker's file, from the built-in pack or the encrypted cache.
pub(crate) fn file_of(app: &App, s: &Session, id: &str) -> Option<Vec<u8>> {
    crate::starter::file(id)
        .map(<[u8]>::to_vec)
        .or_else(|| app.files.sticker(&s.seed, id))
}

/// Puts the starter pack first, once, for every identity.
pub(crate) fn ensure_starter(s: &mut Session) {
    let starter = crate::starter::pack();
    if !s.profile.packs.iter().any(|p| p.id == starter.id) {
        s.profile.packs.insert(0, starter);
    }
}

#[tauri::command]
pub async fn sticker_packs(app: State<'_, Arc<App>>) -> Result<Packs, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    Ok(Packs {
        packs: s.profile.packs.clone(),
        recent: s.profile.recent.clone(),
    })
}

/// A sticker file, as raw bytes.
#[tauri::command]
pub async fn sticker_file(app: State<'_, Arc<App>>, id: String) -> Result<Response, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    file_of(&app, s, &id)
        .map(Response::new)
        .ok_or_else(|| "this computer does not have that sticker".into())
}

/// Checks and caches one file for a pack being made; the body is the file.
#[tauri::command]
pub async fn import_sticker(app: State<'_, Arc<App>>, request: Request<'_>) -> Result<PackItem, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("send the file's bytes".into());
    };
    let (_, format) = stickers::check_file(bytes)?;
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    let id = app.files.save_sticker(&s.seed, bytes)?;
    Ok(PackItem {
        id,
        emoji: String::new(),
        format,
    })
}

/// Makes a pack from imported stickers.
#[tauri::command]
pub async fn create_pack(app: State<'_, Arc<App>>, title: String, items: Vec<NewItem>) -> Result<Pack, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let mut checked = Vec::with_capacity(items.len());
    for item in items {
        let bytes = file_of(&app, s, &item.id).ok_or("a sticker in the pack was not imported")?;
        let (_, format) = stickers::check_file(&bytes)?;
        checked.push(PackItem {
            id: item.id,
            emoji: item.emoji.trim().to_owned(),
            format,
        });
    }
    let pack = stickers::make_pack(&title, checked, true)?;
    s.profile.packs.retain(|p| p.id != pack.id);
    s.profile.packs.push(pack.clone());
    app.save(s)?;
    Ok(pack)
}

#[tauri::command]
pub async fn remove_pack(app: State<'_, Arc<App>>, pack: String) -> Result<(), String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    if pack == crate::starter::pack().id {
        return Err("the starter pack is built in".into());
    }
    s.profile.packs.retain(|p| p.id != pack);
    app.save(s)
}

/// Sends a sticker from an installed pack.
#[tauri::command]
pub async fn send_sticker(app: State<'_, Arc<App>>, peer: String, pack: String, id: String) -> Result<Line, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let owner = s.profile.packs.iter().find(|p| p.id == pack).ok_or("that pack is not installed")?;
    let item = owner.items.iter().find(|i| i.id == id).ok_or("that sticker is not in the pack")?.clone();
    let title = owner.title.clone();
    let file = file_of(&app, s, &id).ok_or("this computer does not have that sticker")?;
    let content = Content::Sticker {
        id: id.clone(),
        emoji: item.emoji.clone(),
        pack: pack.clone(),
        title: title.clone(),
    };
    let at = now();
    let peer_hex = hex::encode(address(&peer)?);
    let timer = expiry::timer_of(&s.profile, &peer_hex);
    let sent = deliver_for(&app, s, &peer, &frame::encode_timed(&content, &file, timer)?, at, timer).await?;
    let ends = expiry::deadline(timer, s.clock.tick(&mut s.profile));
    s.profile.recent.retain(|r| r != &id);
    s.profile.recent.insert(0, id.clone());
    s.profile.recent.truncate(RECENT_LIMIT);
    let line = Line {
        id: hex::encode(sent),
        peer: peer_hex,
        mine: true,
        text: item.emoji,
        at,
        sticker: Some(StickerRef { id, format: item.format, pack, title }),
        outbox: Some(hex::encode(sent)),
        timer,
        deadline: ends,
        ..Line::default()
    };
    remember(&mut s.profile, line.clone());
    app.save(s)?;
    Ok(line)
}

/// Asks the sender of a sticker for its whole pack.
#[tauri::command]
pub async fn request_pack(app: State<'_, Arc<App>>, peer: String, pack: String) -> Result<(), String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    deliver(&app, s, &peer, &stickers::encode(&Content::PackRequest { pack }, &[])?, now()).await?;
    app.save(s)
}

/// Queues one plaintext and tries to send it. Queued and saved before the
/// network is touched, so a failed send stays in the outbox and `poll` retries
/// it. The session is never rewound (ADR-074, decision 0). Returns the
/// outbox id, which the caller keeps to show the message's delivery state.
pub(crate) async fn deliver(app: &App, s: &mut Session, peer: &str, plain: &[u8], at: u64) -> Result<[u8; 32], String> {
    deliver_for(app, s, peer, plain, at, 0).await
}

/// [`deliver`] for a message with a disappearing timer: relays keep it at
/// most `timer` seconds (0: the usual TTL).
pub(crate) async fn deliver_for(
    app: &App,
    s: &mut Session,
    peer: &str,
    plain: &[u8],
    at: u64,
    timer: u64,
) -> Result<[u8; 32], String> {
    let relay = relay_addr(&s.profile)?;
    let to = address(peer)?;
    let sent = courier::prepare_for(&mut s.client, &relay, to, plain, at, timer)
        .await
        .map_err(|e| why(&e))?;
    app.save(s)?;
    // A failed flush leaves the entry queued; `poll` retries it, so the send counts.
    let _attempt = courier::flush(&mut s.client, &relay, now()).await;
    app.save(s)?;
    Ok(sent)
}

/// Packs being received, and requests already answered this run.
#[derive(Default)]
pub(crate) struct Incoming {
    /// By pack id: the pack and which items have arrived.
    packs: HashMap<String, (Pack, Vec<bool>)>,
    /// (peer, pack) pairs answered since the app started: a peer asking again
    /// and again gets the pack once, not once per request.
    answered: HashSet<(String, String)>,
}

/// What one structured message did.
pub(crate) enum Got {
    /// A new line to show and keep.
    Line(Line),
    /// A reaction changed the line at this index of the history.
    Touched(usize),
}

/// Handles one structured message from `from`, whose envelope id is `id`.
/// Returns what to show, if anything. A malformed one is dropped: a peer can
/// send anything. A kind this build does not know becomes a plain notice.
pub(crate) async fn receive(
    app: &App,
    s: &mut Session,
    incoming: &mut Incoming,
    from: [u8; 32],
    id: [u8; 32],
    plain: &[u8],
) -> Option<Got> {
    let peer = hex::encode(from);
    let line = |text: String| Line {
        id: hex::encode(id),
        peer: peer.clone(),
        text,
        at: now(),
        ..Line::default()
    };
    let decoded = match stickers::decode(plain) {
        Ok(decoded) => decoded,
        Err(e @ maya_chat::frame::FrameError::UnknownKind) => return Some(Got::Line(line(e.to_string()))),
        Err(_) => return None,
    };
    let timer = decoded.timer;
    if timer != 0 {
        // Only an app that reads timers sends a timed message.
        s.profile.timer_peers.insert(peer.clone());
    }
    match decoded.content {
        Content::Sticker { id: sticker, emoji, pack, title } => {
            let (_, format) = stickers::check_file(&decoded.file).ok()?;
            app.files.save_sticker(&s.seed, &decoded.file).ok()?;
            Some(Got::Line(Line {
                sticker: Some(StickerRef { id: sticker, format, pack, title }),
                timer,
                ..line(emoji)
            }))
        }
        Content::Text => {
            // `decode` checked the body is text.
            let text = String::from_utf8(decoded.file).ok()?;
            Some(Got::Line(Line { timer, ..line(text) }))
        }
        Content::Timer { secs } => {
            // `decode` refused a value outside TIMER_OPTIONS. Applies to
            // messages sent after it; every stored line keeps its own timer.
            s.profile.timers.insert(peer.clone(), secs);
            s.profile.timer_peers.insert(peer.clone());
            let by = expiry::name_of(&s.profile, &peer);
            Some(Got::Line(Line { id: String::new(), notice: true, ..line(expiry::notice(secs, &by)) }))
        }
        Content::Reaction { target, emoji, on, at } => {
            let mark = Mark { mine: false, emoji, on, at };
            store::react(&mut s.profile.history, &peer, &target, mark).map(Got::Touched)
        }
        Content::Reply { target, quote } => {
            // `decode` checked the body is text.
            let text = String::from_utf8(decoded.file).ok()?;
            Some(Got::Line(Line {
                reply: Some(ReplyRef { target, quote }),
                timer,
                ..line(text)
            }))
        }
        Content::PackRequest { pack } => {
            if incoming.answered.insert((peer.clone(), pack.clone())) {
                answer_request(app, s, &peer, &pack).await;
            }
            None
        }
        Content::PackItem { pack, index } => {
            app.files.save_sticker(&s.seed, &decoded.file).ok()?;
            let title = pack.title.clone();
            let entry = incoming
                .packs
                .entry(pack.id.clone())
                .or_insert_with(|| (pack.clone(), vec![false; pack.items.len()]));
            *entry.1.get_mut(usize::from(index))? = true;
            install_if_complete(app, s, incoming, &pack.id)
                .then(|| Got::Line(line(format!("Sticker pack \u{201c}{title}\u{201d} added"))))
        }
    }
}

/// Sends every sticker of `pack` to `peer`, but only for a pack this user
/// made or installed, so a request cannot make the app send arbitrary files.
async fn answer_request(app: &App, s: &mut Session, peer: &str, pack: &str) {
    let Some(pack) = s.profile.packs.iter().find(|p| p.id == pack).cloned() else {
        return;
    };
    for (index, item) in pack.items.iter().enumerate() {
        let Some(file) = file_of(app, s, &item.id) else { continue };
        let Ok(index) = u16::try_from(index) else { break };
        let content = Content::PackItem { pack: pack.clone(), index };
        let Ok(plain) = stickers::encode(&content, &file) else { continue };
        if deliver(app, s, peer, &plain, now()).await.is_err() {
            return;
        }
    }
}

/// Installs a pack once every one of its files is on this computer.
fn install_if_complete(app: &App, s: &mut Session, incoming: &mut Incoming, pack: &str) -> bool {
    let Some((full, got)) = incoming.packs.get(pack) else { return false };
    let complete = full
        .items
        .iter()
        .zip(got)
        .all(|(item, seen)| *seen || file_of(app, s, &item.id).is_some());
    if !complete {
        return false;
    }
    let mut installed = full.clone();
    installed.mine = false;
    incoming.packs.remove(pack);
    if s.profile.packs.iter().any(|p| p.id == installed.id) {
        return false;
    }
    s.profile.packs.push(installed);
    true
}
