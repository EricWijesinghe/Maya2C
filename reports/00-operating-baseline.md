# 00 — Operating baseline (first verification sweep)

**Date:** 2026-09-28 · **Commit:** `71c926a` (`master`) · **Command:**
`cargo xtask sweep --clean` (run from a copy of the xtask binary so
`cargo clean` could delete the target directory on Windows)

**Machine:** Intel Core Ultra 9 275HX, 31.4 GB RAM, Windows 11 Pro Insider
Preview build 29671 · `rustc 1.99.0-nightly (da80ed070 2026-07-14)` ·
`cargo-nextest 0.9.143` · own git worktree (`D:\Maya2C-ci`) and its own
target directory, cleaned first. Another session was working in
`D:\Maya2C` at the same time and agreed not to start a workspace build
during the run.

Full per-step output: [sweeps/2026-09-28.md](sweeps/2026-09-28.md).
Machine-readable: `sweeps/2026-09-28.json` (read by `cargo xtask status`).

## Result

| Step | Result | Seconds | Notes |
|---|---|---|---|
| fmt | ok | 3 | |
| unsafe | ok | 15 | see "What the sweep found" — ran under the wrong bash, re-run below |
| build (`--workspace --all-targets`, from clean) | ok | 1,392 | 23 min 12 s cold, with `jobs = 4` |
| clippy (CI gate: `-D clippy::unwrap_used`) | ok | 134 | |
| nextest `--workspace --no-fail-fast` | ok | 891 | **2,972 passed, 0 failed, 11 skipped** |
| ledger (`coverage --verify-targets`) | ok | 12 | |
| spec-coverage | ok | 0 | |
| readiness `--check` | ok | 0 | |
| lint-debt | **false pass** | 4 | clippy never ran; real re-run: 1,140 = baseline — see below |
| doc-coverage | FAIL | 0 | tool did not run; after fixes: ok, 99% — see below |
| cargo deny | ok | 5 | |

nextest's own summary line, verbatim:

```
     Summary [ 883.401s] 2972 tests run: 2972 passed (11 slow), 11 skipped
```

## What the sweep found

1. **The lint ratchet reported a pass it had not earned.** Output:
   `lint_debt: 0 diagnostics (baseline 1140) … ok` after 4 seconds. A
   Windows program that starts `bash` gets System32's WSL bash, which has no
   cargo, so clippy never ran and the script counted zero warnings. This
   is the failure Standing Order 1 describes. Fixed twice:
   `scripts/lint_debt.sh` now fails unless clippy reached `Finished`, and
   `xtask sweep` resolves Git's bash on Windows (`SWEEP_BASH` overrides).
2. **doc-coverage did not run** for the same reason (`rustc: command not
   found`), and failed rather than passing, which is the right way round.
3. **`sweep --clean` deleted its own log directory** on the first attempt
   (`os error 3`): the log directory lives in the target directory that
   `cargo clean` removes. Fixed in `71c926a`.
4. **Documented counts had drifted.** CLAUDE.md said 143 subsystems (62
   working); `features.toml` has 147 (61 verified, 66 working, 20
   planned). Corrected in `c43c279`.
5. **PROGRESS.md had stale entries**: ADR-026 unticked although accepted on
   2026-09-27, and a section header naming a deleted branch. Corrected.
   "Phase B — physical layout — not started" is also stale (the tree
   already has `crates/ bins/ apps/`); left for a reconciliation pass
   (BACKLOG.md).

No feature's `features.toml` status was downgraded: every gate that ran
passed, and the ledger gate confirms each `working`/`verified` entry names a
test target that exists.

## Re-run of the bash steps with the right bash

[sweeps/2026-09-28-1-bash.md](sweeps/2026-09-28-1-bash.md). The
working tree then carried the uncommitted bash fix on top of `71c926a`
(the record names only the commit; recording a dirty tree is in BACKLOG.md).

| Step | Result | Seconds | Output |
|---|---|---|---|
| unsafe | ok | 7 | `ok: every first-party unsafe block and impl carries a SAFETY comment` |
| lint-debt | ok | 70 | `lint_debt: 1140 diagnostics (baseline 1140)` — the real count, equal to the baseline |
| doc-coverage | **FAIL** | 2,094 | `error: unresolved link to FEE_SINK` in `crates/node/src/state/fees.rs:14`, so `custom-l1-node` could not be documented |

The doc-coverage gate runs only in `nightly.yml`, so this failure was not
visible on pull requests. The link was fixed in this session (it now names
`super::shielded::FEE_SINK`), and `cargo doc -p custom-l1-node --no-deps`
with `-D rustdoc::broken-intra-doc-links` finished cleanly. The gate's
re-run then **passed**: `doc-coverage` ok in 207 s, `TOTAL 9719 9692 99%` against the 90% floor ([sweeps/2026-09-28-2-doc-coverage.md](sweeps/2026-09-28-2-doc-coverage.md)).

## Not verified by this sweep

- Line coverage (never measured; M1 criterion 8).
- The CI-only jobs: fuzzing, Kani, the XDP program, TPM against swtpm,
  firmware under QEMU. They run on GitHub, not here.
- Anything behind `#[ignore]` (11 tests; each states its reason).
