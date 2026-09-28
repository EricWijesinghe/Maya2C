//! `maya2c debug` — the time-travel debugger over a recorded call.
//!
//! The call runs once, traced ([`maya_vm::trace`]); stepping forward and
//! backward then moves a cursor over the recording and shows storage and
//! events as they stood at that step. Commands:
//!
//! | command | effect |
//! |---|---|
//! | `n` / `next` | one step forward |
//! | `b` / `back` | one step backward |
//! | `g <k>` / `goto <k>` | jump to step `k` (0 = before the call) |
//! | `s` / `storage` | storage of every touched key at the cursor |
//! | `e` / `events` | events emitted up to the cursor |
//! | `l` / `list` | every step, the cursor marked |
//! | `p` / `profile` | gas per source line (contracts built with `-g`) |
//! | `q` / `quit` | leave |

use std::collections::BTreeMap;
use std::fmt::Write as _;

use maya_vm::trace::{Step, Timeline};

/// A cursor over a recorded call.
pub struct Session {
    timeline: Timeline,
    /// Each step's source position, when the contract carried DWARF.
    lines: Vec<Option<crate::lines::Line>>,
    /// Gas the guest spent before each step's host call, since the previous
    /// one.
    step_gas: Vec<Option<u64>>,
    cursor: usize,
    /// Gas for the whole call; per-step gas is not recorded.
    gas_used: Option<u64>,
    /// How the call ended.
    outcome: String,
}

fn short(bytes: &[u8]) -> String {
    const SHOWN: usize = 16;
    // Contracts key storage by names more often than not: `admin` reads
    // better than `61646d696e`.
    if !bytes.is_empty() && bytes.iter().all(u8::is_ascii_graphic) {
        return format!("\"{}\"", String::from_utf8_lossy(bytes));
    }
    let text = hex::encode(&bytes[..bytes.len().min(SHOWN)]);
    if bytes.len() > SHOWN {
        format!("{text}…({} bytes)", bytes.len())
    } else {
        text
    }
}

fn value(v: Option<&Vec<u8>>) -> String {
    v.map_or_else(|| "absent".to_string(), |b| short(b))
}

/// One step, as the debugger prints it.
#[must_use]
pub fn describe(step: &Step) -> String {
    match step {
        Step::Read { key, value: v, .. } => format!("read  {} = {}", short(key), value(v.as_ref())),
        Step::Write {
            key, before, after, ..
        } => {
            format!(
                "write {} : {} -> {}",
                short(key),
                value(before.as_ref()),
                short(after)
            )
        }
        Step::Event(e) => format!("event {} {}", short(&e.topic), short(&e.data)),
        Step::Balance { address, value } => format!("balance_of {} = {value}", short(address)),
        Step::Caller(c) => format!(
            "caller = {}",
            c.map_or_else(|| "none".into(), |a| short(&a))
        ),
        Step::Height(h) => format!("block_height = {h}"),
        Step::Randomness(r) => format!(
            "block_randomness = {}",
            r.map_or_else(|| "none".into(), |r| short(&r))
        ),
        Step::Oracle { feed, value } => format!("oracle {} = {value:?}", short(feed)),
        Step::Zkml(v) => format!("verify_zkml = {v:?}"),
    }
}

impl Session {
    /// A session over `timeline`, cursor before the first step.
    #[must_use]
    pub fn new(timeline: Timeline, gas_used: Option<u64>, outcome: String) -> Self {
        Self {
            timeline,
            lines: Vec::new(),
            step_gas: Vec::new(),
            cursor: 0,
            gas_used,
            outcome,
        }
    }

    /// Attaches each step's source position (see [`crate::lines`]).
    #[must_use]
    pub fn with_lines(mut self, lines: Vec<Option<crate::lines::Line>>) -> Self {
        self.lines = lines;
        self
    }

    /// Attaches the gas spent before each step (see [`crate::record`]).
    #[must_use]
    pub fn with_step_gas(mut self, step_gas: Vec<Option<u64>>) -> Self {
        self.step_gas = step_gas;
        self
    }

    /// Gas per source line: the gas spent leading up to each host call,
    /// summed by the line that made the call. Gas after the last host call
    /// belongs to no step and is not counted.
    #[must_use]
    pub fn gas_by_line(&self) -> BTreeMap<(String, u64), u64> {
        let mut out = BTreeMap::new();
        for (line, gas) in self.lines.iter().zip(&self.step_gas) {
            if let (Some(line), Some(gas)) = (line, gas) {
                *out.entry((line.file_name().to_owned(), line.line))
                    .or_insert(0) += gas;
            }
        }
        out
    }

    fn profile(&self) -> String {
        let by_line = self.gas_by_line();
        if by_line.is_empty() {
            return "no source lines: build the contract with debug info (-g)".into();
        }
        by_line
            .iter()
            .fold(String::new(), |mut out, ((file, line), gas)| {
                let _ = writeln!(out, "  {file}:{line}  {gas} gas");
                out
            })
    }

