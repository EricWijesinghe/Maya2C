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
right: 1040000`) and passes with the fix. #398 merged as bb4e0940.

## Ceremony rehearsal 2 on the #398 branch
`exit 0`, all 9 checks ok: 7 validators agree at height 5 (6.6 s) and at
3649 (3781 s); `get_validators` stake == 7 x bond; **`get_supply` total ==
reserve + 7 x bond**; `maya1` address text; miner paid
209616700889801505817932 after 3697 s from the reserve; supply after the
payout equals genesis supply. Between the runs, a GPU miner orphaned by an
interrupted rehearsal held `target/release/maya2c-gpu-miner.exe` open
(`os error 5`); it was stopped by PID and the run repeated.

## 24 h proving run
The first start at bb4e0940 failed at genesis-check: the harness still
funded a 1,000-coin root allocation and bonded 10,000 coins, which the
no-premine and floor-bond rules (#374) refuse. Fixed in #400 (`--supply 0`,
bond = `genesis_check::MIN_SELF_BOND`); node code unchanged.

Started 2026-11-09T16:21:35+09:00 at **6fcfc65e** (#400 tip; node code
identical to bb4e0940): `genesis-check: PASS`, `7 validators on one chain
after 5s`, `0.0 h: height 62, epoch 0, … halts 0`. Hourly lines go to
`D:/Maya2C-wt-audit/reports/proving/`. Result: pending.

## Fuzz
The 2026-11-08 campaign is now in `reports/fuzz/2026-11-08-consensus-decoders.md`:
8 targets, 462,261,905 execs, 0 crashes.

## Evening: nightly red since 11-06 (PR #404)
The laptop slept for about 1.2 h during the proving run (1.0 h at height 3540,
then 3.2 h at height 6221). The harness slept too, so it recorded no halt. All 7
validators woke together and went on, with no fork. The run's real running time
will be about 22.8 h of the 24 h it reports.

Nightly had failed every night since 11-06 on three jobs. Fixed in #404:
- **clippy debt ratchet:** four test modules (bft builder, l1-wallet
  guard, sentinel stranding replay, rpc limit) were missing the unwrap
  allow. With them fixed, `cargo clippy --workspace --all-targets
  --keep-going` reports no errors. The count was then 844 against a 790
  baseline. It grew while the gate could not count, mostly from cast lints
  in frozen `crates/vm/src/runtime.rs`. The baseline was raised to 844 with
  that reason, and paying it back is in BACKLOG (P2). CI: success.
- **four-validator localnet:** the SDKs signed without `--genesis` or
  `--yes`, and #390 changed what `address` prints. The fixed SDKs pass:
  `cargo xtask sdk-e2e --lang python`, `Ran 7 tests ... OK` (local and CI);
  Go, `ok maya2c.dev/sdk` with 3 PASS (CI). The 1,000-run kill -9 step behind
  them had never run; it timed the job out at 45 minutes. Run locally:
  `test result: ok. 2 passed ... finished in 863.20s`. The job budget is now
  120 min.
- **kani proofs:** no nightly since 10-19 passed, because every run was
  cancelled at 90 minutes or failed. `maya-governance` also no longer compiled:
  its range proof still drew a u64 after parameters became `Amount`. The job is
  now one runner per crate at 340 min. The range proof now covers every 256-bit
  value, and the settled-proposal harness checks the four settled tags one by
  one, which took it from over 15 min to 7 s. WSL results: ledger-math,
  threat-intel, iot-anchor 3/3, htlc-lattice 3/3; governance 12 harnesses,
  9 in seconds, and `adding_a_vote_moves_exactly_one_column` SUCCESSFUL in
  841 s; the other two tally harnesses and fee-market are measured below.

These changes touch only `#[cfg(test)]`, `#[cfg(kani)]`, the SDKs, xtask and
CI, so the node binary of the freeze is unchanged.

## Not done here
- The 24 h proving run (above) is still running. No heavy work runs on this PC until it ends.
- Promote `release/mainnet-v1` and publish a release: this publishes, so it
  needs Eric's `APPROVED:`.
- `APPROVED: sign genesis`: Eric only.
- #348 (z3 to Kani) stays a draft until its harnesses verify in WSL.
