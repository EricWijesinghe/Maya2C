# Limits baseline, limit-breaker Lane 0 (2026-11-15)

Plan: `.claude/plans/limit-breaker-2026-11-15.md` section 3, Lane 0. Report only: no node or consensus code changed.

Status legend: MEASURED (raw output in this report), TRACED (code read, `file:line`, no run), arithmetic (computed from a cited constant or a reported figure, labelled), MISSING (not measured, reason given), NOT VERIFIED.

Provenance: the trace, bench and netem results below were produced by three read-only or bench sub-runs in this workflow. They are restated here from their outputs. The harness copies under `reports/limits-baseline/` are the exact sources those runs used. Nothing in this report was re-run by the author of this branch.

## Environment

| Item | Value | Source |
|---|---|---|
| Host CPU | Intel(R) Core(TM) Ultra 9 275HX, 24 cores / 24 logical threads | bench output |
| Host RAM | 31.4 GiB; TotalPhysicalMemory 33,690,607,616 B; TotalVisibleMemorySize 32,900,984 KiB | bench output |
| Free RAM at checks | 7.3 GiB (earlier, bench); 10.68 GiB (11,199,416 KiB, final bench check); 9.22 GB of 31.38 GB (netem gate) | bench, netem |
| OS | Windows 11 Pro Insider Preview 10.0.29680 (build 29680); Git Bash MSYS 3.6.9-b4195d69.x86_64. No Linux kernel on this host. | bench |
| Toolchain | rustc 1.99.0-nightly (da80ed070 2026-07-14); cargo 1.99.0-nightly (59800466c 2026-07-07) | bench |
| Build wrapper | `cargo-slot.sh` (slot lock), `CARGO_BUILD_JOBS=2`, sccache wrapper set by the slot script | bench |
| Commit measured (bench) | 2090300ccf0daa1be1481e7d7d6a34897cd9160e (worktree HEAD) | bench |
| Commit traced (trace) | c7ecc66a (origin/master at that fetch) | trace |
| Branch base for this report | origin/master 64b4bc1c (fetched at report time) | git |
| Commit gap | c7ecc66a..64b4bc1c (master moved 2 commits after the trace; the diff was not inspected) | git fetch |
| Docker | not running (netem gate failed; Docker Desktop not started) | netem |
| Build directory | deleted after runs; no processes killed | bench |

Caveat: this is a single Windows host with commit charge and RAM pressure from other agents. It is not the 8 GiB or 16 GiB reference host, not a multi-host network, and not a network figure. Every timing below is single-process unless marked otherwise.

Not verified for this report: whether commits between 2090300c, c7ecc66a and 64b4bc1c change any measured code path. Rerun with the commands in "Reproduce" to settle it.

## C9: round interval and pacing (plan item C9)

| Value | Status | Evidence |
|---|---|---|
| Round interval, genesis default 500 ms (`round_interval_ms`) | TRACED | `crates/node/src/genesis.rs:420-422` |
| Round interval is hashed into the genesis id | TRACED | `crates/node/src/genesis.rs:1225` |
| Genesis validation bounds `batch_size` and `anchor_timeout_ms` only; `round_interval_ms` has no lower bound, so 0 would remove pacing | TRACED (read of validate block; not run) | `crates/node/src/genesis.rs` ~705-715 |
| Pacing gate: `if round > 0 && early && !full { return }`; the 500 ms gap is skipped when mempool is full | TRACED | `crates/dag-bft/src/validator.rs:774-777` |
| Anchor-round wait for a missing leader is not bypassed by a full mempool; lasts `anchor_timeout_ms` (genesis default 1,000) | TRACED | `crates/dag-bft/src/validator.rs:760-771`; `crates/node/src/genesis.rs:412-414` |
| Pacing clock is local (driver-supplied `now_ms`); the interval value itself is genesis-committed, with no height-keyed schedule | TRACED | `bins/maya2c-node/src/bft.rs:303`; `crates/dag-bft/src/validator.rs:808` |
| Anchors on even rounds only (`leader(round)` returns None for odd rounds) | TRACED | `crates/dag-bft/src/vertex.rs:138-141` |
| Rounds at 500 ms, code bound: <= 2 rounds/s | arithmetic (TRACED constant) | 1 / 0.5 s |
| Anchors at 500 ms, code bound: <= 1 anchor/s | arithmetic (TRACED) | even rounds only |
| Blocks at 500 ms, code bound: <= 1 block/s without a full mempool; no bound from the interval when mempool is full | arithmetic (TRACED); the full-mempool rate is MISSING | `crates/node/src/consensus/bft/driver.rs:764,784` (one sub-dag per anchor, one block per sub-dag) |
| Measured block rate, normal, reported | reported, NOT re-run here: 0.964 blk/s (~3,470 blocks/h) | `reports/proving/2026-11-09.md:39` |
| Measured block rate, 24 h run average | reported, NOT re-run here: 73,178 blocks / 86,400 s = 0.847 blk/s (includes a ~1.2 h laptop sleep gap and host contention) | `reports/proving/run-6fcfc65e.log` final line `PASS: 24 h, no fork, height 73178`; `2026-11-09.md:39-40` |
| Round rate and anchor rate at n=7, one-way delay 25 ms, 1% loss | MISSING: gate failed (free RAM 9.22 GB < 14 GB required; Docker not running). Not run. | netem |
| Round rate and anchor rate at n=7, one-way delay 75 ms, 1% loss | MISSING: same gate failure | netem |
| Round rate and anchor rate at n=7, one-way delay 150 ms, 1% loss | MISSING: same gate failure | netem |
| Blocks/s per setting (25/75/150 ms) | MISSING: not run | netem |
| Finality latency p50/p95 per setting | MISSING: not run | netem |

