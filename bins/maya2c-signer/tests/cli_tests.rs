//! The `maya2c-signer` binary, driven as an operator would drive it. Every key
//! here is generated in a temp directory for the test; none is a real key.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use maya_crypto_pq::suite::MasterSeed;
use maya_signer::channel::{self, Identity};
use maya_signer::keystore;
use maya_signer::protection::Kind;
use maya_signer::service::{Request, Response};
use tempfile::TempDir;

const PASSPHRASE: &str = "test-only passphrase 7f3a";
/// Long enough for a debug-build ML-DSA handshake, short enough that a signer
/// wedged by a silent peer fails the test instead of hanging it.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_maya2c-signer"))
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// A temp dir holding a passphrase file, a validator keystore and an
/// identity keystore, both made by `init`.
struct Setup {
    dir: TempDir,
}

impl Setup {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("pass"), format!("{PASSPHRASE}\n")).unwrap();
        std::fs::write(dir.path().join("wrong"), "not the passphrase\n").unwrap();
        let s = Self { dir };
        for ks in ["validator.ks", "identity.ks"] {
            let out = s.run(&[
                "init",
                "--keystore",
                path(&s.p(ks)),
                "--passphrase-file",
                path(&s.p("pass")),
            ]);
            assert!(
                out.status.success(),
                "init: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        s
    }

    fn p(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn run(&self, args: &[&str]) -> Output {
        bin().args(args).output().unwrap()
    }

    fn serve_args(&self, allow: &str, listen: &str) -> Vec<String> {
        [
            "serve",
            "--keystore",
            path(&self.p("validator.ks")),
            "--passphrase-file",
            path(&self.p("pass")),
            "--protection",
            path(&self.p("protection.jsonl")),
            "--identity-keystore",
            path(&self.p("identity.ks")),
            "--allow-node",
            allow,
            "--listen",
            listen,
        ]
        .map(str::to_owned)
        .to_vec()
    }

    /// The signer's channel public key, read the way a node operator would
    /// have recorded it.
    fn signer_pk(&self) -> Vec<u8> {
        let seed = keystore::open(&self.p("identity.ks"), PASSPHRASE.as_bytes()).unwrap();
        Identity::from_seed(&seed).public_key().to_vec()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn node_identity(b: u8) -> Identity {
    Identity::from_seed(&MasterSeed::from_bytes([b; 32]))
}

/// A running `serve`, killed (by its own handle) on drop.
struct Server {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn serve(s: &Setup, allowed: &Identity) -> Server {
    let mut child = bin()
        .args(s.serve_args(&hex(allowed.public_key()), "127.0.0.1:0"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let addr = loop {
        let line = lines.next().expect("signer exited before serving").unwrap();
        if let Some(a) = line.strip_prefix("maya2c-signer: serving on ") {
            break a.trim().parse().unwrap();
        }
    };
    // Keep draining so a refusal message never blocks the signer on a full pipe.
    std::thread::spawn(move || lines.for_each(drop));
    Server { child, addr }
}

fn connect(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(CLIENT_TIMEOUT)).unwrap();
    stream
}

fn vote(round: u64) -> Request {
    Request {
        kind: Kind::Vote,
        round,
        author: 0,
        digest: [round.to_le_bytes()[0]; 32],
    }
}

fn ask(ch: &mut channel::Channel<TcpStream>, r: &Request) -> Response {
    ch.send(&serde_json::to_vec(r).unwrap()).unwrap();
    serde_json::from_slice(&ch.recv().unwrap()).unwrap()
}

fn not_a_panic(out: &Output) {
    assert_ne!(
        out.status.code(),
        Some(101),
        "panicked: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success());
}

#[test]
fn a_wrong_passphrase_opens_nothing_and_echoes_nothing() {
    let s = Setup::new();
    let out = s.run(&[
        "export",
        "--keystore",
        path(&s.p("validator.ks")),
        "--passphrase-file",
        path(&s.p("wrong")),
        "--protection",
        path(&s.p("protection.jsonl")),
    ]);
    not_a_panic(&out);
    assert!(out.stdout.is_empty(), "printed on a wrong passphrase");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !all.contains("not the passphrase") && !all.contains(PASSPHRASE),
        "{all}"
    );
    // The keystore file is unchanged and still opens with the right one.
    assert!(keystore::open(&s.p("validator.ks"), PASSPHRASE.as_bytes()).is_ok());
}

#[test]
fn malformed_allow_node_hex_is_an_error_not_a_panic() {
    let s = Setup::new();
    // "+f+f" is what `u8::from_str_radix` alone would accept. The listen
    // address is unusable so a parser that accepts it fails later, not hangs.
    for bad in ["zz", "abc", "+f+f", "é0", "0x00", ""] {
        let out = bin()
            .args(s.serve_args(bad, "not-an-address"))
            .output()
            .unwrap();
        not_a_panic(&out);
        let err = String::from_utf8_lossy(&out.stderr);
        if bad.is_empty() {
            // Empty decodes to an empty key: no node will ever match it.
            continue;
        }
        assert!(err.contains("bad hex"), "{bad:?} accepted: {err}");
    }
}

#[test]
fn serve_refuses_an_unpinned_node_and_signs_for_the_pinned_one() {
    let s = Setup::new();
    let node = node_identity(1);
    let server = serve(&s, &node);
    let signer_pk = s.signer_pk();

    let stranger = channel::client(connect(server.addr), &node_identity(2), &signer_pk);
    assert!(stranger.is_err(), "an unpinned node completed a handshake");

    let mut ch = channel::client(connect(server.addr), &node, &signer_pk).unwrap();
    assert!(matches!(ask(&mut ch, &vote(1)), Response::Signed(_)));
}

#[test]
fn a_silent_connection_does_not_lock_the_pinned_node_out() {
    let s = Setup::new();
    let node = node_identity(1);
    let server = serve(&s, &node);
    // Connects and never speaks: before the handshake bound, the signer
    // waited on it forever and served nobody else.
    let _silent = TcpStream::connect(server.addr).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let mut ch = channel::client(connect(server.addr), &node, &s.signer_pk())
        .expect("the pinned node was locked out by a silent peer");
    assert!(matches!(ask(&mut ch, &vote(1)), Response::Signed(_)));
}

#[test]
fn an_oversize_frame_drops_the_connection_and_the_signer_keeps_serving() {
    let s = Setup::new();
    let node = node_identity(1);
    let server = serve(&s, &node);
    let signer_pk = s.signer_pk();

    let mut ch = channel::client(connect(server.addr), &node, &signer_pk).unwrap();
    // A length prefix far past the 64 KiB frame cap, then a little body.
    let stream = ch.stream_mut();
    stream
        .write_all(&(16u32 * 1024 * 1024).to_le_bytes())
        .unwrap();
    let _ = stream.write_all(&[0u8; 1024]);
    let mut buf = [0u8; 1];
    // The signer hangs up rather than allocating or answering.
    let read = stream.read(&mut buf);
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "signer answered an oversize frame"
    );

    let mut again = channel::client(connect(server.addr), &node, &signer_pk).unwrap();
    assert!(matches!(ask(&mut again, &vote(2)), Response::Signed(_)));
}
