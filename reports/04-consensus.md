# 04 — Consensus, mining and scaling

**Machine:** cloud VM, 4 × Intel Xeon @ 2.80 GHz, 15 GiB RAM, Ubuntu 24.04.4,
kernel 6.18.44, `nightly-2026-07-15`. "Virtual" time is `maya-sim`'s modelled
time; "wall" time is this machine's.

## Verdict

| DONE WHEN criterion | State | Evidence |
|---|---|---|
| All three consensus modes run in sim/ | **Met** (engine level) | §2, `crates/dag-bft/tests/modes_sim.rs` |
| Reorg and DAG commit tests pass | **Met** | §1, §2 |
| GPU/CPU parity passes (or skips with a reason) | **Skips with a reason** — no GPU on this machine | §4 |
| `reports/04-consensus.md` has measured TPS and latency | **Ordering throughput and round latency measured in the simulator; TPS as defined by the Production Standing Orders is not measurable** — the node does not run DAG-BFT | §3 |

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