## T2: block-count promises (plan item T2)

| Value | Status | Evidence |
|---|---|---|
| PoW retarget basis: RETARGET_INTERVAL 100, TARGET_BLOCK_TIME 15 s, EXPECTED_TIMESPAN 1,500 s, MAX_ADJUSTMENT_FACTOR 4 | TRACED | `crates/node/src/consensus/difficulty.rs:29,32,35,38` |
| DAG epoch: EPOCH_LENGTH = 30,000 blocks; DAG_ACTIVATION_HEIGHT = EPOCH_LENGTH | TRACED | `crates/node/src/crypto/dag/mod.rs:80-94` |
| DAG epoch test pins 30,000 x 15 s = 450,000 s | TRACED | `crates/node/src/crypto/dag/mod.rs:326-331` |
| Work half-life: WORK_HALF_LIFE_BLOCKS = 5,760, applied as block heights; doc says "about a day at 15 s" | TRACED | `crates/node/src/governance/work.rs:46-52,52,89,111-112` |
| Staking epoch boundary: `height % epoch_blocks == 0`; epoch_blocks from genesis (plan correction 1 uses 3,600) | TRACED | `crates/node/src/state/staking_exec.rs:303`; `crates/node/src/genesis.rs:297,1010` |
| Wall-clock reads in epoch, DAG dataset, work paths | TRACED: grep for SystemTime / Instant::now / now_ms in staking/epoch.rs, staking_exec.rs, dag/, work.rs returned no match. Wall time enters only via vertex timestamps and the median-time rule. | `crates/dag-bft/src/validator.rs:808` |
| Live DAG-BFT node retarget | TRACED: `ChainConfig::dag_bft()` sets `fixed_target: true`, `verify_pow: false`; `next_target` returns parent target; retarget() unreachable on the DAG-BFT node. See C8. | `crates/node/src/consensus/chain.rs:141-145,388-390` |
| The approved literals 450,000 and 86,400 are not consensus constants in code (only a test assert at dag/mod.rs:329 and a doc division at :330) | TRACED | `crates/node/src/crypto/dag/mod.rs:326-331` |
| 450,000 blocks in time, at 1 blk/s | arithmetic: 450,000 s = 5.21 d | |
| 450,000 blocks in time, at 2 blk/s | arithmetic: 225,000 s = 2.60 d | |
| 86,400 blocks in time, at 1 blk/s | arithmetic: 1.00 d | |
| 86,400 blocks in time, at 2 blk/s | arithmetic: 43,200 s = 12 h | |
| 450,000 blocks at reported 0.964 blk/s | arithmetic on reported rate (not re-run): 466,804 s = 5.40 d | `2026-11-09.md:39` |
| 450,000 blocks at reported 0.847 blk/s | arithmetic on reported rate (not re-run): 531,286 s = 6.15 d | run-6fcfc65e.log |
| 86,400 blocks at reported 0.964 / 0.847 blk/s | arithmetic: 1.04 d / 1.18 d | same |
| EPOCH_LENGTH 30,000 in time at 1 / 2 blk/s | arithmetic: 8.33 h / 4.17 h; at 0.964 / 0.847: 8.64 h / 9.84 h | |
| WORK_HALF_LIFE 5,760 in time at 1 / 2 blk/s | arithmetic: 1.6 h / 48 min; at 0.964 / 0.847: 1.66 h / 1.89 h | |
| Disk and state growth per block | MISSING (see "Other Lane 0 measurements") | |

