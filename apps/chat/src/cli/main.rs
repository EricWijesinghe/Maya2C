//! `maya-chat` — the command-line messenger and relay.
//!
//! ```text
//! maya-chat --home DIR init                       create an identity
//! maya-chat --home DIR address                    print it
//! maya-chat --home DIR relay --listen /ip4/0.0.0.0/tcp/4001 [--listen /ip4/127.0.0.1/tcp/4002/ws]
//! maya-chat --home DIR publish --relay ADDR       publish a prekey bundle
//! maya-chat --home DIR send --relay ADDR --to HEX TEXT...
//! maya-chat --home DIR recv --relay ADDR          fetch and print messages
//! maya-chat --home DIR chat --relay ADDR [--to HEX] [--poll SECS]
//!                                                 interactive: lines are
//!                                                 sent, replies printed
//! maya-chat --home DIR bot --relay ADDR [--poll SECS]
//!                                                 the welcome contact:
//!                                                 answers whoever writes
//! ```
//!
//! The identity seed is stored in `DIR/identity.key`, unencrypted in this
//! first version: protect the directory as you would a private key.
//! Passphrase protection comes with the keystore shared with the wallet.
//! Sessions are kept in `DIR/sessions.bin`, encrypted under a key derived
//! from the seed, so a later run can read follow-up messages and refuses a
//! replayed handshake.

mod bot;
mod repl;

use std::path::{Path, PathBuf};

use libp2p::Multiaddr;
use maya_chat::client::{Client, Received};
use maya_chat::identity::Identity;
use maya_chat::net;
use maya_chat::relay::Relay;
use maya_chat::{Address, ChatError, courier, now};

/// How long a published prekey stays valid, seconds.
const PREKEY_TTL: u64 = 30 * 24 * 3_600;

struct Args {
    home: PathBuf,
    command: String,
    relay: Option<String>,
    listen: Vec<String>,
    to: Option<String>,
    epoch: u32,
    stamp_bits: u32,
    poll: u64,
    text: Vec<String>,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        home: PathBuf::from(".maya-chat"),
        command: String::new(),
        relay: None,
        listen: Vec::new(),
        to: None,
        epoch: 1,
        stamp_bits: maya_chat::relay::STAMP_BITS,
        poll: repl::DEFAULT_POLL_SECS,
        text: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--version" => {
                println!("maya-chat {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--home" => a.home = PathBuf::from(value()?),
            "--relay" => a.relay = Some(value()?),
            "--listen" => a.listen.push(value()?),
            "--to" => a.to = Some(value()?),
            "--epoch" => a.epoch = value()?.parse().map_err(|_| "--epoch is a number")?,
            "--stamp-bits" => {
                a.stamp_bits = value()?.parse().map_err(|_| "--stamp-bits is a number")?;
            }
            "--poll" => {
                a.poll = value()?
                    .parse()
                    .ok()
                    .filter(|s| *s > 0)
                    .ok_or("--poll is a positive number of seconds")?;
            }
            _ if a.command.is_empty() => a.command = arg,
            _ => a.text.push(arg),
        }
    }
    Ok(a)
}

fn identity(home: &Path) -> Result<Identity, String> {
    let path = home.join("identity.key");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e} (run `maya-chat init` first)", path.display()))?;
    let bytes = hex::decode(text.trim()).map_err(|_| "identity.key is not hex")?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "identity.key is not 32 bytes")?;
    Ok(Identity::from_seed(seed))
}

