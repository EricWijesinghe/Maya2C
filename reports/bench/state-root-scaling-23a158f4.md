# state_root cost against account count (ADR-062 measurement)

- commit: 23a158f4 (master) plus the example `crates/node/examples/state_root_scaling.rs`
- os: windows 11 (Microsoft Windows 10.0.29680), 24 logical cores, other processes running
- command: `cargo run --profile perf -p custom-l1-node --example state_root_scaling`
- what is timed: the whole `StateDB::state_root()` call on a warm RocksDB (read every account, hash every leaf, build the dense tree); median of 5 runs after one untimed run

| accounts | median per root | min | max |
|---:|---:|---:|---:|
| 1,000 | 0.88 ms | 0.77 | 1.03 |
| 10,000 | 6.82 ms | 6.36 | 7.10 |
| 100,000 | 72.47 ms | 66.62 | 79.56 |
| 300,000 | 206.52 ms | 166.52 | 211.61 |

Reading: the cost is linear in accounts, about 0.7 microseconds per account, as the code
says. Nothing here is a surprise; it is now a number.

What it supports: every validator pays this once per block, so the dense tree costs
about 70 ms a block at 100,000 accounts and about 200 ms at 300,000. Against a 15 s
PoW block that is under 2%; against a DAG-BFT block in the 1 to 2 second range
(the localnet `bench-tps` produced 8 blocks in 12 s) it is 10% at 300,000 accounts,
and a million accounts would be over 600 ms by extrapolation, which is the part this
run did not measure.

What it does not show: a block-apply time (signatures and execution are not in it); that
the sparse tree is faster (that path recomputes from the same merged map, so only an
incremental update, which is not built, would change the curve); behaviour past 300,000
accounts; a cold cache; any other machine. One run per size set; no repeat on another day.
