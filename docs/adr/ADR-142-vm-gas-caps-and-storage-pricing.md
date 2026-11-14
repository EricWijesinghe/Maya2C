# ADR-142: Contract gas caps per transaction and per block, native work priced in fuel, lazy per-key contract storage

**Status:** Accepted, 2026-11-14 (Eric, 2026-11-14 MCQ: "cap + price now"). Re-freeze change: consensus behaviour, lands before genesis.
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
`[239,979,600, 500,000,000]`, checked when a proposal is made and again when
it executes (invariant 12). They change only through a governance proposal
and its timelock — the scheduled path; no transaction, council act or node
setting moves them. They are read from the parameter table through the
overlay at the call site (invariant 17).

- **Per transaction (GAS-1):** a call's `gas_limit` must be at most
  `min(vm.max_tx_gas, vm.max_block_gas)`. Refused in block validation
  (`GasAboveCap`) and at mempool admission, before the signature check.
- **Per block (GAS-2):** a block's contract gas starts at 0; a call needs
  `gas_limit ≤ vm.max_block_gas − spent` (`BlockGasExhausted`), and adds the
  gas it used. A deployment adds `deploy_fuel(len)` under the same rule. A
  revealed sealed action that fails still adds its `gas_limit`, because it
  ran; the old code discarded the failed attempt's overlay, which would have
  let revealed calls run to their limit outside the budget.
- The block builder already trial-stages each transaction and passes over one
  that fails, so a call that does not fit is left for a later block, like an
  over-size transaction under ADR-072.

The floor is `call_fuel(512 KiB, cold) + 4,000,000`: the largest module a
deployment may carry stays callable under any cap a vote can set.

### 2. Native work priced in fuel, charged before the work (GAS-3, GAS-4)

`crates/vm/src/gas.rs`. One rule: **about 1 ns of measured worst-case native
work per unit of fuel**.

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
copied; sizes over the limits are refused before the charge. A charge that
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
  run. A sender can therefore make every builder run a call up to
  `vm.max_tx_gas` (≈250 ms) and pay nothing. The cap bounds each attempt where
  before nothing did; charging failed calls means including them with their
  fee, which changes STF-8 and is a separate decision.
- **The contracts layer of the state root is still folded over all contract
  code and storage every block** (`merged_contracts`, ROOT-5). Per-block cost
  grows with total contract state, and no gas price bounds that. The write
  price caps growth at ~1 MiB a block; an incremental contracts root (as
  ADR-062a did for accounts) is the fix and is open.
- **Timing is from one machine**, under load from other builds. A slower
  validator validates the same block more slowly, never differently: every
  charge is deterministic. Re-measure with `gas_price_calibration_tests` and
  `vm_gas_timing_tests` before moving either cap.
- **No `storage_delete` host function exists**; a zero-length write is the
  clear and is priced as a write.
