# 2026-10-28 — DAG-BFT adversarial schedules, and the lost-fetch stall

Branch `feat/bft-adversarial-schedules`, commit `8c27c49f`. Omega brief item A1
(`docs/master-prompts/omega.md`).

## What was built

`crates/dag-bft/tests/adversarial_schedules.rs` runs four unmodified
`Validator`s against an adversary that owns the network and seat 3 (f = 1):

- Network: delivers in any order, drops, duplicates, starves one node, and
  advances any node's clock.
- Byzantine seat: equivocates to chosen subsets, harvests every vote sent to
  it, and broadcasts a certificate for any vertex that reaches a quorum. It
  cannot sign as an honest seat; in the node every vote is ML-DSA signed.

Checked on every schedule: agreement (committed sequences are prefixes),
no duplicate commit, one certified vertex per (round, author) across honest
DAGs. After the schedule, liveness after GST: the network turns reliable,
seat 3 crashes, and every honest node must commit twice more within 4,000
steps. The engine runs at the node's own pacing
(`resend_interval_ms = anchor_timeout_ms / 2`, `bins/maya2c-node/src/bft.rs:154`).

This is bounded random search, not a proof. A machine-checked model is A2.

## The checker was tested before it was trusted

Three deliberate engine mutations, each reverted afterwards:

| Mutation | Random search, 192 cases | Random search, 6,000 cases | Hand-built DAG test |
|---|---|---|---|
| Vote for every proposal (vote-once rule off) | caught: "two certified vertices for (1, 3)" | — | — |
| Skip indirect anchor commits | **missed** | caught after 258.8 s: "nodes 0 and 2 disagree" | caught: "X must reach A through B" |
| Direct commit on one link, not f + 1 | **missed** | — | caught: "A was committed on one link" |

The two misses are why the hand-built DAG tests exist and run on every PR.

## What it found: a real liveness bug

The first 10,000-case run failed on the unmodified engine, in liveness:

```
Test failed: no progress after GST: commits before [8, 8, 8, 0], after [8, 8, 8, 0]
node 0: round 5 committed-round 2 [r0:0123 r1:0123 r2:0123 r3:0123 r4:0123 r5:..2.]
node 1: round 5 committed-round 2 [r0:0123 r1:0123 r2:0123 r3:0123 r4:0123 r5:..2.]
node 2: round 5 committed-round 2 [r0:0123 r1:0123 r2:0123 r3:012. r4:012. r5:..2.]
```

Node 2 held seat 3's round-4 certificate in its buffer, waiting for the
round-3 parent. It had asked for that parent once. When the fetch or its
reply was lost, every re-delivery of the round-4 certificate returned at
`buffer.contains_key` (`validator.rs`, `on_cert`), and `accept` skipped
parents that were themselves buffered. So the parent was never asked for
again, node 2 never voted, and with seat 3 down nothing certified.

Regression test `a_lost_fetch_is_asked_for_again`, run before the fix:

```
test a_lost_fetch_is_asked_for_again ... FAILED
a re-delivery must fetch it again
```

Fix: `fetch_missing` re-requests the ancestors a buffered certificate waits
on, through buffered parents, on every re-delivery and on first buffering.

A second liveness failure that turned up after the fix was the checker's
own fault. The simulator's `resend_interval_ms = 0` re-broadcasts on every
tick, on top of an adversarial backlog of up to ~475 messages. It recovered
by itself within 40,000 steps. The checker now uses the node's pacing.

## Reviews

- **rust-reviewer:** approve. LOW items fixed: dedupe fetches, a comment on
  the insert-then-walk order, a transitive test
  (`a_lost_fetch_behind_a_buffered_parent_is_asked_for_again`), test tidying.
- **security-reviewer, HIGH:** the retry could amplify. One re-delivery
  walked the whole buffered chain, and every fetch is a gossip frame the
  whole network carries. Fixed: each digest fetched once, at most 2n fetches
  per call, and a re-delivery retries at most once per
  `resend_interval_ms`. Test: `replaying_a_buffered_certificate_is_throttled`
  (100 replays inside the interval give 0 fetches).
- **security-reviewer, MEDIUM, pre-existing:** `envelope.from` is not bound
  to the transport peer, so a fetch can be aimed at a victim. The cap bounds
  it now. Binding it is backlog work.
- **security-reviewer, pre-existing:** `driver.rs:443` logs a certificate to
  `certs.log` before the engine checks its signatures. That is the next PR.

## Real output

`cargo test -p maya-dag-bft --release` (all targets): 16 + 8 + 8 + 10 + 2
passed, 0 failed.

`MAYA_SCHEDULE_CASES=10000 cargo test -p maya-dag-bft --release --test adversarial_schedules`:

```
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 89.32s
real	2m2.841s
```

`cargo clippy -p maya-dag-bft --tests --release`: no warning in the changed
files (one pre-existing warning in `tests/auth_and_restart.rs`).

`cargo xtask coverage`: "features.toml: 164 register entries + 156
subsystems, all claims backed."

Node BFT tests (`bft_node_tests`, `bft_follower_tests`): **NOT VERIFIED
locally.** The first attempt used the main checkout's warm target
(`CARGO_TARGET_DIR=D:/Maya2C/target`) and failed with
`expected Amount, found u64` at `crates/node/src/genesis.rs:241`. Cargo had
reused a `maya-fee-market` artifact built from another worktree's u256
source: path crates in different worktrees share artifact slots in one
target directory, and freshness is judged by mtime. A cold build in this
worktree's own target was not run, with 20.6 GiB free on D:. The PR's CI
run is the evidence for these two tests.

## Found on the way: a shared target across worktrees is unsafe

The failure above means a test can pass against source other than the
source in front of you. The parallel session uses one shared target for all
of its worktrees and has been told. Rule going forward: one target directory
per worktree, or `cargo clean -p <crate>` for the touched crates before a run
whose result is claimed.
