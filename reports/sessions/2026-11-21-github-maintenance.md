# GitHub maintenance, 2026-11-21

Scope: Eric 2026-11-15, keep GitHub updated and maintain the needed branches. PRs owned by other
workflows (refreeze, followups, genesis-push, audit-to-prs lists) were not touched. All figures below
come from the workflow's inventory and agent outputs; anything not seen in command output is marked
NOT VERIFIED.

## Inventory (open PRs at start)

| Class | Count |
|---|---|
| draft-park | 30 |
| conflicting | 20 |
| failing | 10 |
| pending | 1 |
| green-ready | 3 |
| owned-skip | 34 |

## Actions taken

- Queued #612, #615, #633 in `merge-queue.txt` (all required checks green; only report-only jobs
  failed: cargo vet, fuzz and offsec advisories). Verifier later saw all three MERGED, with `ci`
  runs `success` (44395691878, 44414161687, 44318051191). Files: #612 `sdks/sdk-js` package files,
  #615 `offsec-sandbox/Cargo.lock`, #633 `.gitignore`. #615 lock packages were not cross-checked
  against `node-deps.txt` (NOT VERIFIED; offsec-sandbox is outside the node closure).
- Resolved master conflicts in `docs/adr/README.md` and `docs/prior-art/README.md` (kept both
  sides' rows) and pushed: #472 `df19b8a5..6e45a486`, #477 `e989c3cd..5d41763e`. Both appended to
  the merge queue. CI on those heads had not finished when the agent reported (NOT VERIFIED).
  The ADR index rows are not in numeric order; cosmetic.
- #629 (cargo-deny fix) is MERGED. Post-merge master `ci` state was not checked (NOT VERIFIED).
- CI queue: 32 queued runs, over the 20 threshold, so no dependabot or draft reruns. Nothing
  cancelled. Left alone: `ci|master` 44883406484 (SHA 8252491f, looks superseded by f2305189 run
  44884446965), `visual-baseline` 44899563361 on `scratch/l01-baselines` (workflow_dispatch, may
  belong to a lane).
- Branch hygiene: no branches deleted, no PRs closed (both candidate lists empty).
- Release: no release exists. preview.1 run 43612676586 `cancelled` (macOS jobs cancelled, no
  build error). preview.2 run 44264484552 on the #629 merge commit is `waiting`: every build job
  `success`, only `updater-sign` waits on the `release` environment reviewer (Eric,
  `current_user_can_approve: true`). Not approved by us. Keep-latest-two cleanup applies after.
- Dev log: #539 merged; `xtask/src/devlog.rs` present. `cargo xtask devlog week` could not run: two
  attempts hit the 600 s limit on the memory gate ("RAM free ~7 GB < 8 GB"); gate not bypassed.
  Nothing posted. Discussions category ready (repo id `R_kgDOUImKjg`, category id
  `DIC_kwDOUImKjs4DLGGH`, empty). Rerun when RAM free >= 8 GB:
  `cargo xtask devlog week --since 2026-11-14`. README drift check (`devlog counts --check`,
  `roadmap --check`): NOT VERIFIED, same gate.

## Left: next coding lanes (root cause)

Merge conflicts, too large for a merge-only push:

- #414 indexer API v1: `crates/api-gateway/src/limit.rs`, CATALOG.md, features.toml
- #436 [WIP] fuzz targets: nightly.yml, security-audit.yml, fuzz/Cargo.toml, proof-ratchet.txt; fuzz
  targets `payment_uri_decode`, `pool_identity_decode` also red (WIP)
- #460 wallet undo-send: 17 files incl. Cargo.lock, wallet-gui core; reds visual regression, cargo-deny (old runs)
- #488 Forged Light 2 tokens: ci.yml, Cargo.lock, xtask, docs/site lockfile
- #489 agent-registry: `bins/maya2c-mcp/tests/mcp_tasks.rs`
- #491 hub GPU auto-tuner and #525 joules per share: both `apps/hub/src-tauri/src/gpu.rs` (resolve together)
- #495 chat mini-apps: 17 files, chat-gui, apps/chat pay, wallet chat_pay
- #496 intent solver: Cargo.lock, `bins/maya2c-mcp/src/main.rs`, ADR index
- #498 C++ SDK and #508 DevEx lane C: both `.github/workflows/sdks.yml`
- #501 [post-genesis] sponsored fees: `crates/node/src/state/proof.rs`; stays draft
- #513 chat reactions: chat-gui store/stickers, apps/chat client, features.toml
- #519 wallet-extension: architecture-vision.md, CATALOG.md; reds clippy, nextest, cargo-deny (old)
- #541 manual-only updates: `apps/app-update/src/lib.rs`, chat-gui lib.rs, CATALOG.md; 2 checks pending
- #545 Ledger clear-sign: Cargo.lock, wallet-gui lib.rs; red clippy run 44229398515
- #546 invariant 36: Cargo.lock, xtask/Cargo.toml
- #620 notary: Cargo.lock, `bins/maya2c-cli/Cargo.toml`

Failing, not fixable by a master merge:

- Dependabot #601 #606 #607 #609 #610 #611 #616 #635 #636: cargo-deny red. For #606 the failing
  run 44426306865 shows wasmtime advisories (RUSTSEC-2026-0566 and -0567 among others). #641
  (`[re-freeze]` wasmtime 48.0.10, RUSTSEC-2026-0566..0573) is owned by another workflow. Re-check
  after #641 merges. #601 chat-gui reds and #610 wallet 360 px layout: cause NOT VERIFIED.
  #606 and #635 touch lockfiles and may be freeze-guard owned-skip (NOT VERIFIED). No pushes made.
- #637 rust-minor-and-patch (root): reds reality ledger, spec conformance, clippy, nextest, nested
  lock (runs 44887436934, 44887436908). Root lock bump likely touches node deps, so it is a
  freeze-guard owned-skip candidate (NOT VERIFIED). Not queued.

Queue note: `merge-queue.txt` has duplicate entries (638, 472, 477); merged entries are harmless.

## Unmerged branches with no PR (for Eric)

| Branch | Ahead | Last commit |
|---|---|---|
| attacknet-reports | 13 | 2026-10-19 attacknet: 2026-10-19 (master d6e0e213) |
| feat/aws-validators | 5 | 2026-10-19 infra(aws-free): run on any Linux VM |
| feat/finality-latency-bench | 2 | 2026-11-12 fix(xtask): name run_load's return type |
| fix/explorer-render-network | 1 | 2026-11-13 fix(explorer): pass the network to clear-sign render |
| scratch/l01-baselines | 12 | 2026-11-21 fix(design): lift the light fills to mid-tone (active, leave) |
| wip/fix/dex-zero-price-pool | 1 | 2026-11-02 wip: saved after an unexpected shutdown (unbuilt) |
| wip/fix/hub-guard-unix-keyfile | 1 | 2026-11-02 wip: saved after an unexpected shutdown (unbuilt) |

## Verifier findings

No findings wrong, nothing to undo. #472 and #477 changes are docs-only, no conflict markers left,
PRs OPEN and MERGEABLE with mergeStateStatus BLOCKED while CI ran.
