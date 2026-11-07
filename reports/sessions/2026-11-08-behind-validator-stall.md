# 2026-11-08 — a validator opened behind the tip: stall and rejoin

Base: `fix/follower-serves-held-checkpoint` at `d3056276` (PR #381).
Toolchain `rustc 1.99.0-nightly (da80ed070 2026-07-14)`, Windows 11 Pro
10.0.29680, `CARGO_BUILD_JOBS=4`.

## The finding

`bft_node_tests::a_validator_opened_behind_the_tip_does_not_stall_the_builders`
(ignored) opened validator 2's engine a few blocks behind the network after
a long outage. All three builders sat at height 105 for 30 intervals.

## Root cause: two problems, one in the harness and one in the engine

### 1. The stall: a harness artifact

The mesh ran the engine with `Params::default()`'s `resend_interval_ms: 0`.
That is the simulator's value. `maya2c-node` derives `anchor_timeout_ms / 2`,
which is 500 ms on every genesis in the tree (`bins/maya2c-node/src/bft.rs:306`).
At 0, two rate limits switch off:

- a lagging engine re-fetches a buffered certificate's ancestors on every
  duplicate delivery (`on_cert`, `last_refetch_ms`);
- peers answer fetches with no budget (`on_fetch`, `fetch_replies`).

Each reply that is itself still waiting on parents triggers more fetches. The
mesh's single FIFO filled with validator 2's `Fetch` frames, and the builders'
proposals and votes queued behind them. Trace added to the test (removed
afterwards), queue length and frames by `(sender, kind)` after each interval:

```
TRACE q1825  h105 r252 c250 | h105 r252 c250 | h102 r242 c242 | h105 r252 c250 {(2, "F"): 1644, (3, "C"): 181}
TRACE q5457  h105 r252 c250 | ... {(2, "F"): 4896, (3, "C"): 559, ...}
TRACE q16338 h105 r253 c250 | ... {(2, "F"): 14580, ...}
TRACE q19830 h105 r253 c250 | ... {(2, "F"): 15534, (0, "C"): 4230, ...}
```

Changing only `resend_interval_ms` to 500 in `Mesh::new`, the same test
passed: builders went from 105 to 1172 and validator 2's engine reached their
round.

```
TRACE q0  h105 r252 c250 | h105 r252 c250 | h102 r242 c242 | h105 r252 c250 {}
TRACE q5  h151 r349 c346 | h151 r349 c346 | h108 r349 c346 | h151 r349 c346
TRACE q6  h1172 r2390 c2388 | h1172 r2390 c2388 | h1125 r2390 c2388 | h1172 r2390 c2388
test result: ok. 1 passed; 0 failed; ... finished in 191.44s
```

The node binary does not hit this with any genesis in the tree. **Latent,
not fixed:** a genesis with `anchor_timeout_ms` of 0 or 1 gives the binary a
`resend_interval_ms` of 0, and `GenesisConfig::validate` does not refuse it.

### 2. A behind engine that never rejoins: a real engine bug

The new 50+ block variant opens the engine *before* the import, as
`maya2c-node` does at startup. Validator 2 is 61 blocks behind, live for 8
intervals, then imports and follows. The builders kept going, but its engine
stayed at the resume round for good, with an empty queue:

```
TRACE q0 h111 r268 c266 ffalse | h111 r268 c266 ffalse | h108 r258 c258 ftrue | h111 r268 c266 ffalse
...
TRACE q0 h132 r324 c322 ffalse | h132 r324 c322 ffalse | h129 r258 c258 ftrue | h132 r324 c322 ffalse
its engine trails by 66 rounds
```

While it was live and behind, the engine buffered the network's current
certificates, waiting on parents that nobody holds any more. The import then
called `resume_after(258)`, which moved the DAG horizon to 258. That made
those buffered certificates' parents count as held (`missing_parents`
exempts rounds at or below the horizon). They stayed buffered anyway, because
`drain_buffer` only ran after a successful `accept`, and none could happen:

