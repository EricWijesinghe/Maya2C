//! The window's profile-card and theme commands (ADR-140 build PR 4), and what
//! `poll` does with a received card.
//!
//! A card leaves this computer only from [`send_card`], for one contact the
//! user chose, and only the card [`profile::Sharing::card_for`] gives that
//! contact; a contact shown nothing is sent nothing. Avatar, frame and
//! wallpaper files are imported with `import_sticker` (the sticker caps and
//! the encrypted cache) and checked again for their role here. Themes and
//! wallpapers never leave the computer.

use std::sync::Arc;

use maya_chat::frame::{self, Content};
use maya_chat::profile::{self, Card, Draft, Sharing, Show};
use maya_chat::theme::{self, BUILT_IN, Look, Looks, Theme, Wallpaper};
use maya_chat::now;
use serde::Serialize;
use tauri::State;

use crate::store::Profile;
use crate::{App, Session, address, sticker_cmds};

/// The card `peer` would be sent, or why none can be: the peer must be a
/// contact, and the user must have chosen a card for them (directly or as
/// the default).
pub(crate) fn card_to_send<'a>(profile: &'a Profile, peer: &str) -> Result<&'a Draft, String> {
    if !profile.contacts.iter().any(|c| c.address == peer) {
        return Err("cards are sent only to contacts".into());
    }
    profile.cards.card_for(peer).ok_or_else(|| "this contact is shown no card".into())
}

/// Checks a draft before it is kept: the same name, bio and theme rules a
/// receiver applies, and files this computer holds that fit their role.
fn check_draft(app: &App, s: &Session, draft: &Draft) -> Result<Draft, String> {
    let name = profile::sanitize_name(&draft.name)?;
    let bio = profile::sanitize_bio(&draft.bio)?;
    if !theme::is_theme(&draft.theme) {
        return Err(frame::FrameError::BadCard.into());
    }
    for (id, check) in [
        (&draft.avatar, profile::check_avatar as fn(&[u8]) -> Result<(), frame::FrameError>),
        (&draft.frame, profile::check_frame),
    ] {
        if let Some(id) = id {
            let bytes = sticker_cmds::file_of(app, s, id).ok_or("import the file first")?;
            check(&bytes)?;
        }
    }
    Ok(Draft { name, bio, ..draft.clone() })
}

/// The user's cards and who sees which.
#[tauri::command]
pub async fn my_cards(app: State<'_, Arc<App>>) -> Result<Sharing, String> {
    let guard = app.session.lock().await;
    Ok(guard.as_ref().ok_or("create an identity first")?.profile.cards.clone())
}

/// Saves the card `key` ("main", "work"); the first one made becomes the
/// default. Nothing is sent.
#[tauri::command]
pub async fn save_card(app: State<'_, Arc<App>>, key: String, draft: Draft) -> Result<Sharing, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let key = profile::sanitize_name(&key).map_err(|_| "give the card a short name")?;
    let draft = check_draft(&app, s, &draft)?;
    s.profile.cards.cards.insert(key.clone(), draft);
    s.profile.cards.default.get_or_insert(key);
    app.save(s)?;
    Ok(s.profile.cards.clone())
}

/// Deletes a card. Contacts who already hold it keep it: deletion here is
/// not a request to them.
#[tauri::command]
pub async fn remove_card(app: State<'_, Arc<App>>, key: String) -> Result<Sharing, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    s.profile.cards.cards.remove(&key);
    if s.profile.cards.default.as_deref() == Some(key.as_str()) {
        s.profile.cards.default = None;
    }
    app.save(s)?;
    Ok(s.profile.cards.clone())
}

/// Sets the default card (`None`: contacts without their own choice see nothing).
#[tauri::command]
pub async fn set_default_card(app: State<'_, Arc<App>>, key: Option<String>) -> Result<Sharing, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    if key.as_ref().is_some_and(|k| !s.profile.cards.cards.contains_key(k)) {
        return Err("no such card".into());
    }
    s.profile.cards.default = key;
    app.save(s)?;
    Ok(s.profile.cards.clone())
}

/// Chooses what one contact is shown.
#[tauri::command]
pub async fn set_card_visibility(app: State<'_, Arc<App>>, peer: String, show: Show) -> Result<Sharing, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let peer = hex::encode(address(&peer)?);
    match show {
        Show::Default => s.profile.cards.per_contact.remove(&peer),
        other => s.profile.cards.per_contact.insert(peer, other),
    };
    app.save(s)?;
    Ok(s.profile.cards.clone())
}

