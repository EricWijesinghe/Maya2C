# Session 2026-09-28 — operating system

**Goal:** set up how every session works (Eric's operating brief), no
product features. Worktree `D:\Maya2C-ci`, because another session was
working in `D:\Maya2C` on `feat/utopia-chat`.

## Done, with the command output that shows it

1. **Branches.** `master` fast-forwarded to the finished work; PR #1 merged by
   that push; PR #2 closed as superseded; extra branches deleted.
2. **CI red → fixed** (`06202e1`). The CI log said:
   ```
   error: unsafe without a SAFETY comment:
     ./bins/maya2c-cli/tests/source_lines_tests.rs:20: unsafe {
     ./crates/multivm/src/sbf.rs:91: let mapping = unsafe { MemoryMapping::new(...) }
   ```
   plus rustfmt diffs. After: `ok: every first-party unsafe block and impl
   carries a SAFETY comment`, `cargo fmt --all --check` clean, and
   `1 test run: 1 passed` for the source-lines test (its hard-coded
   `probe.rs:12` now computed). GitHub at `71c926a`: rustfmt, clippy,
   reality ledger, spec conformance, cargo-deny, security-audit all
   `success`; nextest (core) still running at handover.
3. **`cargo xtask status` and `cargo xtask sweep`** (`c43c279`, `71c926a`,
   and this commit). `cargo nextest run -p xtask`: `22 tests run: 22
   passed`. First `status` output, before any sweep, printed MISSING for
   every absent figure and `61 verified, 66 working, 0 stub, 20 planned`.
4. **Documents:** STATE, DECISIONS, BACKLOG, RISKS, docs/MISSION, EMPIRE,
   ECOSYSTEM, prior-art/p2p-chat; Operating Protocol in CLAUDE.md.
   `claims-check: 63 public files scanned, 0 unauthorised claims`;
   `guides-check: … 0 name something missing`.
5. **First clean sweep:** `reports/00-operating-baseline.md` —
   `2972 tests run: 2972 passed (11 slow), 11 skipped`.
6. **Executive report:** `reports/executive/2026-09-28.md`.

## Found and fixed along the way

- `sweep --clean` deleted its own log directory (`os error 3`).
- **Lint ratchet false pass**: `lint_debt: 0 diagnostics (baseline 1140) …
  ok` in 4 s, because a Windows program's `bash` is WSL's, without cargo.
  Fixed in the script (requires `Finished`) and the sweep (Git bash).
  Real: `lint_debt: 1140 diagnostics (baseline 1140)`.
- **doc-coverage red** on `error: unresolved link to FEE_SINK`; fixed;
  `TOTAL 9719 9692 99%`.
- CLAUDE.md counts stale (143 → 147 subsystems); PROGRESS.md stale entries.

## Red-team pass on `status` / `sweep`

Tried: no evidence at all (prints MISSING, no numbers — test); a sweep
from an older commit (flagged stale — test); a nextest run that died while
building (no counts rather than zero — test); a partial rerun (written
separately, never replaces `latest.json`). Not covered: a dirty tree is
not recorded (BACKLOG.md).

## Decisions

DECISIONS.md, 2026-09-28 entries (seven).

## Unfinished

GitHub nextest (core) on `master`; Eric's decisions (STATE.md, "Blocked on
Eric").
