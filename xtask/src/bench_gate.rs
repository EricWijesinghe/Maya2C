//! `cargo xtask bench-gate` — the performance regression gate (Master Prompt 12 §5).
//!
//! Parses the tracked benchmarks' output into metrics, compares each with the
//! last value recorded for the same runner in `benchmarks/history.csv`, and
//! fails when one is more than 5 % worse. `--record` appends the run.
//!
//!     cargo bench -p maya-parallel-exec --bench speedup      > speedup.log
//!     cargo bench -p custom-l1-node --bench apply_pipeline   > apply.log
//!     cargo xtask bench-gate --runner bench-01 speedup.log apply.log
//!     cargo xtask bench-gate --runner bench-01 --record speedup.log apply.log
//!
//! A comparison is only meaningful on one fixed machine, so history rows carry
//! the runner name and a run is compared only against its own runner. With no
//! prior row for that runner the gate records nothing and passes: a first run
//! is a baseline, not a verdict. A PR that regresses on purpose names the
//! metric in `benchmarks/OVERRIDES.md`; the gate reports it but does not fail.
//!
//! Exit codes: 0 pass, 1 regression or I/O failure, 2 usage error or no
//! tracked metric in the logs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use regex::Regex;

use crate::pyfmt;

const THRESHOLD: f64 = 0.05;
const FIELDS: [&str; 6] = ["date", "commit", "runner", "metric", "value", "better"];

/// Metric name, regex over the bench output, and which direction is better.
const PATTERNS: &[(&str, &str, &str)] = &[
    (
        "apply.verify_1t_tx_s",
        r"verify\s+1 thread :.*?([\d.]+) tx/s",
        "higher",
    ),
    (
        "apply.verify_all_cores_tx_s",
        r"verify\s+\d+ threads:.*?([\d.]+) tx/s",
        "higher",
    ),
    (
        "apply.apply_1t_tx_s",
        r"apply\s+1 thread :.*?([\d.]+) tx/s",
        "higher",
    ),
    (
        "apply.apply_allocs_per_tx",
        r"apply\s+1 thread :.*?([\d.]+) allocs/tx",
        "lower",
    ),
    ("apply.peak_rss_mib", r"peak RSS: ([\d.]+) MiB", "lower"),
    (
        "pexec.uniform_w20000_waves_4t_speedup",
        r"uniform w20000\s+4\s+waves\s+\S+\s+\S+\s+([\d.]+)",
        "higher",
    ),
    (
        "pexec.uniform_w2000_optimistic_4t_speedup",
        r"uniform w2000\s+4\s+optimistic\s+\S+\s+\S+\s+([\d.]+)",
        "higher",
    ),
    (
        "pexec.sequential_w2000_tx_s",
        r"uniform w2000\s+1\s+sequential\s+\S+\s+([\d.]+)",
        "higher",
    ),
];

const USAGE: &str = "usage: cargo xtask bench-gate --runner NAME [--record] LOG [LOG ...]";

struct Args {
    runner: String,
    record: bool,
    logs: Vec<PathBuf>,
}

/// `(metric, value, better)`, keyed by metric so output is sorted by name.
type Metrics = BTreeMap<String, (f64, &'static str)>;

pub fn run(args: &[String]) -> ExitCode {
    let Some(parsed) = parse_args(args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match gate(&parsed) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("bench_gate: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Option<Args> {
    let mut runner = None;
    let mut record = false;
    let mut logs = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--runner" => runner = Some(it.next()?.clone()),
            "--record" => record = true,
            flag if flag.starts_with('-') => return None,
            log => logs.push(PathBuf::from(log)),
        }
    }
    if logs.is_empty() {
        return None;
    }
    Some(Args {
        runner: runner?,
        record,
        logs,
    })
}

fn gate(a: &Args) -> Result<ExitCode, String> {
    let root = crate::workspace_root();
    let texts = a
        .logs
        .iter()
        .map(|p| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display())))
        .collect::<Result<Vec<_>, _>>()?;
    let current = parse(&texts)?;
    if current.is_empty() {
        eprintln!("bench_gate: no tracked metric found in the given logs");
        return Ok(ExitCode::from(2));
    }
    let history = root.join("benchmarks").join("history.csv");
    let prior = last_values(&history, &a.runner)?;
    let skip = overridden(&root.join("benchmarks").join("OVERRIDES.md"))?;

    let mut failed = false;
    for (name, (value, better)) in &current {
        let Some(old) = prior.get(name) else {
            println!(
                "  {name:<44} {:>12}   (no baseline for {})",
                pyfmt::fixed(*value, 2),
                a.runner
            );
            continue;
        };
        let worse = regression(*old, *value, better);
        let mut verdict = "ok";
        if worse > THRESHOLD {
            let overridden = skip.contains(name);
            verdict = if overridden {
                "REGRESSED (overridden)"
            } else {
                "REGRESSED"
            };
            failed |= !overridden;
        }
        let pct = -worse * 100.0;
        let sign = if pct.is_sign_negative() { "-" } else { "+" };
        println!(
            "  {name:<44} {:>12}   was {}   {sign}{}%   {verdict}",
            pyfmt::fixed(*value, 2),
            pyfmt::fixed(*old, 2),
            pyfmt::fixed(pct.abs(), 1),
        );
    }
    for (name, _, _) in PATTERNS {
        if !current.contains_key(*name) {
            println!("  {name:<44} NOT MEASURED (pattern absent from logs)");
        }
    }
    if a.record {
        record(&history, &a.runner, &current)?;
        println!(
            "bench_gate: recorded {} metrics for {}",
            current.len(),
            a.runner
        );
    }
    if failed {
        eprintln!("bench_gate: FAIL — a tracked metric regressed more than 5%");
        return Ok(ExitCode::FAILURE);
    }
    println!("bench_gate: pass");
    Ok(ExitCode::SUCCESS)
}

