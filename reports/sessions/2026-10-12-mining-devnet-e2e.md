# 2026-10-12 — Mining lane end to end on a local devnet

First run of the real binaries with mining switched on: one `maya2c-node`
validator plus `maya2c-gpu-miner --mine`, both from `target/debug` on top of
`655248b8`. It found four bugs. All four are fixed, and the run was repeated.

## Setup

- Work dir `D:/Temp/maya-mining-devnet`. Single validator committee, RPC `127.0.0.1:32500`.
- Genesis: `bft.staking.epoch_blocks = 20`, fees on (base fee 1), `bft.mining`:

| Field | Value |
|---|---|
| `initial_epoch_reward` | 1,000,000 |
| `halving_epochs` | 100 |
| `miner_share_bps` | 7,000 |
| `target_shares` | 10 |
| `max_shares_per_block` | 2 |
| `initial_target` | `00ff…ff` |
| `dag` | `testing` |

- `MINING_RESERVE` was funded with 1,000,000,000. Per share: 700,000 / 10 = 70,000. The validators' part per epoch is 300,000.
- Miner: `--mine --json --threads 4`, CPU light mode, `timeout 180`. Hardware: laptop with an NVIDIA GeForce RTX 5070 Laptop GPU. The GPU was not used; this run was CPU only. Last reported rate: 509 H/s.

## Run 1 (before fixes, 120 s)

```
get_balance(miner)   balance 700000  nonce 32
get_balance(reserve) balance 997200000
get_chain_info       height 147
accepted (pool)      145     refused 50 ("wrong epoch")
balance by epoch     700000 reached in epoch 1, then flat through epoch 6
```

The miner was paid for epoch 1 only. Two causes:

1. **Consensus.** `apply_mining_share` applied a share after the epoch's miner budget was spent. It paid 0, bumped the nonce and took a block slot fee-free. 22 of the 32 landed shares paid nothing.
2. **Miner.** Shares mined for an epoch that had already closed were evicted from the pool, but the miner had already handed them tx nonces. That gap stalled every later share forever.

## Fixes

- `crates/node/src/state/mining_exec.rs`:
  - Refuse a share when `share_reward() == 0` ("this epoch's miner budget is spent").
  - Refuse when the reserve pays 0 ("the mining reserve is empty").
  - The mempool uses the same rule, so it now refuses these shares too.
  - New test: `a_share_past_the_epoch_budget_is_refused_by_pool_and_block`. It failed before the fix (pool admitted the share, line 254) and passes after.
- `bins/maya2c-gpu-miner/src/share.rs`:
  - The nonce counter resets at each new epoch's work.
  - The miner idles while `share_reward == 0`.
  - It refreshes work right after a refusal and also stops when the same header pays nothing; the refresh interval went from 10 s to 3 s.
  - `dag` is parsed only for active work. Before mining opened, the RPC's empty `dag` crashed the work parse.
- `apps/hub`:
  - "Earned" is now the growth of the payout account's on-chain balance. Before, it summed accepted shares, which in run 1 was 145 accepted against 10 paid, about 14× too high.
  - New test `earned_is_on_chain_balance_growth_not_accepted_shares`.
  - The feed now says "pays up to … when it lands".

## Run 3 (all fixes, fresh genesis, 180 s, miner started before mining opened)

```
get_balance(miner)   balance 5600000  nonce 80
get_balance(reserve) balance 992000000
get_chain_info       height 171
events: 2 idle "mining opens at the first epoch boundary"
        28 idle "this epoch's miner budget is spent; waiting for the next epoch"
        124 share accepted by pool, 6 refused "this epoch's miner budget is spent"
```

Reconciliation:
- Miner: 80 landed shares × 70,000 = 5,600,000, equal to the balance. Every landed share paid.
- Reserve: 1,000,000,000 − 992,000,000 = 8,000,000 = 5,600,000 miner + 8 boundaries (heights 20…160) × 300,000 validators. Nothing was minted.

Run 2 (miner fixes 1–2 only) was 78 × 70,000 = 5,460,000, which also reconciled. It surfaced the two remaining miner bugs: 225 budget-spent refusals from same-header work, and the `dag` parse error.

## Checks

```
cargo test -p custom-l1-node --test mining_rewards_tests   11 passed; 0 failed
cargo test -p custom-l1-node --lib mining                    8 passed; 0 failed
apps/hub/src-tauri: cargo test                               9 passed; 0 failed
cargo clippy -p custom-l1-node --all-targets (CI flags)      Finished, no warnings
cargo clippy -p maya2c-gpu-miner --all-targets (CI flags)    Finished, no warnings
apps/hub/src-tauri: cargo clippy --all-targets -D warnings   Finished, no warnings
cargo fmt --check (node, miner, hub)                         clean
```

## Not verified

- **Hub window against this devnet.** NOT VERIFIED: the GUI was not driven. Only the miner events it consumes were exercised, and its parsing is covered by unit tests.
- **GPU hashing.** NOT VERIFIED: it does not exist yet; the miner is CPU light mode only.
- **Multi-validator mining.** NOT VERIFIED: this was a committee of 1.

## Review follow-up: retarget

