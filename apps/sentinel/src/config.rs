//! Command line. Flags only: the ntfy topic is read from the environment, so
//! it never lands in a shell history or a process listing.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::detect::Thresholds;

/// Default poll interval. Halts are seen within the halt window plus one poll.
pub const DEFAULT_POLL_MS: u64 = 5_000;
/// Default log file, in the working directory.
pub const DEFAULT_LOG: &str = "maya2c-sentinel.log";
/// Most pushes in one window before the rest are logged only.
pub const DEFAULT_RATE_MAX: usize = 6;
/// Length of the push rate-limit window.
pub const DEFAULT_RATE_WINDOW_MS: u64 = 600_000;

/// A resolved run configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `(name, url)` for every node to watch.
    pub nodes: Vec<(String, String)>,
    /// Wall time between polls.
    pub poll_ms: u64,
    /// Detector limits.
    pub thresholds: Thresholds,
    /// Where each alert line is appended.
    pub log: PathBuf,
    /// Print pushes instead of sending them.
    pub dry_run: bool,
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// `--help`: print usage and exit 0.
    Help,
    /// Run the watchdog.
    Run(Config),
}

/// Usage text, printed on `--help` and after a bad flag.
pub const USAGE: &str = "usage: maya2c-sentinel --node NAME=URL [--node NAME=URL]... \
[--poll-secs 5] [--halt-secs 30] [--unreachable-secs 45] [--min-peers 2] \
[--strand-blocks 20] [--strand-secs 120] [--log PATH] [--dry-run]\n\
env MAYA2C_SENTINEL_NTFY_TOPIC: the ntfy topic (required unless --dry-run)";

/// Parses the arguments after the program name.
///
/// # Errors
///
/// Returns a message for an unknown flag, a missing value, a malformed
/// `NAME=URL`, a duplicate name, a non-HTTP URL, or no `--node` at all.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut nodes: Vec<(String, String)> = Vec::new();
    let mut poll_ms = DEFAULT_POLL_MS;
    let mut t = Thresholds::default();
    let mut log = PathBuf::from(DEFAULT_LOG);
    let mut dry_run = false;

    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        if flag == "--help" || flag == "-h" {
            return Ok(Command::Help);
        }
        if flag == "--dry-run" {
            dry_run = true;
            continue;
        }
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--node" => nodes.push(node(&value)?),
            "--poll-secs" => poll_ms = secs(&flag, &value)?,
            "--halt-secs" => t.halt_ms = secs(&flag, &value)?,
            "--unreachable-secs" => t.unreachable_ms = secs(&flag, &value)?,
            "--min-peers" => t.min_peers = number(&flag, &value)?,
            "--strand-blocks" => t.strand_blocks = number(&flag, &value)?,
            "--strand-secs" => t.strand_ms = secs(&flag, &value)?,
            "--log" => log = PathBuf::from(value),
            _ => return Err(format!("unknown flag {flag}")),
        }
    }

    if nodes.is_empty() {
        return Err("at least one --node NAME=URL is required".into());
    }
    let mut names = HashSet::new();
    if let Some((dup, _)) = nodes.iter().find(|(n, _)| !names.insert(n.as_str())) {
        return Err(format!("node name {dup} given twice"));
    }
    Ok(Command::Run(Config {
        nodes,
        poll_ms,
        thresholds: t,
        log,
        dry_run,
    }))
}

fn node(value: &str) -> Result<(String, String), String> {
    let (name, url) = value
        .split_once('=')
        .ok_or_else(|| format!("--node expects NAME=URL, got {value}"))?;
    if name.is_empty() {
        return Err("--node name is empty".into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "--node {name}: URL must start with http:// or https://"
        ));
    }
    Ok((name.to_owned(), url.to_owned()))
}

/// Seconds on the command line, as milliseconds. Zero is refused: a zero
/// window would alert on every poll.
fn secs(flag: &str, value: &str) -> Result<u64, String> {
    let s: u64 = value
        .parse()
        .map_err(|_| format!("{flag} expects whole seconds, got {value}"))?;
    if s == 0 {
        return Err(format!("{flag} must be at least 1"));
    }
    Ok(s.saturating_mul(1_000))
}

fn number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects a whole number, got {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn defaults_match_the_documented_thresholds() {
        let Ok(Command::Run(c)) = parse_args(args(&["--node", "seed=http://127.0.0.1:8545"]))
        else {
            panic!("expected a run config");
        };
        assert_eq!(c.poll_ms, 5_000);
        assert_eq!(c.thresholds.halt_ms, 30_000);
        assert_eq!(c.thresholds.min_peers, 2);
        assert!(!c.dry_run);
        assert_eq!(c.log, PathBuf::from(DEFAULT_LOG));
    }

    #[test]
    fn rejects_bad_input_with_a_reason() {
        assert!(parse_args(args(&[])).is_err(), "no node");
        assert!(parse_args(args(&["--node", "seed"])).is_err(), "no =");
        assert!(
            parse_args(args(&["--node", "seed=ftp://x"])).is_err(),
            "scheme"
        );
        assert!(
            parse_args(args(&["--node", "a=http://x", "--node", "a=http://y"])).is_err(),
            "dup"
        );
        assert!(
            parse_args(args(&["--node", "a=http://x", "--halt-secs", "0"])).is_err(),
            "zero"
        );
        assert!(
            parse_args(args(&["--node", "a=http://x", "--bogus", "1"])).is_err(),
            "flag"
        );
    }

    #[test]
    fn help_and_dry_run_parse() {
        assert_eq!(parse_args(args(&["--help"])), Ok(Command::Help));
        let Ok(Command::Run(c)) = parse_args(args(&["--node", "a=https://x", "--dry-run"])) else {
            panic!("expected a run config");
        };
        assert!(c.dry_run);
    }
}
