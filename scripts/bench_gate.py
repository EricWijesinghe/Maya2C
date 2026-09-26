#!/usr/bin/env python3
"""Performance regression gate (Master Prompt 12 §5).

Parses the output of the tracked benchmarks into metrics, compares each to the
last value recorded for the same runner in benchmarks/history.csv, and fails
when any is more than 5 % worse. `--record` appends the run to the history.

    cargo bench -p maya-parallel-exec --bench speedup      > speedup.log
    cargo bench -p custom-l1-node --bench apply_pipeline   > apply.log
    python3 scripts/bench_gate.py --runner bench-01 speedup.log apply.log
    python3 scripts/bench_gate.py --runner bench-01 --record speedup.log apply.log

A comparison is only meaningful on one fixed machine, so history rows carry the
runner name and a run is compared only against its own runner. With no prior
row for that runner the gate records nothing and passes: a first run is a
baseline, not a verdict.

Override: a PR that regresses on purpose adds a line to benchmarks/OVERRIDES.md
naming the metric and the reason; the gate then reports that metric but does
not fail on it.
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HISTORY = ROOT / "benchmarks" / "history.csv"
OVERRIDES = ROOT / "benchmarks" / "OVERRIDES.md"
THRESHOLD = 0.05
FIELDS = ["date", "commit", "runner", "metric", "value", "better"]

# metric name, regex over the bench output, which direction is better
PATTERNS = [
    ("apply.verify_1t_tx_s", r"verify\s+1 thread :.*?([\d.]+) tx/s", "higher"),
    ("apply.verify_all_cores_tx_s", r"verify\s+\d+ threads:.*?([\d.]+) tx/s", "higher"),
    ("apply.apply_1t_tx_s", r"apply\s+1 thread :.*?([\d.]+) tx/s", "higher"),
    ("apply.apply_allocs_per_tx", r"apply\s+1 thread :.*?([\d.]+) allocs/tx", "lower"),
    ("apply.peak_rss_mib", r"peak RSS: ([\d.]+) MiB", "lower"),
    ("pexec.uniform_w20000_waves_4t_speedup", r"uniform w20000\s+4\s+waves\s+\S+\s+\S+\s+([\d.]+)", "higher"),
    ("pexec.uniform_w2000_optimistic_4t_speedup", r"uniform w2000\s+4\s+optimistic\s+\S+\s+\S+\s+([\d.]+)", "higher"),
    ("pexec.sequential_w2000_tx_s", r"uniform w2000\s+1\s+sequential\s+\S+\s+([\d.]+)", "higher"),
]


def parse(texts: list[str]) -> dict[str, tuple[float, str]]:
    joined = "\n".join(texts)
    found = {}
    for name, pattern, better in PATTERNS:
        m = re.search(pattern, joined)
        if m:
            found[name] = (float(m.group(1)), better)
    return found


def last_values(runner: str) -> dict[str, float]:
    if not HISTORY.exists():
        return {}
    out = {}
    with HISTORY.open(newline="") as f:
        for row in csv.DictReader(f):
            if row["runner"] == runner:
                out[row["metric"]] = float(row["value"])
    return out


def overridden() -> set[str]:
    if not OVERRIDES.exists():
        return set()
    return {m.group(1) for m in re.finditer(r"`([a-z0-9_.]+)`", OVERRIDES.read_text())}


def regression(old: float, new: float, better: str) -> float:
    """Fractional change in the bad direction (positive = worse)."""
    if old == 0:
        return 0.0
    return (old - new) / old if better == "higher" else (new - old) / old


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runner", required=True)
    ap.add_argument("--record", action="store_true")
    ap.add_argument("logs", nargs="+", type=Path)
    args = ap.parse_args()
    current = parse([p.read_text() for p in args.logs])
    if not current:
        print("bench_gate: no tracked metric found in the given logs", file=sys.stderr)
        return 2
    missing = [name for name, _, _ in PATTERNS if name not in current]
    prior, skip = last_values(args.runner), overridden()
    failed = False
    for name, (value, better) in sorted(current.items()):
        if name not in prior:
            print(f"  {name:<44} {value:>12.2f}   (no baseline for {args.runner})")
            continue
        worse = regression(prior[name], value, better)
        verdict = "ok"
        if worse > THRESHOLD:
            verdict = "REGRESSED (overridden)" if name in skip else "REGRESSED"
            failed |= name not in skip
        print(f"  {name:<44} {value:>12.2f}   was {prior[name]:.2f}   {-worse:+.1%}   {verdict}")
    for name in missing:
        print(f"  {name:<44} NOT MEASURED (pattern absent from logs)")
    if args.record:
        commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
        today = dt.date.today().isoformat()
        new_file = not HISTORY.exists()
        HISTORY.parent.mkdir(exist_ok=True)
        with HISTORY.open("a", newline="") as f:
            w = csv.DictWriter(f, fieldnames=FIELDS)
            if new_file:
                w.writeheader()
            for name, (value, better) in sorted(current.items()):
                w.writerow({"date": today, "commit": commit, "runner": args.runner, "metric": name, "value": value, "better": better})
        print(f"bench_gate: recorded {len(current)} metrics for {args.runner}")
    if failed:
        print(f"bench_gate: FAIL — a tracked metric regressed more than {THRESHOLD:.0%}", file=sys.stderr)
        return 1
    print("bench_gate: pass")
    return 0


if __name__ == "__main__":
    sys.exit(main())
