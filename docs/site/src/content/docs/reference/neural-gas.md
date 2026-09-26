---
title: 'Neural base-fee gain'
editUrl: false
# GENERATED from docs/neural-gas.md by scripts/ingest.mjs. Edit the source, not this.
---
**Status: RESEARCH.** Nothing in consensus calls it. The fee market it extends
is itself switched on nowhere (`FeeConfig::DISABLED`, [fee-market.md](/reference/fee-market)),
and the neural rule has a second switch, `neural_activation_height`, which is
`u64::MAX` in every shipped configuration.

| Piece | Where |
|---|---|
| The integer network, the six features' order, compiled weights | `crates/fee-market/src/model/` |
| The rule and its envelope | `crates/fee-market/src/rule.rs`, Kani proof in `crates/fee-market/src/proofs.rs` |
| Extracting features from a block | `crates/node/src/neural_gas/` |
| Demand simulator, training, quantization, evaluation | `neural-gas-trainer` (never linked by the node) |
| Constants shared across the two halves | `crates/node/tests/neural_fee_tests.rs` |
| Cost | `crates/node/benches/gas_predictor.rs` |

## What the brief assumed, and what is true

| Brief | This chain | What was built |
|---|---|---|
| "zkML-driven" | Every validator holds the features and the weights. A proof of inference proves only what re-running proves; one halo2 verification costs as much as re-running a ~5.3 M-multiply model (`docs/zkml.md`), and the network here is 112 | **native integer inference**, no proof |
| a quantized network "inside the node state machine" | Invariant 20: no floats and no ONNX runtime in a consensus rule | `i16`/`i32` weights, `i64` accumulators, bound by construction. Training and quantization happen in `neural-gas-trainer`, which the node does not link |
| trained on "historical DAG vertex metrics" | The DAG is the proof-of-work dataset, not a transaction graph. `genesis.json` allocates nothing; there is no history | a **synthetic** demand simulator. The model learns the simulator's assumptions, which are written down in `bins/neural-gas-trainer/src/simulator.rs` |
| memory allocation depth | allocator- and node-dependent: a fee reading it would split the chain | declared contract fuel per byte |
| state access overlap | execution builds one merged overlay per block | accounts named by more than one transaction (static) |
| inter-shard dependencies | no shards execute (`blockgraph` is RESEARCH) | transactions whose accounts span two `blockgraph` shards — hypothetical, and labelled so |
| "per-byte base fees" | correct: there is no block gas | the base fee stays per byte |
| **replace** the linear curve | EIP-1559 is a controller whose ±1/8 bound is its security | the network sets a **gain** on EIP-1559's step, inside its envelope |
| sub-second validation under 100,000 tx/s bursts | a hybrid signature is 11,165 bytes and ~0.18 ms to verify: 100,000 tx/s is 1.1 GB/s and ~18 CPU-seconds per second. A block holds ~650 signed transactions; `blockgraph`'s structural ceiling is ~8,738 tx/s | the benchmark **measures** the fee rule's added cost per block and per burst, and admission throughput, and asserts nothing it cannot back |

## The rule

```text
linear = parent · |size − target| / target / denominator
over target:   step = linear · clamp(gain, 1.0, 2.0)   next = parent + max(step, 1)
under target:  step = linear · clamp(gain, 0.0, 1.0)   next = parent − step
at target:     next = parent
next = max(next, floor)
```

`gain` is the network's output, `[0, 20,000]` basis points; 10,000 is
EIP-1559 exactly, everywhere.

### Why the gain is one-sided

The network's inputs are block contents, and the block producer chooses block
contents. 80% of the base fee burns, so a producer gains from a lower fee.

The first version of this rule let the gain range over `[0, 2]` in both
directions and capped every move at one `1/denominator` step. Review on
2026-09-14 found the lever that left: fill a block over target with features
that drive the gain to zero, and EIP-1559's forced one-eighth rise becomes a
rise of **one unit** per block. From a base fee of 1,000 that is roughly 125
blocks to price a burst instead of 8. The fee never fell, but a producer could
hold it down.

A producer wants rises slow and falls fast, so the network may only make rises
faster and falls slower. For every weight file:

1. **Never below EIP-1559.** The fee is at least what the linear rule sets on
   the same inputs. Whatever features a producer writes, the lever points the
   wrong way for them.
2. **Direction.** Over target, the fee never falls. Under target, it never
   rises. At target it holds.
3. **Speed.** A rise is at most twice EIP-1559's; a fall at most EIP-1559's.
4. **Floor.** Never below `floor`.

The Kani proof `the_neural_rule_never_leaves_the_envelope` covers every input.

What this gives up: the network cannot cut the fee faster than EIP-1559 when
demand collapses, and a rival producer who wants fees *higher* can steer them up
— bounded by the doubled rise.

### Integer bounds

