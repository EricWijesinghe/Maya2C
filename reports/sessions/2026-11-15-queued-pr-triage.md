# Queued PR triage, 2026-11-15

Base: master 2090300c (#520 merged: libp2p-gossipsub 0.50.1, RUSTSEC-2026-0534).
Scope: the 20 queued non-draft PRs in D:/Temp/claude-locks/merge-queue.txt. Re-freeze PRs (#500 #506 #522 #540 #542 #544 #549) and postgen bases were not touched.
Method: `gh pr update-branch` (merge of master into the PR branch, no force push), at most 4 in flight, poll with `gh pr checks`.

## Table

| PR | Branch | Behind master (before) | update-branch | Required checks now (cargo-deny, rustfmt, nextest core, clippy) | Remaining real failure / root cause |
|---|---|---|---|---|---|
| #471 | docs/sealed-mempool-and-workclaim | 29 | done | all pass (43 pass, 1 skip) | none |
| #410 | feat/address-text-leftovers | 2 | done | all pass (46 pass, 1 skip) | none |
| #512 | docs/adr-140-chat-expression | 11 | done | pending at last poll (40 pass, 3 pending) | not yet known |
| #486 | feat/w1a-trust-apps | 11 | done | pending at last poll (43 pass, 1 pending: nextest core) | not yet known |
| #468 | docs/adr-110-115-gaps | 11 | done | pending at last poll (1 pass, 28 pending) | not yet known |
| #469 | docs/adr-117-124-gaps | 11 | done | pending at last poll (29 pending) | not yet known |
| #470 | docs/adr-125-130-gaps | 11 | not updated (time) | failing before update: cargo-deny | cargo-deny: RUSTSEC-2026-0534 on the old base; fixed by #520, so the update should clear it |
| #472 | docs/adr-131-134-options | 11 | not updated (time) | failing before update: cargo-deny | same as #470 |
| #477 | docs/adr-135-v4-umbrella | 11 | not updated (time) | failing before update: cargo-deny | same as #470 |
| #480 | feat/w1b-monitor-ledger | 11 | not updated (time) | failing before update: cargo-deny, clippy, nextest (core) | cargo-deny as #470. clippy and nextest not diagnosed |
| #481 | feat/gateway-webhooks | 11 | not updated (time) | failing before update: cargo-deny, advisories and licences, clippy, nextest (core) | cargo-deny as #470. clippy and nextest not diagnosed |
| #453 | feat/netfault-relay | 11 | not updated (time) | failing before update: cargo-deny, supply chain (cargo vet, report-only) | cargo-deny as #470. vet is report-only, not a required check |
| #454 | feat/audit-supply | 11 | not updated (time) | failing before update: cargo-deny, clippy | cargo-deny as #470. clippy not diagnosed |
| #449 | feat/notary | 11 | not updated (time) | failing before update: cargo-deny, clippy, nextest (core) | cargo-deny as #470. clippy and nextest not diagnosed |
| #444 | feat/status-v2 | 11 | not updated (time) | failing before update: cargo-deny | cargo-deny as #470 |
| #433 | feat/site-live-network | 2 | not updated (time) | failing before update: cargo-deny, clippy, nextest (core) | cargo-deny as #470. clippy and nextest not diagnosed |
| #497 | feat/hw-entropy-health | 2 | not updated (time) | failing before update: cargo-deny, clippy, nextest (core) | clippy failure not read from #497's own log; the failing log checked was #539's (see its row). nextest not diagnosed |
| #441 | feat/chat-pay | 5 | not attempted: DIRTY | mergeStateStatus DIRTY, conflicts with master | merge conflict. Not resolved here (per lane rules). Needs a rebase by its owner |
| #539 | feat/github-presence | 2 | not updated (time) | failing before update: cargo-deny, rustfmt, nextest (core), clippy | clippy: `needless_pass_by_value` in error mappers (run 43490659952, lines `fn http(error: reqwest::Error)`, `fn lattice(error: maya_htlc_lattice::Error)`, `fn shape(error: maya_identity::Error)`). rustfmt and nextest not diagnosed |
| #553 | fix/site-integrity-links | 0 | not needed (already on master) | no failing checks listed at last read; mergeStateStatus BLOCKED | not diagnosed |

## Root causes found in this session

1. cargo-deny on older PR bases: `error[vulnerability]: Unbounded backoff state from PRUNE messages for unknown topics` (RUSTSEC-2026-0534, libp2p-gossipsub). Run 43378870790, branch docs/adr-110-115-gaps (base before #520). Fixed on master by #520. Updating each PR to master clears this. Verified on the updated PRs: #471 and #410 pass cargo-deny (earlier poll). #512 (run 43621418063, job 132529668356) and #486 (run 43621422419, job 132536490100) pass cargo-deny at the last poll. #468 and #469: cargo-deny state not recorded (no matching check line at the last poll).
2. The toolchain-install error in the same log (`override toolchain 'nightly-2026-07-15-x86_64-unknown-linux-musl' is not installed`) is printed but cargo-deny still ran to its own verdict, so it is not the failure cause. Advisory was.
3. clippy on #539: error-mapper functions take their error by value (`needless_pass_by_value`), read from run 43490659952. #497's clippy failure was not read from its own log; it is suspected to be the same lint, unverified. The fix belongs to the PR owner, not this triage.

## Not verified

- Required-check results for #512, #486, #468, #469: still pending at the last poll. Their final state is not recorded here.
- Remaining PRs not updated because of the time budget (about 3 hours): #470, #472, #477, #480, #481, #453, #454, #449, #444, #433, #497, #539. Their cargo-deny failures are expected to clear with the update, but this is a prediction, not a result.
- nextest and clippy causes for #480, #481, #454, #449, #433 and #497 were not read from logs.
- Local cargo was not run (CI is the compiler for this lane).
