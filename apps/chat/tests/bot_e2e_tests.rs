//! The welcome bot over real sockets: a relay and a bot run as `maya-chat`
//! processes, and a new identity that writes to the bot gets a sealed reply.

use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_maya-chat");
/// Longest the whole exchange may take; minting postage in a debug build is
/// the slow step, twice (the message and the reply).
const DEADLINE: Duration = Duration::from_secs(180);

/// A child process killed when the test ends, pass or fail.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        // Runs until killed; a failure here means it already exited.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawns `maya-chat` with `args` and returns the first stdout line that
/// starts with `prefix`. Stdout is drained on a thread for the rest of the
/// process's life, so a later print never meets a closed pipe.
fn spawn_until(home: &Path, args: &[&str], prefix: &'static str) -> (Killed, String) {
    let mut child = Command::new(BIN)
        .arg("--home")
        .arg(home)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn maya-chat");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut tx = Some(tx);
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.starts_with(prefix)
                && let Some(tx) = tx.take()
            {
                // The test may have timed out and dropped the receiver.
                let _ = tx.send(line);
            }
        }
    });
    let line = rx.recv_timeout(DEADLINE).expect("process announced itself");
    (Killed(child), line)
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
fn a_new_identity_that_writes_to_the_bot_is_welcomed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (bot_home, alice) = (dir.path().join("bot"), dir.path().join("alice"));
    let (_relay, line) = spawn_until(
        &dir.path().join("relay"),
        &["relay", "--listen", "/ip4/127.0.0.1/tcp/0"],
        "relay listening on /ip4/127.0.0.1/",
    );
    let relay = line.trim_start_matches("relay listening on ").to_owned();

    let bot_addr = run(&bot_home, &["init"]).trim().to_owned();
    let (_bot, line) = spawn_until(
        &bot_home,
        &["bot", "--relay", &relay, "--poll", "1"],
        "welcome bot ",
    );
    assert!(line.contains(&bot_addr), "{line}");

    run(&alice, &["init"]);
    run(&alice, &["send", "--relay", &relay, "--to", &bot_addr, "hello"]);

    let started = Instant::now();
    loop {
        let got = run(&alice, &["recv", "--relay", &relay]);
        if got.contains(&format!("from {bot_addr}: Welcome to Maya Chat")) {
            break;
        }
        assert!(started.elapsed() < DEADLINE, "no welcome yet: {got}");
        std::thread::sleep(Duration::from_millis(500));
    }

    // The session carries on: a command gets its own answer.
    run(&alice, &["send", "--relay", &relay, "--to", &bot_addr, "ping"]);
    let started = Instant::now();
    loop {
        let got = run(&alice, &["recv", "--relay", &relay]);
        if got.contains(&format!("from {bot_addr}: pong")) {
            break;
        }
        assert!(started.elapsed() < DEADLINE, "no pong yet: {got}");
        std::thread::sleep(Duration::from_millis(500));
    }
}