Features are clamped to `±4.0` in Q16 (`|x| ≤ 2^18`), weights are `i16` in Q12
(`|w| ≤ 2^15`), biases `i32` in Q28. A hidden unit's accumulator is below
`2^36`, the output's below `2^44`, the scaled gain below `2^58`. The bound
follows from the types, and a test drives every weight to its extreme.

## Training

`cargo run --release -p maya-neural-gas-trainer` regenerates
`crates/fee-market/src/model/weights_v1.rs` deterministically (SplitMix64, fixed
seed, single-threaded float arithmetic). `--check` fails if the committed file
differs from a fresh run — exact on the platform that generated it; the file,
not the float run, is what consensus would read.

The simulator's assumptions, all of which the model inherits:

- demand in bytes has unit price elasticity;
- a calm baseline near 80% of target with small noise;
- bursts of three kinds — transfer, DEX, contract — each with its own duration,
  size multiplier, overlap, fuel and shard spread;
- blocks cap at twice the target.

The label for each block is the gain that would have set next block's demand to
exactly the target, given the next block's true demand. Training states come
half from the linear rule and half from random gains, so the model sees the
states its own decisions would produce.

### What training produced (2026-09-14)

40,000 simulated blocks, 12 epochs. Closed loop on a separate seed, 20,000
blocks, for both versions of the rule:

| Rule | float gain MSE | mean \|size − target\| / target | mean \|Δfee\| / fee | blocks at cap | envelope violations |
|---|---|---|---|---|---|
| linear (EIP-1559) | — | 0.1427 | 0.0174 | 2.41% | 0 |
| neural, two-sided gain (withdrawn) | 0.418 | 0.1103 | 0.0204 | 2.59% | 0 |
| **neural, one-sided gain (committed)** | 0.141 | **0.1394** | 0.0207 | **1.85%** | 0 |

Read all of it.

- **The safe rule gave up most of the improvement.** The two-sided gain held
  blocks 23% closer to target, largely by *lowering* fees quickly after bursts —
  the same freedom that let a producer hold fees down. Clamping falls to at most
  EIP-1559's pace leaves a 2.3% improvement in size deviation.
- **What remains is fewer capped blocks.** 1.85% of blocks at the size cap
  against 2.41%: a quarter fewer. The network learned to take a larger step at
  the start of a burst, which is the one direction it is still allowed.
- **It costs fee volatility.** The fee moves 19% more per block than under
  EIP-1559.
- **The lower MSE is not a better model.** One-sided labels are clamped to
  narrower ranges, so there is less to get wrong.

On this simulator, the learned gain is a small improvement bought with more
fee movement, and the manipulation resistance is what made it small. That is
the finding. Whether it is worth a second fee rule is a policy question the
simulator cannot answer. `bins/neural-gas-trainer/tests/evaluation_tests.rs` pins
only the direction of the size result, so a retrain that loses fails loudly
instead of shipping quietly.

## Measured (2026-09-14)

`cargo bench --bench gas_predictor -- --quick`, release, one machine. Quick
mode takes few samples, so read the last row as "no difference visible",
not as a ranking.

| Benchmark | Time | Throughput |
|---|---|---|
| `neural_fee/gain` — one inference, 112 multiply-accumulates | **18.6 ns** | |
| `neural_fee/block/64` — extract features + gain + rule | 102 µs | |
| `neural_fee/block/650` — a full 8 MiB block | **1.05 ms** | |
| `neural_fee/burst_100k` — 154 full blocks | 160 ms | 626,000 tx/s |
| `mempool/admit_signed` — real admission, signatures verified | 15.0 ms / 64 tx | **4,281 tx/s** |
| `block_validation/linear` — 64 signed transfers + linear rule | 20.6 ms | |
| `block_validation/neural` — same block + features + neural rule | 19.0 ms | |

What the numbers say:

- **The network is free.** 18.6 ns is noise beside anything else a block
  does. Nearly all of a block's 1.05 ms is feature extraction, and nearly all of
  that is `Transaction::sender()` hashing both public keys once per transaction.
  A validator has already derived every sender when it checked signatures;
  passing those in instead of recomputing them would remove most of it. Not done
  here, because it would change `extract`'s signature to take something only the
  apply path has.
- **The fee rule does not bound a burst; signatures do.** Computing features
  and fees for 100,000 transactions takes 160 ms. *Admitting* them takes about
  23 seconds single-threaded, at 4,281 tx/s — and every one of those
  transactions is 11,165 bytes of signature before it is anything else. The
  brief's "sub-second validation under 100,000 tx/s" is not a property a fee
  rule can have or lose.
- **Validation is unchanged within measurement.** The rule adds about 0.1 ms to
  a 64-transaction block against ~20 ms of apply.

## Before anyone picks an activation height

1. Real demand. Every number the trainer reports is about the simulator.
2. Wiring the fee market into the state transition at all
   ([fee-market.md](/reference/fee-market), "What activation requires").
3. Features become consensus the moment the rule activates. A change to
   `extract` after that is a hard fork, as is a new weight file — which is why
   a second model would be `MODEL_V2` behind its own activation height, never
   an edit to `MODEL_V1`.
