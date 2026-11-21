# ADR-142: Contract gas caps per transaction and per block, native work priced in fuel, lazy per-key contract storage

**Status:** Accepted, 2026-11-14 (Eric, 2026-11-14 MCQ: "cap + price now"). Re-freeze change: consensus behaviour, lands before genesis. Amended 2026-11-14 after the PR #549 review (see the end): shape-priced compiles, priced memory, failed work counted, a sealed budget, warm and cold reads, ceiling 300 M. Amendment 4 (2026-11-21): the re-freeze review's R-2 verified, R-5 scan skipped and priced, R-6 boundary measured (build- and ISA-dependent), R-10 wording, the caps wait the validity-rule lead.
**Date:** 2026-11-14
**Related:** [ADR-034](ADR-034-vm-reference-types-off.md) (VM feature lockdown), [ADR-072](ADR-072-max-block-bytes.md) (block byte cap), [ADR-029](ADR-029-fee-market-live.md) (fees price bytes), [ADR-062](ADR-062-state-layout-before-genesis.md), ADR-131 (P-25 state pricing / rent, options, PR #472), ADR-135 (parameter tags 11–16, PR #477); invariants 7, 12, 17, 21, 24, 25, 33; `spec/10-vm-gas.md` (GAS-1..GAS-6)

## Context

Checked in the tree on 2026-11-14 (`a0ee0ad6`):

- **`gas_limit` was sender-chosen with no cap.** `ContractCall::gas_limit` is a
  `u64` on the wire; `StateDB::call_contract` handed it straight to
  `Vm::execute`. A call naming `u64::MAX` was a valid transaction. Nothing
  bounded a block's contract work either, so the block-byte cap (ADR-072) was
  the only limit, and a few hundred bytes of transaction can name any amount
  of work. `ParameterKey::VmDefaultGasLimit` existed and nothing read it.
- **Native work was unmetered.** Gas is wasmtime fuel, one unit per wasm
  operator. A host function costs one unit (its `call`) whatever it does:
  `storage_write` cloned up to 4 KiB into a map, `emit_event` copied a topic
  of *unbounded* length (only the payload was size-checked), `get_balance_u256`
  wrote 32 bytes. Measured below, these ran at 10–200 ns per unit of fuel
  against 0.3–0.7 ns for guest code.
- **Every call loaded the contract's whole storage.** `call_contract` ran
  `contract_storage()` — a prefix scan of every key the contract ever wrote —
  then merged the block's writes by iterating *every* contract's overlay
  writes. Unpriced, and growing with the contract's history.
- **Every call built a new engine and compiled the module.** `Vm::new()` per
  call (measured 1–19 ms) with a fresh module cache, so Cranelift ran on the
  whole module on every call. Unpriced.
- **Deployments compiled unpriced**, bounded only by block bytes: ten
  512 KiB modules fit in a 5 MiB block.

wasmtime 48.0.8 already charges `memory.copy` / `memory.fill` /
`memory.init` one unit per byte (`VariableOperatorCost`), so bulk memory is
not an amplifier here (measured 0.011–0.013 ns per unit).

## Decision

### 1. Two governed caps with compiled bounds

`ParameterKey::VmMaxTxGas` (tag 17, `vm.max_tx_gas`) and
`ParameterKey::VmMaxBlockGas` (tag 18, `vm.max_block_gas`). Tags 11–16 are
ADR-135's. Both default to 250,000,000 and are bounded to
`[239,979,600, 300,000,000]` (the ceiling was 500 M until the amendment
below), checked when a proposal is made and again when
it executes (invariant 12). They change only through a governance proposal
and its timelock — the scheduled path; no transaction, council act or node
setting moves them. They are read from the parameter table through the
overlay at the call site (invariant 17).

- **Per transaction (GAS-1):** a call's `gas_limit` must be at most
  `min(vm.max_tx_gas, vm.max_block_gas)`. Refused in block validation
  (`GasAboveCap`) and at mempool admission, before the signature check.
- **Per block (GAS-2):** a block's contract gas starts at 0; a call needs
  `gas_limit ≤ vm.max_block_gas − spent` (`BlockGasExhausted`), and adds the
  gas it used. A deployment adds its cold price under the same rule, before
  it compiles. A revealed sealed action that fails still adds the gas it
  used, because it ran; the old code discarded the failed attempt's overlay,
  which would have let revealed calls run to their limit outside the budget.
  Since the amendment revealed actions count against a budget of their own,
  and the builder keeps the gas of the failed calls it tries.
- The block builder already trial-stages each transaction and passes over one
  that fails, so a call that does not fit is left for a later block, like an
  over-size transaction under ADR-072.

The floor is `call_fuel(512 KiB, cold) + 4,000,000`: the largest module a
deployment may carry stays callable by a plaintext call under any cap a vote
can set. (Not by a revealed sealed call cold: its cap is the sealed budget,
a quarter of the block cap; see the second amendment.)

### 2. Native work priced in fuel, charged before the work (GAS-3, GAS-4)

`crates/vm/src/gas.rs`. One rule: **about 1 ns of measured native work per
unit of fuel, for the slowest shape of each kind of work that was
measured**. This table is the first cut, on ordinary shapes; the amendment
below adds the adversarial shapes and the prices they forced.

| Charge | Fuel | Measured (dev profile, this machine) |
|---|---|---|
| per call, before instantiation | 50,000 | warm call of an empty contract: 47.2–49.5 µs |
| per module byte, first call in the block (compile) | 450 | cold call: 222–432 ns per byte over three module shapes |
| per module byte, later calls in the block | 1 | instantiation copies data segments (bulk memory 0.013 ns/B) |
| every `env` host call | 600 | 93–562 ns per call (`block_height` … `get_balance_u256`) |
| `storage_read` lookup | 5,000 + 1/key byte | in-memory host 413–650 ns; RocksDB node read: see below |
| `storage_read` value | 1/byte | (650 − 413) ns / 4,224 B ≈ 0.06 ns/B |
| `storage_write` | 5,000 + 240/byte | priced by persistence, see below |
| `emit_event` | 600 + 1/byte of topic and data | 2.6 µs for 8 KiB |

Each is charged from the call's own arguments, before anything is read or
copied; sizes over the limits are refused before the charge, and since the
amendment the refusal ends the call. A charge that
cannot be paid traps out of fuel, exactly like running out in the guest. The
charges are host-side `set_fuel` arithmetic, so they are identical under
Cranelift and Pulley (`tier_differential_tests.rs`, corpus includes storage
writes and events).

**Storage writes are priced by what a byte costs forever**, not by the
nanoseconds of the write: at 240 fuel a byte, a block of 250 M gas writes at
most ~1 MiB of new contract state (`250e6 / 240 = 1.04e6` bytes), against the
5 MiB a block can carry as transaction bytes.

**Refunds (GAS-6):** none. Gas is not billed in coin, so there is nothing to
refund; an overwrite or empty write pays like any write. A later fee per gas
that refunds cleared storage must cap the refund at a fraction of the gas
used.

### 3. Warm and cold calls, decided by the block, not by a cache

The node now holds one `Vm` per `StateDB`. The first call to a contract in a
block is **cold**: it compiles the module and pays 450 fuel a byte. The
compiled module is kept in the block's overlay (`Overlay::vm_modules`), and
later calls to that contract in the same block are **warm**: they reuse it
and pay 1 fuel a byte. Which applies is a function of the block's
transaction order only. The module cache may make a cold call faster than
its price; nothing makes a warm call slower, because the overlay holds the
module. Builder trials clone the overlay, so a trial's modules go with it.

### 4. Lazy per-key storage

`ChainHost` reads committed storage one key at a time through a shared
`Arc<rocksdb::DB>` (`StateDB::db` became `Arc<DB>`), behind a map of only
this block's earlier writes to the contract, taken as an ordered range of the
overlay. A RocksDB error during a lazy read fails the call as `Storage` after
it returns, never as a missing key. `lazy_storage_reads_exactly_what_the_whole_storage_load_did`
(`crates/node/src/state/vm_exec_tests.rs`) checks every probe key on 40
randomized storages — committed keys, overlay overwrites, absent keys, the
empty key, prefix-nested keys, and neighbouring contracts whose ids differ in
the last byte — against the old whole-storage load, before and after the call
writes.

## The derivation: a full block of worst-case work

`vm.max_block_gas` × (ns per fuel of the slowest priced work) = worst-case
contract time per block. With every charge above at ≤ ~1 ns per unit,
250,000,000 gas is ~250 ms, a quarter of the 1 s block time, leaving the
rest for signatures, the state root, the commit and the network. A block
producer stages the block twice (selection, then the preview root), so its
worst case is about twice that.

Measured end to end, `apply_block` on a block filled to the cap with one kind
of work (`crates/node/tests/vm_gas_timing_tests.rs`, dev profile — the node
and VM unoptimised, so a release node is faster):

| Block (all at the 250 M cap) | Txs | `apply_block` | ns per unit of budget |
|---|---|---|---|
| host calls (`get_balance_u256` loop) | 10 | 229.4 ms | 0.918 |
| deployments, ten distinct 54 KB compile-heavy modules (243.9 M) | 10 | 249.6 ms | 1.023 |
| cold calls to those ten modules (243.9 M) | 10 | 230.6 ms | 0.945 |
| committed storage reads, 1-byte values (RocksDB hits) | 10 | 78.4 ms | 0.313 |
| guest compute (`i64.div_u` loop) | 1 | 36.5 ms | 0.146 |
| storage writes, 230 × 4 KiB = 920 KiB of new state | 10 | 9.7 ms | 0.039 |

Run `cargo test -p custom-l1-node --test vm_gas_timing_tests -- --nocapture
--test-threads 1`, 2026-11-14, on the owner's Windows 11 PC (shared with
other builds), dev profile. Each figure includes the block's signature
checks, its state root and its commit. The worst block is ~250 ms, so the
derivation's target holds within a few percent, and every block is inside
the 1 s block time with ~4× headroom.

