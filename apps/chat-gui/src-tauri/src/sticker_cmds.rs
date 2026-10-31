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
use crate::store::{Line, RECENT_LIMIT, StickerRef};
use crate::{App, Session, address, relay_addr, remember, why};

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
    deliver(&app, s, &peer, &stickers::encode(&content, &file)?).await?;
    s.profile.recent.retain(|r| r != &id);
    s.profile.recent.insert(0, id.clone());
    s.profile.recent.truncate(RECENT_LIMIT);
    let line = Line {
        peer: hex::encode(address(&peer)?),
        mine: true,
        text: item.emoji,
        at: now(),
        sticker: Some(StickerRef { id, format: item.format, pack, title }),
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
    deliver(&app, s, &peer, &stickers::encode(&Content::PackRequest { pack }, &[])?).await?;
    app.save(s)
}

/// Sends one plaintext. On failure it never rewinds an established session.
pub(crate) async fn deliver(app: &App, s: &mut Session, peer: &str, plain: &[u8]) -> Result<(), String> {
    let relay = relay_addr(&s.profile)?;
    let to = address(peer)?;
    let established = s.client.has_session(&to);
    if let Err(e) = courier::deliver(&mut s.client, &relay, to, plain, now()).await {
        // An established session keeps its advanced chain even though the send
        // failed: the relay may hold the envelope with only the reply lost, and
        // rewinding would seal the next message under the same key and nonce
        // (two-time pad; ADR-074). The receiver skips a missing index. Only a
        // session this send opened is rewound: the retry handshakes afresh.
        if established {
            app.save(s)?;
        } else {
            *s = App::session(&app.files, s.seed.clone())?;
        }
        return Err(why(&e));
    }
    Ok(())
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

/// Handles one structured message from `from`. Returns the line to show, if
/// any. A malformed one is dropped: a peer can send anything.
pub(crate) async fn receive(
    app: &App,
    s: &mut Session,
    incoming: &mut Incoming,
    from: [u8; 32],
    plain: &[u8],
) -> Option<Line> {
    let decoded = stickers::decode(plain).ok()?;
    let peer = hex::encode(from);
    match decoded.content {
        Content::Sticker { id, emoji, pack, title } => {
            let (_, format) = stickers::check_file(&decoded.file).ok()?;
            app.files.save_sticker(&s.seed, &decoded.file).ok()?;
            Some(Line {
                peer,
                mine: false,
                text: emoji,
                at: now(),
                sticker: Some(StickerRef { id, format, pack, title }),
            })
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
            install_if_complete(app, s, incoming, &pack.id).then(|| Line {
                peer,
                mine: false,
                text: format!("Sticker pack \u{201c}{title}\u{201d} added"),
                at: now(),
                sticker: None,
            })
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
        if deliver(app, s, peer, &plain).await.is_err() {
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
