//! `maya2c dap` as an editor drives it: the real binary on stdio, framed DAP
//! messages in and out, over the real `contracts/nft-game` module.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use maya2c_cli::dap::{Frame, read_message, write_message};
use serde_json::{Value, json};

fn message(from: &mut BufReader<ChildStdout>) -> Value {
    match read_message(from).unwrap().unwrap() {
        Frame::Message(v) => v,
        Frame::Skipped(why) => panic!("the adapter sent {why}"),
    }
}

struct Editor {
    child: Child,
    to: ChildStdin,
    from: BufReader<ChildStdout>,
    seq: u64,
}

impl Editor {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_maya2c"))
            .arg("dap")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let to = child.stdin.take().unwrap();
        let from = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            to,
            from,
            seq: 0,
        }
    }

    /// Sends a request; returns its response and every event before the next
    /// request's turn (the adapter writes the response first, then events).
    fn ask(&mut self, command: &str, arguments: &Value) -> (Value, Vec<Value>) {
        self.seq += 1;
        let request =
            json!({"seq": self.seq, "type": "request", "command": command, "arguments": arguments});
        write_message(&mut self.to, &request).unwrap();
        let response = message(&mut self.from);
        assert_eq!(response["request_seq"], self.seq);
        let expected_events = match command {
            "launch" if response["success"] == true => 2,
            "initialize" | "next" | "stepBack" | "continue" | "reverseContinue" | "goto" => 1,
            _ => 0,
        };
        let events = (0..expected_events)
            .map(|_| message(&mut self.from))
            .collect();
        (response, events)
    }

    fn line(&mut self) -> u64 {
        let (trace, _) = self.ask("stackTrace", &json!({"threadId": 1}));
        trace["body"]["stackFrames"][0]["line"].as_u64().unwrap()
    }
}

fn nft_game() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target-contracts/wasm32-unknown-unknown/release/nft_game.wasm")
        .display()
        .to_string()
}

#[test]
fn an_editor_steps_a_call_forward_and_back_over_dap() {
    let mut editor = Editor::start();
    let (init, events) = editor.ask("initialize", &json!({"adapterID": "maya2c"}));
    assert_eq!(init["body"]["supportsStepBack"], true);
    assert_eq!(events[0]["event"], "initialized");

    let (bad, _) = editor.ask("launch", &json!({"program": "does-not-exist.wasm"}));
    assert_eq!(
        bad["success"], false,
        "a missing program is answered, not fatal"
    );

    // `init` by the admin: op 0 then the admin address.
    let admin = "01".repeat(32);
    let input = format!("00{admin}");
    let (launch, events) = editor.ask(
        "launch",
        &json!({"program": nft_game(), "input": input, "caller": admin}),
    );
    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(events[1]["body"]["reason"], "entry");
    assert_eq!(editor.line(), 1, "before the call");

    // `init` is one host call: the write of "admin". Stepping past the end
    // stays at the end.
    let (_, stopped) = editor.ask("next", &json!({"threadId": 1}));
    assert_eq!(stopped[0]["event"], "stopped");
    assert_eq!(editor.line(), 2);
    editor.ask("next", &json!({"threadId": 1}));
    editor.ask("continue", &json!({"threadId": 1}));
    let end = editor.line();
    assert_eq!(end, 2, "clamped at the last step");

    // The admin key holds the admin after the write; stepping back shows it
    // absent again.
    let admin_value = |editor: &mut Editor| {
        let (v, _) = editor.ask("variables", &json!({"variablesReference": 1}));
        let vars = v["body"]["variables"].as_array().unwrap().clone();
        let admin = vars.iter().find(|v| v["name"] == "\"admin\"").unwrap();
        admin["value"].as_str().unwrap().to_owned()
    };
    assert!(admin_value(&mut editor).starts_with("0101"));
    editor.ask("stepBack", &json!({"threadId": 1}));
    assert_eq!(editor.line(), 1);
    assert_eq!(
        admin_value(&mut editor),
        "absent",
        "time travel: nothing written yet"
    );
    editor.ask("reverseContinue", &json!({"threadId": 1}));
    assert_eq!(editor.line(), 1);

    // The listing is the frame's source; goto jumps to a line; the console
    // takes the CLI's commands.
    let (source, _) = editor.ask("source", &json!({"sourceReference": 1}));
    let lines = source["body"]["content"].as_str().unwrap().lines().count();
    assert_eq!(u64::try_from(lines).unwrap(), end);
    editor.ask("goto", &json!({"threadId": 1, "targetId": 2}));
    assert_eq!(editor.line(), 2);
    let (eval, _) = editor.ask("evaluate", &json!({"expression": "l"}));
    assert!(eval["body"]["result"].as_str().unwrap().contains("steps"));
    let (bp, _) = editor.ask("setBreakpoints", &json!({"breakpoints": [{"line": 3}]}));
    assert_eq!(bp["body"]["breakpoints"][0]["verified"], false);

    editor.ask("disconnect", &json!({}));
    assert!(editor.child.wait().unwrap().success());
}

#[test]
fn a_bad_body_is_skipped_and_the_session_goes_on() {
    let mut editor = Editor::start();
    // Well framed, not JSON: reported, not fatal.
    let junk = b"Content-Length: 5\r\n\r\nhello";
    std::io::Write::write_all(&mut editor.to, junk).unwrap();
    let note = message(&mut editor.from);
    assert_eq!(note["event"], "output");
    assert!(
        note["body"]["output"]
            .as_str()
            .unwrap()
            .contains("not JSON")
    );
    let (init, _) = editor.ask("initialize", &json!({}));
    assert_eq!(init["success"], true, "still serving");
    editor.ask("disconnect", &json!({}));
    assert!(editor.child.wait().unwrap().success());
}

#[test]
fn framing_without_a_length_ends_the_session_cleanly() {
    let no_length: &[u8] = b"X-Other: 1\r\n\r\n{}";
    assert!(maya2c_cli::dap::serve(no_length, Vec::new()).is_err());
    let long = format!("Content-Length: {}\r\n\r\n", "9".repeat(2_000));
    assert!(
        maya2c_cli::dap::serve(long.as_bytes(), Vec::new()).is_err(),
        "over-long header line"
    );
}
