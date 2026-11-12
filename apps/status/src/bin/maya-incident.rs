//! `maya-incident`: the operator's tool for the incident log.
//!
//! ```text
//! maya-incident keygen --out KEYFILE
//! maya-incident publish --dir DIR --key KEYFILE --id ID --title TEXT --body TEXT
//!     [--severity warning|critical] [--report https://...] [--started SECS] [--ended SECS]
//! maya-incident verify --dir DIR [--operators FILE]
//! ```
//!
//! `publish` appends a signed record. A new incident needs `--started`; an
//! existing one (a drafted halt, say) takes its times from the log. The
//! operator's public key must already be in `DIR/operators.txt`, which
//! `keygen` prints. `verify` checks every line and prints the head hash:
//! record that somewhere off the machine to notice a truncated tail.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use maya_status::incident_store::{self as store, Entry, Kind, Severity};
use maya_status::incidents;
use maya_status::operator::{self, Operator};

const USAGE: &str = "usage:
  maya-incident keygen --out KEYFILE
  maya-incident publish --dir DIR --key KEYFILE --id ID --title TEXT --body TEXT
      [--severity warning|critical] [--report https://...] [--started SECS] [--ended SECS]
  maya-incident verify --dir DIR [--operators FILE]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("maya-incident: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--flag value` pairs after the subcommand. A flag without a value, or a
/// bare word, is refused.
fn flags(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let name = flag
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument {flag}\n{USAGE}"))?;
        let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
        out.insert(name.to_owned(), value.clone());
    }
    Ok(out)
}

fn required(f: &BTreeMap<String, String>, name: &str) -> Result<String, String> {
    f.get(name)
        .cloned()
        .ok_or_else(|| format!("--{name} is required\n{USAGE}"))
}

fn run(args: &[String]) -> Result<String, String> {
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    let f = flags(rest)?;
    match cmd.as_str() {
        "keygen" => keygen(&PathBuf::from(required(&f, "out")?)),
        "publish" => publish(&f),
        "verify" => verify(&PathBuf::from(required(&f, "dir")?), &operators_path(&f)?),
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

fn operators_path(f: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    match f.get("operators") {
        Some(p) => Ok(PathBuf::from(p)),
        None => Ok(PathBuf::from(required(f, "dir")?).join("operators.txt")),
    }
}

/// Writes a new seed file (never overwrites one) and prints the public key.
fn keygen(out: &Path) -> Result<String, String> {
    let (seed, op) = Operator::generate()?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    writeln!(file, "{}", hex::encode(*seed)).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(format!(
        "key written to {} (keep it off the repository)\npublic key, add to operators.txt:\n{}",
        out.display(),
        hex::encode(op.public_key())
    ))
}

fn publish(f: &BTreeMap<String, String>) -> Result<String, String> {
    let dir = PathBuf::from(required(f, "dir")?);
    let trusted = operator::read_operators(&operators_path(f)?)?;
    let seed_text =
        std::fs::read_to_string(required(f, "key")?).map_err(|e| format!("key: {e}"))?;
    let seed = operator::parse_seed(&seed_text)?;
    let op = Operator::from_seed(*seed);
    let id = required(f, "id")?;
    let existing = incidents::fold(&store::load(&dir, &trusted).map_err(|e| e.to_string())?);
    let prior = existing.iter().find(|i| i.id == id);
    let started = match (f.get("started"), prior) {
        (Some(s), _) => s
            .parse()
            .map_err(|_| "--started must be Unix seconds".to_owned())?,
        (None, Some(p)) => p.started,
        (None, None) => return Err("--started is required for a new incident".into()),
    };
    let ended = match f.get("ended") {
        Some(e) => Some(
            e.parse()
                .map_err(|_| "--ended must be Unix seconds".to_owned())?,
        ),
        None => prior.and_then(|p| p.ended),
    };
    let severity = match f.get("severity").map(String::as_str) {
        None | Some("critical") => Severity::Critical,
        Some("warning") => Severity::Warning,
        Some(_) => return Err("--severity is warning or critical".into()),
    };
    let entry = Entry {
        kind: Kind::Signed,
        id: id.clone(),
        recorded_at: now(),
        started,
        ended,
        severity,
        title: required(f, "title")?,
        body: required(f, "body")?,
        report: f.get("report").cloned(),
    };
    let record = store::append(&dir, &trusted, &entry, Some(&op)).map_err(|e| e.to_string())?;
    Ok(format!(
        "published {id} as record {} (hash {})",
        record.body.seq, record.hash
    ))
}

fn verify(dir: &Path, operators: &Path) -> Result<String, String> {
    let trusted = operator::read_operators(operators)?;
    let records = store::load(dir, &trusted).map_err(|e| e.to_string())?;
    Ok(format!(
        "verified {} records, head {}",
        records.len(),
        store::head(&records)
    ))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn flags_refuse_a_bare_word_or_a_missing_value() {
        assert!(flags(&args(&["stray"])).is_err());
        assert!(flags(&args(&["--dir"])).is_err());
        assert_eq!(flags(&args(&["--dir", "x"])).unwrap()["dir"], "x");
    }

    #[test]
    fn publish_then_verify_round_trips_through_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("op.key");
        let out = run(&args(&["keygen", "--out", key.to_str().unwrap()])).unwrap();
        let public = out.lines().last().unwrap().to_owned();
        std::fs::write(dir.path().join("operators.txt"), format!("{public}\n")).unwrap();
        let d = dir.path().to_str().unwrap();
        let k = key.to_str().unwrap();

        let published = run(&args(&[
            "publish",
            "--dir",
            d,
            "--key",
            k,
            "--id",
            "manual-1",
            "--started",
            "100",
            "--title",
            "Test incident",
            "--body",
            "A rehearsal.",
        ]))
        .unwrap();
        assert!(
            published.starts_with("published manual-1 as record 0"),
            "{published}"
        );
        let verified = run(&args(&["verify", "--dir", d])).unwrap();
        assert!(verified.starts_with("verified 1 records"), "{verified}");
    }

    #[test]
    fn keygen_never_overwrites_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("op.key");
        let k = key.to_str().unwrap();
        run(&args(&["keygen", "--out", k])).unwrap();
        assert!(run(&args(&["keygen", "--out", k])).is_err());
    }

    #[test]
    fn a_new_incident_without_a_start_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("op.key");
        run(&args(&["keygen", "--out", key.to_str().unwrap()])).unwrap();
        let d = dir.path().to_str().unwrap();
        let err = run(&args(&[
            "publish",
            "--dir",
            d,
            "--key",
            key.to_str().unwrap(),
            "--id",
            "x",
            "--title",
            "t",
            "--body",
            "b",
        ]))
        .unwrap_err();
        assert!(err.contains("--started"), "{err}");
    }
}
