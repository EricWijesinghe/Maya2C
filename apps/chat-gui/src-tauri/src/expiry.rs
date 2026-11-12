//! Disappearing messages on this device (ADR-140 build PR 2).
//!
//! A chat's timer is set by a `Timer` frame either side may send, and each
//! message carries the timer it was sent with, so changing the timer never
//! reaches older messages. The sender's copy counts from sending; the
//! receiver's from first display (the chat is open on screen). At the
//! deadline the line goes, with its reactions, any sticker file only it
//! used, and the quote link of any reply to it; its text is zeroized.
//!
//! **Time.** Nothing here is consensus, so wall-clock time is allowed, but a
//! clock set back must never lengthen a message's life (threat model T12,
//! red-team item 10). [`Clock`] reports the latest of the wall clock, the
//! monotonic clock since the app started, and the latest value it ever
//! reported (stored in the profile as `clock_floor`). A clock set forward
//! deletes early, which shortens a life and is accepted.
//!
//! **The limit.** This deletes copies held by honest apps. [`LIMIT`] is the
//! copy the window shows wherever the timer is switched on.

use std::time::Instant;

use maya_chat::frame;
use zeroize::Zeroize;

use crate::store::{Line, Profile};

/// The timer a new chat starts with: off. ADR-140 question 2, recommended
/// option (a), which Eric has not answered yet; changing the default is this
/// one constant.
pub const DEFAULT_TIMER: u64 = 0;
/// The number of chats at which the app suggests timers, once.
pub const PROMPT_AT_CHATS: usize = 3;
/// What the window says wherever the timer is switched on (ADR-140 section 5).
pub const LIMIT: &str = "Maya2C Chat asks the other person's app to delete. It is deleted on their device only if their app is honest: a modified app, a screenshot, or a camera pointed at the screen can keep a copy, and no messenger can stop that.";
/// Shown to the sender while the peer has not proven its app reads timers.
pub const MAY_NOT_SUPPORT: &str = "The other side may not support disappearing messages: an older app keeps them forever.";
/// The one-time suggestion, in the third chat.
pub const PROMPT: &str = "You can make messages in a chat disappear after a time you choose. It is off unless you turn it on.";

/// Seconds that never run backwards: see the module docs.
#[derive(Debug)]
pub struct Clock {
    wall_at_start: u64,
    started: Instant,
}

/// The latest of the wall clock, the monotonic reading and the stored floor.
#[must_use]
pub fn effective_now(wall: u64, monotonic: u64, floor: u64) -> u64 {
    wall.max(monotonic).max(floor)
}

impl Clock {
    /// Starts counting from the later of `wall` and the stored `floor`.
    #[must_use]
    pub fn start(wall: u64, floor: u64) -> Self {
        Self { wall_at_start: wall.max(floor), started: Instant::now() }
    }

    /// The current time, and the profile's floor raised to it.
    pub fn tick(&self, profile: &mut Profile) -> u64 {
        let monotonic = self.wall_at_start.saturating_add(self.started.elapsed().as_secs());
        let now = effective_now(maya_chat::now(), monotonic, profile.clock_floor);
        profile.clock_floor = now;
        now
    }
}

/// The timer for new messages in the chat with `peer`.
#[must_use]
pub fn timer_of(profile: &Profile, peer: &str) -> u64 {
    profile.timers.get(peer).copied().unwrap_or(DEFAULT_TIMER)
}

/// When a line sent now with `timer` is deleted; 0 for no timer.
#[must_use]
pub fn deadline(timer: u64, now: u64) -> u64 {
    if timer == 0 { 0 } else { now.saturating_add(timer) }
}

/// Starts the clock of every received timed line in the chat with `peer`
/// that has not been shown before. Returns whether any started.
pub fn start_clocks(history: &mut [Line], peer: &str, now: u64) -> bool {
    let mut started = false;
    for line in history.iter_mut().filter(|l| l.peer == peer && !l.mine && l.timer != 0 && l.deadline == 0) {
        line.deadline = deadline(line.timer, now);
        started = true;
    }
    started
}

/// What one sweep removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// Ids of the lines removed (lines without an id are removed too).
    pub ids: Vec<String>,
    /// Cached sticker files no remaining line, pack or recent list uses.
    pub stickers: Vec<String>,
}

fn sticker_in_use(profile: &Profile, id: &str) -> bool {
    profile.history.iter().any(|l| l.sticker.as_ref().is_some_and(|s| s.id == id))
        || profile.packs.iter().any(|p| p.items.iter().any(|i| i.id == id))
        || profile.recent.iter().any(|r| r == id)
}