    /// The source position of the step the cursor just passed, if known.
    #[must_use]
    pub fn source(&self) -> Option<&crate::lines::Line> {
        self.cursor
            .checked_sub(1)
            .and_then(|i| self.lines.get(i))
            .and_then(Option::as_ref)
    }

    fn at(&self, index: usize) -> String {
        self.lines
            .get(index)
            .and_then(Option::as_ref)
            .map_or_else(String::new, |l| format!("  ({}:{})", l.file_name(), l.line))
    }

    /// Steps passed so far (0 = before the call).
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Steps in the recording.
    #[must_use]
    pub fn total(&self) -> usize {
        self.timeline.steps().len()
    }

    /// Moves the cursor to `k`, clamped to the recording.
    pub fn goto(&mut self, k: usize) {
        self.cursor = k.min(self.total());
    }

    /// Storage of every touched key at the cursor, as `(key, value)` text.
    #[must_use]
    pub fn storage_pairs(&self) -> Vec<(String, String)> {
        self.timeline
            .storage_after(self.cursor)
            .iter()
            .map(|((_, key), v)| (short(key), value(v.as_ref())))
            .collect()
    }

    /// Events emitted up to the cursor, as `(topic, data)` text.
    #[must_use]
    pub fn event_pairs(&self) -> Vec<(String, String)> {
        self.timeline
            .events_after(self.cursor)
            .iter()
            .map(|e| (short(&e.topic), short(&e.data)))
            .collect()
    }

    /// Gas for the whole call and how it ended.
    #[must_use]
    pub fn summary(&self) -> (Option<u64>, &str) {
        (self.gas_used, &self.outcome)
    }

    /// The recording as text, one line per step after a first line for
    /// "before the call": line `k + 1` is the state after step `k`.
    #[must_use]
    pub fn listing(&self) -> String {
        std::iter::once("before the call".to_owned())
            .chain(self.timeline.steps().iter().map(describe))
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    }

    /// Where the cursor is, and the step it just passed.
    #[must_use]
    pub fn position(&self) -> String {
        let total = self.timeline.steps().len();
        match self
            .cursor
            .checked_sub(1)
            .and_then(|i| self.timeline.steps().get(i))
        {
            Some(step) => format!(
                "[{}/{total}] {}{}",
                self.cursor,
                describe(step),
                self.at(self.cursor - 1)
            ),
            None => format!("[0/{total}] before the call"),
        }
    }

    /// Runs one command; `None` means quit.
    pub fn command(&mut self, line: &str) -> Option<String> {
        let mut words = line.split_whitespace();
        let total = self.timeline.steps().len();
        let out = match words.next().unwrap_or("") {
            "n" | "next" => {
                self.cursor = (self.cursor + 1).min(total);
                self.position()
            }
            "b" | "back" => {
                self.cursor = self.cursor.saturating_sub(1);
                self.position()
            }
            "g" | "goto" => match words.next().and_then(|w| w.parse::<usize>().ok()) {
                Some(k) if k <= total => {
                    self.cursor = k;
                    self.position()
                }
                _ => format!("goto needs a step from 0 to {total}"),
            },
            "s" | "storage" => self.storage(),
            "e" | "events" => self.events(),
            "l" | "list" => self.list(),
            "p" | "profile" => self.profile(),
            "q" | "quit" => return None,
            "" => String::new(),
            other => format!("unknown command `{other}` (n, b, g <k>, s, e, l, p, q)"),
        };
        Some(out)
    }

    fn storage(&self) -> String {
        let view = self.timeline.storage_after(self.cursor);
        if view.is_empty() {
            return "no storage touched".into();
        }
        view.iter().fold(String::new(), |mut out, ((_, key), v)| {
            let _ = writeln!(out, "  {} = {}", short(key), value(v.as_ref()));
            out
        })
    }

    fn events(&self) -> String {
        let events = self.timeline.events_after(self.cursor);
        if events.is_empty() {
            return "no events yet".into();
        }
        events.iter().fold(String::new(), |mut out, e| {
            let _ = writeln!(out, "  {} {}", short(&e.topic), short(&e.data));
            out
        })
    }

    fn list(&self) -> String {
        let mut out = format!(
            "{} steps; {}; gas {} (whole call)\n",
            self.timeline.steps().len(),
            self.outcome,
            self.gas_used
                .map_or_else(|| "n/a".into(), |g| g.to_string())
        );
        for (i, step) in self.timeline.steps().iter().enumerate() {
            let mark = if i + 1 == self.cursor { ">" } else { " " };
            let _ = writeln!(out, "{mark} {:>3} {}{}", i + 1, describe(step), self.at(i));
        }
        out
    }
}
