# GitHub maintenance 2026-11-18

Base: origin/master 51ca8375 (#629 merged 2026-11-18T00:55:29Z). Live state read via gh.

## Inventory

- Brief counts: pending 2, failing 32, owned-skip 30, draft-park 32, conflicting 13.
- Large items: #614 and #613 are major bumps (typescript 5 to 7, vitest 4 to 5) in sdks/sdk-js. Not node-touching, but need human review before any queue. Not queued.
- Queued workflow runs now: 46 (`gh run list --status queued --limit 50`, count 46). The ACTIONS pass reported 14 after its reruns; that figure is stale. Above the 20 threshold, so dependabot and draft reruns were not done.

## Actions taken (from the workflow passes, with verifier notes)

| PR / branch | Action | Output line |
|---|---|---|
| #629 | merged (fix for cargo-deny on master) | `#629 MERGED 2026-11-18T00:55:29Z` |
| #540, #538, #524, #500 | update-branch refused | `Cannot update PR branch due to conflicts`. Owners must resolve. |
| #6 dependabot ci runs (apps/wallet-gui/ui #610, dashboard #609, wallet-gui/src-tauri #608, ledger-maya2c #607, hub/src-tauri #606, chat-gui/src-tauri #605) | `gh run rerun` on 44016653916, 44016561541, 44016348141, 44016169107, 44016103506, 44015207311 | rerun returned no output; run status not re-read |
| #587 sec/cli-wallets | pushed 804fcab5: moved admission_tests below main() (clippy items_after_test_module) | branch head matches; CI NOT VERIFIED |
| #582 sec/chat-relay | pushed cf47a119: collapsed nested if (clippy collapsible_if) | head matches; CI NOT VERIFIED |
| #578 sec/pool-miner | pushed 105e8b87: settle() now propagates rebroadcast flags (test a_dropped_broadcast_is_sent_again...) | head matches; CI NOT VERIFIED |
| #576 sec/api-gateway | pushed e0b20c70: merged origin/master | head matches; not queued |
| #597 sec/misc | pushed fbb76021: merged origin/master (Cargo.lock auto-merged) | head matches; not queued |
| #539 feat/github-presence | two pushes: 45f771e3 (ROADMAP regenerated), 55135f9c (first ws ping delayed one interval; `cargo test -p maya-api-gateway --test event_stream_tests` gave `8 passed; 0 failed`) | live head 55135f9c, BLOCKED |
| #480, #482, #444 | merges staged locally only, not pushed; cargo verification never ran (slot unavailable) | NOT VERIFIED |

- No merges, no queue appends, no `gh pr merge`, no `--admin`.
- No cancellations. Queued runs ci 44205295887 and security-audit 44205295915 (sec/node-net, #589) left alone.
- Hygiene: no deletes, no closes. Superseded-PR list was empty.
- Releases: none exist (`gh release list` empty). Preview.1 release-binaries run 43612676586 is `cancelled` (5 macOS jobs cancelled, 9 success). No re-tag.
- Dev log not posted: #539 not merged, and `cargo xtask devlog` is not on master. Discussion category "Dev log" id DIC_kwDOUImKjs4DLGGH is ready.

## Verifier findings

- **Wrong action, not undone:** #539 received two pushes this run (45f771e3, 55135f9c), breaking the one-push-per-PR rule. A third commit, 9260dd3a `fix(docs): refresh README ledger counts...`, is on the branch and not in the action log, so its source is unknown. Undo only with owner confirmation: `git push origin 55135f9c:refs/heads/feat/github-presence` would drop 9260dd3a and 45f771e3. Nothing changed here.
- Verified: heads of #576 e0b20c70, #578 105e8b87, #587 804fcab5, #597 fbb76021, #582 cf47a119 match the report.
- Not verified: the node-deps status of the #597 Cargo.lock auto-merge; the test-weakening question for the #578 and #539 pushes (needs a diff read).
- Not verified: master CI run 44211774350 (in progress at check time).

## Left (next coding lanes)

1. Conflicts needing owner resolution: #540 (feat/incremental-state-root, [re-freeze]), #538, #524, #500 (fix/block-time-governance-windows, [re-freeze]). Root cause: branches diverged from master; update-branch refuses.
2. Conflicts on drafts: #541 (app-update, lib.rs + chat-gui lib.rs + CATALOG), #525 and #491 (hub gpu.rs), #519 (architecture-vision.md), #513 (chat-gui lib.rs, stickers, store), #508 (api.md), #496 (adr README), #488 (ci.yml, needs review), #460 (wallet-gui core, 7 files), #441 (in merge-queue.txt, DIRTY across 8+ wallet-gui files). #441 should be removed from the queue until fixed.
3. Cargo-deny reds that need master merged (not yet done for these): #578/#587/#582 after #629; #597 and #621/#624/#628 as listed in the brief. Re-check after #629 merge.
4. Failing non-deny reds: #517/#516 (clippy, doc links), #509 (nextest core, old run), #436 (fuzz payment_uri_decode, pool_identity_decode), #479 (visual regression, rustfmt), #587/#582 clippy (pushed fixes unverified).
5. Dependabot majors #614 (typescript 7.0.2) and #613 (vitest 5.1.0): human review decision.
6. Queue health: 46 queued runs. Reruns of dependabot and draft-PR cancelled runs wait until the queue is under 20.
7. #480 DIRTY (Cargo.lock regen needed, slot blocked), #482 and #444 merge commits not pushed.

## Unmerged branches with no open PR (for Eric, not deleted)

| Branch | Ahead of master | Last commit |
|---|---|---|
| attacknet-reports | 13 | 8f1b5858 2026-10-19 attacknet report |
| feat/aws-validators | 5 | 1c994ff6 2026-10-19 infra(aws-free) |
| feat/finality-latency-bench | 2 | df2bd2a2 2026-11-12 xtask type_complexity fix |
| fix/explorer-render-network | 1 | dd23f76b 2026-11-13 explorer compile fix (may deserve a PR) |
| wip/fix/dex-zero-price-pool | 1 | 15c6fee3 2026-11-02 wip |
| wip/fix/hub-guard-unix-keyfile | 1 | 23aecf0f 2026-11-02 wip |

postgen/v2, postgen/v3, postgen/v4 are ancestors of master (14bbd2f8), ahead 0, no open PR. Not deleted: postgen/* is owned by other workflows. Say "delete postgen/v2 v3 v4" to remove them.

## Not done

- No merge-queue.txt appends this run (no candidate was green; sec/* PRs need CI results first).
- No draft reruns, no security-audit reruns, no master-run reruns.