## T5: DAG and work rescale (plan item T5)

| Value | Status | Evidence |
|---|---|---|
| Approved 450,000 and 86,400 are not in code as constants | TRACED | `crates/node/src/crypto/dag/mod.rs:326-331` |
| Meaning at 1 blk/s and 2 blk/s | arithmetic (see T2 table): 450,000 = 5.21 d / 2.60 d; 86,400 = 1.00 d / 12 h | |
| Meaning at reported measured rates (0.964 and 0.847 blk/s) | arithmetic on reported rates, not re-run: 450,000 = 5.40 d / 6.15 d; 86,400 = 1.04 d / 1.18 d | `2026-11-09.md:39`; run-6fcfc65e.log |
| Work rescale only after the interval decision (Lane 0) | plan text; no measurement needed for the decision itself | `.claude/plans/limit-breaker-2026-11-15.md` line 95 |

Note: 86,400 = 5,760 x 15 s, the seconds count behind the work half-life doc. As a block count it is one day only at 1 blk/s.

## C3: certificate bytes and cost at n (plan item C3)

Vote signature: ML-DSA-87 validator suite, SIGNATURE_LEN 4,627 B, PUBLIC_KEY_LEN 2,592 B. Quorum formula from code: f = floor((n-1)/3), q = n - f (equal weights).

| n | f | q (code: n-f) | sig bytes (q x 4,627) | payload per bench (sig + u16 ids) | wire votes-only (8 + q x 4,637) | sign q (median, ms) | verify q serial, median (ms) | build (container, median, ms) |
|---|---|---|---|---|---|---|---|---|
| 7 | 2 | 5 | 23,135 | 23,145 | 23,193 | 2.097 | 1.196 | 0.0123 |
| 16 | 5 | 11 | 50,897 | 50,919 | 51,015 | 4.120 | 2.208 | 0.0043 |
| 32 | 10 | 22 (code); bench used 21 (see conflict) | bench 97,167 (21 votes); code-derived 101,794 (22) | bench 97,209 (21 votes); code-derived 101,838 (22) | code-derived 102,022 (22) | bench 7.766 (21 votes) | bench 3.812 (21 votes) | bench 0.0183 (21 votes) |
| 64 | 21 | 43 | 198,961 | 199,047 | 199,399 | 16.139 | 7.258 | 0.0233 |
| 100 | 33 | 67 | 310,009 | 310,143 | 310,687 | 20.273 | 8.581 | 0.0319 |

Minimal certificate, code-derived (42 B vertex header + 32 B x P parents + 8 B vote count + q x 4,637 B, P = q, empty batch): n=7: 23,395 B; n=16: 51,409 B; n=32: 102,768 B (q=22); n=64: 200,817 B; n=100: 312,873 B. (TRACE; arithmetic on the per-vote encoding.)

CONFLICT, n=32: the code quorum is q = W - f = 32 - 10 = 22 (`crates/dag-bft/src/vertex.rs:105-106,118-119`). The bench harness `qc_limits.rs` computes `quorum = 2*((n-1)/3)+1` = 21 at n=32 (`reports/limits-baseline/qc_limits.rs.txt` line 18). The two formulas agree for n = 7, 16, 64, 100 (n mod 3 = 1) and differ for n = 32 (n mod 3 = 2). The bench n=32 row is therefore not the code quorum. It must be re-run with q = n - f before any n=32 figure is used.

Byte-accounting note: the bench `total_payload_bytes` excludes the 8 B vote count and the 8 B length prefix per signature in the wire encoding. Wire votes-only = bench payload + 8 + 8q. Check at n=100: 310,143 + 8 + 536 = 310,687 (matches TRACE).

Verify time: the bench measured serial verification of each quorum signature only (median of 5, 1 thread). Full certificate verify (decode, membership, quorum check) is MISSING. The TRACE listed certificate verify as MISSING; the bench's signature-verify figure is not the same quantity.

## Correction 2: anchors on even rounds, 500 ms, measured rate (plan section 0, item 2)

