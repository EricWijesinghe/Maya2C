# GitHub maintenance, 2026-11-17

Base: origin/master dc65ea7a (#600 merged). Run by a workflow subagent. Every result below is from a command output in this run unless marked NOT VERIFIED.

## Inventory (snapshot at start of this run)

- Coordinator counts: pending 1, green-ready 1, failing 6, owned-skip 8, conflicting 10.
- Live `gh pr list --state open --limit 200` at report time: 98 open PRs. Of these 60 are non-draft and 38 are draft. Non-draft states: BLOCKED 27, DIRTY 14. Draft states: UNSTABLE 23, BLOCKED 7, CLEAN 12, DIRTY 15. The earlier figure of 86 in the action log was an older snapshot.
- Merge queue `D:/Temp/claude-locks/merge-queue.txt` (15 entries): 603 480 477 472 469 454 444 441 539 576 582 587 597 578 586. At report time 603, 477, 472, 469, 454, 539, 576, 582, 587, 597, 578, 586 are BLOCKED; 480, 444, 441 are DIRTY and are skipped by the loop until they are rebased.
- Master head: dc65ea7a. #600 (site Dependabot bump) MERGED.

## Actions taken

1. Queue update: 600 was NOT appended. The earlier step reported that it was, but the queue file never contained 600. Moot, because #600 merged (dc65ea7a `chore(deps): Bump the site group in /docs/site with 3 updates (#600)`).
2. Conflict merges (master merged into branch, pushed, all still BLOCKED on checks):
   - #597 sec/misc: 321668ae. One conflict in `docs/security/THREAT_MODEL_SIGNER_CHAIN.md`, master side kept.
   - #586 sec/site-integrity: 9198fc7a then a75f99f2. Second push fixes a duplicate step name in `site.yml` introduced by the first push. Verified: no duplicate step names on a75f99f2.
   - #477 docs/adr-135-v4-umbrella: e7da2a38.
   - #472 docs/adr-131-134-options: 4310eb97.
   - #469 docs/adr-117-124-gaps: 6c5ac9ab.
3. CI-red fixes (pushed; CI runs queued, NOT VERIFIED at report time):
   - #578 sec/pool-miner: 18f1a586 (clean master merge). Old red was nextest `payout::tests::a_dropped_broadcast_is_sent_again_while_its_nonce_is_unspent`, `bins/pool-service/src/payout.rs:808`, run 43760409761. Root cause NOT VERIFIED.
   - #539 feat/github-presence: 9260dd3a. Root cause VERIFIED in run 43756978679, reality-ledger job: `xtask: README.md is stale: run cargo xtask devlog counts`. Local `cargo xtask devlog counts --check` printed `README.md matches features.toml`, exit 0.
   - #454 feat/audit-supply: 07a1424f. Root cause VERIFIED in run 43652762426: `bins/maya2c-audit/src/numbers.rs:85` needless_borrow and `observe.rs:94-95` field_reassign_with_default. Local clippy NOT VERIFIED (slot held about an hour, job killed).
4. Security PRs merged by the queue loop (per inventory and verifier): #579 #580 #581 #583 #585 #588 #590 #591 #592 #593 #594 #595. #600 MERGED.
5. Security PR updates pushed (CI NOT VERIFIED):
   - #576 sec/api-gateway: 4721b950. Conflict in `crates/api-gateway/src/limit.rs`, resolved by moving the loopback-peer gate into the shared `client_address` helper. Compile NOT VERIFIED locally.
   - #582 sec/chat-relay: 64e117db (includes a redundant empty commit after the merge).
   - #587 sec/cli-wallets: bc95aff8. Prior red run 43758469365 (clippy lint on `fn http(error: reqwest::Error)`).
6. Release: no GitHub releases exist (`gh api .../releases` returned 0). Tag `node-v0.2.0-preview.1` exists on origin at 0a74eac5. Run 43612676586 cancelled (9 build/test jobs succeeded; 5 macOS jobs cancelled; sign/updater-sign/release skipped). Cause of cancel NOT VERIFIED. No publish was run.
7. Branch hygiene: no deletes, no closes. Remote branches fully merged into master are only `postgen/v2`, `postgen/v3`, `postgen/v4`, which belong to another workflow and were left alone.
8. Not done: no force-push, no `gh pr merge`, no `--admin`, no `gh run cancel`, no local cargo build.

## Left open (root cause, next lane)

- #587 sec/cli-wallets, #597 sec/misc: sec/cli-wallets red clippy on `fn http(error: reqwest::Error)` in `crates/archive/src/kubo.rs:34`, the same lint on master (`htlc_payload.rs:124`). Fix lives in `crates/archive` and `crates/node`, which are node-paths, so it is freeze-restricted. Coordinator decision needed.
- #582 sec/chat-relay: clippy red inherited from the same archive lint, plus the chat-gui red fixed by #603. Clean merge with master.
- #576 sec/api-gateway: needs a human decision on the merged form. Master refactored `client_address()`, which has no loopback gate. The branch adds a loopback-peer gate on client-supplied IP headers. Resolved once in 4721b950; CI compile still pending.
- #444 feat/status-v2: conflict in `apps/status/src/main.rs` test module (about 33 lines, two overlapping test additions around a shared `Shared` constructor). Not mechanical. DIRTY, in queue, skipped by loop.
- #482 feat/v3-ai-debug-mcp: `bins/maya2c-mcp/src/node.rs` real logic conflict, plus CATALOG conflict. Not pushed.
- #441 feat: chat payment requests (ADR-088), DIRTY. Conflicts in `apps/wallet-gui` core `payment.rs`, `wallet.rs`, `tests/payment_tests.rs`, src-tauri `commands.rs`, `deeplink.rs`, ui `bridge.rs`. Large. In queue, skipped.
- #480 feat: public invariant monitor and validator ledger (CATALOG W1), DIRTY. Conflicts in Cargo.lock, Cargo.toml, architecture-vision, CATALOG, features.toml. Red clippy, nextest (core), cargo-deny, chat-gui. Large. In queue, skipped.
- #603 fix/chat-gui-lock (master CI fix, first in queue): OPEN, BLOCKED. 18 checks SUCCESS, 1 FAILURE (`supply chain (cargo vet, report-only)`), 1 NEUTRAL, 1 SKIPPED, 24 pending at last read. Not pushed to.
- #601 dependabot actions bump: waits on #603, workflow files only, no node paths.
- #578, #582, #587 reds: root causes for nextest (payout test) and clippy (rustup network error on #582/#587 runs) NOT VERIFIED. Re-run needed after CI.
- Master CI: startup_failure seen on `fix(ceremony,custody)` run. Not investigated.
- Release `node-v0.2.0-preview.1`: no release published; cancellation cause NOT VERIFIED.

## Freeze check (node binary)

- None of the touched PR diffs in this run hit `node-paths.txt`.
- Root `Cargo.lock` package changes for #578, #597, #454 do not hit `node-deps.txt` (verifier).
- #594 merged 2026-11-17T03:46:22Z. Conflicting reports: the release step said it bumps webpki packages in `node-deps.txt`; the verifier read the diff as touching only `fuzz/Cargo.lock`, `offsec-sandbox/Cargo.lock`, `deny.toml` and the security-audit workflow (not the root lock). Not re-checked in this run: review it.

## Unmerged branches with no PR (kept, for Eric)

| branch | ahead | last commit | PR state |
|---|---|---|---|
| attacknet-reports | 13 | 2026-10-19 attacknet (master d6e0e213) | no PR |
| feat/aws-validators | 5 | 2026-10-19 infra(aws-free) | no PR |
| feat/finality-latency-bench | 2 | 2026-11-12 fix(xtask) clippy type_complexity | #425 CLOSED |
| fix/explorer-render-network | 1 | 2026-11-13 fix(explorer) clear-sign network | #518 CLOSED |
| wip/fix/dex-zero-price-pool | 1 | 2026-11-02 wip (unbuilt) | no PR |
| wip/fix/hub-guard-unix-keyfile | 1 | 2026-11-02 wip (unbuilt) | no PR |

## Verifier findings (applied to this report)

- Wrong claim corrected: update-queue said 600 was queued. The queue never held 600. #600 merged anyway.
- Confirmed: no deletes or closes; no release deleted; pushed heads match the claimed SHAs; #586 duplicate step name fixed; sec PRs listed as merged are MERGED.
- Open: #578, #582, #587 BLOCKED on CI reds, root causes unconfirmed. #576, #586, #597 not verified green. #441, #480 DIRTY but still in queue (harmless: loop skips them).
- Correction: the earlier "86 open PRs" figure is stale; live count is 98.

## Next lanes (from this run)

1. Coordinator decision on the `crates/archive/src/kubo.rs:34` clippy lint (node-path, freeze). Unblocks #587, #582, #597 and master.
2. #441 and #480: rebase or split; both are large.
3. #444 and #482: manual conflict resolution.
4. CI re-run on #578, #539, #454, #576, #582, #587 after current queue drains; NOT VERIFIED until real output.
