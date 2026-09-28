//! End to end over real sockets: the `maya-chat` binary runs a relay on
//! localhost, two identities publish, send and receive through it.

use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_maya-chat");

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
