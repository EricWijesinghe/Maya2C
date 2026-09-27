//! `maya2c dap` — the time-travel debugger behind the Debug Adapter Protocol
//! (Master Prompt 24: "a debugger integrated in VS Code").
//!
//! An editor starts `maya2c dap` and speaks DAP over stdin/stdout. `launch`
//! records the call once ([`crate::record_spec`]); from then on every request
//! is a move of the cursor over the recording, so **step back is exact and
//! re-executes nothing** — the adapter advertises `supportsStepBack`.
//!
//! What the editor sees: one frame whose source is the recording itself
//! (line `k + 1` is the state after host call `k`, line 1 is before the call),
//! and three scopes — storage at the cursor, events so far, the call. The
//! debug console accepts the CLI's commands (`n`, `b`, `g 4`, `s`, `e`, `l`).
//!
//! Limits, as for the CLI: steps are host calls, not source lines (contracts
//! carry no DWARF and the VM has no per-instruction hook), so breakpoints are
//! accepted but never verified.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::debug::Session;
use crate::{CallSpec, parse_caller, parse_hex};

/// Largest message body accepted; a DAP request is a few hundred bytes.
const MAX_BODY: usize = 1 << 20;
/// Default gas when `launch` names none, as `maya2c debug`.
const DEFAULT_GAS: u64 = 10_000_000;
const THREAD: u64 = 1;
const FRAME: u64 = 1;
const LISTING: u64 = 1;
const SCOPE_STORAGE: u64 = 1;
const SCOPE_EVENTS: u64 = 2;
const SCOPE_CALL: u64 = 3;

/// Longest header line accepted.
const MAX_HEADER_LINE: u64 = 1_024;
/// Most header lines before the blank one.
const MAX_HEADER_LINES: usize = 16;

/// One framed message, or one the adapter skipped without losing the stream.
#[derive(Debug)]
pub enum Frame {
    /// A JSON message.
    Message(Value),
    /// A well-framed body that was too large or not JSON; the stream is still
    /// in step, so the session goes on.
    Skipped(String),
}

fn content_length(r: &mut impl BufRead) -> anyhow::Result<Option<usize>> {
    let mut length = None;
    for _ in 0..=MAX_HEADER_LINES {
        let mut line = String::new();
        let read = std::io::Read::take(&mut *r, MAX_HEADER_LINE).read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }
        anyhow::ensure!(
            line.ends_with('\n'),
            "DAP header line over {MAX_HEADER_LINE} bytes"
        );
        let line = line.trim_end();
        if line.is_empty() {
            return length
                .map(Some)
                .ok_or_else(|| anyhow::anyhow!("DAP message without Content-Length"));
        }
        if let Some(n) = line.strip_prefix("Content-Length:") {
            length = Some(n.trim().parse::<usize>()?);
        }
    }
    anyhow::bail!("more than {MAX_HEADER_LINES} DAP header lines")
}

/// Reads one `Content-Length`-framed message; `None` at end of input.
///
/// # Errors
///
/// Framing the adapter cannot resynchronise from: a missing or non-numeric
/// `Content-Length`, an over-long header, or input ending mid-message. A body
/// over [`MAX_BODY`] or one that is not JSON is read past and reported as
/// [`Frame::Skipped`] instead.
pub fn read_message(r: &mut impl BufRead) -> anyhow::Result<Option<Frame>> {
    let Some(length) = content_length(r)? else {
        return Ok(None);
    };
    if length > MAX_BODY {
        let wanted = u64::try_from(length)?;
        let skipped = std::io::copy(
            &mut std::io::Read::take(&mut *r, wanted),
            &mut std::io::sink(),
        )?;
        anyhow::ensure!(skipped == wanted, "input ended inside a DAP message");
        return Ok(Some(Frame::Skipped(format!(
            "a {length}-byte message, over {MAX_BODY}"
        ))));
    }
    let mut body = vec![0; length];
    r.read_exact(&mut body)?;
    Ok(Some(match serde_json::from_slice(&body) {
        Ok(v) => Frame::Message(v),
        Err(e) => Frame::Skipped(format!("a message that is not JSON: {e}")),
    }))
}

/// Writes one framed message.
///
/// # Errors
///
/// The writer failed.
pub fn write_message(w: &mut impl Write, message: &Value) -> anyhow::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