| Claim under test | Status | Evidence |
|---|---|---|
| Anchors land on even rounds only | CONFIRMED (code) | `crates/dag-bft/src/vertex.rs:138-141`; doc `vertex.rs:133-137`; `crates/dag-bft/src/commit.rs:4` |
| At 500 ms, anchor rate about 1/s (code bound) | arithmetic/TRACED: <= 1 anchor/s | see C9 |
| Measured rate 0.96-1.03 blk/s (plan) | PARTLY CONFIRMED: 0.964 blk/s reported (2026-11-09.md:39); 0.847 blk/s average over 24 h (run-6fcfc65e.log). The upper bound 1.03 blk/s is NOT found in the supplied data. | reports/proving |
| Measured anchor rate (not block rate) | MISSING: not measured in this lane (no anchor count was logged) | |
| Explanation of gap to 1 blk/s | NOT VERIFIED: no measurement isolates the cause | |

## Correction 20: certificate vote type and size (plan section 0, item 20)

| Claim | Status | Evidence |
|---|---|---|
| Certificate votes are ML-DSA-87 validator signatures, 4,627 B | CONFIRMED (code) | `crates/node/src/consensus/bft/auth.rs:8-9,88,105`; `crates/node/src/crypto/validator_keys.rs:21,40-41` (const assert SIGNATURE_LENGTH == 4627) |
| Tx hybrid (11,165 B) is NOT used for votes | CONFIRMED (code) | `crates/node/src/consensus/bft/auth.rs`, `validator_keys.rs` |
| n=100, 67 signatures: 310,009 B (67 x 4,627) | MEASURED (exact match in bench output) | `sig_bytes=310009` in the qc_limits output |
| Plan's 748,055 B (67 x 11,165 B) | NOT the code path (TRACED); not measured | |
| Per-vote wire overhead: voter u16 (2 B) + length u64 (8 B) = 10 B; per vote 4,637 B on wire | TRACED (`crates/node/src/consensus/bft/wire.rs:182-188`) | |
| Count of votes in a built certificate (exactly q, or more) | NOT VERIFIED: builder not read (`crates/dag-bft/src/validator.rs` ~551, ~846) | |
| Sign and verify time for q signatures at n=100 | MEASURED: sign 20.273 ms (median of 5, 1 thread); verify 8.581 ms (serial, median of 5) | qc_limits output |

## M11: "millions of miners" (plan item M11)

| Value | Status | Evidence |
|---|---|---|
| No miner count cap; limits are on shares | TRACED | `crates/node/src/state/mining.rs:257-263,278-279` (per-share reward = miner_budget / target_shares; retarget toward target_shares per epoch) |
| Per-block share cap `max_shares_per_block` | TRACED | `crates/node/src/state/mining_exec.rs:135-136` |
| Genesis-ceremony sample: target_shares 10, max_shares_per_block 2 (sample, not approved mainnet) | TRACED | `bins/genesis-ceremony/src/main.rs:1299-1300` |
| Paid share size: signed MiningShare transaction, hybrid signature | MEASURED: 13,192 B (`encoded_bytes=13192`); plan assumed 13,215 B | limits_dag_share output |
| hashimoto_light at mainnet params, per call (median of 5 batches of 200 calls) | MEASURED: 5,140.7 us (batches 4943.3, 5100.7, 5140.7, 5181.4, 5190.5 us) | limits_dag_share output |
| Cache generation, epoch 0 (64 MiB cache) | MEASURED: 690.6 ms | limits_dag_share output |
| Share verify cost (signed share tx signature verify) | MISSING: not measured for shares. Transfer verify (a proxy, not a share) measured at 0.188-0.189 ms/tx, 1 thread (see B18). | |
| Miner count achievable under the limits | MISSING: no figure claimed; the plan requires no number | |

## B18: apply and verify time per block (plan item B18)

Workload: 400 hybrid transfers, 100 per block, apply over RocksDB StateDB, 24 cores. Average signed transfer wire size 13,239 B (5,295,600 B / 400), so 79.2 transfers per MiB (arithmetic: 1,048,576 / 13,239).

