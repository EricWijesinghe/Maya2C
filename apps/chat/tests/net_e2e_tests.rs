//! End to end over real sockets: the `maya-chat` binary runs a relay on
//! localhost, two identities publish, send and receive through it.

use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_maya-chat");
/// Longest a step may take before a test fails instead of hanging; minting
/// postage in a debug build is the slowest step, well under this.
const LINE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// A child process killed when the test ends, pass or fail.
struct Relay(Child);

impl Drop for Relay {
    fn drop(&mut self) {
        // The relay runs until killed; a failure here means it already exited.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_relay(home: &Path) -> (Relay, String) {
    let mut child = Command::new(BIN)
        .args(["--home"])
        .arg(home)
        .args(["relay", "--listen", "/ip4/127.0.0.1/tcp/0"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn relay");
    let stdout = child.stdout.take().expect("stdout");
    let relay = Relay(child);
    let line = BufReader::new(stdout)
        .lines()
        .map(|l| l.expect("read relay output"))
        .find(|l| l.starts_with("relay listening on /ip4/127.0.0.1/"))
        .expect("relay printed its address");
    let address = line.trim_start_matches("relay listening on ").to_owned();
    (relay, address)
}

fn run(home: &Path, args: &[&str]) -> String {
    let out = Command::new(BIN)
        .arg("--home")
        .arg(home)
        .args(args)
        .output()
        .expect("run maya-chat");
    assert!(
        out.status.success(),
        "maya-chat {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

#[test]
fn two_people_chat_through_a_relay() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (alice, bob) = (dir.path().join("alice"), dir.path().join("bob"));
    let (_relay, relay) = start_relay(&dir.path().join("relay"));

    let alice_addr = run(&alice, &["init"]).trim().to_owned();
    let bob_addr = run(&bob, &["init"]).trim().to_owned();
    assert_eq!(run(&bob, &["address"]).trim(), bob_addr);

    run(&bob, &["publish", "--relay", &relay]);
    run(
        &alice,
        &["send", "--relay", &relay, "--to", &bob_addr, "hello", "bob"],
    );
    run(
        &alice,
        &["send", "--relay", &relay, "--to", &bob_addr, "second"],
    );

    let got = run(&bob, &["recv", "--relay", &relay]);
    assert!(
        got.contains(&format!("from {alice_addr}: hello bob")),
        "{got}"
    );
    assert!(got.contains(&format!("from {alice_addr}: second")), "{got}");
    assert!(got.contains("2 message(s)"), "{got}");
    // The mailbox was emptied.
    assert!(run(&bob, &["recv", "--relay", &relay]).contains("0 message(s)"));

    // Sessions outlive the process: Bob replies without Alice's prekey, and
    // Alice's next run reads it, then a follow-up in the same session.
    run(
        &bob,
        &["send", "--relay", &relay, "--to", &alice_addr, "hi alice"],
    );
    let got = run(&alice, &["recv", "--relay", &relay]);
    assert!(got.contains(&format!("from {bob_addr}: hi alice")), "{got}");
    run(
        &alice,
        &["send", "--relay", &relay, "--to", &bob_addr, "third"],
    );
    let got = run(&bob, &["recv", "--relay", &relay]);
    assert!(got.contains(&format!("from {alice_addr}: third")), "{got}");
}

#[test]
fn init_refuses_to_overwrite_an_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    run(dir.path(), &["init"]);
    let out = Command::new(BIN)
        .arg("--home")
        .arg(dir.path())
        .arg("init")
        .output()
        .expect("run");
    assert!(!out.status.success());
}

#[test]
fn an_interactive_chat_sends_lines_and_shows_replies() {
    use std::io::Write as _;

    let dir = tempfile::tempdir().expect("tempdir");
    let (alice, bob) = (dir.path().join("alice"), dir.path().join("bob"));
    let (_relay, relay) = start_relay(&dir.path().join("relay"));
    run(&alice, &["init"]);
    let bob_addr = run(&bob, &["init"]).trim().to_owned();
    run(&bob, &["publish", "--relay", &relay]);

    let mut chat = Relay(
        Command::new(BIN)
            .arg("--home")
            .arg(&alice)
            .args(["chat", "--relay", &relay, "--poll", "60"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn chat"),
    );
    let mut stdin = chat.0.stdin.take().expect("stdin");
    // Read on a thread with a deadline per line: a chat that stops talking
    // fails this test by name instead of hanging the whole CI job.
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = chat.0.stdout.take().expect("stdout");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.expect("read chat output")).is_err() {
                break;
            }
        }
    });
    let next = || {
        rx.recv_timeout(LINE_DEADLINE)
            .expect("the chat printed nothing within the deadline")
    };
    assert!(next().starts_with("chatting as "));

    writeln!(stdin, "/to {bob_addr}").expect("write");
    assert!(next().starts_with("now talking to "));
    writeln!(stdin, "hello from the prompt").expect("write");
    assert_eq!(next(), "sent");

    let got = run(&bob, &["recv", "--relay", &relay]);
    assert!(got.contains("hello from the prompt"), "{got}");
    let alice_addr = run(&alice, &["address"]).trim().to_owned();
    run(
        &bob,
        &["send", "--relay", &relay, "--to", &alice_addr, "hi back"],
    );

    // End of input: the chat checks the mailbox once more, then leaves.
    drop(stdin);
    assert_eq!(next(), format!("from {bob_addr}: hi back"));
    assert!(chat.0.wait().expect("wait").success());
}

#[test]
fn a_relay_is_reachable_by_name() {
    // `/dns4/localhost/...` resolves through the OS, as `seed1.maya2c.dev` will.
    let dir = tempfile::tempdir().expect("tempdir");
    let (_relay, relay) = start_relay(&dir.path().join("relay"));
    let by_name = relay.replacen("/ip4/127.0.0.1/", "/dns4/localhost/", 1);
    let bob = dir.path().join("bob");
    run(&bob, &["init"]);
    assert!(run(&bob, &["publish", "--relay", &by_name]).contains("published prekey"));
}

#[test]
fn a_relay_serves_over_websocket_as_well() {
    // Behind a tunnel or HTTP proxy the relay listens on `/ws`; the same
    // identities chat through it exactly as over raw TCP.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("relay");
    let mut child = Command::new(BIN)
        .arg("--home")
        .arg(&home)
        .args([
            "relay",
            "--listen",
            "/ip4/127.0.0.1/tcp/0",
            "--listen",
            "/ip4/127.0.0.1/tcp/0/ws",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn relay");
    let stdout = child.stdout.take().expect("stdout");
    let _relay = Relay(child);
    let ws = BufReader::new(stdout)
        .lines()
        .map(|l| l.expect("read relay output"))
        .filter_map(|l| l.strip_prefix("relay listening on ").map(str::to_owned))
        .find(|a| a.starts_with("/ip4/127.0.0.1/") && a.contains("/ws/"))
        .expect("a /ws listen address");
    let (alice, bob) = (dir.path().join("alice"), dir.path().join("bob"));
    let alice_addr = run(&alice, &["init"]).trim().to_owned();
    let bob_addr = run(&bob, &["init"]).trim().to_owned();
    run(&bob, &["publish", "--relay", &ws]);
    run(
        &alice,
        &["send", "--relay", &ws, "--to", &bob_addr, "over websocket"],
    );
    let got = run(&bob, &["recv", "--relay", &ws]);
    assert!(
        got.contains(&format!("from {alice_addr}: over websocket")),
        "{got}"
    );
}