The RocksDB reads ran against a warm memtable and block cache. A read that
misses the block cache and goes to disk costs more than its 5,000 fuel; that
is the case most likely to break the rule on a slow disk, and it is why the
read base is 5,000 rather than the ~600 the in-memory host measured.

## Consequences

- A call to a 512 KiB contract costs ~236 M gas cold, so one such call fills
  most of a block; a 20 KiB contract costs ~9 M cold and ~70 K warm. This is
  the real cost of compiling: before this change the node paid it on every
  call, unpriced.
- Genesis and chain id are unchanged by this ADR alone; it is consensus
  behaviour and rides the planned re-genesis. The parameter table encodes only
  explicit entries, so the two new keys do not move the genesis state root.
- `vm.default_gas_limit` stays in the table and is still not read.

## Not done, and why

- **No fee per unit of gas.** Gas still bounds time and state growth; it does
  not move coin. Pricing gas in coin is an economics decision (CLAUDE.md:
  ask Eric), and the byte-priced fee market (ADR-029) is what a call pays.
- **A failing plaintext call is still free to submit.** Invariant 7 / STF-8:
  a failing transaction fails its block, so a producer drops it after a trial
  run, and it pays nothing. Since the amendment the gas it ran counts against
  the block being built, so failed calls cost a builder at most one block's
  budget; charging their sender means including them with their fee, which
  changes STF-8 and is a separate decision (raised with Eric).
- **The contracts layer of the state root is still folded over all contract
  code and storage every block** (`merged_contracts`, ROOT-5). Per-block cost
  grows with total contract state, and no gas price bounds that: the write
  price limits how fast state grows (~1 MiB a block), not the fold's cost. An
  incremental contracts root (as ADR-062a did for accounts) is the fix and is
  open.
- **Timing is from one machine**, under load from other builds. A slower
  validator validates the same block more slowly, never differently: every
  charge is deterministic. Re-measure with `gas_price_calibration_tests` and
  `vm_gas_timing_tests` before moving either cap.
- **No `storage_delete` host function exists**; a zero-length write is the
  clear and is priced as a write.

## Amendment, 2026-11-14: the PR #549 review (Eric 2026-11-14 MCQ, same decision)

