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

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use maya_crypto_pq::suite::MasterSeed;
use maya_signer::backend::KeystoreBackend;
use maya_signer::channel::{self, Identity};
use maya_signer::keystore;
use maya_signer::protection::SlashingDb;
use maya_signer::service::{Request, Service};
use zeroize::Zeroizing;

/// How long a connection may take to authenticate. Without a bound, a peer
/// that connects and sends nothing would hold a worker for as long as it liked.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Connections held at once, in total. A stalled peer costs one of these for
/// at most `HANDSHAKE_TIMEOUT`; past the cap, new connections are dropped.
const MAX_CONNECTIONS: usize = 16;
/// Connections held at once from one source address. The node keeps a single
/// channel, so a few per address is ample, and one address cannot take them all.
const MAX_PER_PEER: usize = 4;

#[derive(Default)]
struct Admission {
    total: usize,
    per_peer: HashMap<IpAddr, usize>,
}

/// Admits connections up to `MAX_CONNECTIONS` in total and `MAX_PER_PEER` per address.
#[derive(Clone, Default)]
struct Gate(Arc<Mutex<Admission>>);

/// One held connection; releasing it on drop frees its share of the gate.
struct Permit {
    gate: Gate,
    ip: IpAddr,
}

impl Gate {
    fn admit(&self, ip: IpAddr) -> Option<Permit> {
        let mut a = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let held = a.per_peer.get(&ip).copied().unwrap_or(0);
        if a.total >= MAX_CONNECTIONS || held >= MAX_PER_PEER {
            return None;
        }
        a.total += 1;
        *a.per_peer.entry(ip).or_default() += 1;
        Some(Permit {
            gate: self.clone(),
            ip,
        })
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut a = self.gate.0.lock().unwrap_or_else(PoisonError::into_inner);
        a.total = a.total.saturating_sub(1);
        let held = a.per_peer.entry(self.ip).or_default();
        *held = held.saturating_sub(1);
        if *held == 0 {
            a.per_peer.remove(&self.ip);
        }
    }
}

/// A stream with one deadline over every read, not a timeout per read: a
/// per-read timeout restarts on each byte, so a peer sending one byte every
/// few seconds would hold the slot indefinitely.
struct Deadline {
    stream: TcpStream,
    until: Option<Instant>,
}

impl Deadline {
    /// Lifts the deadline once the peer is authenticated.
    fn clear(&mut self) -> std::io::Result<()> {
        self.until = None;
        self.stream.set_read_timeout(None)
    }
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(until) = self.until {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

impl Write for Deadline {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

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
                // `from_str_radix` alone accepts a sign: "+f" would parse as 0x0f.
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
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
    let protection = PathBuf::from(flag(f, "protection")?);
    let db = SlashingDb::open(&protection).map_err(|e| e.to_string())?;
    // Block attestations (ADR-038) get a database beside the DAG one: heights
    // and rounds are different counters, and must not share a floor.
    let attestations = SlashingDb::open(&protection.with_extension("attestations.jsonl"))
        .map_err(|e| e.to_string())?;
    // Requests are still serialised through one protection database: each
    // session takes the lock only for the duration of one `handle` call.
    let service = Arc::new(Mutex::new(
        Service::new(backend, db).with_attestations(attestations),
    ));
    let identity = Arc::new(identity);
    let allowed = Arc::new(allowed);
    let listener = TcpListener::bind(flag(f, "listen")?).map_err(|e| e.to_string())?;
    eprintln!(
        "maya2c-signer: serving on {}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    let gate = Gate::default();
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // Over a cap, the connection is dropped at once rather than queued
        // behind stalled peers.
        let Ok(peer) = stream.peer_addr() else {
            continue;
        };
        let Some(permit) = gate.admit(peer.ip()) else {
            continue;
        };
        let (service, identity, allowed) = (
            Arc::clone(&service),
            Arc::clone(&identity),
            Arc::clone(&allowed),
        );
        let spawned = std::thread::Builder::new().spawn(move || {
            let _permit = permit;
            session(stream, &identity, &allowed, &service);
        });
        if spawned.is_err() {
            eprintln!("maya2c-signer: could not start a connection worker");
        }
    }
    Ok(())
}

/// Serves one node connection until it closes or sends a frame that does not parse.
fn session(
    stream: TcpStream,
    identity: &Identity,
    allowed: &[Vec<u8>],
    service: &Mutex<Service<KeystoreBackend>>,
) {
    // A write that blocks (a peer that stops reading) must not hold the
    // worker either, during the handshake or after it.
    if stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)).is_err() {
        return;
    }
    let stream = Deadline {
        stream,
        until: Some(Instant::now() + HANDSHAKE_TIMEOUT),
    };
    let Ok(mut ch) = channel::server(stream, identity, allowed) else {
        eprintln!("maya2c-signer: refused a connection that failed authentication");
        return;
    };
    // An authenticated node may sit idle between requests.
    if ch.stream_mut().clear().is_err() {
        return;
    }
    while let Ok(frame) = ch.recv() {
        let Ok(req) = serde_json::from_slice::<Request>(&frame) else {
            break;
        };
        let resp = match service.lock() {
            Ok(mut s) => s.handle(&req),
            // A poisoned lock means an earlier request panicked mid-decision:
            // stop signing rather than trust the state.
            Err(_) => break,
        };
        let Ok(bytes) = serde_json::to_vec(&resp) else {
            break;
        };
        if ch.send(&bytes).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod admission_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
    }

    #[test]
    fn one_address_cannot_take_more_than_its_share() {
        let gate = Gate::default();
        let _held: Vec<_> = (0..MAX_PER_PEER)
            .map(|_| gate.admit(ip(1)).unwrap())
            .collect();
        assert!(gate.admit(ip(1)).is_none());
        assert!(
            gate.admit(ip(2)).is_some(),
            "another address is still served"
        );
    }

    #[test]
    fn the_total_cap_refuses_new_addresses() {
        let gate = Gate::default();
        let _held: Vec<_> = (0..MAX_CONNECTIONS)
            .map(|n| gate.admit(ip(u8::try_from(n).unwrap())).unwrap())
            .collect();
        assert!(gate.admit(ip(200)).is_none());
    }

    #[test]
    fn a_released_connection_frees_its_share() {
        let gate = Gate::default();
        let mut held: Vec<_> = (0..MAX_PER_PEER)
            .map(|_| gate.admit(ip(1)).unwrap())
            .collect();
        assert!(gate.admit(ip(1)).is_none());
        held.pop();
        assert!(gate.admit(ip(1)).is_some());
    }
}

fn main() -> Result<(), String> {
    // Operators ask a deployed binary what it is before anything else.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
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
