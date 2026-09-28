//! `maya-chat` — the command-line messenger and relay.
//!
//! ```text
//! maya-chat --home DIR init                       create an identity
//! maya-chat --home DIR address                    print it
//! maya-chat --home DIR relay --listen /ip4/0.0.0.0/tcp/4001
//! maya-chat --home DIR publish --relay ADDR       publish a prekey bundle
//! maya-chat --home DIR send --relay ADDR --to HEX TEXT...
//! maya-chat --home DIR recv --relay ADDR          fetch and print messages
//! ```
//!
//! The identity seed is stored in `DIR/identity.key`, unencrypted in this
//! first version: protect the directory as you would a private key.
//! Passphrase protection comes with the keystore shared with the wallet.

use std::path::{Path, PathBuf};

use maya_chat::client::Client;
use maya_chat::identity::Identity;
use maya_chat::net::{self, Request, Response};
use maya_chat::relay::fetch_bytes;
use maya_chat::{Address, ChatError, now};

/// How long a published prekey stays valid, seconds.
const PREKEY_TTL: u64 = 30 * 24 * 3_600;

struct Args {
    home: PathBuf,
    command: String,
    relay: Option<String>,
    listen: Option<String>,
    to: Option<String>,
    epoch: u32,
    text: Vec<String>,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        home: PathBuf::from(".maya-chat"),
        command: String::new(),
        relay: None,
        listen: None,
        to: None,
        epoch: 1,
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
            "--listen" => a.listen = Some(value()?),
            "--to" => a.to = Some(value()?),
            "--epoch" => a.epoch = value()?.parse().map_err(|_| "--epoch is a number")?,
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

async fn ask(relay: &str, request: Request) -> Result<Response, String> {
    let addr = relay.parse().map_err(|e| format!("{relay}: {e}"))?;
    match net::call(&addr, request).await.map_err(|e| e.to_string())? {
        Response::Refused(why) => Err(format!("the relay refused: {why}")),
        other => Ok(other),
    }
}

async fn send(a: &Args) -> Result<(), String> {
    let me = identity(&a.home)?;
    let relay = a.relay.as_deref().ok_or("--relay is required")?;
    let to = address(a.to.as_deref().ok_or("--to is required")?)?;
    let Response::Bundle(Some(bundle)) = ask(relay, Request::Bundle(to)).await? else {
        return Err("the relay has no prekey for that address; ask them to `publish`".into());
    };
    let mut client = Client::new(me);
    let envelope = client
        .seal(to, Some(&bundle), a.text.join(" ").as_bytes(), now())
        .map_err(|e| e.to_string())?;
    ask(relay, Request::Deposit(envelope)).await?;
    println!("sent to {}", hex::encode(to));
    Ok(())
}

async fn recv(a: &Args) -> Result<(), String> {
    let me = identity(&a.home)?;
    let relay = a.relay.as_deref().ok_or("--relay is required")?;
    let Response::Challenge(nonce) = ask(relay, Request::Challenge(me.address())).await? else {
        return Err("unexpected answer to a challenge request".into());
    };
    let signature = me
        .sign(&fetch_bytes(&me.address(), &nonce))
        .map_err(|e| e.to_string())?;
    let request = Request::Fetch {
        address: me.address(),
        identity_key: me.public_key().to_vec(),
        signature,
    };
    let Response::Envelopes(envelopes) = ask(relay, request).await? else {
        return Err("unexpected answer to a fetch".into());
    };
    let mut client = Client::new(me);
    for envelope in &envelopes {
        match client.open(envelope, now()) {
            Ok(r) => println!(
                "from {}: {}",
                hex::encode(r.from),
                String::from_utf8_lossy(&r.text)
            ),
            Err(ChatError::NoSession) => {
                eprintln!("skipped a message for a session this run does not hold");
            }
            Err(e) => eprintln!("could not open a message: {e}"),
        }
    }
    println!("{} message(s)", envelopes.len());
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
            let listen = a
                .listen
                .as_deref()
                .unwrap_or("/ip4/0.0.0.0/tcp/4001")
                .parse()
                .map_err(|e| format!("--listen: {e}"))?;
            net::run_relay(relay_key(&a.home)?, listen)
                .await
                .map_err(|e| e.to_string())
        }
        "publish" => {
            let me = identity(&a.home)?;
            let bundle = me
                .prekey_bundle(a.epoch, now() + PREKEY_TTL)
                .map_err(|e| e.to_string())?;
            ask(
                a.relay.as_deref().ok_or("--relay is required")?,
                Request::Publish(bundle),
            )
            .await?;
            println!(
                "published prekey {} for {}",
                a.epoch,
                hex::encode(me.address())
            );
            Ok(())
        }
        "send" => send(&a).await,
        "recv" => recv(&a).await,
        other => Err(format!(
            "unknown command `{other}`: init, address, relay, publish, send, recv"
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
