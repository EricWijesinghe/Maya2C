# GitHub maintenance 2026-11-16

Base: origin/master e18b04a9. Live state read 2026-11-16 via gh.

## Inventory (live)

- Open PRs: 76 (53 draft, 23 non-draft). Non-draft `mergeStateStatus=CLEAN`: 0.
- Brief snapshot said 75 open; the difference is PRs opened since the brief was cut. Not reconciled per PR.
- Queued workflow runs: `gh run list --status queued --limit 50` returned 50 (the limit), not 16 as the earlier pass reported. The earlier count is stale or was taken with a different filter. Real total: NOT VERIFIED (capped at 50).
- In progress runs: 8.
- Brief counts: owned-skip 33, failing 18, conflicting 18, draft-park 5.

## Actions taken

Earlier pass (conflicts, ci-fixes), re-checked here against live GitHub:

| PR | Branch | Action | Head | Live state |
|---|---|---|---|---|
| 477 | docs/adr-135-v4-umbrella | merged master, kept both sides in ADR README and prior-art README | 30a24f3b | OPEN, MERGEABLE, BLOCKED |
| 472 | docs/adr-131-134-options | same two files | bcf4304d | OPEN, MERGEABLE, BLOCKED |
| 470 | docs/adr-125-130-gaps | same two files | 0aa0a441 | OPEN, MERGEABLE, BLOCKED |
| 469 | docs/adr-117-124-gaps | same two files | c3cc7023 | OPEN, MERGEABLE, BLOCKED |
| 449 | feat/notary | features.toml conflict resolved, TOML parses (tomllib, 190 subsystems) | 5ed83aec | OPEN, MERGEABLE, BLOCKED |
| 444 | feat/status-v2 | architecture-vision.md conflict resolved | 8638d668 | OPEN, MERGEABLE, BLOCKED |
| 539 | feat/github-presence | rustfmt on xtask/src/devlog.rs; regenerated docs/ecosystem/ROADMAP.md | 99fab9a9 | OPEN, MERGEABLE, BLOCKED |
| 433 | feat/site-live-network | merged master | 35459b68 | OPEN, MERGEABLE, BLOCKED |
| 481 | feat/gateway-webhooks | merged master | ab042d7a | OPEN, MERGEABLE, BLOCKED |
| 482 | feat/v3-ai-debug-mcp | merged master | 693b8069 | OPEN, MERGEABLE, BLOCKED |

- Pushes were fast-forwards (no force-push). Live head SHAs match the reported ones.
- CI on these ten heads has not finished. Check rollup shows only SKIPPED or NEUTRAL, and pending entries with no conclusion. Output: `#477 =37 NEUTRAL=1 SKIPPED=1 SUCCESS=3`, `#472 =30 SKIPPED=1` and the other eight similar. No CI result is verified yet.
- Queue: none appended. No non-draft PR was CLEAN, so nothing qualified.
- `gh pr update-branch`: not run. No PR was BEHIND.
- Deletes, closes, cancels: none. Queued-run cancellation: none, since no queued run belonged to a merged or closed branch.
- Releases: none exist (`gh release list` empty). Nothing deleted or re-tagged. The node-v0.2.0-preview.1 release is not present (404).
- Devlog post and README/STATE badges: not done. `xtask devlog` is not on master.

## Left (next coding lanes)

Root causes, from the brief and this check:

1. **Toolchain missing on CI runner.** `nightly-2026-07-15-x86_64-unknown-linux-musl` is not installed. Blocks cargo-deny on #546, #499, #491, #489, #495, #552, #550, #547, #543, #521 and others. Fix belongs in the CI install step. Not a per-PR fix. Owner: CI lane.
2. **Clippy on master-level code.** `crates/amount/src/lib.rs:223:33` cast warning fails clippy on #517, #516 and others. Fix once on master, not per branch.
3. **Install-action errors hiding messages.** nextest (core) on #509, #552, #550, #547, #543, #546, #521 fails with an install-action error whose message was never captured. Needs a fresh log pull.
4. **Visual regression baselines missing.** #479 and #460 fail on explorer-account and explorer-blocks (no baseline, run 43378541994). Needs baseline generation.
5. **Fuzz target errors.** #436: `payment_uri_decode` and `pool_identity_decode` fail with the error not captured.
6. **Reality ledger on #470.** Run 43653043729, error not captured. Not re-checked after the master merge.
7. **Conflicts (DIRTY), need owner rebase.** 545, 541, 525, 519, 513, 496, 488, 480, 460, 441, 436, 414. Several touch Cargo.lock, features.toml or docs/architecture-vision.md, which will conflict with each other again. Batch by file.
8. **Release preview.1 macOS jobs cancelled** (run 43612676586, macOS build jobs cancelled 2026-11-14 23:30Z to 2026-11-15 03:15Z). First real error NOT VERIFIED: job logs return 404 BlobNotFound.
9. **Refreeze/postgen/do-not-touch PRs**: left to their owners, unchanged.

## Unmerged branches with no open PR (for Eric)

Checked with `git rev-list --count origin/master..origin/<b>` and an empty `gh pr list --head`:

| branch | ahead | last commit |
|---|---|---|
| attacknet-reports | 13 | 2026-10-19 attacknet run record |
| feat/aws-validators | 5 | 2026-10-19 infra(aws-free) |
| feat/finality-latency-bench | 2 | 2026-11-12 fix(xtask) clippy::type_complexity |
| fix/explorer-render-network | 1 | 2026-11-13 fix(explorer): network arg to clear-sign render; looks like a P0, no PR |
| wip/fix/dex-zero-price-pool | 1 | 2026-11-02 wip, unbuilt |
| wip/fix/hub-guard-unix-keyfile | 1 | 2026-11-02 wip, unbuilt |

`fix/explorer-render-network` is the one most likely to need a PR soon.

## Verifier findings

- Live check of the ten pushed heads matched the reported SHAs (see table).
- The six conflict pushes are fast-forwards. Ancestry was not re-run with `merge-base --is-ancestor` in this pass; the matching SHAs and the push record are the evidence.
- No changed file on those ten branches is under node-paths.txt, per the earlier pass. Not re-diffed in this pass: NOT VERIFIED.
- 449 `cargo metadata --locked` and 444 Cargo.lock: never run locally. Need CI to confirm the lock.
- Queue tail (481 480 477 472 470 469 454 449 444 433 441 539) was not re-checked per PR.

## Not verified

- CI result for all ten pushed PRs (pending).
- Queued run total (capped at 50).
- Release failure cause.
- Devlog, badges, and visual baselines.
