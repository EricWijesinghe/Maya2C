# 04 — Consensus, mining and scaling

**Machine:** cloud VM, 4 × Intel Xeon @ 2.80 GHz, 15 GiB RAM, Ubuntu 24.04.4,
kernel 6.18.44, `nightly-2026-07-15`. "Virtual" time is `maya-sim`'s modelled
time; "wall" time is this machine's.

## Verdict

| DONE WHEN criterion | State | Evidence |
|---|---|---|
| All three consensus modes run in sim/ | **Met** (engine level) | §2, `crates/dag-bft/tests/modes_sim.rs` |
| Reorg and DAG commit tests pass | **Met** | §1, §2 |
| GPU/CPU parity passes (or skips with a reason) | **Met** (2026-09-27): 7/7 on an RTX 5070 Laptop GPU, including the five GPU-vs-CPU cases | §6 |
| `reports/04-consensus.md` has measured TPS and latency | **Met** (2026-09-27): the node runs DAG-BFT (ADR-027); finalized TPS and finality latency measured through the node's own code and binary | §6 |

## 0. The design (ADR-015)

DAG-BFT (Narwhal certification + Bullshark commit rule) orders and finalizes;
work never orders anything on mainnet. `argonblake-pow` (what the node runs
today) and `pouw-lattice` (RESEARCH) share one most-work fork choice. The
consensus mode is a **genesis** parameter, not a `config.toml` key: the node's
config is built so no consensus value can be set per host, and the brief's
"set in config" would contradict that (reported, not worked around).

## 1. The PoW chain the node runs (full workspace run)

```
tests/consensus_tests.rs         :: test result: ok. 26 passed; 0 failed; 0 ignored; ... in 2.70s
tests/dag_tests.rs               :: test result: ok. 13 passed; 0 failed; 0 ignored; ... in 6.51s
tests/chaos_simulator.rs         :: test result: ok. 13 passed; 0 failed; 0 ignored; ... in 12.62s
tests/attack_simulation_tests.rs :: test result: ok. 8 passed; 0 failed; 0 ignored; ... in 1.85s
tests/byzantine_guard_tests.rs   :: test result: ok. 3 passed; 0 failed; 0 ignored; ... in 25.01s
tests/latency_sim_tests.rs       :: test result: ok. 3 passed; 0 failed; 0 ignored; ... in 16.21s
```

Retargeting, most-cumulative-work fork choice and reorg with state rollback
are covered by `consensus_tests.rs` and `restart_tests.rs`.

## 2. DAG-BFT engine and all three modes in the simulator

`cargo test -p maya-dag-bft` (8 unit + 6 simulation tests, all pass):

| Test | What it shows |
|---|---|
| `five_validators_over_a_wide_area_link_commit_the_same_order` | 5 validators, 20–80 ms links, 0.1% loss, 0.5% reordering: anchor and transaction logs prefix-consistent on every node, no transaction ordered twice |
| `one_crashed_validator_of_five_does_not_stop_commits` | f = 1 crash: honest nodes keep committing, consistently |
| `a_minority_partition_stalls_nobody_forever_and_heals_consistently` | 3/2 split for 10 virtual s, then heal: logs consistent; minority within 3 anchors; equal-length logs give equal state roots |
| `a_one_third_partition_without_quorum_halts_rather_than_forks` | 4 validators split 2/2: nothing commits on either side (fails safe) |
| `both_work_modes_converge_on_one_chain_and_one_state_root` | argonblake-pow and pouw-lattice, 5 nodes, 200 blocks: identical chains 6 deep, one state root |
| `the_same_transactions_give_the_same_root_whichever_mode_...` | the state machine is mode-agnostic |

## 3. Throughput and latency — what could be measured

**Ordering throughput in the simulator** (`five_validators_...`, 30 virtual
seconds, 500 transaction references per vertex, saturated mempools):

```
dag-bft sim: 5 validators, wide-area link: 466000 txs ordered in 30 virtual s (15533 tx/virtual-s, 201 rounds, 99 anchors); an ordering figure, not TPS
```

Round time ≈ 149 ms (3 message delays on a 20–80 ms link); commit latency ≈ 2
rounds ≈ 0.3 s virtual. **Bottleneck:** by construction, `batch_size × n /
round_time`; in a real deployment it is leader/worker bandwidth and signature
verification, which the simulator does not charge for.

**Real CPU cost of the commit rule** (`cargo bench -p maya-dag-bft --bench
ordering_bench`, all validators in one thread, free delivery):