fn parse(texts: &[String]) -> Result<Metrics, String> {
    let joined = texts.join("\n");
    let mut found = Metrics::new();
    for (name, pattern, better) in PATTERNS {
        let rx = Regex::new(pattern).map_err(|e| e.to_string())?;
        if let Some(caps) = rx.captures(&joined) {
            let raw = &caps[1];
            let value: f64 = raw
                .parse()
                .map_err(|_| format!("{name}: `{raw}` is not a number"))?;
            found.insert((*name).to_string(), (value, *better));
        }
    }
    Ok(found)
}

fn last_values(path: &Path, runner: &str) -> Result<HashMap<String, f64>, String> {
    let mut out = HashMap::new();
    if !path.exists() {
        return Ok(out);
    }
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let records = parse_csv(&text);
    let Some((header, rows)) = records.split_first() else {
        return Ok(out);
    };
    let column = |name: &str| {
        header
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| format!("{}: no `{name}` column", path.display()))
    };
    let (runner_col, metric_col, value_col) =
        (column("runner")?, column("metric")?, column("value")?);
    for row in rows {
        let (Some(r), Some(m), Some(v)) =
            (row.get(runner_col), row.get(metric_col), row.get(value_col))
        else {
            continue;
        };
        if r == runner {
            let value: f64 = v
                .parse()
                .map_err(|_| format!("{}: bad value `{v}`", path.display()))?;
            out.insert(m.clone(), value);
        }
    }
    Ok(out)
}

fn overridden(path: &Path) -> Result<BTreeSet<String>, String> {
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rx = Regex::new(r"`([a-z0-9_.]+)`").map_err(|e| e.to_string())?;
    Ok(rx.captures_iter(&text).map(|c| c[1].to_string()).collect())
}

/// Fractional change in the bad direction (positive = worse).
fn regression(old: f64, new: f64, better: &str) -> f64 {
    if old == 0.0 {
        return 0.0;
    }
    if better == "higher" {
        (old - new) / old
    } else {
        (new - old) / old
    }
}

fn record(path: &Path, runner: &str, current: &Metrics) -> Result<(), String> {
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(crate::workspace_root())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let new_file = !path.exists();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut out = String::new();
    if new_file {
        out.push_str(&pyfmt::csv_row(&FIELDS));
    }
    for (name, (value, better)) in current {
        let v = pyfmt::repr(*value);
        let fields = [
            today.as_str(),
            commit.as_str(),
            runner,
            name.as_str(),
            v.as_str(),
            *better,
        ];
        out.push_str(&pyfmt::csv_row(&fields));
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(out.as_bytes()).map_err(|e| e.to_string())
}

/// `csv.reader` over the files this crate writes: quoted fields, `\r\n` or
/// `\n` line ends, blank lines skipped as `DictReader` skips them.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => record.push(std::mem::take(&mut field)),
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                let blank = record.len() == 1 && record[0].is_empty();
                if !blank {
                    records.push(std::mem::take(&mut record));
                }
                record.clear();
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{parse, parse_csv, regression};

    #[test]
    fn regression_is_positive_in_the_bad_direction() {
        assert!((regression(100.0, 90.0, "higher") - 0.1).abs() < 1e-12);
        assert!((regression(100.0, 110.0, "lower") - 0.1).abs() < 1e-12);
        assert!(regression(0.0, 5.0, "higher").abs() < 1e-12);
    }

    #[test]
    fn parse_finds_the_first_match_per_metric() {
        let log = "apply  1 thread : 12.5 tx/s 3.0 allocs/tx\npeak RSS: 64.0 MiB\n".to_string();
        let found = parse(&[log]).expect("patterns compile");
        assert!((found["apply.apply_1t_tx_s"].0 - 12.5).abs() < 1e-9);
        assert!((found["apply.peak_rss_mib"].0 - 64.0).abs() < 1e-9);
        assert!(!found.contains_key("pexec.sequential_w2000_tx_s"));
    }

    #[test]
    fn csv_reader_round_trips_quotes_and_crlf() {
        let rows = parse_csv("a,\"b,c\"\r\n\r\n1,\"x\"\"y\"\r\n");
        let got: Vec<Vec<&str>> = rows
            .iter()
            .map(|r| r.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(got, vec![vec!["a", "b,c"], vec!["1", "x\"y"]]);
    }
}
