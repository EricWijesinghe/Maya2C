#!/usr/bin/env python3
"""Generate reports/11-gap-register.md (Master Prompt 11 §1).

Lists every TODO / FIXME, `todo!()`, `unimplemented!()`, `panic!()` outside
test code, `#[ignore]`d test, and mock/stub/fake on a production path, with
file:line, owning crate and a severity assigned by RULE, not by judgement:

  P0 (blocks mainnet)  in a mainnet-core crate (ADR-016) and not test code
  P1 (blocks testnet)  in a service or client crate a testnet runs
  P2 (later)           everything else, and every #[ignore]

A `panic!` counts only outside `#[cfg(test)]` modules and `tests/`, `benches/`,
`examples/` directories. Test-module detection is lexical: everything after
the first `#[cfg(test)]` in a file is treated as test code, which is how this
tree lays out its unit tests. That is a heuristic and the report says so.
A module declared under `#[cfg(kani)]` (the Kani proofs) is verification code,
compiled into no binary, and is treated the same way.

A row whose line, or one of the three lines above it, carries a
`reviewed-guard:` comment is listed as "guard" instead of P0/P1: a reviewer
has written down why reaching it is a bug worth a panic (DECISIONS.md,
2026-09-28). It stays in the register so the call can be disagreed with.

Run from the repo root: python3 scripts/gap_register.py > reports/11-gap-register.md
"""

import re
import subprocess
import sys
from pathlib import Path

CORE = {
    "crates/node", "crates/crypto-pq", "crates/vm", "crates/fee-market", "crates/governance",
    "crates/ledger-math", "crates/zk-stark", "crates/dag-bft", "bins/maya2c-node",
}
TESTNET = {
    "crates/api-gateway", "apps/explorer", "apps/faucet", "crates/telemetry", "bins/l1-wallet",
    "apps/wallet-gui/core", "crates/light-client", "crates/custody-mpc", "bins/maya2c-miner",
}
PATTERNS = [
    ("TODO/FIXME", re.compile(r"//.*\b(TODO|FIXME|XXX)\b")),
    ("todo!()", re.compile(r"\btodo!\s*\(")),
    ("unimplemented!()", re.compile(r"\bunimplemented!\s*\(")),
    ("panic!()", re.compile(r"\bpanic!\s*\(")),
    ("#[ignore]", re.compile(r"#\[ignore")),
    ("mock/stub on a non-test path", re.compile(r"\b(Mock|mock_|Stub|stub_|Fake|fake_)[A-Za-z_]*\b")),
]


GUARD_MARK = "reviewed-guard:"
GUARD_WINDOW = 3


def kani_modules(files: list[str]) -> set[str]:
    """Files declared as `#[cfg(kani)] mod x;` (proof code, in no binary)."""
    found = set()
    for f in files:
        if not f.endswith("/lib.rs"):
            continue
        lines = Path(f).read_text(errors="replace").splitlines()
        for i, line in enumerate(lines[:-1]):
            if "#[cfg(kani)]" not in line:
                continue
            m = re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", lines[i + 1])
            if m:
                found.add(f"{f[:-len('lib.rs')]}{m.group(1)}.rs")
    return found


def owner(path: str, members: list[str]) -> str:
    best = ""
    for m in members:
        if path.startswith(m + "/") and len(m) > len(best):
            best = m
    return best or "(outside the workspace)"


def severity(kind: str, crate: str, in_test: bool) -> str:
    if kind == "#[ignore]" or in_test:
        return "P2"
    if crate in CORE:
        return "P0"
    if crate in TESTNET:
        return "P1"
    return "P2"


def main() -> int:
    files = subprocess.run(["git", "ls-files", "*.rs"], capture_output=True, text=True, check=True).stdout.split()
    members = []
    for line in Path("Cargo.toml").read_text().splitlines():
        m = re.match(r'\s*"([^"]+)",', line)
        if m and "/" in m.group(1) or (m and m.group(1) in ("sim", "econ", "xtask")):
            members.append(m.group(1))
    skip_dirs = ("fuzz/", "offsec-sandbox/", "apps/wallet-gui/ui/target", "target", "attic/")
    proofs = kani_modules(files)
    rows = []
    for f in files:
        if f.startswith(skip_dirs):
            continue
        is_test_file = any(p in f for p in ("/tests/", "/benches/", "/examples/")) or f in proofs
        in_test = is_test_file
        lines = Path(f).read_text(errors="replace").splitlines()
        for n, line in enumerate(lines, 1):
            if "#[cfg(test)]" in line:
                in_test = True
            for kind, rx in PATTERNS:
                if not rx.search(line):
                    continue
                if kind in ("panic!()", "mock/stub on a non-test path") and in_test:
                    continue
                if kind == "#[ignore]" and not in_test:
                    continue
                crate = owner(f, members)
                sev = severity(kind, crate, in_test)
                window = lines[max(0, n - 1 - GUARD_WINDOW):n]
                if sev in ("P0", "P1") and any(GUARD_MARK in w for w in window):
                    sev = "guard"
                rows.append((sev, f, n, crate, kind, line.strip()[:110]))
    rows.sort(key=lambda r: (r[0], r[1], r[2]))
    counts = {s: sum(1 for r in rows if r[0] == s) for s in ("P0", "P1", "P2", "guard")}
    print("# Gap register (Master Prompt 11 §1)\n")
    print("Generated by `python3 scripts/gap_register.py` from the working tree; do not edit by hand.")
    print("Severity is assigned by the rule in the script's docstring, so a reviewer can")
    print("disagree with the rule rather than with a thousand individual calls.\n")
    print(f"**{len(rows)} items: {counts['P0']} P0, {counts['P1']} P1, {counts['P2']} P2, {counts['guard']} reviewed guards.**\n")
    print("Caveats: test-module detection is lexical (after the first `#[cfg(test)]`), so a")
    print("`panic!` in a test helper above that line is counted; a `panic!` that documents an")
    print("unreachable invariant is still counted, because the register lists them for review,")
    print("it does not judge them. A `guard` row carries a `reviewed-guard:` comment saying")
    print("why reaching it is a bug; it is not a P0/P1 gap but stays listed for review.\n")
    print("| Sev | Location | Crate | Kind | Line |")
    print("|---|---|---|---|---|")
    for sev, f, n, crate, kind, text in rows:
        text = text.replace("|", "\\|")
        print(f"| {sev} | `{f}:{n}` | `{crate}` | {kind} | `{text}` |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