A security review of the budget refusal found that, with overshoot refused,
`shares` could no longer exceed `target_shares` and the retarget could only
ease. Fixed with `MiningRecord::spent_after` (ADR-044, "Changes from the first
devnet run"). Tests: `a_budget_spent_early_hardens_the_target_by_how_early` and
`a_spent_after_past_the_epoch_is_refused_on_decode` (unit), and
`a_budget_drained_early_hardens_the_next_epochs_target` (integration). The
latter failed to compile before the field existed, and passes now.

```
cargo test -p custom-l1-node --test mining_rewards_tests   12 passed; 0 failed
cargo test -p custom-l1-node --lib mining                    10 passed; 0 failed
cargo clippy -p custom-l1-node --all-targets (CI flags)      Finished, no warnings
```

## Run 4: retarget on the real node, shipped miner (150 s)

The node was rebuilt with `spent_after` (`dd050e13`). The miner was the one
`cargo xtask hub-bundle` ships (`apps/hub/src-tauri/target/release/maya2c-gpu-miner.exe`,
release, 4 threads): 7,741 H/s, against 509 H/s for the debug build.

```
share target by epoch (first 4 bytes):
  1 00ffffff  2 00e8ba2e  3 004685fe  4 001c3599
  5 000e1acc  6 00070d66  7 00066945  8 0005d410
shares: 219 accepted by pool, 7 refused
get_balance(miner)  balance 5110000  nonce 73   (73 x 70,000 = 5,110,000)
```

The target hardens while the budget drains early, and the steps shrink as
demand meets the target, which is the convergence ADR-044 intends. On the
pre-fix node (run 3's build), the same miner left the target at the genesis
floor through epoch 95.

## Windows installers

`cargo xtask hub-bundle` (exit 0):

```
Finished `release` profile [optimized] target(s) in 4m 19s     (miner)
Finished `release` profile [optimized] target(s) in 2m 48s     (hub)
 6377472 bytes  apps/hub/src-tauri/target/release/bundle/msi/Maya2C Hub_0.1.0_x64_en-US.msi
 4333942 bytes  apps/hub/src-tauri/target/release/bundle/nsis/Maya2C Hub_0.1.0_x64-setup.exe
```

The installers are unsigned, so Windows SmartScreen will warn on first run.
The Hub window from the installer was NOT VERIFIED, because the GUI was not driven.

## Linux packages (WSL Ubuntu 24.04)

The Tauri Linux deps were installed in WSL. Then, from `/mnt/d/Maya2C`:
`CARGO_TARGET_DIR=/root/maya-target cargo xtask hub-bundle` (exit 0).

```
Finished `release` profile [optimized] target(s) in 8m 33s     (miner)
Finished `release` profile [optimized] target(s) in 1m 50s     (hub)
 6680725 bytes  Maya2C Hub-0.1.0-1.x86_64.rpm
85015032 bytes  Maya2C Hub_0.1.0_amd64.AppImage
 6678462 bytes  Maya2C Hub_0.1.0_amd64.deb
dpkg -c: usr/bin/maya-hub, usr/bin/maya2c-gpu-miner   (miner beside the hub, where it looks)
```

The Linux miner was run against `https://rpc.maya2c.dev/rpc`:
- `start` event, then `get_mining_work: "method not available through the gateway"`.
- The api-gateway's default-deny allowlist did not include the method.
- Fixed in `7a9a24b4`. The address is validated at the gateway, and the tests failed first and then passed.
- Live only once the gateway and node are redeployed. They were not touched.

## Android

- `cargo tauri android build --apk --debug` built with the JDK at `C:\Program Files\Java\jdk-21.0.12`. The system `JAVA_HOME` names a removed 21.0.11.
- The x86_64 APK was installed on the `salon_avd` emulator (API 34), started `-read-only`. It showed the live public testnet with all four tabs.
- Screenshots: session scratchpad only, not committed.

## GPU mining (RTX 5070 Laptop)

Parity on the device: `cargo test --release -p maya-wgpu-miner --features gpu`
reports 10 passed, including `the_gpu_digest_matches_the_nodes`. The debug build
of the same tests ran over 20 minutes without finishing, so it was stopped; use
release.

`cargo xtask mining-localnet --gpu --seconds 150` (new; exit 0):

```
mining-localnet: 4 validators on one chain
mining-localnet: first share paid 32.1232511s after the miner started
mining-localnet: validator 3 down; shares kept paying (4.0134866s)
mining-localnet: miner balance 3150000 on all four nodes
mining-localnet: reserve paid out 4650000
mining-localnet: shares sent 323, refused 4, last rate 714426 H/s
mining-localnet: targets by epoch 1:00ffffff 2:00ffffff 3:00745d17 4:002e8ba2 5:0019dbcc
mining-localnet: PASS in 150.1576833s
```

Reconciliation: the reserve paid out 4,650,000 = 3,150,000 to the miner
(45 × 70,000) + 5 boundaries × 300,000 to the committee. The rate is for the
4 MiB test dataset; a mainnet-dataset (4 GiB) rate is MISSING.

## Incident: the live seed was stopped four times by this session

Stopping the local devnet with `taskkill /IM maya2c-node.exe` also killed the
live maya-testnet-1 seed, which runs under the same image name. Watchdog
restarts are logged at 00:49, 00:54, 01:24 and 02:04 KST. The seed was down
for up to 5 minutes each time. The chain kept finalizing, and the validators
v2..v4 run as `maya2c-validator.exe` and were not hit. Separately, the
committee now shows 3 (v4 `validator: null`). The checkpoints that would date
that change have been pruned, so the cause is NOT ESTABLISHED.

The first `mining-localnet` run was also aborted before it started nodes:
localnet's ports (p2p 31100) collide with the live seed. Fixed:
`localnet::ports_free()` refuses occupied ports, and `mining-localnet` uses
p2p 33100+ and rpc 34000+. Test processes are now killed by PID only.
