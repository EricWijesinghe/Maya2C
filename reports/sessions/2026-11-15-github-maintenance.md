# GitHub maintenance, 2026-11-15

Scope: open PR triage, merge-queue hygiene, branch hygiene, conflict and CI-fix pushes, release check. Branch base at write time: origin/master `ab489faf` (moved from `99cffe12` during this run because #452 merged).

## URGENT: #452 merged to master (post-genesis dependency bump)

- `gh pr view 452`: `state: MERGED`, `mergedAt: 2026-11-14T23:25:22Z`, merge commit `ab489faf`.
- Merge loop log: `08:25:23 452: merged (6/6)` in `D:/Temp/claude-locks/merge-queue.log`.
- The verifier removed `452` from `merge-queue.txt` after the loop had already merged it, so the removal did not undo anything.
- `ab489faf` changes `crates/crypto-pq/Cargo.toml`: `ml-kem =0.3.3 -> =0.3.4` and `ml-dsa =0.1.1 -> =0.1.2` (the verifier named only ml-kem; ml-dsa is also bumped). It also changes `Cargo.lock`, `apps/chat-gui/src-tauri/Cargo.lock`, `apps/wallet-gui/src-tauri/Cargo.lock`, `fuzz/Cargo.lock`, `offsec-sandbox/Cargo.lock`.
- `crates/node/Cargo.toml` has `maya-crypto-pq` as a normal dependency, so this changes the frozen node's signature and KEM crates. AGENT-RULES makes that post-genesis: draft only, never merged.
- No revert was opened. Master is PR-only and the revert decision belongs to Eric. Options: (a) revert PR as `[post-genesis]` draft and re-freeze review; (b) keep the bump and re-validate the freeze gates. Eric decides.
- Process gap: the merge loop has no post-genesis guard. It merged a Dependabot PR whose diff touches node-linked crates.

## Inventory (coordinator snapshot, 88 open non-refreeze/postgen PRs)

| Bucket | Count |
|---|---|
| owned-skip | 10 |
| draft-park | 43 |
| pending | 10 |
| conflicting | 12 |
| failing | 12 |
| green-ready | 1 |

Re-checked this run: 26 PRs in `merge-queue.txt` (`481 480 477 472 470 469 468 454 453 449 444 433 497 441 539 556 557 559 561 563 564 565 566 567 568 418`), all non-draft and OPEN. Path check on the non-doc files of each queued PR found no file under `crates/node` or `bins/maya2c-node`. Direct normal dependencies of those two manifests do not include the crates the queued PRs change (deposit-watcher, entropy, api-gateway, wallet-core, audit, chat, status, cli, checkout, invariant-monitor). Transitive effect through `Cargo.lock`: NOT VERIFIED (no `cargo tree` run, per CI-first rule).

## Actions taken

1. **Queue:** `452` appended, then removed by the verifier. The loop had already merged it (see above). Queue file otherwise unchanged. Duplicate check: none.
2. **Superseded master runs cancelled:** `43639619268` (ci) and `43639619281` (site) on `64b4bc1c`: `completed cancelled`. `43639102563` (ci) on `6e36cf3b`: `completed cancelled`. Newer master runs exist (ci `43646047584`, site `43647953579`). CodeQL run `43648085301` for #563 left alone (different head SHA).
3. **Conflict merges (master merged into head, no force-push):**
   - #539 `feat/github-presence`: `034d1fd8`. Conflict in `ci.yml`; both steps kept.
   - #454 `feat/audit-supply`: `1998ae66`. Conflicts in `docs/architecture-vision.md` and `features.toml`; TOML parses, 187 subsystems.
   - #418 `feat/merchant-checkout`: `74199e67`. Conflict in `Cargo.toml`; both members kept. Appended to queue.
   - Verifier: all three OPEN and MERGEABLE at push time. Not re-run in this report.
4. **CI-fix merges (master merged into head):**
   - #497 `7ea2d79f`, #477 `77fbdeeb`, #472 `1a24b5c1`, #470 `84177621`, #453 `14352b0e`: cargo-deny gossipsub advisory, fixed on master by `2090300c` (libp2p-gossipsub 0.50.1).
   - #469 `b49a20f2`: merge plus one fix. Reality ledger failed on dead links `ADR-122 -> ADR-114-contract-timers.md` and `ADR-124 -> ADR-115-post-quantum-vrf.md`. Replaced with plain text. Links must be restored if #468 lands.
   - CI on all six: NOT VERIFIED. Queue log at 08:25 still shows #469 `reality ledger` and #470/#472/#477 `cargo-deny` red, all from before these pushes.
5. **Branch hygiene:** no deletions, no closes. 0 merged-no-PR branches. 80 unmerged branches have open PRs (untouched).
6. **Releases:** `gh release list` returns 0 releases. Tag `node-v0.2.0-preview.1` exists with no release. Run `43612676586` (release-binaries) cancelled, five macOS jobs cancelled 1-16 min after start, no step failure, cause NOT VERIFIED. Not re-tagged.
7. **Dev log:** not posted. Depends on #539, still pending.

Left alone by rule: #500 #506 #522 #540 #542 #544 #549 #555, `refreeze/*`, `postgen/*`, `[post-genesis]` and `[needs-eric]` PRs (#569 draft, #562, #558). Nothing merged directly; no `--admin`.

## What is left (next coding lanes)

- **Conflicting (9 of 12 listed):** #541 (CATALOG.md), #525 (adr README, architecture-vision), #519 (architecture-vision; reds: Analyze x2, advisories, clippy), #513 (features.toml), #488 (ci.yml, Cargo.lock, adr README), #480 (Cargo.lock, Cargo.toml, architecture-vision, CATALOG, features.toml; in queue, will not merge until rebased), #460 (wallet-gui src-tauri commands.rs, lib.rs; base for #479), #441 (9 files in wallet-gui and docs; in queue, will not merge; Analyze red), #414 (explorer model.rs, features.toml).
- **Failing (5 of 12 listed here):** #482 (advisories/licences, cargo-deny, CodeQL), #481 (advisories, clippy, nextest core), #449 (clippy, nextest core, cargo-deny), #433 (nextest core, cargo-deny, clippy), #468 (fuzz `iso20022_decode`, run `43635829641`, first error NOT VERIFIED). Lane: merge master into #481/#449/#433/#482 to pick up `2090300c`, as done for the six CI-fix PRs. Not done in this run.
- **#468 blocking red:** fix `iso20022_decode` fuzz target first.
- **#452 decision for Eric** (see top).
- **Queue entries that are not merge-ready:** #480, #441 (conflicting), #468 (red). The loop will not merge them while red, but they should leave the queue until fixed.
- **Skipped for cap:** #444 `feat/status-v2`: same cargo-deny red; merge master next.

## Unmerged branches with no PR (for Eric)

Count reported by hygiene: 8. Names listed: 7. One branch unidentified: NOT VERIFIED.

- `attacknet-reports` (ahead 13, last 2026-10-19)
- `feat/aws-validators` (ahead 5, last 2026-10-19)
- `feat/finality-latency-bench` (ahead 2, last 2026-11-12)
- `fix/explorer-render-network` (ahead 1, last 2026-11-13)
- `wip/fix/dex-zero-price-pool` (ahead 1, last 2026-11-02)
- `wip/fix/hub-guard-unix-keyfile` (ahead 1, last 2026-11-02)
- `release/mainnet-v1` (ahead 5, last 2026-11-04, from the #358 merge; flagged, not in brief)

## Verifier findings

- #452 was merged, not merely queued. The "undone" removal did not revert anything. See top section.
- #452 bumps ml-dsa as well as ml-kem.
- The merge loop has no post-genesis guard.
- Queue includes #480 and #441, which the inventory marked "will not merge".
- Queued PRs' node-link status checked by path and direct dependency only; transitive status NOT VERIFIED.
- #469 link restoration pending on #468.
- No CI result in this report is verified green.
