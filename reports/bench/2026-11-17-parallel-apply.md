# parallel_apply: block application with the parallel verify phase off and on (v3 P1)

- commit measured: `28e7207c` (branch `feat/v3-parallel-exec`, base `14bbd2f8` on `postgen/v3`)
- machine: Intel Core Ultra 9 275HX, 24 logical cores, 31.4 GiB RAM, Windows 11 Pro Insider Preview build 29708
- toolchain: rustc 1.99.0-nightly (da80ed070 2026-07-14)
- command: `cargo bench -p custom-l1-node --bench parallel_apply --profile dev`
- profile: **dev**, not release. Dependencies (ML-DSA, SLH-DSA, BLAKE3, RocksDB) are `opt-level = 3` through `[profile.dev.package."*"]`; the node crate itself is unoptimized. A release run (fat LTO) was not made: NOT VERIFIED.
- mix: 512 single-output hybrid (ML-DSA-65 + SLH-DSA-SHA2-128s) transfers, 32 senders x 16 nonces, 2 blocks of 256, round-robin by nonce. Signing excluded. Only `apply_block` is timed.
- reps: 5, off and on alternating, each on a fresh RocksDB in a temp dir. Shared machine: other agents' builds were running.
- every rep asserted the off and on roots equal.

## Output

```
parallel_apply: 512 hybrid transfers, 256/block, 5 reps, 24 cores, debug-assertions ON (not an optimized build)
  cold rep 0: off   210.755 ms  on    56.977 ms
  cold rep 1: off   243.798 ms  on    53.025 ms
  cold rep 2: off   209.328 ms  on    41.267 ms
  cold rep 3: off   215.800 ms  on    47.755 ms
  cold rep 4: off   211.252 ms  on    44.856 ms
cold: median off   211.252 ms (2424 tx/s)  on    47.755 ms (10721 tx/s)  off/on x4.42
  warm rep 0: off    11.332 ms  on    12.018 ms
  warm rep 1: off    11.339 ms  on    14.374 ms
  warm rep 2: off    10.032 ms  on    11.386 ms
  warm rep 3: off    10.832 ms  on    11.189 ms
  warm rep 4: off     8.771 ms  on    10.484 ms
warm: median off    10.832 ms (47270 tx/s)  on    11.386 ms (44969 tx/s)  off/on x0.95
warm: NO GAIN measured: the parallel phase was not faster here
```

## What it says

- **Cold cache** (a node applying blocks whose transactions it never verified:
  replay, catch-up after downtime, a fresh sync): x4.42 faster apply on this
  machine at 256 transactions per block. `prewarm_verification` caps threads at
  one per 32 transactions, so a 256-transaction block uses 8 of the 24 cores.
- **Warm cache** (the steady state of a DAG-BFT validator, whose mempool and
  builder already verified every transaction): **no gain**, x0.95. The phase
  hashes every transaction's wire bytes again to look it up and spawns threads
  for nothing.
- Decision: the phase stays dark (`PARALLEL_APPLY_ACTIVATION_HEIGHT = u64::MAX`).
  A gain exists only on the catch-up path, and a validator in steady state would
  pay a small cost. Not tuned toward either number.

## Not measured

- Release (LTO) build; separate machines; blocks other than plain transfers;
  replayed testnet history through both paths.
- Stateful parallel execution: not built (ADR-146).