/// Serves DAP until `disconnect` or end of input.
///
/// # Errors
///
/// A transport failure, or framing [`read_message`] cannot recover from. A
/// failing request is answered, and a skipped message is reported on the
/// debug console; neither ends the session.
pub fn serve(mut input: impl BufRead, mut output: impl Write) -> anyhow::Result<()> {
    let mut adapter = Adapter::default();
    while let Some(frame) = read_message(&mut input)? {
        let request = match frame {
            Frame::Message(request) => request,
            Frame::Skipped(why) => {
                let text = format!("maya2c dap skipped {why}\n");
                let note = adapter.event("output", &json!({"category": "stderr", "output": text}));
                write_message(&mut output, &note)?;
                continue;
            }
        };
        for message in adapter.handle(&request) {
            write_message(&mut output, &message)?;
        }
        if adapter.finished {
            break;
        }
    }
    Ok(())
}

/// The adapter's state: the recording, once launched.
#[derive(Default)]
pub struct Adapter {
    seq: u64,
    session: Option<Session>,
    /// Set by `disconnect` / `terminate`.
    pub finished: bool,
}

type Reply = Result<Value, String>;

impl Adapter {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn event(&mut self, event: &str, body: &Value) -> Value {
        json!({"seq": self.next_seq(), "type": "event", "event": event, "body": body})
    }

    fn stopped(&mut self, reason: &str) -> Value {
        let description = self.session.as_ref().map(Session::position);
        self.event("stopped", &json!({"reason": reason, "threadId": THREAD, "description": description, "allThreadsStopped": true}),
        )
    }

    /// Answers one request with the messages to send, response first.
    pub fn handle(&mut self, request: &Value) -> Vec<Value> {
        let command = request["command"].as_str().unwrap_or_default().to_owned();
        let args = &request["arguments"];
        let (reply, after) = self.dispatch(&command, args);
        let (success, body, message) = match reply {
            Ok(body) => (true, body, None),
            Err(why) => (false, Value::Null, Some(why)),
        };
        let response = json!({
            "seq": self.next_seq(), "type": "response", "request_seq": request["seq"],
            "success": success, "command": command, "message": message, "body": body,
        });
        std::iter::once(response).chain(after).collect()
    }

    fn dispatch(&mut self, command: &str, args: &Value) -> (Reply, Vec<Value>) {
        match command {
            "initialize" => {
                let initialized = self.event("initialized", &json!({}));
                (Ok(capabilities()), vec![initialized])
            }
            "launch" => self.launch(args),
            "configurationDone" | "setExceptionBreakpoints" => (Ok(json!({})), vec![]),
            "setBreakpoints" => (Ok(unverified_breakpoints(args)), vec![]),
            "threads" => (
                Ok(json!({"threads": [{"id": THREAD, "name": "call"}]})),
                vec![],
            ),
            "disconnect" | "terminate" => {
                self.finished = true;
                (Ok(json!({})), vec![])
            }
            "next" | "stepIn" | "stepOut" | "stepBack" | "continue" | "reverseContinue"
            | "goto" => self.step(command, args),
            _ => (self.inspect(command, args), vec![]),
        }
    }

    fn session(&self) -> Result<&Session, String> {
        self.session
            .as_ref()
            .ok_or_else(|| "no call launched".to_owned())
    }

    fn launch(&mut self, args: &Value) -> (Reply, Vec<Value>) {
        match spec_from(args).and_then(|spec| crate::record_spec(&spec)) {
            Ok(session) => {
                let (gas, outcome) = session.summary();
                let summary = format!(
                    "recorded {} host calls; {outcome}; gas {} (whole call)\n",
                    session.total(),
                    gas.map_or_else(|| "n/a".into(), |g| g.to_string())
                );
                self.session = Some(session);
                let output =
                    self.event("output", &json!({"category": "console", "output": summary}));
                let stopped = self.stopped("entry");
                (Ok(json!({})), vec![output, stopped])
            }
            Err(e) => (Err(format!("launch: {e:#}")), vec![]),
        }
    }

    fn step(&mut self, command: &str, args: &Value) -> (Reply, Vec<Value>) {
        let Some(session) = self.session.as_mut() else {
            return (Err("no call launched".into()), vec![]);
        };
        let target = match command {
            "next" | "stepIn" | "stepOut" => session.cursor() + 1,
            "stepBack" => session.cursor().saturating_sub(1),
            "continue" => session.total(),
            "reverseContinue" => 0,
            // `goto` targets are listing lines; line k + 1 is step k.
            _ => usize::try_from(args["targetId"].as_u64().unwrap_or(1).saturating_sub(1))
                .unwrap_or(usize::MAX),
        };
        session.goto(target);
        let body = if command == "continue" {
            json!({"allThreadsContinued": true})
        } else {
            json!({})
        };
        let stopped = self.stopped(if command == "goto" { "goto" } else { "step" });
        (Ok(body), vec![stopped])
    }