| Measure | Run 1 | Run 2 | Status |
|---|---|---|---|
| Verify, 1 thread, ms/tx | 0.189 | 0.188 | MEASURED |
| Verify, 1 thread, ms/MiB (arithmetic) | 14.97 | 14.89 | arithmetic on measured |
| Verify, 24 threads, ms/tx | 0.033 | 0.027 | MEASURED |
| Verify, 24 threads, ms/MiB (arithmetic) | 2.61 | 2.14 | arithmetic on measured |
| Apply, 1 thread (includes verify), ms/tx | 0.221 | 0.216 | MEASURED |
| Apply, 1 thread, ms/MiB (arithmetic) | 17.50 | 17.11 | arithmetic on measured |
| State work alone, ms/tx | 0.032 | 0.028 | MEASURED |
| State work alone, ms/MiB (arithmetic) | 2.53 | 2.22 | arithmetic on measured |
| Peak RSS | NOT MEASURED (`/proc/self/status` unavailable on Windows; bench prints so) | | MISSING |
| Write amplification | NOT MEASURED (`/proc/self/io` unavailable) | | MISSING |
| ADR-072 reference: 1.062 ms/transfer; ~100 ms/MiB apply; ~37 ms/MiB verify (4 threads) | NOT REPRODUCED on this host: measured 0.216-0.221 ms/tx apply, 0.188-0.189 ms/tx verify (1 thread) | ADR-072 (one host) |

Allocation counts from bench: verify 2 allocs/tx; apply 14 allocs/tx; state work 12 allocs/tx.

## S2: downtime slash (plan item S2)

| Value | Status | Evidence |
|---|---|---|
| downtime_slash_bps 100; downtime_threshold_bps 5,000; jail_epochs 2 (DEVNET defaults) | TRACED | `crates/staking/src/params.rs:45-47` |
| Mainnet genesis spreads `Params::DEVNET` for staking params; only min_self_bond, min_delegation, max_validators set there | TRACED | `crates/node/src/genesis.rs:370-378` |
| Detection: authored x 10,000 < threshold x expected (expected counts skipped anchor slots) | TRACED | `crates/staking/src/epoch.rs:93-106`; slot counting `crates/node/src/state/staking_exec.rs` ~378-393; MAX_SLOTS_PER_BLOCK 1,000,000 at :35 |
| Slash per down epoch: 100 bps | TRACED | `crates/staking/src/epoch.rs:137` |
| Jail until epoch + 1 + jail_epochs; jailed-but-seated not re-slashed (S-030) | TRACED | `crates/staking/src/epoch.rs:94-96,121-134` |
| Repeatable slash from one DoS (cycle of at least 3 epochs, 100 bps each, from code) | NOT VERIFIED: no dynamic run | |
| Downtime slash behaviour measured | MISSING: not measured | |
| Downtime values are DEVNET, not approved mainnet (plan correction 23) | TRACED (value source confirmed) | genesis.rs:370-378 |

## S15: post-genesis concentration cap (plan item S15)

| Value | Status | Evidence |
|---|---|---|
| Node-level per-address stake cap | NONE found (TRACED) | `crates/staking/src/concentration.rs:16,40` is analysis only; exported at `crates/staking/src/lib.rs:34,45`; used only by `crates/staking/tests/scenario.rs:12,214`; no consensus, state or admission call |
| xtask genesis check: MAX_SHARE_BPS 2,000; MIN_VALIDATORS 7 | TRACED | `xtask/src/genesis_check.rs:18-19` |
| Check keys by operator string; flags any operator > 2,000 bps | TRACED | `xtask/src/genesis_check.rs:376-395` |
| Seven addresses held by one person pass the xtask check (plan correction 9) | CONFIRMED by construction (keyed per address string) | genesis_check.rs:376-383 |
| Post-genesis per-address cap | NOT IMPLEMENTED (post item) | |
| Measured value | not applicable (no measurement item) | |

## Other Lane 0 measurements

