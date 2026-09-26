#!/usr/bin/env python3
"""Re-derive every features.toml status from a test log (Master Prompt 11 §1).

An entry keeps `working`/`verified` only if every test it names ran in the
given log and its binary reported `0 failed`. A test that did not run at all
(not in the log) counts as evidence missing, not as a pass. Entries backed by
a `gate` (a command, not a test) are reported as NEEDS-GATE and keep their
status, because a test log cannot say anything about them.

Usage: python3 scripts/reality_audit.py <cargo-test-log> [--markdown]
"""

import re
import sys
import tomllib
from pathlib import Path


def ran_binaries(log: str) -> dict[str, tuple[int, int]]:
    """Maps a test source path or crate name to (passed, failed)."""
    results: dict[str, tuple[int, int]] = {}
    current = None
    for line in log.splitlines():
        m = re.search(r"Running (.+?) \(target/[^/]+/deps/([a-z0-9_]+)-[0-9a-f]+\)", line)
        if m:
            current = (m.group(1), m.group(2))
            continue
        m = re.match(r"test result: \w+\. (\d+) passed; (\d+) failed", line)
        if m and current:
            src, dep = current
            p, f = int(m.group(1)), int(m.group(2))
            for key in (src, dep):
                a, b = results.get(key, (0, 0))
                results[key] = (a + p, b + f)
            current = None
    return results


def check(test: str, runs: dict[str, tuple[int, int]]) -> str:
    if test.startswith("crate:"):
        crate = test[len("crate:"):].replace("-", "_")
        hits = [v for k, v in runs.items() if k == crate]
    else:
        # `crates/x/tests/foo.rs` appears in the log as `tests/foo.rs`, and
        # several crates can have a `tests/foo.rs`; match on the stem through
        # the dep name, which is unique per binary.
        stem = Path(test).stem
        hits = [v for k, v in runs.items() if k == stem]
    if not hits:
        return "not-run"
    return "fail" if any(f > 0 for _, f in hits) else "pass"


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    runs = ran_binaries(Path(sys.argv[1]).read_text(errors="replace"))
    ledger = tomllib.loads(Path("features.toml").read_text())
    rows, changes = [], 0
    for table in ("subsystem", "feature"):
        for e in ledger.get(table, []):
            before = e["status"]
            if before not in ("working", "verified"):
                continue
            tests = e.get("tests", [])
            if not tests:
                rows.append((e["id"], before, before, "NEEDS-GATE: " + e.get("gate", "")[:60]))
                continue
            verdicts = {t: check(t, runs) for t in tests}
            if all(v == "pass" for v in verdicts.values()):
                after, why = before, "all named tests ran and passed"
            elif any(v == "fail" for v in verdicts.values()):
                after, why = "stub", "a named test FAILED: " + ", ".join(t for t, v in verdicts.items() if v == "fail")
            else:
                after, why = "stub", "not in this run: " + ", ".join(t for t, v in verdicts.items() if v == "not-run")
            if after != before:
                changes += 1
            rows.append((e["id"], before, after, why))
    md = "--markdown" in sys.argv
    if md:
        print("| id | before | after | evidence |")
        print("|---|---|---|---|")
    for r in rows:
        if md:
            print(f"| `{r[0]}` | {r[1]} | {'**' + r[2] + '**' if r[1] != r[2] else r[2]} | {r[3]} |")
        elif r[1] != r[2] or "NEEDS" in r[3]:
            print(f"{r[0]:40} {r[1]:9} -> {r[2]:9} {r[3]}")
    print(f"\n{len(rows)} claims examined, {changes} would change status on this log's evidence.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
