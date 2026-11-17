# ADR-144: State and fee rule hardening for the re-freeze (bundle 3)

**Status:** Accepted, 2026-11-17. Eric pre-approved this item into re-freeze
bundle 3 on 2026-11-17 (DECISIONS.md, same date). Lands through PR #584.
**Date:** 2026-11-17
**Related:** [ADR-029](ADR-029-fee-market-live.md) (base fee),
[ADR-044](ADR-044-mining-rewards-lane.md) (fee-free mining shares),
[ADR-082](ADR-082-genesis-committed-upgrade-levers.md) (upgrade schedule),
[docs/dex.md](../dex.md), invariants 7, 8, 12, 17 and 25.

## Context

A security review of the state layer found four places where a rule as
written in the docs and the rule as executed differ. Two of them are now
carried by other re-freeze PRs, so this ADR records the two that stay here and
where the others went.

## Decision

### 1. A fee-free mining share carries nothing (spec FEE-7)

ADR-044 exempts a mining share from the base fee because it moves nothing: a
new miner holds no coin. The executor and the mempool tested only that the
share had no outputs. A share's inputs are carried in its encoding too, so a
share with inputs is not the zero-value share the exemption describes.

From this change a share is base-fee exempt only when it has **no inputs and
no outputs**. One predicate, `state::fee_exec::is_free_share`, is used by the
executor (`charge_fee`) and by the mempool (admission, sender accounting), so
the two cannot drift. Any other share pays the base fee on its size like any
other transaction (FEE-5). A share with an input and no output cannot pay,
so the block that carries it is invalid (`FeeTooLow`) and the mempool refuses
it before relaying.

The base fee itself, its floor and its price are unchanged: this narrows who
is exempt and moves no economic parameter.

### 2. A governed DEX rate a pool cannot carry never fails a block

`DexProtocolFeeBps` is governed (invariant 17) and is applied at execution to
each pool's own LP rate. When the sum passes the fee ceiling, the pool cannot
carry the governed rate. Before this change that case returned an error from
the end-of-block batch settlement, which would make every block that touched
the pool invalid: one pool's fee schedule could stop the chain. Invariant 7
already says a batch the pool cannot price is a no-op, never an error.

- **Batched swaps:** the batch is refunded, every intent's input goes back to
  its trader, and the pool does not move. The same refund path the
  drained-pool case already used.
- **Order book:** fills now take the governed protocol rate, as
  `docs/dex.md` ("Only the protocol's rate applies to a book trade, and only
  to the taker") already states; the code read the stored record, whose
  protocol rate is always zero. When the pool cannot carry the governed rate,
  the book rests for that block with no funds moved.
- **Routes:** unchanged. A route that names such a pool is refused as a
  transaction error, as before, so it is never included.
- Only the fee-ceiling rejection maps to "cannot carry". A storage or decode
  error reading the parameter still fails the block: a node-local read failure
  must not change what a block does.

The governed default is 0 bps, so nothing changes at genesis.

### 3. Confidential-AI single-use guards (RESEARCH crate, not consensus)

`maya-confidential-ai` `Participant` refuses a second share set and a second
masked input per instance. The guard is per instance, not per round: fresh
entropy per round remains a caller requirement and is now stated on
`Participant::new`. Nothing in consensus calls this crate.

### Moved to other PRs

- **Settlement range recheck** (a queued value out of range at execution
  expires the proposal instead of failing the block): carried by #575, with its
  own test `a_passed_change_out_of_range_at_execution_expires_instead_of_halting`.
  Dropped here to avoid a duplicate.
- **Work-claim schedules:** #570 (with #506) makes an entry naming an
  unimplemented feature bit, work claims included, valid but unsupported, so a
  binary halts at its activation instead of refusing it at construction. A
  construction-time refusal here would contradict that design (it makes the
  validity of the block that writes the entry depend on the binary). Dropped
  here.

## Invariant analysis

- **7** strengthened: the governed-rate case now refunds rather than erroring.
- **8, 25:** refunds and fills write through the overlay with existing
  prefixes (`credit_asset`, order records); no new prefix, so the undo journal
  and the state root cover them as before.
- **12, 17:** the governed rate is read from the parameter table at
  execution, now for book fills as well as swaps.
- **24, 28:** no new apply path; refunds conserve supply, so the conservation
  guard is unaffected.
- No float, no `HashMap` iteration order, no wall-clock read, no `unsafe`.

## Consequences

- Consensus change: a block that a previous binary accepted (a fee-free share
  with an input) is refused by this one, and a block that a previous binary
  refused (a batch on a pool that cannot carry the governed rate) is accepted.
  Both land before genesis in the re-freeze, so no live chain is affected.
- Spec: FEE-7 in `spec/04-fees.md`, reference `maya_spec_ref::fees::share_fee_exempt`,
  vectors in `spec/tests/fees.json`, replayed by `crates/node/tests/conformance.rs`
  and `spec/verifier-ts/verify.ts`. The DEX rule has no spec section yet
  (`docs/dex.md` is its description) and is pinned by `crates/node/tests/dex_tests.rs`.

## Alternatives considered

- **Refuse a share with inputs outright** instead of charging the base fee.
  Equivalent in effect today (a share with no outputs cannot pay), but a new
  validity rule rather than a narrower exemption; the narrower exemption keeps
  one fee rule for every transaction kind.
- **Clamp the governed DEX rate** to what each pool can carry. Rejected: it
  would charge a rate governance did not choose, and the clamp would itself be
  an economic rule.
