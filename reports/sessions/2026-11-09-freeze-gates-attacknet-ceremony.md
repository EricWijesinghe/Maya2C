# 2026-11-09: open PRs landed, bench re-run, attacknet x10, freeze gates, ceremony rehearsal

One machine (Windows 11, 24 logical cores), localhost. Every figure below is
copied from a command's own output.

## Merged
- #390 Bech32m text in wallet CLI, dev CLI, pool. CodeQL alert 1222
  (`rust/cleartext-logging`, `bins/l1-wallet/src/main.rs:350`) dismissed as a
  false positive: it prints the public address and RPC balance.
- #384 OSS-Fuzz project definition.
- #395 post-genesis halt/risk/bridge records need no re-genesis. Its rustfmt
  failure was fixed here. Tests: `cargo test -p custom-l1-node --lib
  state::commitments`, `test result: ok. 6 passed; 0 failed`.
- #397 attacknet 2026-11-08 reports (two false FAILs, then PASS) and the
  pre-#383 bench-tps runs.
- The attacknet below-quorum fix (d1902641) was already on master inside the
  #394 squash, so it needed no separate PR.

## bench-tps at b6b5f536 (`cargo xtask bench-tps`, release binaries)
| mix | accepted | finalized | finalized/s |
|---|---:|---:|---:|
| 1000 senders x 4 | 4000 | 4000 | 413.3 |
| 3000 senders x 2 | 6000 | 6000 | 390.2 |
| 32 senders x 40 | 1280 | 1280 | 245.0 |

Reports: `reports/bench/tps-1794192095.md`, `tps-1794191941.md` and
`tps-1794191977.md`. At 6029a4ba, before #383, the same command gave 31.3/s
with 3013 of 6000 transfers finalized.

## Weighted attacknet x10 at b6b5f536
`cargo xtask attacknet --weighted --rounds 10` gave **PASS**, with no fork
anywhere through height 3482 (`reports/attacknet/2026-11-09-1.md`). Every
round's "restart ahead (untrusted)" step answers the gate-10 question left
open by #382: a validator restarted behind a lagging peer refused that peer
and rejoined, in 2.0 to 2.1 s in rounds 8 to 10.

## Freeze gates at 8e764b6f (`/d/Temp/freeze/gates.sh`)
| gate | result |
|---|---|
| `cargo xtask coverage --verify-targets` | exit 0; 164 register entries + 163 subsystems, all claims backed |
| `cargo xtask spec-coverage` | exit 0; 62 rules, 164 vectors; 59 consensus rules covered, 0 gaps |
| `cargo xtask claims-check` | exit 0 |
| `cargo xtask release-check` | `release-check: PASS` |
| `cargo deny check` | advisories ok, bans ok, licenses ok, sources ok |
| `cargo nextest run --workspace` | `4016 tests run: 4016 passed (14 slow), 14 skipped` |

The gate script was fixed before this run. On 6029a4ba the nextest line was
filtered with `rg -E`, which is ripgrep's *encoding* flag, so no summary was
printed. Each pipeline also exited with `tee`'s status, so a failure could not
show. It now uses `rg -e` and `bash -o pipefail`. The 6029a4ba nextest result
is therefore NOT VERIFIED; this run replaces it.

From b6b5f536 to 8e764b6f only `crates/node/src/state/commitments.rs` changed
(#395, test code) plus reports, so the bench and attacknet results above
apply to the frozen tree.

## Ceremony rehearsal 1 at 8e764b6f (`cargo xtask ceremony-rehearsal`)
8 checks ok, 1 FAIL:
- [ok] 7 validators agree at height 5, 6.6 s after start; still agree at
  height 3693 (3814 s)
- [ok] chain id `maya-mainnet`; reserve address text starts with `maya1`
- [ok] accounts + staked == reserve + 7 x 1,000-coin bond
- [ok] a miner was paid 209616700889801505817932 base units from the reserve
  after 3805 s; supply after the payout equals genesis supply, so nothing was
  minted
- **[FAIL] `get_supply` total leaves out staked funds.** This is the gap
  recorded in the 2026-11-08 dry run, and it was left failing on purpose.

**Fixed:** #398 makes `get_supply` total add `Staking::held()` (bonds,
delegations, unbonding). This is RPC only; consensus and state are unchanged.
Test `get_supply_counts_what_staking_holds` fails on master (`left: 1000000
right: 1040000`) and passes with the fix. Rehearsal 2 runs on the #398
branch; its result is below.

## Fuzz
The 2026-11-08 campaign is now in `reports/fuzz/2026-11-08-consensus-decoders.md`:
8 targets, 462,261,905 execs, 0 crashes.

## Not done here
- 24 h proving run on the final freeze sha. It starts after #398 merges, and
  no heavy work may run during it.
- Promote `release/mainnet-v1` and publish a release: this publishes, so it
  needs Eric's `APPROVED:`.
- `APPROVED: sign genesis`: Eric only.
- #348 (z3 to Kani) stays a draft until its harnesses verify in WSL.
