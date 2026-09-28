//! `maya-chat chat`: an interactive session over one relay.
//!
//! Each line typed is sent to the current peer; the mailbox is checked every
//! `--poll` seconds and once more when input ends, so a scripted session
//! (stdin from a pipe) still sees the replies that arrived meanwhile.
//! Commands: `/to HEX` switches peer, `/quit` leaves.

use std::io::BufRead as _;
use std::time::Duration;

use libp2p::Multiaddr;
use maya_chat::client::Client;
use maya_chat::{Address, courier, now};
use tokio::sync::mpsc;

use super::{Args, address, load_client, relay_addr, save_client, show, why};

/// How often the mailbox is checked by default, seconds.
pub const DEFAULT_POLL_SECS: u64 = 5;

/// What one line of input asks for.
enum Line {
    Send(String),
    To(Address),
    Quit,
    Nothing,
}

fn parse(line: &str) -> Result<Line, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(Line::Nothing);
    }
    if line == "/quit" {
        return Ok(Line::Quit);
    }
    if let Some(hex) = line.strip_prefix("/to ") {
        return address(hex.trim()).map(Line::To);
    }
    if line.starts_with('/') {
        return Err(format!("unknown command {line}: /to HEX, /quit"));
    }
    Ok(Line::Send(line.to_owned()))
}

/// Stdin is read on its own thread: a blocking read would otherwise stall
/// the runtime and the mailbox polls with it. The channel closes at EOF.
fn input() -> mpsc::UnboundedReceiver<std::io::Result<String>> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

struct Chat {
    home: std::path::PathBuf,
    relay: Multiaddr,
    client: Client,
    peer: Option<Address>,
}

impl Chat {
    async fn send(&mut self, text: &str) -> Result<(), String> {
        let to = self.peer.ok_or("no peer yet: /to HEX")?;
        match courier::deliver(&mut self.client, &self.relay, to, text.as_bytes(), now()).await {
            Ok(()) => {
                save_client(&self.home, &self.client)?;
                println!("sent");
                Ok(())
            }
            Err(e) => {
                // The chain may have advanced past a message the relay never
                // took; the saved state has not.
                self.client = load_client(&self.home)?;
                Err(why(&e))
            }
        }
    }

    async fn poll(&mut self) -> Result<(), String> {
        let results = courier::collect(&mut self.client, &self.relay, now())
            .await
            .map_err(|e| why(&e))?;
        if !results.is_empty() {
            save_client(&self.home, &self.client)?;
            show(&results);
        }
        Ok(())
    }

    /// Handles one line; `false` once the user asked to leave.
    async fn line(&mut self, line: &str) -> Result<bool, String> {
        match parse(line)? {
            Line::Send(text) => self.send(&text).await?,
            Line::To(peer) => {
                self.peer = Some(peer);
                println!("now talking to {}", hex::encode(peer));
            }
            Line::Quit => return Ok(false),
            Line::Nothing => {}
        }
        Ok(true)
    }
}

/// Runs the interactive session until `/quit` or end of input.
///
/// # Errors
///
/// Missing identity or relay; errors during the session are printed and
/// the session goes on.
pub async fn chat(a: &Args) -> Result<(), String> {
    let mut chat = Chat {
        home: a.home.clone(),
        relay: relay_addr(a)?,
        client: load_client(&a.home)?,
        peer: a.to.as_deref().map(address).transpose()?,
    };
    println!(
        "chatting as {} (RESEARCH: unaudited). Type to send; /to HEX, /quit.",
        hex::encode(chat.client.identity().address())
    );
    let mut lines = input();
    let mut tick = tokio::time::interval(Duration::from_secs(a.poll));
    loop {
        let result = tokio::select! {
            line = lines.recv() => match line {
                Some(Ok(line)) => match chat.line(&line).await {
                    Ok(true) => Ok(()),
                    Ok(false) => break,
                    Err(e) => Err(e),
                },
                Some(Err(e)) => Err(format!("reading input: {e}")),
                None => break,
            },
            _ = tick.tick() => chat.poll().await,
        };
        if let Err(e) = result {
            eprintln!("{e}");
        }
    }
    chat.poll().await
}

#[cfg(test)]
mod tests {
    use super::{Line, parse};

    #[test]
    fn lines_parse_to_what_they_ask_for() {
        assert!(matches!(parse("  "), Ok(Line::Nothing)));
        assert!(matches!(parse("/quit"), Ok(Line::Quit)));
        assert!(matches!(parse("hello there"), Ok(Line::Send(t)) if t == "hello there"));
        let hex = "ab".repeat(32);
        assert!(matches!(parse(&format!("/to {hex}")), Ok(Line::To(a)) if a == [0xab; 32]));
        assert!(parse("/to nothex").is_err());
        assert!(parse("/unknown").is_err());
    }
}
