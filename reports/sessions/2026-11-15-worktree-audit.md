# Worktree audit 2026-11-15

Scope: `git worktree list` of the shared repo (284 entries: `D:/Maya2C/.claude/worktrees/*`, `F:/wt-*`, `G:/Maya2C-store/archive/*`, `D:/Maya2C-wt-relnotes`, `D:/Temp/claude/wt-docs`, and the main checkout `D:/Maya2C`). Prunable entries skipped.

Method: `git rev-list --count <HEAD> --not --remotes` after `git fetch origin`, so "unpushed" means commits reachable from no remote-tracking ref. Open-PR match means the branch name is an open PR head (85 open PRs at audit time).

Nothing was deleted or moved. Eric or the coordinator decides each worktree and branch. Ahead counts are local-only commits, so a large count usually means an old fork from before a squash or rebase of master, not new work.

## A. Unpushed commits, no open PR (75)

| ahead | branch | worktree | HEAD | last commit subject |
|---|---|---|---|---|
| 1358 | `docs/miner-onboarding` | `G:/Maya2C-store/archive/Maya2C-wt-mine2` | db3559ac | docs: record MAYA ticker (SLIP-44 2992) in listing facts |
| 1356 | `feat/committed-consensus-schedule` | `G:/Maya2C-store/archive/Maya2C-wt-g2` | 23e26cd5 | test(attacknet): drive the S-009 refusal through a single behind source (#367) |
| 1356 | `feat/council-mldsa87-rotation` | `G:/Maya2C-store/archive/Maya2C-wt-g4` | 23e26cd5 | test(attacknet): drive the S-009 refusal through a single behind source (#367) |
| 1356 | `feat/mining-listing-apis` | `G:/Maya2C-store/archive/Maya2C-wt-mine1` | 23e26cd5 | test(attacknet): drive the S-009 refusal through a single behind source (#367) |
| 1356 | `fix/genesis-policy-guards` | `G:/Maya2C-store/archive/Maya2C-wt-g1` | 23e26cd5 | test(attacknet): drive the S-009 refusal through a single behind source (#367) |
| 1356 | `fix/tx-inputs-and-chain-scoped-net` | `G:/Maya2C-store/archive/Maya2C-wt-g3` | 23e26cd5 | test(attacknet): drive the S-009 refusal through a single behind source (#367) |
| 1354 | `fix/certsync-log-refused` | `G:/Maya2C-store/archive/Maya2C-wt-certlog` | 6847852d | fix(node): log each refused or failed certificate-sync source |
| 1353 | `chore/port-bft-devnet` | `G:/Maya2C-store/archive/Maya2C-wt-bftdev` | c88c1af2 | chore: port bft_devnet.py to cargo xtask bft-devnet |
| 1353 | `ci/kani-heavy-jobs` | `G:/Maya2C-store/archive/Maya2C-wt-kanici` | 552bbd67 | ci: run heavy dex Kani harnesses on their own runners |
| 1353 | `ci/release-provenance` | `G:/Maya2C-store/archive/Maya2C-wt-prov` | f8eda3b2 | ci: attest build provenance for every release file, not only SHA256SUMS |
| 1353 | `docs/site-new-features` | `G:/Maya2C-store/archive/Maya2C-wt-site5` | d6b13749 | docs(site): cover chat receipts, validator bootnode fallback, explorer tx |
| 1353 | `feat/status-decentralization` | `G:/Maya2C-store/archive/Maya2C-wt-decent` | cf26eade | feat(status): show committee stake spread from get_validators (W1) |
| 1353 | `fix/follow-loop-failover` | `G:/Maya2C-store/archive/Maya2C-wt-failover` | 0c701949 | fix: a follower rotates off a stalled catch-up source |
| 1353 | `test/attacknet-s009-exercised` | `G:/Maya2C-store/archive/Maya2C-wt-s009` | c6257f26 | test(attacknet): drive the S-009 refusal through a single behind source |
| 1352 | `chore/python-to-rust-3` | `G:/Maya2C-store/archive/Maya2C-wt-depy2` | 8f504f90 | chore: port gen_kheavyhash.py to cargo xtask kheavyhash |
| 1347 | `fix/localnet-wallet-confirm` | `G:/Maya2C-store/archive/Maya2C-wt-red1` | adbf34df | fix(xtask): bench-tps presign passes --yes/--genesis; sweep tests allow unwrap like other  |
| 1346 | `fix/restart-ahead-source-behind` | `G:/Maya2C-store/archive/Maya2C-wt-restartfix` | 8323da22 | fix: name the behind source in its log, and pin the catch-up ordering |
| 1346 | `fix/rwa-canonical-decode` | `G:/Maya2C-store/archive/Maya2C-wt-rwa` | 7f5fca7d | fix: refuse unsorted cap table pages instead of re-sorting them |
| 1345 | `feat/chat-receipt-status` | `G:/Maya2C-store/archive/Maya2C-wt-chatui` | 50370a21 | Merge origin/master into feat/chat-receipt-status |
| 1342 | `feat/hub-toasts` | `G:/Maya2C-store/archive/Maya2C-wt-hub3` | 2bd93ac9 | feat(hub): opt-in OS toasts for alerts that need action |
| 1339 | `docs/progress-merged` | `G:/Maya2C-store/archive/Maya2C-wt-prog` | ed39b233 | docs: PROGRESS lists the 11-03 PRs as merged (they were; line said unmerged) |
| 1339 | `docs/readiness-tee-drift` | `G:/Maya2C-store/archive/Maya2C-wt-docs3` | 98aa0fd0 | docs: regenerate readiness scorecard and align TEE wording with invariant 11 |
| 1339 | `feat/validator-power-guard` | `G:/Maya2C-store/archive/Maya2C-wt-ops` | c35ff323 | feat: refuse validator start on a PC whose power plan can sleep it |
| 1338 | `test/chat-fuzz-chaos` | `G:/Maya2C-store/archive/Maya2C-wt-chatfuzz` | 763e04db | chore: port CI and report Python scripts to cargo xtask (#347) |
| 1325 | `fix/hub-guard-unix-keyfile` | `G:/Maya2C-store/archive/Maya2C-wt-guard` | 98ad95e4 | Merge master into fix/hub-guard-unix-keyfile |
| 1324 | `feat/explorer-kind-column` | `G:/Maya2C-store/archive/Maya2C-wt-explorer` | 865a6a86 | Merge remote-tracking branch 'origin/master' into feat/explorer-kind-column |
| 1323 | `chore/proving-24h` | `G:/Maya2C-store/archive/Maya2C-wt-24h` | 802e66ea | Merge remote-tracking branch 'origin/master' into chore/proving-24h |
| 1322 | `feat/chat-outbox-receipts` | `G:/Maya2C-store/archive/Maya2C-wt-chat` | 7163c735 | style: rustfmt |
| 1322 | `fix/yanked-async-executor` | `G:/Maya2C-store/archive/Maya2C-wt-yank` | 142a4e1c | fix: bump async-executor 1.14.0 -> 1.14.1 (1.14.0 yanked; cargo-deny red on master) |
| 1319 | `docs/drift-2026-11-03` | `G:/Maya2C-store/archive/Maya2C-wt-drift` | 5a785f69 | docs: correct drift found on master 9cdb74e8 (TEE citations, decimals, validator keys, lau |
| 1319 | `feat/hub-multi-rig` | `G:/Maya2C-store/archive/Maya2C-wt-hubc1` | 38520d76 | feat: Hub supervises one miner per device so one crash or heat spike stops one rig, not al |
| 1318 | `docs/adr-079-verifiable-compute` | `G:/Maya2C-store/archive/Maya2C-wt-adr79` | 5c9db33f | docs: record B50 work-claim refusal and cite it from ADR-079 |
| 1315 | `fix/site-concurrency` | `G:/Maya2C-store/archive/Maya2C-wt-privacy` | 6f0daf1d | ci(site): let a running deploy finish instead of cancelling it |
| 1305 | `feat/pool-followups` | `G:/Maya2C-store/archive/Maya2C-wt-pool2` | 1c951792 | style: rustfmt |
| 1305 | `fix/b52-b17-strict-decode` | `G:/Maya2C-store/archive/Maya2C-wt-robust` | 99118926 | test: pin the RWA issuer gate at the real activation constant |
| 1305 | `fix/join-scripts-offline` | `G:/Maya2C-store/archive/Maya2C-wt-join` | 752c68d4 | fix(ci): hash the devnet genesis from stdin on Windows |
| 1305 | `fix/kani-exact-out` | `G:/Maya2C-store/archive/Maya2C-wt-kani` | c2029f03 | fix(kani): split exact-out harness by direction so CBMC finishes |
| 1304 | `docs/decisions-2026-11-02` | `G:/Maya2C-store/archive/Maya2C-wt-docs2` | 6b3c9534 | docs: record 2026-11-02 owner decisions and fix code-vs-doc drift |
| 1304 | `fix/b38-min-out` | `G:/Maya2C-store/archive/Maya2C-wt-fixb38` | dbbe78f4 | fix(wallet): a swap's default min_out is the quote less 0.5%, not 1 |
| 1301 | `promote/mainnet-v1-9913c144` | `G:/Maya2C-store/archive/Maya2C-wt-promote` | ea551c07 | fix: bump yanked async-executor 1.14.0 -> 1.14.1 on the release branch |
| 1297 | `(detached)` | `G:/Maya2C-store/archive/Maya2C-wt-flake` | 9913c144 | Merge pull request #325 from EricWijesinghe/fix/b51-join-sealed-keys |
| 1247 | `docs/mainnet-readiness-assessment` | `G:/Maya2C-store/archive/Maya2C-wt-mra` | 60261777 | docs: mainnet readiness assessment against master 7bbf45f6 |
| 1166 | `cp/release-hub-sig` | `G:/Maya2C-store/archive/Maya2C-wt-rel` | 6cc8148b | ci: ship the Hub's updater files and check its bundled miner in CI |
| 1105 | `fix/dex-zero-price-pool` | `G:/Maya2C-store/archive/Maya2C-wt-px` | 209edb2c | wip: saved after an unexpected shutdown (unbuilt) |
| 853 | `fix/wallet-genesis-pin` | `G:/Maya2C-store/archive/Maya2C-wt-omega` | 4d93cc77 | fix(wallet): mainnet is a pinned genesis id, never a node's name |
| 651 | `(detached)` | `G:/Maya2C-store/archive/Maya2C-wt/attacknet-daily` | 79c3c388 | Merge pull request #144 from EricWijesinghe/feat/ticker-maya |
| 5 | `docs/bench-tps-post-383` | `D:/Temp/claude/wt-docs` | af5a5f23 | docs: proving restarted at 6fcfc65e after the harness genesis fix (#400) |
| 4 | `feat/sdk-rust-and-go-address` | `G:/Maya2C-store/archive/agent-worktrees/agent-af5abd24152a8f8c9` | a05bad9b | test(sdk): record the Rust suite and make the unreachable-host case honest |
| 4 | `feat/wallet-v2-clear-sign` | `G:/Maya2C-store/archive/agent-worktrees/agent-a8b9750279930e7b3` | 46724d66 | fix(wallet): security review of #446 (H1 look-alike window, M1 native contact dialogs, M2  |
| 3 | `checkout-fix` | `G:/Maya2C-store/archive/agent-worktrees/agent-af735decc54cf9339` | 3cb1e053 | fix(checkout): re-review N1-N8: frontier check, quoted amounts, bounded records |
| 3 | `chore/lint-debt-paydown-1` | `G:/Maya2C-store/archive/agent-worktrees/agent-a6c72e1f25bbbd6b0` | 431f0009 | chore(lint): lower clippy::pedantic ratchet 844 -> 828 |
| 3 | `docs/ecosystem-catalog` | `D:/Maya2C` | dc19de72 | docs(catalog): horizon ideas from science fiction, each tied to a need |
| 2 | `docs/competitor-matrix` | `G:/Maya2C-store/archive/agent-worktrees/agent-ac9ec5b4e96fa8ef8` | 4cbff211 | docs(ecosystem): fill competitor cells from primary sources where reachable |
| 2 | `docs/prior-art-horizon-2` | `G:/Maya2C-store/archive/agent-worktrees/agent-a96e61186414f9d42` | c69b2f56 | docs(prior-art): search five horizon ideas (2026-11-10) and link CATALOG section I |
| 2 | `docs/prior-art-horizon-3` | `G:/Maya2C-store/archive/agent-worktrees/agent-ad33ebf4f450d1621` | 82041979 | docs(prior-art): records for 11 horizon ideas, linked from CATALOG section I |
| 2 | `feat/address-text-leftovers` | `G:/Maya2C-store/archive/agent-worktrees/agent-a2103b21fe00cd173` | d89c6664 | feat(address): additive Bech32m text on the sdk-ffi signing key |
| 2 | `feat/light-client-finality` | `G:/Maya2C-store/archive/agent-worktrees/agent-a6fbd03ae2b40db35` | 2ea411e5 | feat: finality light client verifies DAG-BFT checkpoints (ADR-089 M0) |
| 2 | `test/gaps-bridge-spv` | `G:/Maya2C-store/archive/agent-worktrees/agent-ae65e620d79050663` | 735be7c2 | test: pin Core pow_tests vectors to a permalink, add two more, fix clippy |
| 2 | `verify/validator-security` | `D:/Maya2C/.claude/worktrees/wf_eced851b-236-1` | 56383237 | test(signer): red-team sealed keys and slashing-history import |
| 1 | `(detached)` | `G:/Maya2C-store/archive/Maya2C-wt-audit` | 6fcfc65e | fix(proving): mainnet genesis rules: no premine, bonds at the floor |
| 1 | `docs/chat-wallet-v2-adrs` | `G:/Maya2C-store/archive/agent-worktrees/agent-a17d916e5c5f0d349` | 1da57daf | docs: ADR-085 chat-v2 groups on MLS and ADR-086 wallet-v2 (Proposed) |
| 1 | `docs/prior-art-horizon-r0` | `G:/Maya2C-store/archive/agent-worktrees/agent-adb3c969c758377c4` | 92738ba2 | docs(prior-art): search six horizon ideas and link CATALOG section I |
| 1 | `feat/explorer-v2` | `G:/Maya2C-store/archive/agent-worktrees/agent-a81f58d64f1588da1` | 707ea52c | feat(explorer): address pages in Bech32m, mining page, inline SVG charts |
| 1 | `feat/finality-latency-bench` | `G:/Maya2C-store/archive/agent-worktrees/agent-aa0666e31148882b0` | c8318789 | feat: bench-finality, per-transaction submit-to-finalized latency (real run included) |
| 1 | `feat/hub-launch` | `G:/Maya2C-store/archive/agent-worktrees/agent-af705a254ae2c2874` | 7a5c0d4a | feat(hub): open Wallet and Chat from the Hub through OS link schemes |
| 1 | `feat/site-ecosystem-roadmap` | `G:/Maya2C-store/archive/agent-worktrees/agent-a9efdbc6a8503b051` | 53842edb | feat(site): ecosystem roadmap page generated from the catalog |
| 1 | `feat/site-ui-upgrade` | `G:/Maya2C-store/archive/agent-worktrees/agent-aac2f23a3a804e06c` | df9b3c2e | feat(site): audience hero, data-driven facts band, transaction explainer, AA contrast |
| 1 | `feat/testnet-kit` | `G:/Maya2C-store/archive/agent-worktrees/agent-a175db92c086574e9` | 3a6b2b3b | feat: testnet kit and node-in-a-box (xtask kit, xtask node) |
| 1 | `fix/attacknet-honesty` | `G:/Maya2C-store/archive/agent-worktrees/agent-a4d4480ed42bb5738` | 6200040f | fix: attacknet controller faults fail loudly instead of faking success |
| 1 | `fix/kani-tractable-256` | `G:/Maya2C-store/archive/agent-worktrees/agent-aa333303227e1525c` | 0b1ebdf2 | fix(kani): bound the 256-bit harnesses CBMC could not finish |
| 1 | `fix/pr500-review` | `D:/Maya2C/.claude/worktrees/agent-ae6178f3d40014aab` | 1a92e62b | wip: #500 review fixes H3/L1-L4 + M1 tests (genesis validate change not yet written; uncom |
| 1 | `fix/smart-account-recovery-defects` | `G:/Maya2C-store/archive/agent-worktrees/agent-a3d2242413c4dd630` | fec3c433 | fix: smart-account recovery modelled a weaker rule than ADR-056 (ADR-086 F1-F6) |
| 1 | `fix/tooling-silent-skips` | `D:/Maya2C/.claude/worktrees/wf_8e8ae6a1-ba6-54` | 4315c3c6 | fix(xtask,eo-fetch): gates fail on a page they cannot read; no panic on a short scene date |
| 1 | `proofs/amount-kani` | `G:/Maya2C-store/archive/agent-worktrees/agent-a859364b4ae726dad` | 0e35e03a | proof: run maya-amount's Kani harnesses in CI, add decimal and order proofs |
| 1 | `test/gaps-treasury-exec` | `G:/Maya2C-store/archive/agent-worktrees/agent-a54683edea8992153` | 61a2ca4e | test: close Group D gaps (treasury, parallel-exec, dos-guard, api-gateway, smart-account f |

Genesis-relevant branches named in the ledger (`g1`-`g4`: `fix/genesis-policy-guards`, `feat/committed-consensus-schedule`, `fix/tx-inputs-and-chain-scoped-net`, `feat/council-mldsa87-rotation`) are in section A and exist only on this PC under `G:/Maya2C-store/archive/Maya2C-wt-g1..g4`. This audit did not push them.

## B. Unpushed commits, branch has an open PR (9)

| PR | ahead | branch | worktree | HEAD |
|---|---|---|---|---|
| PR#449 | 1 | `feat/notary` | `G:/Maya2C-store/archive/agent-worktrees/agent-a2c9c52b2ac79aba8` | ab50345f |
| PR#418 | 1 | `feat/merchant-checkout` | `G:/Maya2C-store/archive/agent-worktrees/agent-a3fa1821dee0af07f` | 583a46a4 |
| PR#441 | 1 | `feat/chat-pay` | `G:/Maya2C-store/archive/agent-worktrees/agent-a67e056edafac2494` | d0ae1ccd |
| PR#447 | 1 | `postgen/account-authority` | `G:/Maya2C-store/archive/agent-worktrees/agent-a827b7094eb5ce81f` | 29a8a2db |
| PR#444 | 1 | `feat/status-v2` | `G:/Maya2C-store/archive/agent-worktrees/agent-a89edee4bccf0a708` | db468a29 |
| PR#433 | 1 | `feat/site-live-network` | `G:/Maya2C-store/archive/agent-worktrees/agent-aa3396ad5c52643f3` | e4898eaf |
| PR#519 | 4 | `feat/wallet-extension` | `G:/Maya2C-store/archive/agent-worktrees/agent-ada383a010b5bc7d5` | 5f09cb69 |
| PR#420 | 2 | `postgen/shielded-feature-bit` | `G:/Maya2C-store/archive/agent-worktrees/agent-ae57cd9d0e4d3724d` | 61046a9c |
| PR#348 | 1347 | `chore/z3-to-kani` | `G:/Maya2C-store/archive/Maya2C-wt-depy` | ed568098 |

Section B branches may hold local commits never pushed. The GitHub PR head can lag the worktree. Check before any cleanup. PR #348 (`chore/z3-to-kani`, 1347 ahead) and PR #519 (`feat/wallet-extension`, 4 ahead) both show this.

## C. Other items from this lane

- PR #441 `feat/chat-pay`: DIRTY. Master merge conflicts in 9 files (`apps/wallet-gui/core/src/payment.rs`, `core/src/wallet.rs`, `core/tests/payment_tests.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/deeplink.rs`, `ui/src/bridge.rs`, `ui/src/screens.rs`, `docs/architecture-vision.md`, `docs/ecosystem/CATALOG.md`). Both sides add fields and handlers to `PaymentRequest`, so the resolution is not mechanical. Left unresolved; nothing pushed.
- PR #460 `feat/wallet-undo-payee-check`: DIRTY. Master merge conflicts in `apps/wallet-gui/src-tauri/src/commands.rs` (1 hunk, changes the undo-hold clear) and `src-tauri/src/lib.rs` (handler list). Needs a human read; left unresolved; nothing pushed.
- PR #496 `feat/p30-intent-solver`: master merged and pushed as `ced5c150`. Only conflict was `docs/adr/README.md`; ADR-137, ADR-139 and ADR-140 rows all kept. No code touched.
- PR #436 `test/fuzz-app-parsers`: reported only, code owned by another lane. mergeStateStatus BLOCKED, not DIRTY.
- Main checkout `D:/Maya2C` (branch `docs/ecosystem-catalog`): 3 commits ahead of remotes. Not touched.
