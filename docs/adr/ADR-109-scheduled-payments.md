# ADR-109: Scheduled and recurring payments executed at a block height

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-7, CATALOG row `recurring-payments`, [prior-art/scheduled-payments.md](../prior-art/scheduled-payments.md), [ADR-030](ADR-030-vault-accounts.md), [ADR-072](ADR-072-max-block-bytes.md), [ADR-107](ADR-107-smart-account-promotion.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 24, 25, 28, 32; Standing Order 4

## Context

Nothing on this chain executes a user payment later. Height-driven settlement
exists for vault requests (invariant 32), HTLC expiry
(`crates/htlc-lattice/src/timelock.rs`) and the staking epoch
(`crates/node/src/state/staking_exec.rs:312`). CATALOG row
`recurring-payments` suggests smart-account session keys, which is the Cosmos
authz model: a relayer still submits each payment and a subscription dies
with the relayer (prior-art file).

**Why there is no wall clock.** Standing Order 4: consensus-critical code has
no wall-clock time. Two honest nodes do not share a clock; they share a
height. A schedule in seconds would let nodes disagree on whether a payment
is due, which is a chain split. So a schedule is in blocks, and a "monthly"
payment is `2,592,000` blocks at the one-block-per-second target. If blocks
run slow the payment is late in calendar terms; the chain is not wrong, and
the wallet shows the height-to-date estimate as an estimate.

## Decision

**A schedule is a committed record. At the end of each block the node executes
every due schedule in key order, up to a compiled count, from an escrow the
payer funded at creation.**

### State (invariant 25)

New prefix `sp:` in a new committed layer `StateLayer::Scheduled`, in
`RECORD_LAYERS` and the undo journal.

- `sp:s:` ‖ `schedule_id(32)` → `Schedule { payer, payee, amount: U256,
  next_due: u64, interval: u64, remaining: u32, escrow: U256,
  missed: u8 }`. `schedule_id = blake3::derive_key("maya2c schedule v1",
  payer ‖ nonce)`.
- `sp:d:` ‖ `next_due(8 BE)` ‖ `schedule_id` → empty. The due index, in key
  order by height then id, so the block's work is a prefix scan from the
  start, never a map iteration.

### Operations

| TxKind | Rule |
|---|---|
| `CreateSchedule { payee, amount, first_due, interval, runs }` | `first_due > height`; `interval >= MIN_INTERVAL` (3,600 blocks); `runs` in `1..=MAX_RUNS` (120); `escrow = amount * runs + RUN_FEE * runs` debited from the payer now (one `checked_mul` of a native amount by a `u32`; fits because `amount < 2^224` is refused above, bound written at the check). Writes `sp:s:` and `sp:d:`. |
| `CancelSchedule { id }` | Payer only; the unspent escrow returns to the payer; both records deleted. |
| `TopUpSchedule { id, amount }` | Payer only; `escrow += amount`. |

### Execution at a height (the consensus step)

In `apply_block_journaled` after the block's transactions and before the
state root is computed, the node scans `sp:d:` from the start while
`next_due <= height`, at most `MAX_DUE_PER_BLOCK` (256) entries:

1. If `escrow >= amount + RUN_FEE`: credit `payee` with `amount`; move
   `RUN_FEE` to `FEE_COLLECTOR` (it joins the tips, `fees.rs:14-17`); `escrow
   -= amount + RUN_FEE`; `remaining -= 1`; `missed = 0`.
2. Else: `missed += 1`; nothing moves. A payment that cannot be funded is a
   **no-op for that period**, never an error (invariant 7's rule, because a
   failure here would void the block over one payer's balance).
3. If `remaining == 0` or `missed >= MAX_MISSED` (3): delete both records
   and return any escrow to the payer. Otherwise rewrite `sp:d:` at
   `next_due + interval`.

Entries past the 256th stay due and run in the next block, at the front of
the scan because their `next_due` is lowest. That is first-fit by height,
the same shape as invariant 33's selection: a function of the order and the
cap, so every node makes the same choice. A flood of schedules delays other
schedules, never transactions, and each one prepaid its run fee.

The step is part of the apply path (invariant 24): it runs under the undo
journal, the block's declared `state_root` must reproduce it, and a block
that claims a different outcome is refused. It reads only committed state
and the height.

### Relation to vaults and smart accounts

A schedule is created by a signed transaction, so a vault's per-window limit
(invariant 32) applies to the **escrow debit at creation**, not to each run:
the funds leave the vault once. A smart-account spending limit (ADR-107) is
evaluated the same way. Each run moves escrow the chain already holds.

### Fee

Each run costs `RUN_FEE`, a governed parameter (invariant 17) prepaid into
escrow, plus the creation transaction's ordinary byte fee. The creation
transaction's bytes do not include future runs, so a run is not free for the
network; `RUN_FEE` is what pays for the state write.

## Alternatives considered

- **Session key plus relayer (CATALOG's suggestion, Cosmos authz).** Works
  today with ADR-107 and no consensus change, and dies with the relayer.
  Kept as the app-level option; this ADR is the protocol one.
- **Pull by the payee (`Collect { id }`).** The payee submits when due, so
  there is no end-of-block work. Rejected as the only path: a payee who
  forgets or is offline misses the payment, and the payer cannot make a
  payment happen. Could be added as a second trigger later.
- **Time in seconds with a median-time rule.** A median of block timestamps
  is still a clock the chain does not have under Standing Order 4 and
  invariant 9. Rejected.
- **Unbounded due processing per block.** A million due schedules in one
  block is a liveness attack. Rejected; 256 with carry-over.
- **Escrow-free schedules that debit the payer's balance at run time.** Then
  a run competes with the payer's own transactions for balance and the
  outcome depends on block order. Rejected; prepaid escrow.

## Consequences

- Subscriptions, payroll and allowances run without a relayer.
- Every block does bounded extra work; a block with no due schedules does
  nothing and moves no root.
- Three new `TxKind` arms under a new `Module::Scheduled` (invariant 28);
  schedule kinds are not transfer doors (invariant 32), but the escrow debit
  counts toward a vault window.
- One layer, two prefixes, one apply-path step with its undo coverage.
- Wallets show `next_due` as a height and as an estimated date, labelled as
  such.

## Invariants touched

- **24**: a new step in the only apply path, under the journal; the declared
  root must reproduce it.
- **25**: new prefix `sp:` and layer.
- **7**: an unfunded run is a no-op.
- **28**: `Module::Scheduled`.
- **32**: the escrow debit is the vault-limited movement.
- **17**: `RUN_FEE` is read from state, never a `const`.
- Standing Order 4: heights only; key-order scan; no floats.

## Activation

`FeatureSet::SCHEDULED_PAYMENTS = 1 << 17`, compiled
`SCHEDULED_PAYMENTS_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082
with the v4 bundle at **genesis + 15,552,000**. Before activation the
end-of-block step is skipped and the three kinds are refused.

## Test plan

Red tests, `crates/node/tests/scheduled_payment_tests.rs`:

- `a_schedule_runs_at_its_due_height_and_not_one_block_earlier`
- `an_unfunded_run_is_a_no_op_and_the_third_miss_closes_the_schedule`
- `the_257th_due_schedule_runs_in_the_next_block_first`
- `cancel_returns_unspent_escrow_and_deletes_both_records`
- `a_block_whose_root_omits_a_due_run_is_refused` (invariant 24)
- `a_reorg_restores_escrow_next_due_and_the_due_index`
- `a_vault_counts_the_escrow_debit_once_and_runs_not_at_all`
- `the_root_is_unchanged_when_no_schedule_is_due`

Vectors (`spec/03-state.md` SC-1..SC-4): record bytes; a two-run schedule
with the state after each due height; a miss sequence.
`crates/node/tests/chaos_simulator.rs` gains a scenario with 1,000 schedules
due at one height to pin the carry-over.

## Open questions for Eric (economics and pricing only)

1. `RUN_FEE` at activation, and whether it burns or joins the tips (as
   written).
2. `MAX_RUNS` (120 as written, ten years of monthly) and `MIN_INTERVAL`
   (one hour): acceptable, or should they be governed parameters?
