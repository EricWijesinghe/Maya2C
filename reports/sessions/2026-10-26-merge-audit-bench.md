# 2026-10-26/27: merge freeze fixes, audit uncovered areas, measure TPS

## Merged to master

#196 (freeze blockers), #170 (offender unbond), #198 (test-scope unwrap allow),
dependabot #197 #193 #181 #180 #179 #194 #138 #165. #172 closed (superseded by
#194), #164 closed by dependabot.

CI notes: #196's CodeQL gate failed on 7 "hard-coded nonce" alerts, all in test
files (`payload_tests`, `rwa_tests`, `settlement_tests`); dismissed as "used in
tests", the same class as the 23 dismissed on 2026-10-12. #170's `nextest (core)`
failed once in `latency_sim_tests` and then hung 58 min; attempt 3 passed.

## Open PRs from this session

| PR | What |
|---|---|
| #199 | latency sim: publish a distinct block per retry (mesh race) |
| #200 | audit fixes (identity index, vault selection, storage errors, preview key, import_headers, DEX checked_sub, zeroize bound) |
| #201 | BFT wire: vertex payload cap = `MAX_TX_BYTES` (was 1 MiB) |

## Real command output

- `cargo check --workspace --all-targets` on master: Finished, no errors.
- `cargo nextest run --workspace --no-fail-fast` on master: 3386 run, 3382 passed,
  4 failed. The 4 (`design_tests` x2, `drbg_cavp_tests` x2) failed because the
  cached test binaries had `D:\Maya2C-wt-sweep` baked in via `env!("CARGO_MANIFEST_DIR")`;
  after touching the crates, `nextest -p maya-design-system -p maya-entropy`: 40/40.
- `cargo xtask coverage --verify-targets`: 164 register entries + 156 subsystems,
  all claims backed.
- Clippy, CI flags (`-A pedantic -A unwrap_used -D warnings`): clean. Plain
  `--all-targets` had errors in 6 test modules missing the `unwrap_used` opt-out
  (#198 fixed them); after it, zero errors.
- `lint_debt.sh --check`: ok, 774 below baseline.
- On the #200 branch: `nextest -p custom-l1-node -p maya-dex`: 1240 passed, 4 skipped.
- On the #201 branch: wire tests 11/11, `bft_node_tests` + `bft_follower_tests` 17/17.
- Latency sim after the rewrite: 3/3, six consecutive runs (~25 s each).

## bench-tps (the launch-gate number)

`cargo xtask bench-tps --senders 3000 --txs 2`, `MAYA2C_BIN_DIR=target/perf`,
commit `23a158f4`, Windows 11, 24 logical cores, 4 validators on one machine over
localhost, other processes running, signing excluded:

- 6000 of 6000 finalized and visible on a replica, 11.95 s first-submit to
  last-visible over 8 blocks: **502.2 finalized transfers/s**.
- The earlier report (`reports/bench/tps-1792866749.md`, commit `18a5d585`):
  362.5/s over 7 blocks.
- Single runs. The difference between them is not attributed to any change.
- Not measured: contract calls, mixed loads, WAN latency, separate machines.

## Audit

`reports/sessions/2026-10-26-audit-findings.md`.

## Not done

- The re-genesis itself, the explorer/status redeploy, sealing live keys, tagging
  `node-v0.1.0-testnet.5` (draft only, `docs/releases/`), all held for Eric.
- H-1, H-2, timestamp steering, retarget off-by-one: need Eric's decisions.
- PoW under the chain mutex: needs a verify-before-lock split.
- Hub GUI, chat outbox/receipts, wallet send-screen check (#135).
- Open PRs not on this session's list: #195 (genesis ceremony amounts), #163,
  #160.
- A live-history scan for multisig spends with spare approvals.
