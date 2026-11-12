//! Delivery state of the messages this user sent (ADR-074): a read-only view
//! of the sender's outbox, and dismissing an expired entry. The outbox holds
//! sealed envelopes and ids only; nothing here reads key material.

use std::sync::Arc;

use maya_chat::client::ENVELOPE_TTL;
use maya_chat::now;
use maya_chat::outbox::Standing;
use serde::Serialize;
use tauri::State;

use crate::store::Line;
use crate::{App, address};

/// What the sender sees next to a message they sent.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryState {
    /// Queued, and no relay has accepted an attempt yet.
    Sending,
    /// A relay holds it; the recipient's receipt has not come yet.
    Waiting,
    /// A valid signed receipt from the recipient arrived.
    Delivered,
    /// Past its TTL with no receipt.
    Expired,
}

/// One tracked message: its outbox id (hex) and its state.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub state: DeliveryState,
}

/// The state of a message sent at `sent_at`, given what the outbox reports.
/// `None` means it has left the outbox and the expired record: a receipt
/// removed it, or the record dropped it. Only a message past its TTL can be
/// dropped from the record, so the send time tells the two apart.
pub fn state_of(standing: Option<Standing>, sent_at: u64, now: u64) -> DeliveryState {
    match standing {
        Some(Standing::Queued { relayed: false }) => DeliveryState::Sending,
        Some(Standing::Queued { relayed: true }) => DeliveryState::Waiting,
        Some(Standing::Expired) => DeliveryState::Expired,
        None if now >= sent_at + ENVELOPE_TTL => DeliveryState::Expired,
        None => DeliveryState::Delivered,
    }
}

/// The states of `peer`'s tracked messages. `standing` looks an outbox id up.
pub fn entries(
    lines: &[Line],
    peer: &str,
    now: u64,
    standing: impl Fn(&[u8; 32]) -> Option<Standing>,
) -> Vec<Entry> {
    lines
        .iter()
        .filter(|l| l.peer == peer)
        .filter_map(|l| {
            let id = l.outbox.as_ref()?;
            let key = address(id).ok()?;
            Some(Entry {
                id: id.clone(),
                state: state_of(standing(&key), l.at, now),
            })
        })
        .collect()
}

/// Only an expired message can be dismissed; the rest are still on their way.
pub fn check_dismissable(state: DeliveryState) -> Result<(), String> {
    match state {
        DeliveryState::Expired => Ok(()),
        _ => Err("only an expired message can be dismissed".into()),
    }
}

/// Stops tracking the outbox id `id` on its line, once the user dismissed it.
pub fn untrack(lines: &mut [Line], id: &str) {
    for line in lines.iter_mut().filter(|l| l.outbox.as_deref() == Some(id)) {
        line.outbox = None;
    }
}

/// The delivery state of each tracked message sent to `peer`.
#[tauri::command]
pub async fn deliveries(app: State<'_, Arc<App>>, peer: String) -> Result<Vec<Entry>, String> {
    let guard = app.session.lock().await;
    let s = guard.as_ref().ok_or("create an identity first")?;
    let at = now();
    Ok(entries(&s.profile.history, &peer, at, |id| {
        s.client.standing(id, at)
    }))
}