/// Overwrites what a removed line said before its memory is freed.
fn wipe(line: &mut Line) {
    line.text.zeroize();
    if let Some(reply) = line.reply.as_mut() {
        reply.quote.zeroize();
    }
    for mark in &mut line.reactions {
        mark.emoji.zeroize();
    }
}

/// Removes every line whose deadline is at or before `now`, and unlinks
/// replies to them. The caller deletes [`Swept::stickers`] from the cache
/// and saves the profile.
pub fn sweep(profile: &mut Profile, now: u64) -> Swept {
    let due = |l: &Line| l.deadline != 0 && l.deadline <= now;
    if !profile.history.iter().any(due) {
        return Swept::default();
    }
    let (mut gone, kept): (Vec<Line>, Vec<Line>) = std::mem::take(&mut profile.history).into_iter().partition(due);
    profile.history = kept;
    for line in &mut profile.history {
        let unlinked = line
            .reply
            .as_ref()
            .is_some_and(|r| gone.iter().any(|g| !g.id.is_empty() && g.id == r.target && g.peer == line.peer));
        if unlinked {
            // The quote is a hash of the deleted text: a short text could be
            // guessed from it, so it goes with the original.
            if let Some(mut reply) = line.reply.take() {
                reply.quote.zeroize();
            }
        }
    }
    let mut stickers: Vec<String> = gone.iter().filter_map(|l| l.sticker.as_ref().map(|s| s.id.clone())).collect();
    stickers.sort();
    stickers.dedup();
    stickers.retain(|id| !sticker_in_use(profile, id));
    let ids = gone.iter().map(|l| l.id.clone()).collect();
    for line in &mut gone {
        wipe(line);
    }
    Swept { ids, stickers }
}

/// Whether the sender should be told the peer may keep messages forever:
/// the chat has a timer and the peer's app has never shown it reads one.
#[must_use]
pub fn may_not_support(profile: &Profile, peer: &str) -> bool {
    timer_of(profile, peer) != 0 && !profile.timer_peers.contains(peer)
}

/// Whether to show the one-time suggestion now.
#[must_use]
pub fn should_prompt(profile: &Profile) -> bool {
    !profile.timer_prompted && profile.contacts.len() >= PROMPT_AT_CHATS
}

/// The contact's name for `peer`, or the start of the address.
#[must_use]
pub fn name_of(profile: &Profile, peer: &str) -> String {
    profile
        .contacts
        .iter()
        .find(|c| c.address == peer)
        .map_or_else(|| peer.chars().take(8).collect(), |c| c.name.clone())
}