```
   n  batch   tx ordered    wall s      tx-refs/s         msgs
   4    500        80000     0.034        2336463         1539
  10    500       100000     0.147         681468         6147
  20    500       100000     0.349         286726        28234
  50    500       100000     0.759         131741        49784
```

Each row runs *every* validator on one core, so per-validator ordering cost is
wall / n: the commit rule is not the bottleneck at these sizes.

**Not measured:** TPS in the Production Standing Orders' sense
(signature-verified, executed, state-committed, finalized) for DAG-BFT — the
node does not run it; the 50,000 TPS goal is therefore neither met nor missed.
The 5-node cluster "throughput and bottleneck" in the brief is answered by the
simulator figures above, labelled as ordering figures.

## 4. GPU/CPU parity

```
tests/gpu_validation.rs :: running 2 tests
test the_flattened_dataset_addresses_the_same_pages ... ok
test the_cpu_mix_reproduces_the_nodes_hashimoto ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; ... in 3.03s
```

The two CPU-reference layers pass. The five GPU-vs-CPU cases are behind the
`gpu` feature (off by default, because this VM has no adapter); they did not
run, and this report does not claim they did. The parity target in the brief
(`pow_lattice.wgsl` for the lattice puzzle) is not built; the wgpu miner is
for the hashimoto PoW.

## 5. Not built (or not changed) by this work

- Wiring DAG-BFT into `custom-l1-node` (ADR-015 lists the steps).
- Encrypted mempool: `crates/mev` has threshold encryption (pairing-based,
  not PQ, as the brief expects); not re-audited here.
- Lattice PoUW solver with LLL/BKZ: `crates/lattice-pow` verifies only.
- Accelerator HAL (FPGA HDL, photonic/neuromorphic SIMs), thermal scheduler.
- Sharding split/merge at 2^16 scale and 50,000 cross-shard transfers:
  `crates/blockgraph` has elastic shards (4 → 64 leaves) but not the brief's
  1,000-shard or cross-shard 2PC/teleport tests.
- Tiered (fiber/LEO/deep-space) consensus: the Earth–Mars run in
  `hal/link-sim/tests/transport_tests.rs` shows a far sub-DAG's roots arriving
  in order across 3–22 minute delays without slowing the near cluster — the
  batching half of §8 — but there is no Tier 2/3 finality gadget.
- Staking, slashing, rewards — **not built**; launch-blocking (ADR-016).

## 6. Measured on the node (added 2026-09-27)

Machine: Windows 11, Intel family 6 model 198 (24 threads), RTX 5070 Laptop
GPU (driver 616.92). Commit `8ff8f81` unless stated.

### GPU/CPU parity

```
$ cargo test -p maya-wgpu-miner --features gpu --test gpu_validation
test the_flattened_dataset_addresses_the_same_pages ... ok
test the_cpu_mix_reproduces_the_nodes_hashimoto ... ok
test gpu_tests::adapters_report_their_limits ... ok
test gpu_tests::the_gpu_mix_matches_the_cpu_mix ... ok
test gpu_tests::an_empty_batch_dispatches_nothing ... ok
test gpu_tests::a_partial_workgroup_is_computed_correctly ... ok
test gpu_tests::the_gpu_digest_matches_the_nodes ... ok
test result: ok. 7 passed; 0 failed
```

### TPS — as the Production Standing Orders define it

Signature-verified, executed, state-committed, finalized transfers per
second. `crates/node/examples/bft_tps.rs`: 10,000 pre-signed ML-DSA-65 (v7)
transfers from 2,000 accounts; four DAG-BFT validators **in one process on
one machine**, each with its own RocksDB state, so every transaction is
verified, executed and committed four times on the same CPU. No link
latency. Mix: 100% single-output transfers.

| build | finalized tx/s | per-node verify+execute+commit /s | note |
|---|---|---|---|
| release (fat LTO), before the fixes below | 524–679 | 2,097–2,715 | three runs |
| perf profile, re-broadcast limited | 650–916 | 2,599–3,663 | four runs; the machine was shared with a WSL build for some |
| perf profile, + parallel verification | **990** | **3,959** | block building 10.6 s → 6.8 s of 10.1 s |

The largest remaining cost is block building and insertion (67% of wall
time): each transaction is staged three times (builder filter, state-root
preview, apply) and each block computes the state root over every account
twice. Those are the next targets; the numbers above are what the code does
today.

### Finality latency

On the five-process localhost devnet (`scripts/bft_devnet.py`, dev profile,
500 ms round pacing), scraped from node 0's exporter: 31 blocks, finality
mean **0.55 s** (anchor proposal to local commit), p50 and p99 in the ≤ 1 s
bucket; a submitted transfer was visible on all five nodes in 1.9–3.0 s.