/// Signs and sends `peer` the card chosen for them, with a version higher
/// than any sent before, so it replaces what they hold.
#[tauri::command]
pub async fn send_card(app: State<'_, Arc<App>>, peer: String) -> Result<(), String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let peer = hex::encode(address(&peer)?);
    let draft = card_to_send(&s.profile, &peer)?.clone();
    let file = |id: &Option<String>| -> Result<Option<Vec<u8>>, String> {
        id.as_ref()
            .map(|id| sticker_cmds::file_of(&app, &*s, id).ok_or_else(|| "a file of the card is missing".to_string()))
            .transpose()
    };
    let (avatar, art) = (file(&draft.avatar)?, file(&draft.frame)?);
    let version = s.profile.cards.next_version();
    let (card, body) = profile::make_card(&s.seed, &draft, version, avatar.as_deref(), art.as_deref())?;
    let plain = frame::encode(&Content::Profile { card }, &body)?;
    sticker_cmds::deliver(&app, s, &peer, &plain, now()).await?;
    app.save(s)
}

/// The card a contact last sent, if any.
#[tauri::command]
pub async fn peer_card(app: State<'_, Arc<App>>, peer: String) -> Result<Option<Card>, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    Ok(s.profile.peer_cards.get(&peer).cloned())
}

/// Takes a received card: only from a contact, only newer than the one held
/// and from the same signer ([`profile::accept`]). `decode` verified it; its
/// files go to the encrypted cache. Shows no line.
pub(crate) fn receive(app: &App, s: &mut Session, peer: &str, card: Card, body: &[u8]) {
    if !s.profile.contacts.iter().any(|c| c.address == peer)
        || !profile::accept(s.profile.peer_cards.get(peer), &card)
    {
        return;
    }
    let Ok(files) = profile::verify_card(&card, body) else { return };
    for bytes in [files.avatar, files.frame].into_iter().flatten() {
        if app.files.save_sticker(&s.seed, &bytes).is_err() {
            return;
        }
    }
    s.profile.peer_cards.insert(peer.to_owned(), card);
}

/// The themes and the current looks.
#[derive(Serialize)]
pub struct Themes {
    themes: &'static [Theme],
    looks: Looks,
}

#[tauri::command]
pub async fn looks(app: State<'_, Arc<App>>) -> Result<Themes, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    Ok(Themes { themes: BUILT_IN, looks: s.profile.looks.clone() })
}

/// The look of the chat with `peer`: its override over the global look.
#[tauri::command]
pub async fn chat_look(app: State<'_, Arc<App>>, peer: String) -> Result<Look, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    Ok(s.profile.looks.look_for(&peer))
}

/// Sets the global look (`peer` absent) or one chat's override; a chat's
/// override set to the empty look is removed. Bad tokens are dropped and an
/// image wallpaper must be a cached still PNG or WebP.
#[tauri::command]
pub async fn set_look(app: State<'_, Arc<App>>, peer: Option<String>, look: Look) -> Result<Looks, String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let look = look.cleaned();
    if let Wallpaper::Image { id } = &look.wallpaper {
        let bytes = sticker_cmds::file_of(&app, s, id).ok_or("import the wallpaper first")?;
        theme::check_wallpaper(&bytes)?;
    }
    match peer {
        None => s.profile.looks.global = look,
        Some(p) if look == Look::default() => {
            s.profile.looks.per_chat.remove(&hex::encode(address(&p)?));
        }
        Some(p) => {
            s.profile.looks.per_chat.insert(hex::encode(address(&p)?), look);
        }
    }
    app.save(s)?;
    Ok(s.profile.looks.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Contact;

    #[test]
    fn no_card_leaves_without_a_contact_and_a_chosen_card() {
        let bob = "bb".repeat(32);
        let mut p = Profile::default();
        p.cards.cards.insert("main".into(), Draft { name: "Ann".into(), ..Draft::default() });
        assert!(card_to_send(&p, &bob).is_err(), "not a contact");
        p.contacts.push(Contact { name: "Bob".into(), address: bob.clone() });
        assert!(card_to_send(&p, &bob).is_err(), "no default and no choice: nothing");
        p.cards.default = Some("main".into());
        assert_eq!(card_to_send(&p, &bob).unwrap().name, "Ann");
        p.cards.per_contact.insert(bob.clone(), Show::Nothing);
        assert!(card_to_send(&p, &bob).is_err(), "a contact shown nothing is never sent one");
    }
}