    fn inspect(&mut self, command: &str, args: &Value) -> Reply {
        match command {
            "stackTrace" => self.session().map(stack_trace),
            "scopes" => self.session().map(|_| scopes()),
            "variables" => self
                .session()
                .and_then(|s| variables(s, args["variablesReference"].as_u64())),
            "source" => self
                .session()
                .map(|s| json!({"content": s.listing(), "mimeType": "text/plain"})),
            "gotoTargets" => {
                let line = args["line"].as_u64().unwrap_or(1);
                Ok(
                    json!({"targets": [{"id": line, "label": format!("step {}", line.saturating_sub(1)), "line": line}]}),
                )
            }
            "evaluate" => self.evaluate(args["expression"].as_str().unwrap_or_default()),
            other => Err(format!("`{other}` is not supported by the maya2c adapter")),
        }
    }

    fn evaluate(&mut self, expression: &str) -> Reply {
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "no call launched".to_owned())?;
        let result = session
            .command(expression)
            .unwrap_or_else(|| "(quit is `disconnect` here)".into());
        Ok(json!({"result": result, "variablesReference": 0}))
    }
}

fn capabilities() -> Value {
    json!({
        "supportsConfigurationDoneRequest": true,
        "supportsStepBack": true,
        "supportsGotoTargetsRequest": true,
        "supportsEvaluateForHovers": false,
        "supportsTerminateRequest": true,
    })
}

fn spec_from(args: &Value) -> anyhow::Result<CallSpec> {
    let program = args["program"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("launch needs `program`, the contract's .wasm"))?;
    let wasm = std::fs::read(program).map_err(|e| anyhow::anyhow!("reading {program}: {e}"))?;
    Ok(CallSpec {
        wasm,
        input: parse_hex("input", args["input"].as_str().unwrap_or_default())?,
        caller: args["caller"].as_str().map(parse_caller).transpose()?,
        height: args["height"].as_u64().unwrap_or(1),
        gas: args["gas"].as_u64().unwrap_or(DEFAULT_GAS),
    })
}

fn unverified_breakpoints(args: &Value) -> Value {
    let count = args["breakpoints"].as_array().map_or(0, Vec::len);
    let one = json!({"verified": false, "message": "maya2c steps host calls, not source lines: use step, step back or goto"});
    json!({"breakpoints": vec![one; count]})
}

fn stack_trace(session: &Session) -> Value {
    json!({
        "stackFrames": [{
            "id": FRAME,
            "name": session.position(),
            "line": session.cursor() + 1,
            "column": 1,
            "source": {"name": "recorded call", "sourceReference": LISTING},
        }],
        "totalFrames": 1,
    })
}

fn scopes() -> Value {
    let scope = |name: &str, reference: u64| json!({"name": name, "variablesReference": reference, "expensive": false});
    json!({"scopes": [scope("Storage", SCOPE_STORAGE), scope("Events", SCOPE_EVENTS), scope("Call", SCOPE_CALL)]})
}

fn variables(session: &Session, reference: Option<u64>) -> Reply {
    let pairs = match reference {
        Some(SCOPE_STORAGE) => session.storage_pairs(),
        // Numbered: two events may share a topic, and names must differ.
        Some(SCOPE_EVENTS) => session
            .event_pairs()
            .into_iter()
            .enumerate()
            .map(|(i, (topic, data))| (format!("{i} {topic}"), data))
            .collect(),
        Some(SCOPE_CALL) => {
            let (gas, outcome) = session.summary();
            vec![
                (
                    "step".into(),
                    format!("{} of {}", session.cursor(), session.total()),
                ),
                ("outcome".into(), outcome.to_owned()),
                (
                    "gas (whole call)".into(),
                    gas.map_or_else(|| "n/a".into(), |g| g.to_string()),
                ),
            ]
        }
        other => return Err(format!("no variables under reference {other:?}")),
    };
    let variables: Vec<Value> = pairs
        .into_iter()
        .map(|(name, value)| json!({"name": name, "value": value, "variablesReference": 0}))
        .collect();
    Ok(json!({"variables": variables}))
}