fn load_client(home: &Path) -> Result<Client, String> {
    let me = identity(home)?;
    match std::fs::read(home.join("sessions.bin")) {
        Ok(saved) => Client::restore(me, &saved).map_err(|e| format!("sessions.bin: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Client::new(me)),
        Err(e) => Err(format!("sessions.bin: {e}")),
    }
}

/// Written to a temporary file and renamed, so a crash mid-write cannot
/// leave a truncated session store behind.
fn save_client(home: &Path, client: &Client) -> Result<(), String> {
    let tmp = home.join("sessions.bin.tmp");
    std::fs::write(&tmp, client.save().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, home.join("sessions.bin")).map_err(|e| e.to_string())
}

fn relay_key(home: &Path) -> Result<libp2p::identity::Keypair, String> {
    let path = home.join("relay.key");
    if let Ok(bytes) = std::fs::read(&path) {
        return libp2p::identity::Keypair::from_protobuf_encoding(&bytes)
            .map_err(|e| e.to_string());
    }
    let key = libp2p::identity::Keypair::generate_ed25519();
    std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
    std::fs::write(
        &path,
        key.to_protobuf_encoding().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(key)
}

fn address(hex_text: &str) -> Result<Address, String> {
    hex::decode(hex_text)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| format!("{hex_text} is not a 64-character hex address"))
}

fn relay_addr(a: &Args) -> Result<Multiaddr, String> {
    let relay = a.relay.as_deref().ok_or("--relay is required")?;
    relay.parse().map_err(|e| format!("{relay}: {e}"))
}

fn why(e: &ChatError) -> String {
    match e {
        ChatError::NoSession => {
            "no session, and the relay has no prekey for that address; ask them to `publish`".into()
        }
        other => other.to_string(),
    }
}

/// Prints what [`courier::collect`] returned; the count of envelopes.
fn show(results: &[Result<Received, ChatError>]) -> usize {
    for result in results {
        match result {
            Ok(r) => println!(
                "from {}: {}",
                hex::encode(r.from),
                String::from_utf8_lossy(&r.text)
            ),
            Err(ChatError::NoSession) => {
                eprintln!("skipped a message for a session this identity does not hold");
            }
            Err(e) => eprintln!("could not open a message: {e}"),
        }
    }
    results.len()
}

async fn send(a: &Args) -> Result<(), String> {
    let relay = relay_addr(a)?;
    let to = address(a.to.as_deref().ok_or("--to is required")?)?;
    let mut client = load_client(&a.home)?;
    courier::deliver(&mut client, &relay, to, a.text.join(" ").as_bytes(), now())
        .await
        .map_err(|e| why(&e))?;
    // Saved only once the relay holds the message: a send that failed must
    // not advance the chain past a message the peer will never see.
    save_client(&a.home, &client)?;
    println!("sent to {}", hex::encode(to));
    Ok(())
}

async fn recv(a: &Args) -> Result<(), String> {
    let relay = relay_addr(a)?;
    let mut client = load_client(&a.home)?;
    let results = courier::collect(&mut client, &relay, now())
        .await
        .map_err(|e| why(&e))?;
    save_client(&a.home, &client)?;
    println!("{} message(s)", show(&results));
    Ok(())
}

async fn run(a: Args) -> Result<(), String> {
    match a.command.as_str() {
        "init" => {
            let path = a.home.join("identity.key");
            if path.exists() {
                return Err(format!(
                    "{} exists; refusing to overwrite an identity",
                    path.display()
                ));
            }
            let me = Identity::generate().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&a.home).map_err(|e| e.to_string())?;
            std::fs::write(&path, hex::encode(me.seed())).map_err(|e| e.to_string())?;
            println!("{}", hex::encode(me.address()));
            Ok(())
        }
        "address" => {
            println!("{}", hex::encode(identity(&a.home)?.address()));
            Ok(())
        }
        "relay" => {
            let listen = if a.listen.is_empty() {
                vec!["/ip4/0.0.0.0/tcp/4001".to_owned()]
            } else {
                a.listen.clone()
            }
            .iter()
            .map(|l| l.parse().map_err(|e| format!("--listen {l}: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
            if a.stamp_bits < maya_chat::relay::STAMP_BITS {
                eprintln!(
                    "warning: postage of {} bits is below the default {}; mailboxes are cheaper to flood",
                    a.stamp_bits,
                    maya_chat::relay::STAMP_BITS
                );
            }
            net::run_relay(relay_key(&a.home)?, listen, Relay::new(a.stamp_bits))
                .await
                .map_err(|e| e.to_string())
        }
        "publish" => {
            let me = identity(&a.home)?;
            let bundle = me
                .prekey_bundle(a.epoch, now() + PREKEY_TTL)
                .map_err(|e| e.to_string())?;
            courier::publish(&relay_addr(&a)?, bundle)
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "published prekey {} for {}",
                a.epoch,
                hex::encode(me.address())
            );
            Ok(())
        }
        "send" => send(&a).await,
        "recv" => recv(&a).await,
        "chat" => repl::chat(&a).await,
        "bot" => bot::run(&a).await,
        other => Err(format!(
            "unknown command `{other}`: init, address, relay, publish, send, recv, chat, bot"
        )),
    }
}

#[tokio::main]
async fn main() {
    let result = match args() {
        Ok(a) => run(a).await,
        Err(e) => Err(e),
    };
    if let Err(e) = result {
        eprintln!("maya-chat: {e}");
        std::process::exit(1);
    }
}