- a re-delivery of a buffered certificate only re-fetches (`on_cert`);
- a new child of one walks into the buffer and finds nothing to fetch
  (`fetch_missing`), so it is buffered too.

The engine never held a quorum at round 258, so it never proposed again. The
same gap exists, less likely, when a commit moves the horizon in `progress`.

The binary opens the engine before `catch_up` and processes frames in
between. So a validator restarted far behind can reach this state on a real
network. It costs that validator's vote, which is the gate-10 shape when
more than one validator is affected.

## The fix

`crates/dag-bft/src/validator.rs`: a `drain_due` flag. It is set wherever
the horizon moves (`resume_after`, and the GC after a commit in
`progress`), and `progress` drains the buffer when it is set. The drain only
inserts certificates that were already signature-checked before they were
buffered, still through `has_parent_quorum` and `Dag::insert_digested`. They
are reported in `Output::inserted`, so the driver logs them to `certs.log` as
before. Nothing is signed, so no slot can be signed twice, and the safety log
is untouched.

`crates/node/tests/bft_node_tests.rs`: the mesh uses the node's
`resend_interval_ms` (500). The ignored test is un-ignored and joined by a
60-block variant. Both assert that the builders keep finalizing, that
validator 2 keeps importing and agrees with the network, that its engine
trails member 0 by at most 4 rounds, and that there are no equivocations.
The 60-block variant also asserts that the builders advanced while the
engine was live and behind.

Control run, `resend_interval_ms` back to 0 with the engine fix in place.
Both tests fail, so the harness parameter alone caused the stall:

```
an engine 61 blocks behind stalled the builders at 105
the network kept going: 105 -> 105, rejoined 102
test result: FAILED. 0 passed; 2 failed; ... finished in 17.18s
```

Engine unit test, written first and failing before the fix:

```
a_certificate_buffered_before_a_resume_enters_once_the_horizon_reaches_it
  panicked: a buffered certificate at the new horizon stayed out of the DAG
```

## Results after the fix

`cargo test -p maya-dag-bft`:
```
test result: ok. 20 passed; 0 failed
test result: ok. 10 passed; 0 failed
test result: ok. 9 passed; 0 failed
test result: ok. 10 passed; 0 failed
test result: ok. 2 passed; 0 failed
test result: ok. 0 passed; 0 failed
```

The 60-block variant with a trace: engine 2 at the builders' round from the
first interval.
```
TRACE q4  h157 r360 c358 ffalse | h157 r360 c358 ffalse | h111 r360 c358 ftrue | h157 r360 c358 ffalse
TRACE q10 h1502 r3050 c3048 ffalse | h1502 r3050 c3048 ffalse | h1455 r3050 c3048 ftrue | h1502 r3050 c3048 ffalse
```

`cargo test -p custom-l1-node --test bft_node_tests`:
```
test a_validator_opened_behind_the_tip_does_not_stall_the_builders ... ok
test a_validator_down_far_past_the_engine_window_rejoins_through_a_checkpoint ... ok
test a_validator_opened_fifty_blocks_behind_does_not_stall_the_builders ... ok
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 197.18s
```

`cargo test -p custom-l1-node --lib`: `ok. 529 passed; 0 failed; 1 ignored`.
`--test bft_follower_tests --test bft_staking_tests`: `ok. 13 passed` and
`ok. 7 passed`. `cargo test -p maya2c-node --bins`: `ok. 82 passed; 0 failed`.
`cargo clippy -p maya-dag-bft -p custom-l1-node -p maya2c-node --all-targets
-- --cap-lints warn`: finished, 0 errors. All warnings are `pedantic`, and none
are on lines this change added.

## Not verified

- The behind validator, though rejoined as a voter, keeps importing about
  one mesh interval behind and never builds again in 30 intervals. That
  comes from the follower source the test poses (one block behind the newest
  checkpoint it has seen), and `may_build_again` needs the tip at the
  previous anchor. Not changed here.
- `scripts/lint_debt.sh --check` was not run.
- No real-binary run (attacknet or localnet) of the 60-block restart.
