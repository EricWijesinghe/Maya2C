//! `maya-chat bot`: the welcome contact a new install talks to.
//!
//! A person who has just installed a messenger has nobody to message, so a
//! first run proves nothing. The bot gives them someone: it publishes a
//! prekey, empties its mailbox every `--poll` seconds and answers each
//! sender over the same sealed session any two people would use. It holds
//! no special key and the relay treats it like anyone else, so its reply
//! is evidence that the round trip works, not a demonstration staged
//! around it.

use std::collections::HashSet;
use std::time::Duration;

use maya_chat::{Address, ChatError, courier, now};

use super::{Args, PREKEY_TTL, identity, load_client, relay_addr, save_client, why};

/// Most senders answered per mailbox check. A reply costs the bot the same
/// postage a message costs its sender, so a flood is rate-limited here
/// rather than turned into unbounded work.
const MAX_REPLIES_PER_ROUND: usize = 20;
/// Longest echo returned, characters: the bot repeats what it read, and
/// must not become a way to bounce large payloads off someone else's key.
const MAX_ECHO_CHARS: usize = 280;
/// How often the prekey is republished, so it never reaches its expiry.
const REPUBLISH_EVERY: Duration = Duration::from_secs(24 * 3_600);

const WELCOME: &str = "Welcome to Maya Chat. You are reading a reply that was sealed \
on my side and opened on yours. Your message reached me through the relay as ciphertext, \
in a session keyed with X-Wing (ML-KEM-768 + X25519), and both of us sign as ML-DSA-65 \
identities. The relay never saw the words. Try: how, ping, help.";

const HOW: &str = "Your address is derived from your ML-DSA-65 public key, in the \
same format a Maya2C wallet uses. When you first wrote, your app fetched my signed prekey \
from the relay and encapsulated a shared secret to it with X-Wing. After that, each \
direction has its own key chain. Every message key is used once and then deleted, so a key \
stolen later cannot read what you already received. Every message also carries postage, a \
small proof of work, so flooding a mailbox costs the sender. This is a research preview and \
has not been externally audited.";

const HELP: &str = "Commands: how (what protects this conversation), ping (a round \
trip), help. Anything else is echoed back so you can see it survive the trip. To talk to \
a person, share your address from the top of the sidebar.";

/// What the bot answers to `text`. `first` is whether this sender has not
/// been greeted since the bot started.
#[must_use]
pub fn reply(text: &str, first: bool) -> String {
    let word = text.trim().to_lowercase();
    let greeting = matches!(word.as_str(), "hi" | "hello" | "hey" | "start" | "/start");
    if first || greeting {
        return WELCOME.to_owned();
    }
    match word.as_str() {
        "how" | "how?" => HOW.to_owned(),
        "ping" => "pong: sealed, relayed and opened in both directions.".to_owned(),
        "help" | "?" => HELP.to_owned(),
        _ => {
            let echo: String = text.trim().chars().take(MAX_ECHO_CHARS).collect();
            format!("I decrypted {} bytes from you: \"{echo}\"", text.len())
        }
    }
}

struct Bot {
    home: std::path::PathBuf,
    relay: libp2p::Multiaddr,
    greeted: HashSet<Address>,
}

impl Bot {
    async fn publish(&self) -> Result<(), String> {
        let bundle = identity(&self.home)?
            .prekey_bundle(1, now() + PREKEY_TTL)
            .map_err(|e| e.to_string())?;
        courier::publish(&self.relay, bundle)
            .await
            .map_err(|e| e.to_string())
    }

    /// Answers one sender. The session store is saved only once the relay
    /// holds the reply, for the reason `send` in `main.rs` gives.
    async fn answer(&mut self, to: Address, text: &[u8]) -> Result<(), String> {
        let first = self.greeted.insert(to);
        let answer = reply(&String::from_utf8_lossy(text), first);
        let mut client = load_client(&self.home)?;
        courier::deliver(&mut client, &self.relay, to, answer.as_bytes(), now())
            .await
            .map_err(|e| why(&e))?;
        save_client(&self.home, &client)
    }

    /// Empties the mailbox and answers up to [`MAX_REPLIES_PER_ROUND`]
    /// senders, one reply each: the last message a sender wrote this round
    /// is the one answered.
    async fn round(&mut self) -> Result<(), String> {
        let mut client = load_client(&self.home)?;
        let results = courier::collect(&mut client, &self.relay, now())
            .await
            .map_err(|e| why(&e))?;
        save_client(&self.home, &client)?;
        let mut latest: Vec<(Address, Vec<u8>)> = Vec::new();
        for result in results {
            match result {
                Ok(r) => match latest.iter_mut().find(|(from, _)| *from == r.from) {
                    Some(slot) => slot.1 = r.text,
                    None => latest.push((r.from, r.text)),
                },
                Err(ChatError::NoSession) => {
                    eprintln!("bot: skipped a message for an unknown session")
                }
                Err(e) => eprintln!("bot: could not open a message: {e}"),
            }
        }
        if latest.len() > MAX_REPLIES_PER_ROUND {
            eprintln!(
                "bot: {} senders this round; answering {MAX_REPLIES_PER_ROUND}",
                latest.len()
            );
        }
        for (from, text) in latest.into_iter().take(MAX_REPLIES_PER_ROUND) {
            match self.answer(from, &text).await {
                Ok(()) => println!("bot: answered {}", hex::encode(from)),
                Err(e) => eprintln!("bot: reply to {} failed: {e}", hex::encode(from)),
            }
        }
        Ok(())
    }
}

/// Runs the bot until killed. Errors in a round are printed and the next
/// round goes on, so a relay restart costs one poll interval, not the bot.
///
/// # Errors
///
/// Missing identity or relay, or a first publish the relay refused.
pub async fn run(a: &Args) -> Result<(), String> {
    let mut bot = Bot {
        home: a.home.clone(),
        relay: relay_addr(a)?,
        greeted: HashSet::new(),
    };
    bot.publish().await?;
    println!(
        "welcome bot {} on {} (RESEARCH: unaudited)",
        hex::encode(identity(&a.home)?.address()),
        bot.relay
    );
    let mut poll = tokio::time::interval(Duration::from_secs(a.poll));
    let mut republish = tokio::time::interval(REPUBLISH_EVERY);
    republish.tick().await; // the first tick is immediate; published above
    loop {
        let result = tokio::select! {
            _ = poll.tick() => bot.round().await,
            _ = republish.tick() => bot.publish().await,
        };
        if let Err(e) = result {
            eprintln!("bot: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ECHO_CHARS, WELCOME, reply};

    #[test]
    fn a_new_sender_is_welcomed_whatever_they_wrote() {
        assert_eq!(reply("anything at all", true), WELCOME);
    }

    #[test]
    fn a_greeting_is_welcomed_again() {
        assert_eq!(reply("  Hello ", false), WELCOME);
    }

    #[test]
    fn commands_are_answered() {
        assert!(reply("ping", false).starts_with("pong"));
        assert!(reply("HOW", false).contains("X-Wing"));
        assert!(reply("help", false).starts_with("Commands"));
    }

    #[test]
    fn anything_else_is_echoed_and_capped() {
        assert_eq!(reply("abc", false), "I decrypted 3 bytes from you: \"abc\"");
        let long = "x".repeat(MAX_ECHO_CHARS * 4);
        let echoed = reply(&long, false);
        assert_eq!(echoed.matches('x').count(), MAX_ECHO_CHARS);
    }
}