/// The line both sides show when a chat's timer changes.
#[must_use]
pub fn notice(secs: u64, by: &str) -> String {
    match frame::timer_label(secs) {
        _ if secs == 0 => format!("Disappearing messages turned off by {by}"),
        Some(label) => format!("Disappearing messages set to {label} by {by}"),
        None => format!("Disappearing messages changed by {by}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Contact, Files, ReplyRef, StickerRef};
    use maya_chat::frame::{Content, Mark, PackItem};

    const PEER: &str = "aa";
    const OTHER: &str = "bb";
    const T0: u64 = 1_800_000_000;
    const YEAR: u64 = 365 * 24 * 3_600;

    fn sent(id: u8, timer: u64, at: u64) -> Line {
        Line {
            id: hex::encode([id; 32]),
            peer: PEER.into(),
            mine: true,
            text: format!("message {id}"),
            at,
            timer,
            deadline: deadline(timer, at),
            ..Line::default()
        }
    }

    fn received(id: u8, timer: u64) -> Line {
        Line { mine: false, deadline: 0, ..sent(id, timer, T0) }
    }

    fn ids(profile: &Profile) -> Vec<String> {
        profile.history.iter().map(|l| l.id.clone()).collect()
    }

    #[test]
    fn a_message_is_deleted_at_its_deadline_and_not_before() {
        let mut profile = Profile { history: vec![sent(1, 30, T0), sent(2, 0, T0)], ..Profile::default() };
        assert_eq!(sweep(&mut profile, T0 + 29), Swept::default(), "one second early: kept");
        assert_eq!(profile.history.len(), 2);
        let swept = sweep(&mut profile, T0 + 30);
        assert_eq!(swept.ids, vec![hex::encode([1; 32])]);
        assert_eq!(ids(&profile), vec![hex::encode([2; 32])], "the untimed line stays");
        assert_eq!(sweep(&mut profile, T0 + 100 * YEAR), Swept::default(), "no timer, never deleted");
    }

    #[test]
    fn the_receiver_counts_from_first_display() {
        let mut history = vec![received(1, 30), Line { peer: OTHER.into(), ..received(2, 30) }, received(3, 0)];
        assert!(start_clocks(&mut history, PEER, T0 + 100));
        assert_eq!(history[0].deadline, T0 + 130, "from display, not from sending");
        assert_eq!(history[1].deadline, 0, "another chat was not shown");
        assert_eq!(history[2].deadline, 0, "no timer, no clock");
        assert!(!start_clocks(&mut history, PEER, T0 + 120), "nothing new to start");
        assert_eq!(history[0].deadline, T0 + 130, "showing it again does not restart it");
        let mut profile = Profile { history, ..Profile::default() };
        assert_eq!(sweep(&mut profile, T0 + 10 * YEAR).ids, vec![hex::encode([1; 32])], "never shown: never started");
    }

    #[test]
    fn a_restart_in_the_middle_of_the_timer_still_deletes_on_time() {
        let dir = std::env::temp_dir().join(format!("maya-chat-gui-expiry-{}", std::process::id()));
        let files = Files::new(dir.clone()).unwrap();
        let mut profile = Profile { history: vec![sent(1, 30, T0)], clock_floor: T0 + 10, ..Profile::default() };
        files.save_profile(&[1; 32], &profile).unwrap();
        // The app closes 10 s in. On the next start the deadline is read back.
        profile = files.profile(&[1; 32]).unwrap();
        assert_eq!(profile.history[0].deadline, T0 + 30, "the deadline was stored");
        assert_eq!(sweep(&mut profile, T0 + 29), Swept::default());
        // Restarted with the clock set back a year: the stored floor and the
        // monotonic clock carry time forward, so the deadline still arrives.
        let start = Clock::start(T0 - YEAR, profile.clock_floor);
        let after_20_s = effective_now(T0 - YEAR, start.wall_at_start + 20, profile.clock_floor);
        assert_eq!(after_20_s, T0 + 30);
        assert_eq!(sweep(&mut profile, after_20_s).ids.len(), 1);
        files.save_profile(&[1; 32], &profile).unwrap();
        assert!(files.profile(&[1; 32]).unwrap().history.is_empty(), "gone from the file too");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_clock_moved_backwards_does_not_extend_a_life() {
        assert_eq!(effective_now(T0 - YEAR, 0, T0), T0, "the floor holds");
        assert_eq!(effective_now(T0 - YEAR, T0 + 5, T0), T0 + 5, "the monotonic clock keeps counting");
        assert_eq!(effective_now(T0 + 9, T0 + 5, T0), T0 + 9);
        let clock = Clock::start(T0 - YEAR, T0);
        let mut profile = Profile { clock_floor: T0, ..Profile::default() };
        let now = clock.tick(&mut profile);
        assert!(now >= T0, "never before the floor");
        assert_eq!(profile.clock_floor, now, "the floor follows the clock");
    }

    #[test]
    fn the_attachment_reaction_and_reply_rows_are_deleted() {
        let dir = std::env::temp_dir().join(format!("maya-chat-gui-expiry-stk-{}", std::process::id()));
        let files = Files::new(dir.clone()).unwrap();
        let only_here = files.save_sticker(&[1; 32], b"GIF89a-only-in-the-timed-line").unwrap();
        let in_pack = files.save_sticker(&[1; 32], b"GIF89a-also-in-a-pack").unwrap();
        let sticker = |id: &str| StickerRef { id: id.into(), format: crate::stickers::Format::Gif, pack: "p".into(), title: "t".into() };
        let mut timed = sent(1, 30, T0);
        timed.reactions = vec![Mark { mine: false, emoji: "\u{1f44d}".into(), on: true, at: T0 + 1 }];
        let mut sticker_line = Line { sticker: Some(sticker(&only_here)), ..sent(2, 30, T0) };
        sticker_line.text = "\u{1f600}".into();
        let shared = Line { sticker: Some(sticker(&in_pack)), ..sent(3, 30, T0) };
        let reply = Line {
            reply: Some(ReplyRef { target: hex::encode([1; 32]), quote: frame::quote_hash("message 1") }),
            ..sent(4, 0, T0)
        };
        let other_chat_reply = Line {
            peer: OTHER.into(),
            reply: Some(ReplyRef { target: hex::encode([1; 32]), quote: "q".into() }),
            ..sent(5, 0, T0)
        };
        let pack = maya_chat::frame::Pack {
            id: "p".into(),
            title: "t".into(),
            items: vec![PackItem { id: in_pack.clone(), emoji: "x".into(), format: crate::stickers::Format::Gif }],
            mine: true,
        };
        let mut profile = Profile {
            history: vec![timed, sticker_line, shared, reply, other_chat_reply],
            packs: vec![pack],
            ..Profile::default()
        };
        let swept = sweep(&mut profile, T0 + 30);
        assert_eq!(swept.ids.len(), 3, "the timed line, with its reactions, and both sticker lines");
        assert_eq!(swept.stickers, vec![only_here.clone()], "a sticker a pack still holds is kept");
        for id in &swept.stickers {
            files.remove_sticker(id).unwrap();
        }
        assert_eq!(files.sticker(&[1; 32], &only_here), None, "the cached file is gone");
        assert!(files.sticker(&[1; 32], &in_pack).is_some());
        assert_eq!(ids(&profile), vec![hex::encode([4; 32]), hex::encode([5; 32])]);
        assert_eq!(profile.history[0].reply, None, "the reply no longer links to, or hashes, the deleted text");
        assert_eq!(profile.history[0].text, "message 4", "the reply itself has its own timer");
        assert!(profile.history[1].reply.is_some(), "a line in another chat is not touched");
        assert!(profile.history.iter().all(|l| l.reactions.is_empty()));
        assert!(files.remove_sticker("../profile").is_err(), "an id is never a path");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_timer_change_does_not_affect_older_messages() {
        let mut profile = Profile::default();
        profile.history.push(sent(1, 0, T0));
        profile.history.push(sent(2, 3_600, T0));
        profile.timers.insert(PEER.into(), 30);
        let timer = timer_of(&profile, PEER);
        profile.history.push(sent(3, timer, T0 + 1));
        assert_eq!(profile.history[1].deadline, T0 + 3_600, "shortening the timer does not shorten an older line");
        assert_eq!(sweep(&mut profile, T0 + 31).ids, vec![hex::encode([3; 32])]);
        assert_eq!(ids(&profile), vec![hex::encode([1; 32]), hex::encode([2; 32])]);
        profile.timers.insert(PEER.into(), 0);
        assert_eq!(sweep(&mut profile, T0 + 3_600).ids, vec![hex::encode([2; 32])], "turning it off does not save an older line");
        assert_eq!(ids(&profile), vec![hex::encode([1; 32])]);
    }

    #[test]
    fn a_30_second_message_is_gone_from_both_stores() {
        // The sender's and receiver's side of one message, through the frame.
        let plain = frame::encode_timed(&Content::Text, b"burn after reading", 30).unwrap();
        let mut alice = Profile { history: vec![sent(1, 30, T0)], ..Profile::default() };
        let decoded = frame::decode(&plain).unwrap();
        let mut bob = Profile {
            history: vec![Line { text: String::from_utf8(decoded.file).unwrap(), ..received(1, decoded.timer) }],
            ..Profile::default()
        };
        start_clocks(&mut bob.history, PEER, T0 + 5);
        assert!(sweep(&mut alice, T0 + 29).ids.is_empty() && sweep(&mut bob, T0 + 34).ids.is_empty());
        assert_eq!(sweep(&mut alice, T0 + 30).ids.len(), 1);
        assert_eq!(sweep(&mut bob, T0 + 35).ids.len(), 1);
        assert!(alice.history.is_empty() && bob.history.is_empty());
    }

    #[test]
    fn timers_are_off_by_default_and_suggested_once_in_the_third_chat() {
        assert_eq!(DEFAULT_TIMER, 0);
        let mut profile = Profile::default();
        assert_eq!(timer_of(&profile, PEER), 0);
        for n in 0..PROMPT_AT_CHATS {
            assert!(!should_prompt(&profile), "{n} chats");
            profile.contacts.push(Contact { name: format!("c{n}"), address: format!("{n:064}") });
        }
        assert!(should_prompt(&profile), "the third chat");
        profile.timer_prompted = true;
        assert!(!should_prompt(&profile), "once");
    }

    #[test]
    fn the_sender_is_warned_until_the_peer_proves_its_app_reads_timers() {
        let mut profile = Profile::default();
        assert!(!may_not_support(&profile, PEER), "no timer, nothing to warn about");
        profile.timers.insert(PEER.into(), 300);
        assert!(may_not_support(&profile, PEER));
        profile.timer_peers.insert(PEER.into());
        assert!(!may_not_support(&profile, PEER));
    }

    #[test]
    fn both_sides_show_who_set_the_timer_and_the_plain_limit() {
        assert_eq!(notice(3_600, "Ann"), "Disappearing messages set to 1 hour by Ann");
        assert_eq!(notice(0, "you"), "Disappearing messages turned off by you");
        for words in ["honest", "screenshot", "camera"] {
            assert!(LIMIT.contains(words), "{words}");
        }
    }
}