| Item | Value | Status | Evidence |
|---|---|---|---|
| Wasm module, 2,093,206 B (1.996 MiB), 64 functions, deterministic engine: validate (`wasmtime::Module::validate`), median of 3 | 5.536 ms (runs 5.2254, 5.5364, 5.8187) | MEASURED | wasm_limits output |
| Wasm compile (`ModuleCache::compile`, cold cache), median of 3 | 96.111 ms (runs 95.0516, 96.1108, 102.7085) | MEASURED | wasm_limits output |
| Wasm peak working set (OS value, whole process) | 8.3 MiB after validate; 11.3 MiB after compile | MEASURED | wasm_limits output |
| Wasm caveat | MAX_MODULE_BYTES 512 KiB (`crates/vm/src/config.rs:55`) is enforced on the deploy path, not in Module::new or validate; this measures engine cost only | TRACED | |
| end_epoch, 10,000 validators, 100,000 delegations, max_validators 10,000 (all active) | median 45.872 ms (runs 41.732, 45.184, 45.977, 45.872, 47.485); active after 10,000 | MEASURED | end_epoch output |
| end_epoch, same state, max_validators 100 (DEVNET default) | median 33.884 ms (runs 37.711, 34.998, 32.889, 33.884, 33.308); active after 100 | MEASURED | end_epoch output |
| end_epoch caveat | single-thread, in-memory, not a RocksDB state figure | | |
| DAG dataset build time and peak RSS (MAINNET 4 GiB dataset, realised 4,294,962,304 B) | MISSING: not built; gate needs about 14 GiB free; free RAM was 7.3 GiB then 10.68 GiB | MISSING | limits_dag_share run gate; `Win32_OperatingSystem` |
| Dataset build on 8 GiB and 16 GiB hosts | MISSING: not measured; no such host available | MISSING | plan Lane 0 |
| Disk growth per block at cap over the 24 h proving run | MISSING: no disk or storage figure in `reports/proving/` (heights only) | MISSING | `reports/proving/2026-11-09.md:9-41`; run-6fcfc65e.log |
| Stagenet WAN netem round, finality, block rates | MISSING (gate failed: RAM 9.22 GB < 14 GB; Docker not running) | MISSING | netem |

## Implications for envelope values

Each lane takes the figure below; anything not listed is MISSING and must not be filled from the plan. No recommendation beyond the numbers.

