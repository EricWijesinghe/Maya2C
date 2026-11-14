# ADR-114: Contract timers — a contract schedules its own call at a height

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-39, [prior-art/contract-timers.md](../prior-art/contract-timers.md), [ADR-026](ADR-026-contract-caller-identity.md), [ADR-109](ADR-109-scheduled-payments.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 17, 21, 24, 25, 28, 33; Standing Order 4

## Context

A contract runs only when a signed `CallContract { contract, input,
gas_limit }` names it (`crates/node/src/core/payload.rs:211-221`). Its
`gas_limit` is "a halting bound, not a price: exceeding it traps the call
and discards its writes, but nothing is billed to the sender"
(`payload.rs:216-220`). The call runs against an owned `ChainHost` that
read the contract's whole storage up front (`crates/node/src/state/contracts.rs:5-16`);
writes reach the overlay only on success (`vm_exec.rs:5-9`, `:146-153`).
There are no contract-to-contract calls, so `caller()` is always the
transaction's signer (`crates/vm/src/host.rs:85-91`). The host ABI is
`block_height`, `caller`, `get_balance`, `get_balance_u256`,
`storage_read`, `storage_write`, `block_randomness`, `oracle_read`,
`oracle_read_u256`, `oracle_feed_age`, `emit_event`
(`crates/vm/src/runtime.rs:661-964`); storage keys are at most 128 bytes
and values 4 KiB (`host.rs:28-31`). `VmDefaultGasLimit`, `VmMaxMemoryPages`
and `VmMaxModuleBytes` are governed (`crates/governance/src/params.rs:72-78`).
Invariant 21: a host function that does native work charges fuel for it
first.

ADR-109 gives the chain an end-of-block step that executes due records in
key order under a compiled cap; the Internet Computer gives each canister a
protocol timer; Substrate's `pallet-scheduler` dispatches a stored call at
a block; Chainlink Automation uses off-chain keepers (prior-art file).

**Finding that contradicts the brief.** The parity row says "fuel charged
at schedule time". Fuel is not priced in coin anywhere in this tree: a call
pays its bytes and nothing for its gas (`payload.rs:216-220`). A timer
therefore cannot prepay "fuel"; it prepays a flat fee, and whether gas
should ever carry a price is a separate economics question this ADR does
not decide.

## Decision

**A contract, during a call, asks the host to run it again at a later
height with a stored input. The request is a committed record; the node
runs every due timer at the end of the block, in key order, bounded, each
as an ordinary contract execution whose failure is a no-op. The scheduling
transaction's signer prepays a flat fee per timer into an escrow the timer
chain draws from.**

### Host ABI (`env`)

| Function | Rule |
|---|---|
| `schedule_timer(due_height: u64, input_ptr, input_len, gas_limit: u64) -> i64` | `due_height > height`; `input_len <= MAX_TIMER_INPUT = 1_024`; `gas_limit <= VmDefaultGasLimit`. Charges fuel for the record write **before** writing (invariant 21: `TIMER_SCHEDULE_FUEL`, calibrated like `fuel_calibration_tests.rs`). Returns the timer index in this call, or a negative code when the escrow cannot cover `TIMER_FEE` (never a trap: a contract decides what to do without one). At most `MAX_TIMERS_PER_CALL = 8` per execution. |
| `cancel_timer(id_ptr) -> i64` | Only the contract that owns the timer. Returns the unspent escrow to the contract's timer escrow (see below). |
| `timer_escrow() -> u64/u256` | The contract's remaining prepaid fee balance. |

The input is opaque to the host, as `ContractCall.input` is.

### State (invariant 25)

New prefix `ct:` in a new committed layer `StateLayer::Timers`, in
`RECORD_LAYERS` and the undo journal:

- `ct:t:` ‖ `timer_id(32)` → `Timer { contract, due: u64, input: Vec<u8>,
  gas_limit: u64 }`; `timer_id = blake3::derive_key("maya2c timer v1",
  contract ‖ height ‖ index)`.
- `ct:d:` ‖ `due(8 BE)` ‖ `timer_id` → empty: the due index, scanned from
  the start.
- `ct:e:` ‖ `contract` → `U256`: the contract's timer escrow in native
  coin.

### Paying

`TxKind::FundTimers { contract, amount }` debits the signer and adds to
`ct:e:contract`; anyone may fund a contract's timers. A `CallContract` that
scheduled `n` timers has `n * TIMER_FEE` moved from `ct:e:contract` to
`FEE_COLLECTOR` at the end of the call (it joins the tips, `fees.rs:14-17`);
if the escrow is short, `schedule_timer` returned the negative code and no
record was written. `TIMER_FEE` is a governed parameter (invariant 17). A
timer run that schedules another timer draws from the same escrow, which
is how a recurring task pays for itself without a signer present.

### Execution at a height (the consensus step)

In `apply_block_journaled`, after transactions and after ADR-109's
scheduled payments (a fixed order, so every node agrees), scan `ct:d:` from
the start while `due <= height`, at most `MAX_TIMERS_PER_BLOCK = 64`:

1. Delete both records (a timer runs once; a recurring timer re-schedules
   itself from inside its run).
2. Run `call_contract` with the stored input and `gas_limit`, `caller()`
   returning `None` (the contract tells a timer from a call by that, as
   dry runs already do, `host.rs:89-91`), and `block_randomness` as of the
   previous block, like any call.
3. A trap or out-of-gas discards the writes and is a **no-op**: a contract's
   failure must not void the block (invariant 7's reason). The timer is
   already deleted; the contract gets no retry unless it scheduled one.

Timers past the 64th stay due and run first in the next block. The step is
in the only apply path (invariant 24): under the journal, the declared
`state_root` must reproduce it, and it reads committed state and the height
only. Worst-case block work is `64 * VmDefaultGasLimit` fuel plus the
storage reads `contracts.rs:12-13` already bounds, stated as the compiled
bound. A breaker on `Module::Vm` (invariant 28) skips the step; due timers
carry over and run when the module resumes.

### Upgrades and code

A timer names a contract id, not code; if the contract's code is replaced
(no such operation exists today), the timer runs the new code. Timers are
not cleared by anything but `cancel_timer` and their own run — the opposite
of the Internet Computer, where an upgrade clears them (prior-art file).

## Alternatives considered

- **Keepers (Chainlink Automation, Gelato).** A relayer submits
  `CallContract` on time; works today, dies with the relayer, and the
  contract cannot tell a keeper from anyone else. Kept as the app option.
- **One global timer per contract (ICP).** Fewer records, but a contract
  that wants two schedules must multiplex in its own storage; the due
  index already handles many per contract.
- **Charging gas at schedule time.** Gas has no price (above). A flat fee
  per run is what the state write and the bounded execution cost the
  network; pricing fuel is its own ADR.
- **Unbounded timers per block.** A liveness attack for one fee each.
  Rejected; 64 with carry-over.
- **Timers that run before transactions.** A timer could then front-run
  the block's transactions by design. After, with ADR-109, fixed order.

## Consequences

- Contracts can keep time: auctions close, vesting ticks, games advance,
  with no keeper.
- One layer, three prefixes, three host functions, one `TxKind` under
  `Module::Vm`, one apply-path step with its undo coverage.
- Every block does bounded extra work; a block with no due timer moves no
  root.
- `FundTimers` is not a transfer door (invariant 32); funding from a vault
  counts toward its window like `CallContract` would.

## Invariants touched

- **21**: `schedule_timer` charges fuel for its native write first.
- **24**: a new step in the only apply path; the root must reproduce it.
- **25**: `ct:` prefix and layer.
- **7**: a failed run is a no-op; an unfunded schedule is a return code.
- **17**: `TIMER_FEE` from state.
- **28**: `Module::Vm` gates timers and `FundTimers`; carry-over on a
  breaker.
- **33**: `MAX_TIMER_INPUT` bounds the record; no transaction bytes grow.
- Standing Order 4: heights, key order, no clock.

## Activation

`FeatureSet::CONTRACT_TIMERS = 1 << 22` (claimed in `adr-claims.txt`),
compiled `CONTRACT_TIMERS_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**. Before activation
the three host functions are absent from the linker (a module importing
them fails to instantiate, as a module importing any unknown `env` symbol
does), `FundTimers` is refused, and the end-of-block step is skipped.

## Test plan

Red tests, `crates/node/tests/contract_timer_tests.rs`:

- `a_timer_runs_at_its_due_height_with_caller_none_and_not_earlier`
- `a_trapping_timer_is_a_no_op_and_the_block_commits`
- `a_timer_with_an_empty_escrow_is_not_written_and_the_call_sees_the_code`
- `a_recurring_timer_pays_each_run_from_the_escrow_until_it_is_dry`
- `the_65th_due_timer_runs_in_the_next_block_first`
- `a_block_whose_root_omits_a_due_timer_is_refused` (invariant 24)
- `a_vm_breaker_carries_due_timers_over_and_runs_them_after`
- `cancel_by_another_contract_is_refused`
- `a_reorg_restores_timer_records_escrow_and_the_due_index`

`crates/vm/tests/fuel_calibration_tests.rs` gains `TIMER_SCHEDULE_FUEL`.
Vectors (`spec/03-state.md` CT-1..CT-3): a timer record; the due key; the
state after one run. `crates/node/tests/chaos_simulator.rs` gains a
scenario with 1,000 timers due at one height.

## Open questions for Eric (economics and pricing only)

1. `TIMER_FEE` at activation, and whether it joins the tips (as written)
   or burns.
2. Should fuel itself be priced in coin (a `GAS_PRICE` parameter), which
   would change `CallContract` as well? Not decided here.
3. `MAX_TIMERS_PER_BLOCK` (64) as a compiled bound or a governed one with a
   compiled ceiling?