/// Dismisses an expired message: removes it from the expired record and stops
/// tracking it. Refused for any other state.
#[tauri::command]
pub async fn dismiss_delivery(app: State<'_, Arc<App>>, id: String) -> Result<(), String> {
    let mut guard = app.session.lock().await;
    let s = guard.as_mut().ok_or("create an identity first")?;
    let key = address(&id)?;
    let at = now();
    let line = s
        .profile
        .history
        .iter()
        .find(|l| l.outbox.as_deref() == Some(id.as_str()))
        .ok_or("no tracked message has that id")?;
    check_dismissable(state_of(s.client.standing(&key, at), line.at, at))?;
    s.client.dismiss(&key);
    untrack(&mut s.profile.history, &id);
    app.save(s)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;
    use maya_chat::client::Client;
    use maya_chat::identity::Identity;

    const T0: u64 = 1_800_000_000;
    const DAY: u64 = 24 * 3_600;

    fn line(peer: &str, id: Option<&str>, at: u64) -> Line {
        Line {
            peer: peer.into(),
            mine: true,
            text: "hi".into(),
            at,
            sticker: None,
            outbox: id.map(str::to_owned),
            pay: None,
        }
    }

    fn hex_id(byte: u8) -> String {
        hex::encode([byte; 32])
    }

    #[test]
    fn a_queued_message_is_sending_until_the_relay_takes_it() {
        assert_eq!(
            state_of(Some(Standing::Queued { relayed: false }), T0, T0),
            DeliveryState::Sending
        );
        assert_eq!(
            state_of(Some(Standing::Queued { relayed: true }), T0, T0),
            DeliveryState::Waiting
        );
    }

    #[test]
    fn an_expired_message_is_expired_whether_or_not_the_record_holds_it_yet() {
        assert_eq!(
            state_of(Some(Standing::Expired), T0, T0),
            DeliveryState::Expired
        );
        assert_eq!(
            state_of(None, T0, T0 + ENVELOPE_TTL),
            DeliveryState::Expired
        );
    }

    #[test]
    fn gone_from_the_outbox_before_its_ttl_means_a_receipt_delivered_it() {
        assert_eq!(
            state_of(None, T0, T0 + ENVELOPE_TTL - 1),
            DeliveryState::Delivered
        );
    }

    #[test]
    fn entries_cover_only_this_peers_tracked_messages() {
        let (a, b) = (hex_id(1), hex_id(2));
        let peer = "aa".repeat(32);
        let lines = vec![
            line(&peer, Some(&a), T0),
            line(&peer, None, T0),
            line("bb".repeat(32).as_str(), Some(&b), T0),
        ];
        let got = entries(&lines, &peer, T0, |id| {
            (id == &[1u8; 32]).then_some(Standing::Queued { relayed: true })
        });
        assert_eq!(
            got,
            vec![Entry {
                id: a,
                state: DeliveryState::Waiting
            }]
        );
    }

    #[test]
    fn only_an_expired_message_is_dismissable() {
        assert!(check_dismissable(DeliveryState::Expired).is_ok());
        for refused in [
            DeliveryState::Sending,
            DeliveryState::Waiting,
            DeliveryState::Delivered,
        ] {
            assert!(check_dismissable(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn untrack_clears_only_the_dismissed_id() {
        let (a, b) = (hex_id(1), hex_id(2));
        let mut lines = vec![line("p", Some(&a), T0), line("p", Some(&b), T0)];
        untrack(&mut lines, &a);
        assert_eq!(lines[0].outbox, None);
        assert_eq!(lines[1].outbox.as_deref(), Some(b.as_str()));
    }

    #[test]
    fn a_real_outbox_runs_sending_then_expired_then_dismissed() {
        let alice = Identity::from_seed([1; 32]);
        let bob = Identity::from_seed([2; 32]);
        let bundle = bob.prekey_bundle(1, T0 + DAY).unwrap();
        let mut client = Client::new(alice);
        let key = client
            .queue(bob.address(), Some(&bundle), b"hi", T0, 1)
            .unwrap();
        let mut lines = vec![line(
            &hex::encode(bob.address()),
            Some(&hex::encode(key)),
            T0,
        )];
        let peer = hex::encode(bob.address());
        let view = |client: &Client, at: u64, lines: &[Line]| {
            entries(lines, &peer, at, |id| client.standing(id, at))
                .into_iter()
                .map(|e| e.state)
                .collect::<Vec<_>>()
        };

        assert_eq!(view(&client, T0, &lines), vec![DeliveryState::Sending]);
        let week = T0 + ENVELOPE_TTL;
        assert_eq!(view(&client, week, &lines), vec![DeliveryState::Expired]);

        let dismissed = client.standing(&key, week);
        assert_eq!(state_of(dismissed, T0, week), DeliveryState::Expired);
        check_dismissable(DeliveryState::Expired).unwrap();
        assert!(client.dismiss(&key));
        untrack(&mut lines, &hex::encode(key));
        assert!(view(&client, week, &lines).is_empty(), "no longer tracked");
    }
}