| Lane | Number it should take | Still MISSING for that lane |
|---|---|---|
| B (envelope framework; depends on Lane 0) | Round interval: genesis 500 ms (code-hashed); C9 bound at 500 ms: <= 1 anchor/s and <= 1 block/s without full mempool. Time basis: 15 s retarget unreachable on DAG-BFT (C8); no decided interval table (T2). | Round and anchor rate under 25/75/150 ms (netem gate); full-mempool block rate; finality latency; the interval decision itself |
| C (header and framing) | Largest certificate in the data: 310,143 B payload at n=100 (310,687 B wire votes-only). Frame and block ceilings are not in the data. | Block size ceiling; certificate verify full path under WAN delay; C1 CEIL from Lane 0 |
| D (tx and witness format) | Average signed transfer 13,239 B; signed paid share 13,192 B (replaces plan's 13,215 B). Hybrid 11,165 B from plan is not re-measured. | Witness-size effect under fee; dedupe cost |
| E (block bounds) | Apply 0.216-0.221 ms/tx (17.1-17.5 ms/MiB, 1 thread); verify 0.188-0.189 ms/tx (14.9-15.0 ms/MiB, 1 thread) at 79.2 tx/MiB; 24-thread verify 2.1-2.6 ms/MiB. ADR-072 numbers are not reproduced here. | Peak RSS and write amplification on Windows; block apply at the proposed cap; disk per block at cap |
| F (committee and certificates) | Sign and serial verify at n=7,16,64,100 (table C3); n=32 must be re-run because the bench used q=21 and code gives q=22. Quorum formula: q = n - floor((n-1)/3). | Certificate verify full path; certificate verify under WAN delay; C1 CEIL; any n=32 figure until re-run |
| G (staking genesis) | DEVNET downtime defaults are the live mainnet values (S2); end_epoch 45.9 ms at 10k validators / 100k delegations, in-memory. | Downtime slash dynamics; reward cost on RocksDB; slash repeatability |
| H (state root and storage) | None measured for incremental root. | Disk per block; write amplification; incremental-root timing (no measurement) |
| I (fee market) | None measured for fees. Apply time per tx (B18) is the only timing input. | Any fee-market figure; burn/treasury measurement |
| J (mining and DAG) | Share 13,192 B; hashimoto_light 5,140.7 us per call at mainnet params; cache gen 690.6 ms (epoch 0). Block rate: 0.964 blk/s reported normal, 0.847 blk/s 24 h average (not re-run). Calendar figures in T2/T5 (450,000 = 5.40 d / 6.15 d at reported rates). | DAG dataset build time and RSS (8 and 16 GiB hosts); share verify cost; disk per block; millions-of-miners figure |
| K (council and guards) | None measured. | Not applicable |
| L (governance activation and keys) | None measured. The 1,440-block activation lead (plan correction 14) is a code value; no measurement in this data. | Governance timing under real block rate |
| M (features, shielded, value-bearing guard) | Share 13,192 B (signed tx); transfer 13,239 B average. | Shielded proof size and verify cost: no measurement in this data |
| N (VM and account) | Engine cost for a 1.996 MiB module: validate 5.536 ms; compile 96.111 ms; RSS 11.3 MiB after compile. | Engine cost at the deploy cap of 512 KiB (not run); deploy-path measurement |
| O (spec, vectors, genesis regeneration, freeze gate) | All measured values above, plus the n=32 re-run. | Every MISSING item above; Lane 0 cannot close until the netem and dataset items run on a host with RAM |

## Reproduce

Environment: Windows 11 Pro 10.0.29680, PowerShell 7 and Git Bash, worktree `D:\Maya2C\.claude\worktrees\<your-worktree>`. Every cargo command goes through the slot lock; one scope at a time; `CARGO_BUILD_JOBS=2`.

Harness sources (text copies in this branch, not compiled into any crate):
- `reports/limits-baseline/qc_limits.rs.txt` -> copy to `crates/crypto-pq/examples/qc_limits.rs`
- `reports/limits-baseline/limits_dag_share.rs.txt` -> copy to `crates/node/examples/limits_dag_share.rs`
- `reports/limits-baseline/wasm_limits.rs.txt` -> copy to `crates/vm/examples/wasm_limits.rs`
- `reports/limits-baseline/end_epoch_limits.rs.txt` -> copy to `crates/staking/examples/end_epoch_limits.rs`
- Bench `crates/node/benches/apply_pipeline.rs` is on origin/master (contains the `MAYA_DUMP_TXS` hook); no copy needed.

Commands (run from your worktree root, one at a time):

```
cp reports/limits-baseline/qc_limits.rs.txt crates/crypto-pq/examples/qc_limits.rs
bash /d/Temp/claude-locks/cargo-slot.sh <branch> -- env CARGO_TARGET_DIR=<unique-target> CARGO_BUILD_JOBS=2 cargo run --release -p maya-crypto-pq --example qc_limits 2>&1 | tail -25
```

Same pattern for the others:
- `cargo run --release -p custom-l1-node --example limits_dag_share`
- `cargo run --release -p maya-vm --example wasm_limits`
- `cargo run --release -p maya-staking --example end_epoch_limits`
- `MAYA_DUMP_TXS=<dir> cargo bench -p custom-l1-node --bench apply_pipeline`, then `ls -l <dir>` for total and average wire bytes (avg = total / count).

Free-RAM gate, before the dataset build or netem: PowerShell `Get-CimInstance Win32_OperatingSystem | Select FreePhysicalMemory, TotalVisibleMemorySize`.

Netem (not run; needs Docker Desktop started by a human and >= 14 GB free): `bash infra/stagenet/gen.sh <bin-dir> <out-dir> 7`; set NETEM to `delay 25ms loss 1%` (then 75 and 150) on every `v<i>` service in `<out-dir>/compose.yml`; `docker compose -f <out-dir>/compose.yml up -d --build`; sample logs for <= 10 min; `docker compose -f <out-dir>/compose.yml down -v`; remove only containers you started, by id.

Cleanup: `rm -rf target "$CARGO_TARGET_DIR"` (own directory only). Never kill by image name.

## Raw output (verbatim lines used above)

qc_limits (`cargo run --release -p maya-crypto-pq --example qc_limits`):
```
qc_limits: ML-DSA-87 validator suite, PUBLIC_KEY_LEN=2592 SIGNATURE_LEN=4627
n=  7 quorum=  5 sig_bytes=23135 vote_id_bytes=10 total_payload_bytes=23145 sign_quorum_ms_median=2.097 build_ms_median=0.012300 verify_serial_ms_median=1.196 repeats=5
n= 16 quorum= 11 sig_bytes=50897 vote_id_bytes=22 total_payload_bytes=50919 sign_quorum_ms_median=4.120 build_ms_median=0.004300 verify_serial_ms_median=2.208 repeats=5
n= 32 quorum= 21 sig_bytes=97167 vote_id_bytes=42 total_payload_bytes=97209 sign_quorum_ms_median=7.766 build_ms_median=0.018300 verify_serial_ms_median=3.812 repeats=5
n= 64 quorum= 43 sig_bytes=198961 vote_id_bytes=86 total_payload_bytes=199047 sign_quorum_ms_median=16.139 build_ms_median=0.023300 verify_serial_ms_median=7.258 repeats=5
n=100 quorum= 67 sig_bytes=310009 vote_id_bytes=134 total_payload_bytes=310143 sign_quorum_ms_median=20.273 build_ms_median=0.031900 verify_serial_ms_median=8.581 repeats=5
```

limits_dag_share:
```
limits_dag_share: mainnet params cache_bytes=67108864 (realised 67108672) dataset_bytes=4294967296 (realised 4294962304) epoch_length=30000
cache generate (epoch 0) ms=690.6
hashimoto_light per call us_median=5140.7 batches_us=[4943.3, 5100.7, 5140.7, 5181.4, 5190.5] calls_per_batch=200 batches=5 (sink=47)
signed paid share (TxKind::MiningShare, hybrid signature) encoded_bytes=13192
```

apply_pipeline (`MAYA_DUMP_TXS=D:/Temp/claude/txdump cargo bench -p custom-l1-node --bench apply_pipeline`), run 1:
```
apply_pipeline: 400 hybrid transfers, 100/block, 24 cores, optimized
verify   1 thread :    0.189 ms/tx      5298 tx/s  2 allocs/tx
verify 24 threads:    0.033 ms/tx     29927 tx/s  (x5.65)
apply    1 thread :    0.221 ms/tx      4521 tx/s  14 allocs/tx (includes verify)
  state work alone:    0.032 ms/tx  (14.7% of apply; 12 allocs/tx)
write amplification: NOT MEASURED (/proc/self/io unavailable on this platform)
peak RSS: NOT MEASURED (/proc/self/status unavailable)
```
run 2:
```
verify   1 thread :    0.188 ms/tx      5318 tx/s  2 allocs/tx
verify 24 threads:    0.027 ms/tx     36863 tx/s  (x6.93)
apply    1 thread :    0.216 ms/tx      4627 tx/s  14 allocs/tx (includes verify)
  state work alone:    0.028 ms/tx  (13.0% of apply; 12 allocs/tx)
```
wire size: `txs=400 total_bytes=5295600 avg_bytes=13239`

wasm_limits (`cargo run --release -p maya-vm --example wasm_limits`):
```
wasm module bytes=2093206 (1.996 MiB) functions=64
validate (wasmtime::Module::validate, deterministic_engine) median_ms=5.536 runs_ms=[5.8187, 5.5364, 5.2254]
peak_working_set_mib_after_validate=8.3
compile (ModuleCache::compile = Module::new, cold cache) median_ms=96.111 runs_ms=[95.0516, 102.7085, 96.1108]
peak_working_set_mib_after_compile=11.3
```

end_epoch_limits (`cargo run --release -p maya-staking --example end_epoch_limits`):
```
end_epoch validators=10000 delegations=100000 max_validators=10000 active_after=10000 median_ms=45.872 runs_ms=[41.732, 45.184, 45.977, 45.872, 47.485]
end_epoch validators=10000 delegations=100000 max_validators=100 active_after=100 median_ms=33.884 runs_ms=[37.711, 34.998, 32.889, 33.884, 33.308]
```

Netem gate:
```
Get-CimInstance Win32_OperatingSystem -> FreeGB=9.22 TotalGB=31.38
docker info -> failed to connect to the docker API at npipe:////./pipe/dockerDesktopLinuxEngine
```

## Not verified, MISSING, and risks

- MISSING: DAG dataset build time and RSS (RAM gate); netem round, anchor, block and finality figures (RAM gate and Docker); disk per block at cap; peak RSS and write amplification on Windows; share verify cost; downtime slash dynamics; full certificate verify path.
- NOT VERIFIED: builder's vote count (exact q or more); whether commits 2090300c..64b4bc1c change measured paths; the plan's 1.03 blk/s upper bound; the cause of the rate gap to 1 blk/s; repeatable slash from one DoS.
- CONFLICT: n=32 quorum (bench 21 vs code 22). The n=32 bench row is invalid for the code quorum until re-run.
- Risk: the wasm figure measures engine cost at 2 MiB, not at the 512 KiB deploy cap; the end_epoch figure is in-memory, not RocksDB.
- Risk: all timings are from a single shared Windows host under memory pressure; not a reference host; not a network figure.