The first cut was reviewed before merge. Its derivation rested on benign
shapes and on success paths, and the review found work that ran below its
price or outside the block's budget. Every finding is fixed here or recorded
below as open with its reason. Measurements are from this PC (Windows 11, the
owner's machine, shared with other builds) on 2026-11-14;
`crates/vm/tests/gas_adversarial_calibration_tests.rs` and
`crates/node/tests/vm_gas_timing_tests.rs` (both `#[ignore]`d: timing) print
them, and `reports/sessions/2026-11-14-vm-gas-caps-review.md` holds the runs.

### What changed

| Finding | Fix | Pinned by |
|---|---|---|
| Compile time is not a function of length: 200 functions of 50,000 locals compiled in 678 ms (release) against a price of 0.78 M, 40,000 `if`s in one function in 10.4 s, 20,000 empty functions in 1.06 s | `crate::shape`: a linear pre-pass before any compile counts functions, declared locals, control operators and calls, refuses more than 1,024 locals or 2,000 control operators in one function and more than 10,000 functions, and the cold price adds `125,000 × functions + 170 × locals + 30,000 × controls + 8,000 × calls`; a module whose cold price is above 235,979,600 is refused at deployment, so the governance floor still keeps every deployed module callable | `shape.rs` tests, `gas_hardening_tests`, `shapes_at_the_limits_compile_inside_their_price` (release) |
| A host call that failed its size check, or a 65th event, was free and did not stop the guest | `runtime::refuse`: every recorded host failure traps at once; the memory limiter traps too | `a_refused_host_call_ends_the_call`, vector `golden-oversized-key-ends-the-call` |
| `emit_event`'s topic had no bound | `MAX_EVENT_TOPIC_BYTES` = 256, checked before the charge | `an_event_topic_over_the_cap_is_refused` |
| Fresh linear memory was unpriced: 11–15 ns per unit of fuel touching new pages | `MEMORY_PAGE_FUEL` = 6,000 per page held, charged when the call returns; a call may hold only the pages its gas can pay for (GAS-7) | `a_call_pays_for_the_linear_memory_it_holds`, `a_call_cannot_hold_more_memory_than_its_gas_pays_for` |
| A deployment that compiled and then failed its import check added no gas | the price is counted before `validate` | `a_deployment_that_fails_validation_still_spends_its_compile`, `a_sealed_deployment_that_fails_validation_still_spends_its_compile` |
| A failed call in block selection threw its gas away, so K failing calls cost every DAG-BFT validator K × the cap | contract calls and deployments stage on the overlay with the transfer undo path; a failed one leaves its gas spent | `a_call_that_fails_while_the_block_is_built_still_spends_its_gas` |
| A failed revealed call was charged its whole `gas_limit`, so one envelope naming the cap voided every later sealed call at its height | a failed call spends `Execution::gas_used`, the gas it ran (its limit only when it ran out) | `a_failed_call_spends_the_gas_it_used_not_the_gas_it_named`, `a_failed_call_reports_the_gas_it_used` |
| Revealed actions ran on what plaintext left, so one gas-filling plaintext call voided every sealed call due in that block | revealed actions get their own budget, `vm.max_block_gas / 4` (GAS-2) | `a_plaintext_call_that_fills_the_block_gas_does_not_void_a_due_sealed_call`, `a_sealed_call_over_the_sealed_budget_is_refused_before_it_runs` |
| Every call copied the block's earlier writes to its contract; every builder trial and sealed attempt cloned all staged contract storage | `Overlay::contract_storage` is an `Arc`'d map the host reads through; contract trials and contract reveals no longer clone the overlay | `lazy_storage_reads_exactly_what_the_whole_storage_load_did` (nothing preloaded), `a_block_that_writes_then_calls_many_times_validates_inside_the_block_time` |
| The read base was measured only on warm reads; the node's RocksDB had no bloom filter | the block's first read of a committed key pays `STORAGE_COLD_READ_FUEL` = 95,000 more (decided by the block's order: keys the block wrote or its applied calls read are warm). A bloom filter was tried and left out: with a cache too small to hold a whole-file filter it made every hit reread it (580 µs against 73 µs) | `a_committed_key_is_cold_once_per_block_and_only_where_it_reaches_disk`, `cold_point_reads_against_the_read_price` |
| The cap's ceiling, 500 M, let a vote push a producer past the block time | ceiling 300 M | `the_gas_caps_cannot_be_voted_unbounded_or_below_the_largest_module`, vectors |
| The 256-entry module cache could hold 256 × 512 KiB modules | also bounded to 32 MiB of module bytes (node-local, not consensus) | `cache_tests` |
| Timing tests asserted wall time on shared CI | `#[ignore]`d; they print, and the session report records the run | — |
| Vectors recomputed constants with the code's own functions | golden `gas_used` for real executions (write, read hit and miss, event, growth, a trap, out of gas, an oversized key, memory past the allowance), replayed by the node on every CI platform | `the_spec_vectors_hold` |

### The revised derivation

The rule is still about 1 ns of measured native work per unit of fuel, now
for the slowest shape *measured* of each kind of work, release build:

| Work, at the limits | Measured | Profile | ns per unit of its price |
|---|---|---|---|
| ~1,760 empty exported functions | 203 ms for 229.9 M | release | 0.88 |
| 2,000 `if`s in each of 3 functions | 168 ms for 196.6 M | release | 0.86 |
| 2,000 `loop`s in each of 3 functions | 158 ms for 188.5 M | release | 0.84 |
| 20,000 `call`s in one function | 124 ms for 178.2 M | release | 0.70 |
| 200 functions × 1,024 locals | 27 ms for 60.5 M | release | 0.45 |
| real contracts: `nft_game.wasm`, `token_swap.wasm` | 4.9 ms / 4.2 ms for 5.3 M / 5.9 M | release | 0.93 / 0.72 (4.5 / 3.6 at the length-only price) |
| touching 256 fresh pages | 4.9–5.7 µs a page over an empty call | release | 1.08–1.14 per unit of the call |
| a cold storage read (1 B values, direct I/O, 1 MiB cache) | 73–113 µs | release | 0.73–1.13 |

Each line is `shapes_at_the_limits_compile_inside_their_price` or
`calibrate_fresh_memory_pages` (`--release`), or `cold_point_reads_against_the_read_price`.
Whole blocks at the 250 M cap, `apply_block` including signatures, the state
root and the commit (`vm_gas_timing_tests`, **dev** profile: the node
unoptimised, Cranelift with debug assertions):

| Block | `apply_block` | ns per unit of gas |
|---|---|---|
| ten deployments of ~50 KB compile-heavy modules (241.5 M) | 347.3 ms | 1.44 |
| 134 calls each touching 16 MiB of fresh memory | 330.1 ms | 1.32 |
| ten cold calls to those modules | 294.4 ms | 1.22 |
| host calls (`get_balance_u256`) | 233.2 ms | 0.93 |
| 19,745 small writes, then 200 warm calls to the same contract | 172.4 ms | 0.53 |
| guest compute (`i64.div_u`) | 39.0 ms | 0.16 |
| storage reads at the cold price (memtable hits) | 19.4 ms | 0.08 |
| storage writes, 920 KiB of new state | 10.1 ms | 0.04 |
| selection of 8 failing calls at the cap (DAG-BFT builder) | 162.9 ms | — |

The dev figures are above 1 ns because of the profile; the release rows are
the derivation's basis. Every block is inside the 1 s block time on this
machine in the dev profile. The contracts root fold, which no price covers,
measured 128 ms over 100,000 committed contract entries and 533 ms over
400,000 (dev); see the consequences below.

A block now spends at most `vm.max_block_gas` on plaintext contract work
plus a quarter of it on revealed sealed actions: 312.5 M at the default
(~313 ms of priced work), 375 M at the 300 M ceiling. A DAG-BFT producer
also runs selection, which since this amendment never runs more than
`vm.max_block_gas` of failed and applied contract work together: ~563 ms at
the default, ~675 ms at the ceiling, inside the 1 s block time.

What this does not cover, stated so nobody reads the table as a proof:

- **The end-of-block bisection.** `consensus/bft/builder.rs` may select up
  to 9 times and build up to 32 times for one anchor
  (`MAX_END_OF_BLOCK_DROPS`, `END_OF_BLOCK_PROBES`, audit finding 3) when a
  transaction stages but fails an end-of-block pass. Each build re-runs the
  block's contract work, so a sub-DAG built to fail that way can cost up to
  about 9 × 250 M + 32 × 312.5 M ≈ 12 G gas of work on one anchor, ~12 s.
  Pre-existing and not contract-specific; bounded, not fixed here. Open, for
  Eric.
- **Shapes not measured:** other CPUs (a validator on a slower core compiles
  more slowly, never differently), ARM, Pulley's compile, real compiled Rust
  contracts as a corpus, functions that escape through tables (priced as if
  exported), deep nesting past 2,000, and compile-time interactions between
  the counted shapes. `shapes_at_the_limits_compile_inside_their_price`
  asserts the measured worst at the limits in release builds only; the dev
  profile keeps Cranelift's debug assertions and runs it about 10× slower.
- **Cold reads** were measured with direct I/O on this PC's SSD, not on a
  slower disk class. With 4 KiB values and a 1 MiB cache the SST index block
  did not fit and every read reread it: 1.4–2.0 ms, far past the price. The
  node's default 512 MiB cache holds index blocks for tens of GiB of state;
  partitioned indexes would remove the case and are node-local tuning, not
  done here.

### Consequences, added

- Compiling is now priced closer to what it costs, so cold calls cost more:
  every function 125,000 and every branch 30,000. A module of 1,000 small
  functions costs ~125 M cold before its bytes. A module deploys only when
  its whole cold price is at most `MAX_COLD_CALL_FUEL`, the length-only price
  of a 512 KiB module: its shape price must fit in `(524,288 − len) × 450`, so
  a 512 KiB module cannot deploy with even one function, and a module of
  `len` bytes holds at most about `(524,288 − len) × 450 / 125,000`
  functions. (The first text said "a 512 KiB module with more than ~1,800
  functions"; that was wrong.) Warm calls are unchanged.
- **Contract calls can still be crowded out for one byte fee a block.** Gas
  has no coin price and `vm.max_tx_gas`'s floor sits near the block cap, so
  one call that runs to ~250 M successfully fills the block's plaintext
  budget, and every other call waits for a later block. Sealed calls no
  longer can be (their own budget). Pricing gas in coin, or a per-sender
  share below the block cap, is an economics decision: raised with Eric.
- **Failed calls are free and stay pooled.** A failing plaintext call is
  never included, so it pays nothing (STF-8), and the DAG-BFT driver keeps
  dropped transactions pooled (`bins/maya2c-node/src/bft.rs`), so it is
  re-proposed every block. Since this amendment its gas counts against the
  block it is tried in: bounded work, but contract calls ordered after it
  can be crowded out. Evicting a transaction after it fails execution is a
  node-policy change outside this PR; including failed calls with their fee
  changes STF-8. Both raised with Eric.
- **Revealed actions share their quarter in envelope-key order.** An
  envelope due at the same height that uses the whole quarter leaves later
  sealed calls at that height out (consumed, BlockGasExhausted). It costs
  that envelope's sender its fee and the gas it really runs; keeping such a
  reveal pending for the next height instead would publish its plaintext a
  block early, which is the front-running sealing prevents.
- **The contracts layer of the state root** is still folded over all
  contract code and storage every block (ROOT-5). Its cost grows with total
  contract state, at up to ~1 MiB of new state a block, and no gas price
  bounds it: the write price limits growth, not the fold. Measured (dev):
  128 ms at 100,000 entries, 533 ms at 400,000. A block of the cheapest
  writes adds ~48,000 entries (250 M / 5,240), so about ten such blocks take
  the fold past half a second on this machine, and a few more past the block
  time, for byte fees only. It belongs with the incremental state root
  (ADR-062a; PR #540 covers accounts only). **This blocks genesis**: either
  the contracts layer becomes incremental, or contract state gets a hard
  ceiling. Raised with Eric.

## Amendment 2, 2026-11-14: the second review of PR #549 (Eric 2026-11-14 MCQ, same decision)

A second review of the PR head (f6652709) found one compile shape the first
amendment did not count, two ways the gas a *failed* call reports was not the
work it did or not the same on every validator, and builder work outside
GAS-2's bound. Fixed here unless marked open. No new ADR number
(`adr-claims.txt`: 142b).

### What changed

| Finding | Fix | Pinned by |
|---|---|---|
| **Multi-value arity was not priced** (CRITICAL). wasmparser allows 1,000 params and 1,000 results per type; the review measured 1,500 `block (type [1000]->[1000])` compiling in 8.23 s for 49.6 M fuel (166 ns/fuel), and 200 chained calls of `[1000]->[1000]` in 1.24 s for 5.66 M (219 ns/fuel), release | `shape::scan` reads the type section: a function type may name at most `MAX_TYPE_ARITY` = 16 params and results together, at most `MAX_CALLS_PER_FUNCTION` = 10,000 calls per function, and the cold price adds `ARITY_FUEL` = 2,500 per value carried by every defined function's own type, every `block`/`if` and every call, and `LOOP_ARITY_FUEL` = 30,000 per value a `loop` carries (measured below) | `a_function_type_past_the_arity_bound_is_refused`, `typed_blocks_and_calls_are_counted_by_the_values_they_carry`, `calls_past_the_per_function_bound_are_refused`, `a_module_with_a_wide_function_type_is_refused_at_deploy`, vectors `golden-wide-type-refused`, `golden-typed-block-pays-for-its-values`, `shape-fuel-arity-16`, `shape-fuel-loop-arity-16` |
| **"About 1 ns of native work per unit" was false for compiles** (HIGH): the review measured 2.3–2.5 ns/fuel at arity 8–16, and the first run here found typed loops at 5.09 | every shape the arity bound still allows re-measured in release (below); typed loops repriced; best run under 1 ns on every shape, single loaded runs up to 1.5 | `shapes_at_the_limits_compile_inside_their_price` (release, `#[ignore]`d), `a_module_with_a_wide_function_type_is_refused_at_deploy` (the loop price) |
| **A stack overflow reported a platform-dependent gas** (HIGH). It fires on native frame size (ISA, Cranelift's detected flags), not fuel; since the first amendment a failed call's gas reaches the state root through the sealed budget and the BFT block through selection | a call that overflows the native stack reports `gas_limit + 6,000 × max_pages`: its limit and every page its gas could have paid for, a function of its gas alone | `a_call_that_recurses_forever_reports_its_whole_limit`, vector `golden-stack-overflow-uses-its-limit-and-every-page` |
| **A failed call's figure was capped at its limit** (MEDIUM). Memory is charged after the compute, so a call that ran out of gas also held up to 256 pages of page faults and zeroing | a failed call reports what it ran plus its memory, uncapped (`Execution::gas_used`; `Outcome::gas_used` of a success is unchanged and never above the limit) | `a_failed_call_reports_the_memory_it_touched_on_top_of_its_limit`, `a_failed_call_reports_the_gas_it_used`, vectors `golden-out-of-gas-uses-its-limit`, `golden-out-of-gas-after-touching-memory` |
| A call refused because `gas_limit` is below its upfront price counted nothing, after a read of up to 512 KiB of code and a full shape scan | it counts `50,000 + len` (GAS-2) | `a_call_below_its_upfront_price_still_counts_its_code_read_and_scan` |
| A sub-DAG can carry several versions of one sender's nonce; each failing one was tried | selection tries at most one failing contract trial per sender (one that did contract work; a nonce refusal, such as a replay, does not count) | `one_sender_gets_one_failed_contract_trial_per_block`, `a_replayed_old_transaction_does_not_shut_its_sender_out` |
| Mempool admission scanned an unsigned deployment's whole module before its signature, and took deployments priced in `(MAX_COLD_CALL_FUEL, block cap]` that `Vm::validate` always refuses | before the signature, length only; after it, the full cold price, refused above `min(block cap, MAX_COLD_CALL_FUEL)` | `a_deployment_above_the_deploy_price_is_refused_at_admission` |
| `settle_sealed` could return early with the overlay left in sealed mode and the plaintext gas dropped | the loop is a helper; the mode and the gas are restored on every path | (structure; every caller discards the overlay on `Err`) |
| "every module GAS-3 lets deploy stays callable" did not hold for revealed sealed calls; "a 512 KiB module with more than ~1,800 functions cannot deploy" was wrong | GAS-5 and the consequences above corrected | vector `admit-sealed-call-to-a-module-priced-over-the-sealed-budget` |
| Instantiation shapes that scale with entries (globals, data segments) were not measured | measured, below | `calibrate_instantiation_shapes` (release) |
| `invariant 21` cited in `docs/architecture-vision.md` (retired) | cites ADR-142 / GAS-3, GAS-4 | — |
| Golden gas came from one x86-64 Windows machine | CI job `vm-gas-aarch64` replays the vectors, the hardening tests and the tier differential on `ubuntu-24.04-arm` when the VM or its vectors change | `.github/workflows/ci.yml` |

### Multi-value: the measurements

Release build, wasmtime 48.0.8 / Cranelift 0.135.8, this PC (Windows 11,
x86-64, shared with other agents' builds), `Vm::validate` timed best of
three, 2026-11-15. Command:
`cargo test --release -p maya-vm --test gas_adversarial_calibration_tests --
--ignored --nocapture --test-threads 1 shapes_at_the_limits`, through the
build slot. Three runs; the first before the loop price below existed.

A first run with `ARITY_FUEL` = 2,500 on every carried value found one shape
still far outside its price: **typed loops**. 3 functions of 2,000
`loop (type [8]->[8]) end` compiled in 726.8 ms for 142.9 M (5.09 ns/fuel).
A scratch series (not committed) showed why: a loop's params cost compile
time per value that grows with the number of loops in the function, so the
total grows with the square of it.

| loops in one function, `loop (type [k]->[])` | k = 8 | k = 16 |
|---|---|---|
| 500 | 27.0 ms | 44.4 ms |
| 1,000 | 93.5 ms | 165.8 ms |
| 2,000 | 416.5 ms | 823.3 ms |

That is about 11.75 ns per value per loop in the function: 23.5 µs a value
at the 2,000 controls a function may hold. Blocks with the same types cost
less (2,000 `block (type [1]->[1])` in one function: 73 ms, against 163 ms
for the same loops) and stay inside their control price; how blocks grow
with their count was not measured. So a `loop`'s values are counted apart (`ModuleShape::loop_arity`)
and priced `LOOP_ARITY_FUEL` = 30,000 each, above the 23.5 µs worst; blocks,
`if`s, calls and function types stay at `ARITY_FUEL` = 2,500 on top of their
control or call price.

With both prices (ns of compile per unit of the whole cold price; "best"
is the lowest of the runs, "loaded" the highest; the loop rows exist only in
runs 2 and 3):

| Shape | Size | Price | best | loaded |
|---|---|---|---|---|
| 3 × 2,000 `block (type [8]->[8])` | 6,137 B | 142.9 M | 0.67 | 0.73 |
| 3 × 2,000 `loop (type [8]->[8])` | — | 1,022.9 M | refused at deploy | |
| 3 × 1,000 `if (type [8]->[8])` | 12,166 B | 205.8 M | 0.96 | 1.04 |
| 3 × 2,000 `block (type []->[16])` | 102,115 B | 186.1 M | 0.77 | 1.18 |
| 3 × 1,999 `br_if` carrying 8 | 24,193 B | 191.4 M | 0.83 | 1.00 |
| 3 × 2,000 `loop (param i32) (result i32)` | 6,121 B | 182.9 M | 0.67 | 0.96 |
| 416 `loop (type [8]->[8])` (most one module pays for) | 1,385 B | 213.0 M | 0.15 | 0.17 |
| 393 of them, each with a back edge carrying 8 | 2,888 B | 213.7 M | 0.16 | 0.19 |
| 416 of them nested | 1,385 B | 213.0 M | 0.14 | 0.14 |
| 4,424 `call` of `[8]->[8]` | 9,011 B | 216.7 M | 0.33 | 0.43 |
| 3,318 `call` of `[16]->[]` | 112,937 B | 210.4 M | 0.20 | 0.29 |
| `br_table` of 10,000 over 16 typed labels | 30,355 B | 17.5 M | 0.20 | 0.35 |
| 256 values live across 10,000 calls | 20,895 B | 89.7 M | 0.58 | 0.80 |
| 2,048 values live across 10,000 calls | 26,271 B | 92.1 M | 0.70 | 0.82 |
| 2,048 values live across 1,000 `br_if` | 13,259 B | 66.1 M | 0.57 | 0.66 |

The shapes the first amendment measured, in the same three runs: best 0.02
to 0.85, loaded up to 1.50 ("loops at the per-function cap", 0.62 best).
The two compiled example contracts (`nft_game`, `token_swap`, ~2.4 KB)
measured 1.05 and 1.06 in the one loaded run that included them: a fixed
~5 ms of engine work per compile that small modules pay through their
function and control prices.

What this supports, and what it does not:

- On every shape measured, the best run is under 1 ns of compile per unit,
  so a cold call at the 250 M cap compiles in about 250 ms. On this machine
  under other agents' builds, single runs reached 1.5 ns (375 ms per
  compile, ~750 ms for a producer that stages a block twice). The test's
  bound (`MAX_NS_PER_FUEL` = 1.5) is about the loaded figure; it does not
  prove the 1 ns rule, it catches a shape priced ten times too low, as the
  typed loops were.
- The review's own shapes are refused, not priced: `[1000]->[1000]` (a type
  over 16 values) and 20,000 calls in one function (over 10,000).
- Not measured: aarch64, other x86-64 ISAs, Pulley. CI job
  `vm-gas-aarch64` replays the gas vectors there; it does not time compiles.

Instantiation shapes that scale with entries, not bytes (warm call against
its warm price, `calibrate_instantiation_shapes`, release): 100,000
immutable globals 0.54 ms (0.99 ns/fuel), 80,000 mutable globals 0.57 ms
(1.27), 70,000 one-byte active data segments 0.06 ms (0.12), 1,000 passive
400 B segments 0.05 ms (0.12); an empty module 0.09 ms (1.86, the call base).
Each is under a millisecond and the warm price is mostly the call base, so
they are recorded, not priced apart.

### Builder work, restated

Applied contract work is at most `vm.max_block_gas`; failed and applied
together at most `vm.max_block_gas + 1,536,000` (the last failure's memory may
pass the budget). Selection still shares one counter between them. A separate
budget for failed work was considered, as the review proposed, and not
taken: a call is only known to fail once it has run, so a bounded failure
budget is the same lever (one call at the cap fills it and every later call
is not tried), and an unbounded one is unbounded builder time. Separate
counters would have raised the builder's worst case to about twice the cap
without letting one honest call through that the shared counter refuses.

### Open, and blocking genesis (BACKLOG.md G1–G3)

- **G1 — a failing contract call is free.** One call naming the cap, from one
  funded sender, leaves every contract call after it in the sub-DAG out,
  every block, for nothing; the one-trial-per-sender rule only stops the same
  sender's re-signed versions. The fix is to include a failed contract call
  with its fee and a nonce bump: a change to STF-8 and an economics decision,
  Eric's. Evicting transactions that failed execution from the pool helps and
  does not close it. Previously listed here as "raised"; it is a blocker.
- **G2 — the contracts root fold** (above): incremental contracts layer or a
  hard ceiling on contract state.
- **G3 — end-of-block bisection** re-runs contract work, ~12 G gas per anchor
  at worst: cache staged results per prefix or charge probes to a per-anchor
  budget.
- Not blockers: envelopes do not state their inner `gas_limit`, so reveals at
  one height share the sealed budget in envelope-key order and a seal is
  accepted without a gas check (state it in the outer envelope and reserve it
  at admission, a wire change); a sealed call to a module priced over the
  sealed budget can apply only warm. A contract's cold price is recomputed by
  a scan at each cold call; storing it at deployment would make both the call
  and admission O(1).
- **Recursion across ISAs**, pre-existing: a call that recurses deep enough to
  overflow on one ISA and not on another succeeds on one and fails on the
  other. `max_wasm_stack` is 512 KiB and Cranelift's ISA flags are not pinned
  (`config.rs`); pinning them narrows x86-64-to-x86-64 differences, not
  x86-64-to-aarch64. A bound on call depth enforced by fuel, not by the native
  stack, would close it; not done here.
- Timing tests stay `#[ignore]`d (shared CI runners); the deterministic
  guards in CI are the shape bounds and their unit tests.

## Amendment 3, 2026-11-18: failed work on its own count (Eric 2026-11-17 pre-approval, re-freeze bundle 3)

Two HIGH findings left open on PR #549. No new ADR number
(`adr-claims.txt`: 142c). This supersedes "Builder work, restated" in
Amendment 2, and the producer figures in the first amendment's derivation.

### Failed contract work no longer shares the applied calls' counter

Amendment 2 kept one counter for failed and applied work in selection. A
failing plaintext call is never included (STF-8), so it pays nothing and
the DAG-BFT driver keeps it pooled: one call at `gas_limit = vm.max_block_gas`
early in the sub-DAG used the whole contract budget every block, and every
contract call after it was refused `BlockGasExhausted`.

Amendment 2 turned down a separate failure budget, reasoning that one call
at the cap fills it. That holds for a budget of one cap checked on its own.
It does not hold for the rule taken here:

- A failed trial's gas moves off `Overlay::vm_gas` (what applied calls are
  admitted against, GAS-2) onto a count only the builder keeps
  (`select_applicable_within`). The block validity rule is unchanged, and
  the builder's applied count is exactly what a validator counts for the
  block it builds.
- A contract transaction is tried only while applied work, failed work and
  the transaction's bound (a call's `gas_limit`, a deployment's cold price)
  fit in `BUILDER_GAS_CAPS × vm.max_block_gas + BUILDER_GAS_SLACK` =
  `2 × cap + 1,536,000`. The slack is the most memory a failed call reports
  past its limit (GAS-7), so a failure at the cap never stops any call that
  passes GAS-1 and GAS-2 after it.

Builder worst case, stated: at most `2 × vm.max_block_gas + 3,072,000` of
failed and applied contract work per selection (the last trial may pass the
budget by its memory), against `vm.max_block_gas + 1,536,000` before. At
the 1 ns/unit derivation a producer's worst case (selection, then the
preview root of the block, including the sealed quarter) is about
503 M + 312.5 M ≈ 816 ms at the 250 M default, and 603 M + 375 M ≈ 978 ms
at the 300 M ceiling. The default holds; **the ceiling leaves about 22 ms
of the 1 s block time for signatures, the state root and the commit.**
Lowering `VM_GAS_CEILING` is a governance-bound change and is not made here:
listed as open, for Eric.

What it closes and what it does not:

- Closed: one failing call, at any `gas_limit` up to the cap, no longer
  crowds out contract calls that apply after it
  (`a_failing_call_at_the_cap_does_not_crowd_out_a_call_that_applies`).
- Still open, G1: failed work past the budget stops further contract trials
  for the rest of the selection. Two failing calls at the cap, from two
  funded senders, get there (`failed_work_past_the_builder_budget_stops_contract_trials`).
  Any finite budget has this lever, because a call is only known to fail
  once it has run. Charging a failed call (include it with its fee and a
  nonce bump, an STF-8 change) is economics: **needs Eric.**

Tests that pinned the shared counter are restated against the builder's
budget: each now puts failing work at the cap first, so the charge under
test is what decides whether the honest call after it fits
(`one_sender_gets_one_failed_contract_trial_per_block`,
`a_call_below_its_upfront_price_still_counts_its_code_read_and_scan`,
`a_failed_call_spends_the_gas_it_used_not_the_gas_it_named`,
`a_deployment_that_fails_validation_still_spends_its_compile`).
`building_a_block_of_failing_max_gas_calls_stays_inside_the_block_time` now
fills two caps of failed work (eight calls reporting a quarter cap each).

### The compile shapes at the arity bound, re-measured

The finding: the "about 1 ns of native work per unit" claim had to be re-checked
in release on the worst shapes the arity bound still allows, and either the
prices hold it or the claim must give the real figures.

Release build, wasmtime 48.0.8, rustc 1.99.0-nightly (da80ed070 2026-07-14),
Intel Core Ultra 9 275HX (24 cores), 31 GB, Windows 11 Pro build 29708,
shared with other agents' builds, 2026-11-19 (run 1 ended before 00:42 KST, runs 2 and 3 started at 00:42
and 01:11), at d25ff726
(runs 2 and 3 with the two attribution shapes added to the test, the VM
unchanged). Command, through the build slot:
`cargo test --release -p maya-vm --test gas_adversarial_calibration_tests --
--ignored --nocapture --test-threads 1 shapes_at_the_limits`. Three runs,
each the best of three compiles (`RUNS`). ns of compile per unit of the whole
cold price:

| Shape | Price | run 1 | run 2 | run 3 |
|---|---|---|---|---|
| 1 × 2,000 `block (type []->[16])` | 186.1 M | **1.231** | 0.771 | 0.768 |
| 1 × 2,000 `block (result` 8 `)` (attribution) | 124.5 M | — | 0.731 | 0.725 |
| 2,000 × (16 `i32.const`, 16 `drop`), no block (attribution) | 130.1 M | — | 0.094 | 0.098 |
| ~1,760 empty exported functions | 229.9 M | 0.899 | 0.653 | 0.566 |
| 3 × 2,000 `loop` (per-function cap) | 188.5 M | 0.857 | 0.763 | 0.522 |
| 3 × 1,999 `br_if` carrying 8 | 191.4 M | 0.835 | 0.709 | 0.656 |
| 3 × 1,000 `if (type [8]->[8])` | 205.8 M | 0.779 | 0.555 | 0.593 |
| 3 × 2,000 `loop (param i32) (result i32)` | 182.9 M | 0.763 | 0.517 | 0.454 |
| worst of the run | | 1.231 | 0.771 | 0.768 |

Every other shape in the test was under 0.70 in every run. What this shows:

- The prices are not changed. In runs 2 and 3 every shape is at or under
  1 ns per unit, the worst 0.771. Run 1 put the same binary and the same
  shape at 1.231. That is the machine's load, not a shape priced too low, so
  a reprice would tune the constants to one loaded run (Standing Order 2).
- The claim is corrected to what was measured: **at or under 1 ns per unit
  on an unloaded run; up to 1.23 on a loaded one** (1.5 in Amendment 2's
  runs). At 1.23 a cold compile at the 250 M cap takes ~308 ms, and the
  producer worst case above (~816 ms at 1 ns) is ~1.0 s. The test's bound,
  `MAX_NS_PER_FUEL` = 1.5, catches a shape priced several times too low. It
  does not prove the 1 ns rule.
- Attribution: the typed-results row's time is in the block results, not in
  the values pushed and dropped inside each block (0.094 without the block).
  Going from 8 to 16 results per block added ~26 µs per block (run 2) for
  ~30,800 more fuel (8 × 2,500 arity plus 24 bytes at 450): ~0.85 ns per unit
  at the margin, ~1.3 if only the arity charge is counted.
- Not measured: aarch64, other x86-64 CPUs, Pulley, an unloaded machine
  (none was available; every run shared the PC with other builds).

Builder selection over two caps of failed work (eight out-of-gas calls,
each reporting a quarter cap, 500 M gas in all), dev profile, same machine
and commit: 56.9 ms
(`cargo test -p custom-l1-node --test vm_gas_timing_tests -- --ignored
--nocapture --test-threads 1 building_a_block_of_failing`). The loop is cheap
guest compute (~0.11 ns per unit), so this shows the budget is enforced. The
time bound comes from the gas bound times the slowest priced work.

## Amendment 4, 2026-11-21: the re-freeze review's R-2, R-5, R-6, R-10 and the gas caps' lead (Eric 2026-11-17 pre-approval, re-freeze bundle)

The AI review of the re-freeze PRs (2026-11-19, findings R-1..R-16) was read
as a list of claims and each was checked against the code before acting.
None of this is an external audit.

### R-2: amendment 3 verified under the review's scenario; the residual stays open

The claim: a failed contract call is free and stays pooled, so one sender
starves contract execution every block. Checked against d25ff726 with the
review's scenario: one call at the cap to a contract that loops until it runs
out of gas, admitted to the pool (admission checks only the caps), ordered
first, then three honest calls, the first naming the cap, in two consecutive
blocks. In both the producer path (`StateDB::select_applicable`) and the
DAG-BFT path (`consensus::bft::build_block` on a committed sub-DAG), every
honest call is included in both blocks, the derived block imports, and the
attacker's nonce stays at 1: it paid nothing and is pooled again
(`a_pooled_failing_call_at_the_cap_leaves_honest_calls_in_every_produced_block`,
`a_pooled_failing_call_at_the_cap_leaves_honest_calls_in_every_bft_block`).
The tests passed on the existing code: this is verification, not a fix.

So the claim is **wrong for one sender** and **right for two**: two funded
senders, each with one failing call at the cap, still fill the builder's
`2 × cap + 1,536,000` and leave every contract call after them untried, every
block, for nothing (`failed_work_past_the_builder_budget_stops_contract_trials`,
G1). No builder budget closes that, because a call is known to fail only once
it has run. Charging failed calls is economics; it is built as a separate draft
PR ("[needs-eric]", decision brief 3 option A), not here.

### R-5: the shape scan of a refused cold call, measured, skipped and priced

The claim: a cold call refused below its price ran the full shape scan before
the gas check and counted `50,000 + len`, so the scan was unpriced.
Confirmed. Measured in release, maya-vm at this branch, wasmtime 48.0.8,
Intel Core Ultra 9 275HX, Windows 11 Pro 29708, shared with other builds,
2026-11-21, best of 30 scans (`cargo test --release -p maya-vm --test
gas_adversarial_calibration_tests -- --ignored --nocapture --test-threads 1
calibrate_the_shape_scan`):

| Module (all within the shape bounds) | Bytes | Scan | ns/byte | ns per unit of `50,000 + len` | ns per unit of `deploy_fuel(len)` |
|---|---|---|---|---|---|
| one body of `nop` | 524,261 | 640.9 µs | 1.222 | **1.116** | 0.0027 |
| one `br_table`, all labels | 524,255 | 609.3 µs | 1.162 | 1.061 | 0.0026 |
| one body of `i32.const 0; drop` | 524,260 | 457.0 µs | 0.872 | 0.796 | 0.0019 |
| 10,000 empty functions | 40,034 | 67.9 µs | 1.696 | 0.754 | 0.0038 |
| one local entry per local (250 × 1,024) | 513,525 | 354.8 µs | 0.691 | 0.630 | 0.0015 |

The old charge was under the 1 ns rule by up to 12% on the scan alone, before
the code read. Not the order of magnitude the review's "before rating this
High" asked about, but under. Two changes:

- **Skipped below the length price** (the review's patch, finished). The cold
  price is `deploy_fuel(len) + shape_fuel`, so a `gas_limit` below
  `deploy_fuel(len)` is out of gas whatever the shape: refused before the scan,
  counting `50,000 + len` as before. Consensus-neutral for stored code, which
  all passed the scan at deployment
  (`a_cold_call_below_the_length_price_is_refused_without_the_shape_scan`; red
  captured: the scan's error came back instead of `OutOfGas`).
- **Priced above it.** A cold call whose `gas_limit` reaches
  `deploy_fuel(len)` but not its whole price is refused after the scan and now
  counts `deploy_fuel(len)`, the part of its price it reached, at most
  0.0038 ns per unit in the table. It counted `50,000 + len` (red captured:
  50,054 counted against 74,300 expected,
  `a_cold_call_refused_after_the_shape_scan_counts_the_length_price`). The
  count is at most the call's `gas_limit`, so the builder's bound is unchanged.
  It is consensus for a revealed sealed call (whose `gas_limit` can reach
  `deploy_fuel(len)` for modules up to 138,777 bytes under the 62.5 M sealed
  budget) and, for a plaintext one, block selection (GAS-9: consensus under DAG-BFT, amendment 6). Stored code that a later
  release's shape bound refuses counts the same, where it counted nothing.

The review's complete fix, a cold price stored at deployment, is not taken:
a new state record (invariant 25) to save a scan that is now priced at its
length.

### R-6: the stack boundary measured; it depends on the build as well as the ISA

The claim: a recursion depth between the x86-64 and aarch64 frame limits
succeeds on one ISA and traps on the other. Confirmed, and wider than claimed;
not reproduced on aarch64 (no aarch64 machine here). Measured on this machine
with `recursion_to(depth)` (one i64 parameter, one add per level,
`measure_the_deepest_recursion_this_isa_completes`, `--ignored`):

| Build of the same source, x86-64 | Deepest completing depth | Gas there |
|---|---|---|
| release | 10,915 (10,916 overflows) | 559,558 |
| dev (test profile) | 10,901 (10,902 overflows) | 559,418 |

Fourteen levels apart on one ISA and one CPU: the overflow point counts native
frames the host build contributes, not only the guest's. So a contract that
recurses to between the two depths splits two x86-64 nodes built differently,
not only an x86-64 node from an aarch64 one. Validators running the same
release binary (reproducible builds) agree with each other; nothing makes a
differently built node agree with them.

A golden vector at the boundary was written first and failed in the dev
profile for exactly this reason, so it is not kept: it would fail CI's `ci`
profile for a reason the vector cannot fix. Kept instead:
`golden-recursion-5000-deep-completes-on-every-build` (499,958 gas), well
inside every boundary seen, which the `vm-gas-aarch64` job replays, so deep
recursion is pinned to the same gas on both ISAs short of the boundary.

Not made build- or ISA-independent, because every way of doing it is larger
than this amendment: a deterministic depth bound needs each function's entry
and exits instrumented with a counter, with a limit far enough below the native
stack that no build or ISA reaches it first (a module rewriter: a new
dependency in the VM crate, which needs Eric, or a hand-written one); executing
consensus on a tier with fixed frames (Pulley) changes every price calibrated
here. Recorded in the spec's Gaps and BACKLOG.md (G4). Until one lands,
validators should run the one published release binary. (Amendment 6
corrects what this paragraph left out: "x86-64 only" is not that condition,
because Windows and Linux x86-64 builds overflow at different depths, and the
host CPU's ISA flags were inferred; they are now named.)

### R-10: GAS-2 says what a block may spend in all

The claim: the code lets a block spend 1.25 × `vm.max_block_gas` while GAS-2
said "no more than `vm.max_block_gas`", and the parameter comment said
something else again. Confirmed; no split, because import replays the same
rule. The code is right and the text was not: GAS-2 now says a block spends at
most `vm.max_block_gas` on plaintext transactions and
`vm.max_block_gas + sealed_gas_budget` in all, and the comment on the bound
states 1.25 × the cap to validate and 3.25 × to produce. Pinned by
`a_block_spends_its_cap_on_plaintext_calls_and_the_sealed_budget_on_reveals`:
both calls complete and together spend more than the cap, in a block that
validates.

### GAS-5: the caps wait the validity-rule lead

#575 (ADR-082 amendment 2026-11-15) holds a passed change to a block-validity
parameter for `MIN_UPGRADE_LEAD_BLOCKS` after execution. This PR now carries
#575, and tags 17 and 18 are classified as validity rules: both refuse a call
or a block. GAS-5 says so, and that the value is range-checked again at
promotion. Pinned by
`a_passed_block_gas_cap_binds_only_after_the_validity_rule_lead` (a call
naming 245 M against a cap voted down to 240 M passes GAS-1 at `E + 1` and at
`E + lead − 1`, and is `GasAboveCap` from `E + lead`) and
`a_waiting_block_gas_cap_out_of_range_at_promotion_is_dropped`. Red captured
with the two keys classified as immediate.

## Amendment 5 (PROPOSED, needs Eric): a failed contract call is included and pays (decision brief 3, option A)

**Status: proposed in a draft PR, not accepted.** It changes who pays for
failed contract work, which is an economics decision.

**Rule (GAS-8).** A plaintext contract call or deployment that passes its
signature, nonce, fee, GAS-1 and GAS-2 checks and then fails in the VM is
included instead of failing its block: its fee is paid, its sender's nonce
advances, it writes nothing, and its failure gas counts against the block's
contract gas. STF-8 gains that one exception. A call naming no contract, a
GAS-1 or GAS-2 refusal and a node-local fault still fail the transaction; the
VM engine failing to start is now `Storage` (a local fault), so no node
includes a call as failed because its own engine did not start. A call
whose engine could not get memory mid-call (an instance, its linear memory,
compiled code) is `VmError::LocalFault` and so `Storage` too (amendment 7),
which GAS-8 never absorbs: the review of this draft found that before
amendment 7 those faults reached the node as `NodeError::Vm` and would have
been included as failed calls on the node short of memory alone.

**What it closes.** G1: a failing call is no longer free or pooled. Once
included its nonce has moved, so the same transaction cannot be proposed
again; repeating the attack costs one signed transaction and one fee per
block, per account.

**What it does not close, and makes visible.** Re-freeze review R-3: the fee
is the byte fee (FEE-5), not a price per unit of gas. One call at the cap,
failing or not, takes the whole block's contract gas for that fee. Under
amendment 3 one failing sender could not do that (its failure had its own
builder count) but two could, for nothing; under this amendment one can, for a
fee. Closing R-3 needs a gas price: a fee that grows with `gas_limit` or
`gas_used`, which is a fee-market design (who sets the price, a governed
parameter and its bounds, refunds or none under GAS-6), not built here.

**Under GAS-9 (amendment 6, merged in after this draft).** Selection keeps
an included failure like any other transaction, so the four `builder_select`
vectors amendment 6 wrote for left-out failures are re-stated here: in each,
the failing call at the cap is kept and every contract call after it is
passed over (`kept ["0"]`, `PassedOver`). That is R-3 as a vector: one call
at the cap, failing, takes the whole block's contract gas for its byte fee.

**Variant not built.** Included failed work could count against a budget of
its own (one cap), so a paying failing call never takes room from calls that
apply. It doubles a validator's worst case (2.25 × cap) and still leaves R-3.
## Amendment 6, 2026-11-22: the #549 review of amendment 4 (Eric 2026-11-17 pre-approval, re-freeze bundle)

Numbered 6 because numbers 4 and 5 are also claimed by two other re-freeze
branches (`refreeze/contracts-root-bound`, `refreeze/contracts-root`); the
coordinator renumbers on merge. The review's five findings were read as
claims and checked against the code first. None of this is an external audit.

### Deployments: scanned only when the scan is counted

The claim: some shape scans in block selection are never counted, so the
GAS-2 bound on the builder did not match the code. Confirmed, all three parts:

- `trial_gas_bound` ran `gas::cold_call_fuel` (a scan of up to 512 KiB) on
  every deployment `builder_may_try` considered, before any count, and a
  deployment it then skipped cost that scan for nothing.
- `deploy_contract` scanned, then returned `BlockGasExhausted` or
  `GasAboveCap` without counting. Nothing counted means the builder neither
  recorded the sender nor moved work to its failed count, so a valid pooled
  deployment that never fits was scanned free in every block.
- The same scan was priced two ways: a cold call refused after it counts
  `deploy_fuel(len)` (amendment 4), a deployment the scan refused counted
  `50,000 + len`.

Now:

- A deployment is checked on its length price `deploy_fuel(len)` before
  anything else (before the scan, the contract-id hash and the code lookup):
  the cold price is that plus the shape's, so one whose length price does not
  fit is refused whatever its shape, counting 0, as a cold call is.
- Once scanned, any refusal (a shape bound, the cap, what the block has left)
  counts `deploy_fuel(len)`, which the first check guarantees fits. An
  admitted deployment counts its whole `cold_call_fuel` before the compile, as
  before.
- Selection bounds a deployment by what the block has left, not by its scan:
  a deployment counts at most that, so the GAS-9 work bound holds, and no
  deployment is scanned to be bounded. The difference from the old bound: a
  deployment is tried while the failed count is at most `cap + 1,536,000`,
  whatever its size, so one failing call at the cap still leaves deployments
  tried (as it leaves calls); after two (G1), small calls still fit but
  deployments do not.

Red, then green (`crates/node/src/state/vm_exec_tests.rs`):
`a_deployment_refused_after_its_shape_scan_counts_the_length_price` (a
500,000-byte module refused `BlockGasExhausted` after its scan counted
nothing: 24,921,650 against 250,000,000 expected),
`a_deployment_whose_shape_is_refused_counts_the_length_price` (50,027 against
62,150), `a_deployment_whose_length_price_does_not_fit_is_refused_before_the_scan`
(the scan's `locals` error came back instead of `BlockGasExhausted`), and
`the_builder_bounds_a_deployment_by_what_the_block_has_left_without_its_scan`
(a deployment the scan refuses was tried one unit past the bound).

Consensus effect: the counts change what a revealed sealed deployment leaves
for the sealed calls after it (GAS-2), and the selection bound changes the
DAG-BFT block (GAS-9). A plaintext deployment that does not fit fails its
block either way; only the error's `needed` figure changes.

### The builder budget is block derivation, not policy

The claim: GAS-2 called the builder's budget, its constants and the
one-failure-per-sender rule "builder policy, not a validity rule", while
under DAG-BFT `build_block` runs `StateDB::select_applicable` on every
validator (its own doc: "This is consensus"), so they decide the derived
block, and no vector pinned them. Confirmed. The clause is now its own rule,
**GAS-9** (block selection), stated as consensus that changes only by a hard
fork, pointed to from CON-12 and CON-13. `BUILDER_GAS_CAPS` and
`BUILDER_GAS_SLACK` say so in their doc comments. Four `builder_select`
vectors in `spec/tests/vm_gas.json`, replayed by `the_spec_vectors_hold`
through `StateDB::select_applicable`:

| Vector | Order | Kept |
|---|---|---|
| `builder-a-failing-call-at-the-cap-then-honest-calls-all-included` | spin at the cap, writer at the cap, two writers at 1 M | all three writers |
| `builder-two-failing-senders-leave-exactly-the-slack` | two spins at the cap, writer at 1,524,000 | the writer |
| `builder-two-failing-senders-exhaust-the-budget` | the same, writer at 1,524,001 | none (`PassedOver`) |
| `builder-a-failed-sender-second-contract-tx-passed-over` | spin, the same sender's writer at the same nonce, another sender's writer | the other sender's |

The two edge vectors pin both constants exactly: two spins report
`2 × (250,000,000 + 6,000)` and the budget is `500,000,000 + 1,536,000`, which
leaves 1,524,000. A PoW producer selects with the same function; there it is
the producer's choice, and GAS-1 and GAS-2 alone decide validity.

### R-6: the condition is one binary, and the CPU no longer moves it

The claims: amendment 4 offered "launch x86-64-only" as a mitigation while
its own figures showed the boundary moving with the build; Cranelift's ISA
flags were inferred from the host CPU, so x86-64 machines could compile
different frames; the release workflow publishes Windows and Linux validator
binaries, the realistic genesis mix, never measured against each other.
Confirmed, and measured. One correction: `release-binaries.yml` publishes
five node binaries, not three (x86-64 and aarch64 Linux, x86-64 Windows, and
two macOS).

Measured on 2026-11-22, Intel Core Ultra 9 275HX, this branch, wasmtime
48.0.8, `recursion_to` (one i64 parameter, one add per level), with the
target named as below; `cargo test [--release] -p maya-vm --test
gas_hardening_tests -- --ignored --nocapture
measure_the_deepest_recursion_this_isa_completes`:

| Target triple (host) | Profile | Deepest completing depth | Gas there |
|---|---|---|---|
| x86_64-pc-windows-msvc (Windows 11 Pro 29708) | release | 10,915 (10,916 overflows) | 559,558 |
| x86_64-pc-windows-msvc | dev | 10,901 | 559,418 |
| x86_64-unknown-linux-gnu (WSL2, kernel 6.18.33.1, same CPU) | release | 10,919 (10,920 overflows) | 559,598 |
| x86_64-unknown-linux-gnu | dev | 10,909 | 559,498 |
| aarch64-unknown-linux-gnu | dev | printed by CI `vm-gas-aarch64` from this push; not read yet | |

So a call recursing to depth 10,916–10,919 applies on a Linux x86-64
validator and fails on a Windows x86-64 one, same CPU, same source: the
state roots split, and under DAG-BFT the derived blocks. The figures are the
maya-vm test binary in each profile; the shipped `maya2c-node` binaries (fat
LTO, `--features production`) were not measured, and the 14-level
release/dev gap says their own depths can differ again.

Changed here: `maya_vm::config` names the build's target triple (passed in by
`crates/vm/build.rs` from cargo's `TARGET`, the string wasmtime's own host
triple is parsed from), so Cranelift uses that triple's baseline ISA flags
instead of every feature the CPU reports. One binary on two CPUs now compiles
a contract to the same code. Fuel is counted on wasm operators before
lowering, so gas is unchanged; every maya-vm and contract-gas test passes
unchanged, and the Windows depths above are the ones amendment 4 measured
with inferred flags (10,915 and 10,901), so this shape's frames did not move
on this CPU. Pinned by
`the_engine_compiles_for_its_named_target_not_for_the_host_cpu` (red with the
old inference: the two engines precompiled one module to identical bytes).
The setting is in the module-cache digest.

What is not changed, because each is Eric's decision (BACKLOG G4, a genesis
blocker): the binary still moves the boundary. The condition that holds is
**every validator runs the same published release binary, one target
triple**, and nothing enforces it. The options are written in G4: operate one
binary (every genesis validator on the Linux x86-64 release binary, a Windows
PC under WSL2 or a VM, other targets published as non-validator builds), a
deterministic call-depth bound by instrumenting function entry and exits
(prior art: parity's `wasm-instrument` stack limiter, which Substrate's
contracts pallet applied to every module), or consensus on the Pulley tier
with every price re-calibrated.

## Amendment 7, 2026-11-22: the review of amendment 6 and of draft #647 (Eric 2026-11-17 pre-approval, re-freeze bundle)

Three findings, read as claims and checked against the code and wasmtime
48.0.8's source first. None of this is an external audit.

### The ISA level: measured, then pinned at x86-64-v2

The claim: naming the target left Cranelift on the triple's bare baseline,
where x86-64 has no SSE4.1 (float `floor`/`ceil`/`nearest`/`trunc` become
libcalls) and no POPCNT (a bit-twiddling sequence); gas is one fuel per
operator either way, so a block of those could cost more wall time per unit
of gas than the calibration measured, and nothing had timed it. Confirmed in
the source (cranelift-codegen-meta 0.135.8 `isa/x86.rs`, defaults `false`),
and measured.

Measured on 2026-11-22, Intel Core Ultra 9 275HX, Windows 11 Pro, release,
`cargo test --release -p maya-vm --lib config:: -- --include-ignored
--nocapture --test-threads 1`
(`measure_float_rounding_and_popcnt_against_the_calibrated_compute`: 20 M
iterations of a loop whose result feeds back, so nothing is dropped; best of
three; ns per unit of fuel):

| Operator | host-inferred (the old calibration) | named baseline (amendment 6) | x86-64-v2 (this amendment) |
|---|---|---|---|
| `i64.div_u` (the calibrated compute) | 0.128 | 0.128 | 0.127 |
| `f64.floor` | 0.563 | 0.406 | 0.519 |
| `f64.ceil` | 0.550 | 0.411 | 0.526 |
| `f64.nearest` | 0.550 | 0.674 | 0.518 |
| `f64.trunc` | 0.549 | 0.632 | 0.516 |
| `f32.floor` | 0.553 | 0.421 | 0.525 |
| `f32.nearest` | 0.555 | 0.511 | 0.517 |
| `i64.popcnt` | 0.066 | 0.219 | 0.066 |
| `i32.popcnt` | 0.073 | 0.198 | 0.074 |
| `i64.clz` | 0.065 | 0.103 | 0.103 |
| `i64.ctz` | 0.066 | 0.089 | 0.088 |

What it shows. Float rounding in a dependent chain is about 0.52–0.67 ns
per unit under every configuration, including the host-inferred one the
earlier calibration ran under: four to five times the `i64.div_u` loop the
block-time derivation cites, so that derivation's guest-compute figure was
never the worst guest compute, before or after amendment 6. The worst here,
0.674 ns per unit on the baseline, is about 169 ms of guest compute for a
block's 250 M gas of plaintext contract work and about 211 ms at GAS-2's
1.25 × bound: inside the 1 s block time, so no operator is repriced. The baseline is the worst
configuration for `nearest`, `trunc` and `popcnt` (3x on popcnt), and its
libcalls go through the host's C runtime, a different one in each published
binary. So the level is pinned, keeping CPU independence:
`maya_vm::config::ISA_FLAGS` turns on `has_sse3`, `has_ssse3`, `has_sse41`,
`has_sse42`, `has_popcnt` and `has_cmpxchg16b` (the x86-64-v2 set) on
x86-64, none elsewhere (aarch64's baseline rounds and counts bits natively;
measured on x86-64 only). They are in the module-cache digest.
`checked_engine` compiles the empty module when an engine is built, so
wasmtime's host check (`check_compatible_with_native_host`) refuses a CPU
without the level at start-up instead of at the first contract call. The
flags are set through `Config::cranelift_flag_enable`, an `unsafe` call: the
`SAFETY` comment says why it holds (wasmtime checks the host has every flag
before it compiles or loads code), and it needs a reviewer as any `unsafe`
outside the exempt crates does. The Pulley tier names its own target and no
native flags.

Pinned by `the_engine_compiles_at_the_x86_64_v2_level_not_the_bare_baseline`
(red with `ISA_FLAGS` empty: the two engines precompiled one module to
identical bytes) and `an_engine_refuses_a_cpu_without_its_isa_level`
(flags past the level stand in for a CPU without it: the engine builds
exactly when the CPU has them). A whole-block figure is
`a_block_of_float_rounding_and_popcnt_validates_inside_the_block_time` in
`vm_gas_timing_tests`, run at x86-64-v2 on the same machine (dev profile,
`cargo test -p custom-l1-node --test vm_gas_timing_tests -- --include-ignored
--nocapture --test-threads 1 guest_compute float_rounding`): one call at the
250 M cap applied in 142.3 ms for `f64.nearest` through memory (0.569 ns
per unit), 22.0 ms for `i64.popcnt` (0.088) and 40.4 ms for the calibrated
`i64.div_u` (0.162). The baseline was not run at block level.

### Faults of one node's engine are not failed calls

The claims: a contract call or deployment failing because the engine could
not get memory (an instance's mapping, its linear memory, compiled code)
reached the node as `NodeError::Vm`, the same as a trap, so (a) draft #647,
which includes a failed call with its fee, would record it as failed on the
node short of memory and apply it on the healthy ones, and (b) block
selection, which is consensus under DAG-BFT (GAS-9), would leave the
transaction out on that node alone. Confirmed: `classify` turned every
error that was not out-of-fuel into `VmError::Trap`, `Module::new` failures
became `InvalidModule`, and the limiter's default `memory_grow_failed`
turned an operating-system refusal during `memory.grow` into wasm's `-1`,
which the contract itself could act on.

The suggested rule, "anything that does not downcast to `wasmtime::Trap` is
local", was checked and **not** taken: wasmtime refuses some modules at
instantiation with a plain error every node produces alike. A declared
table is one: the limiter refuses every table, its initial size included
(`table_growing` returns `false`; wasmtime then bails with "table minimum
size ... exceeds table limits"), so any module with a table fails every
call. Under the suggested rule that call would read as local on every node,
and selection, which passes a local fault up, would stop building blocks on
every validator while it was pooled.

Changed instead: the rule is positive, by type. `maya_vm::error::is_local_fault`
is an `io::Error` (how wasmtime's memory and code mappings report the
operating system's refusal, on Windows and Unix alike) or wasmtime's
`OutOfMemory`, anywhere in the error chain. Such an error is
`VmError::LocalFault` from `classify` (instantiation and execution) and
from `VmError::from_compile` (`Module::new`, through the cache and at
deployment), and the limiter's `memory_grow_failed` passes it up instead of
returning `-1`; growth past a module's own maximum stays `-1`. The node maps
`LocalFault` to `NodeError::Storage` (`vm_exec::vm_failure`), which block
selection and `stage_transfer_or_undo` already pass up and which refuses the
block on apply; the engine failing to start is `Storage` too (carried from
#647). Spec: GAS-9 and two Gaps entries.

Pinned by five `maya-vm` tests in `runtime_local_fault_tests.rs`, four of
them red before the change (memory refused at instantiation and while
running became `Trap`; the limiter swallowed the refusal; a compile refusal
became `InvalidModule`), one guarding the other side (a trap, growth past
the module maximum and a declared table stay the call's), and the node's
`a_fault_of_this_nodes_engine_is_storage_not_a_failed_call`. The fault is
injected through the store's resource limiter, the hook wasmtime calls for
every linear memory, with the error type a refused mapping has. Not done: a
node-level test with the engine's memory refused, since `StateDB` holds its
own engine with no hook; and a local failure wasmtime reports as neither type
is still taken as the call's (recorded in the spec's Gaps).

The table refusal is pre-existing, alike on every node, and not changed here:
whether contracts may use `call_indirect` is a contract-ABI question.
