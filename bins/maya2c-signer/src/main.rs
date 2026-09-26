//! `maya2c-signer` — the remote validator signer (Master Prompt 16 §1).
//!
//! ```text
//! maya2c-signer init    --keystore K --passphrase-file P
//! maya2c-signer serve   --keystore K --passphrase-file P --protection DB
//!                       --identity-keystore I --allow-node <hex ML-DSA-65 pk> --listen 127.0.0.1:9701
//! maya2c-signer export  --keystore K --passphrase-file P --protection DB > interchange.json
//! maya2c-signer import  --keystore K --passphrase-file P --protection DB --file interchange.json
//! ```
//!
//! The passphrase is read from a file (mode 0600, typically a tmpfs mount
//! provisioned by the operator) and wiped after use. It is never taken from
//! the command line or an environment variable value, where `ps` and crash
//! dumps would see it. The channel identity is a second keystore, so the
//! validator key never doubles as a transport key.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::PathBuf;

use maya_crypto_pq::suite::MasterSeed;
use maya_signer::backend::KeystoreBackend;
use maya_signer::channel::{self, Identity};
use maya_signer::keystore;
use maya_signer::protection::SlashingDb;
use maya_signer::service::{Request, Service};
use zeroize::Zeroizing;

fn args() -> Result<(String, BTreeMap<String, String>), String> {
    let mut it = std::env::args().skip(1);
    let cmd = it
        .next()
        .ok_or("usage: maya2c-signer <init|serve|export|import> [--flag value]…")?;
    let mut flags = BTreeMap::new();
    while let Some(k) = it.next() {
        let v = it.next().ok_or(format!("{k} needs a value"))?;
        flags.insert(k.trim_start_matches("--").to_string(), v);
    }
    Ok((cmd, flags))
}

fn flag<'a>(f: &'a BTreeMap<String, String>, k: &str) -> Result<&'a str, String> {
    f.get(k)
        .map(String::as_str)
        .ok_or(format!("--{k} is required"))
}

fn passphrase(f: &BTreeMap<String, String>) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut bytes =
        Zeroizing::new(std::fs::read(flag(f, "passphrase-file")?).map_err(|e| e.to_string())?);
    while bytes.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        bytes.pop();
    }
    Ok(bytes)
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or("bad hex".to_string())
        })
        .collect()
}

fn open_seed(f: &BTreeMap<String, String>, which: &str) -> Result<MasterSeed, String> {
    keystore::open(&PathBuf::from(flag(f, which)?), &passphrase(f)?).map_err(|e| e.to_string())
}

fn serve(f: &BTreeMap<String, String>) -> Result<(), String> {
    let backend = KeystoreBackend::new(&open_seed(f, "keystore")?);
    let identity = Identity::from_seed(&open_seed(f, "identity-keystore")?);
    let allowed = vec![unhex(flag(f, "allow-node")?)?];
    let db = SlashingDb::open(&PathBuf::from(flag(f, "protection")?)).map_err(|e| e.to_string())?;
    let mut service = Service::new(backend, db);
    let listener = TcpListener::bind(flag(f, "listen")?).map_err(|e| e.to_string())?;
    eprintln!(
        "maya2c-signer: serving on {}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    // One connection at a time: requests are serialised through one
    // protection database, which is the point.
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Ok(mut ch) = channel::server(stream, &identity, &allowed) else {
            eprintln!("maya2c-signer: refused a connection that failed authentication");
            continue;
        };
        while let Ok(frame) = ch.recv() {
            let Ok(req) = serde_json::from_slice::<Request>(&frame) else {
                break;
            };
            let resp = service.handle(&req);
            if ch
                .send(&serde_json::to_vec(&resp).map_err(|e| e.to_string())?)
                .is_err()
            {
                break;
            }
        }
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let (cmd, f) = args()?;
    match cmd.as_str() {
        "init" => {
            let seed = MasterSeed::generate().map_err(|e| e.to_string())?;
            let pk = keystore::create(
                &PathBuf::from(flag(&f, "keystore")?),
                &seed,
                &passphrase(&f)?,
            )
            .map_err(|e| e.to_string())?;
            let hex = pk.iter().fold(String::new(), |mut s, b| {
                use std::fmt::Write;
                let _ = write!(s, "{b:02x}");
                s
            });
            println!("{hex}");
            Ok(())
        }
        "serve" => serve(&f),
        "export" => {
            let backend = KeystoreBackend::new(&open_seed(&f, "keystore")?);
            let db = SlashingDb::open(&PathBuf::from(flag(&f, "protection")?))
                .map_err(|e| e.to_string())?;
            let pk = maya_signer::backend::SignerBackend::public_key(&backend).to_vec();
            println!(
                "{}",
                serde_json::to_string_pretty(&db.export(&pk)).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        "import" => {
            let backend = KeystoreBackend::new(&open_seed(&f, "keystore")?);
            let mut db = SlashingDb::open(&PathBuf::from(flag(&f, "protection")?))
                .map_err(|e| e.to_string())?;
            let file = serde_json::from_str(
                &std::fs::read_to_string(flag(&f, "file")?).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let pk = maya_signer::backend::SignerBackend::public_key(&backend).to_vec();
            let n = db.import(&file, &pk).map_err(|e| e.to_string())?;
            println!("imported {n} records");
            Ok(())
        }
        other => Err(format!("unknown command {other}")),
    }
}
