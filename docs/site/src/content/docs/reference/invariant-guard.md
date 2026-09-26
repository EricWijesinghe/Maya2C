---
title: 'The Invariant Guard'
editUrl: false
# GENERATED from docs/invariant-guard.md by scripts/ingest.mjs. Edit the source, not this.
---
What has to be true of a block before it commits, and what happens when it is
not.

- `crates/node/src/state/invariant_guard/conservation.rs` — the one equation every block satisfies.
- `crates/node/src/state/invariant_guard/anomaly.rs` — the checks that are a judgement rather than a rule.
- `crates/node/src/state/invariant_guard/breaker.rs` — the per-module circuit breaker and its record.
- `crates/node/src/state/invariant_guard/limits.rs` — every threshold, each with its reasoning.
- `crates/node/src/state/db.rs` — `stage_block`, the single place the guard is called.
- `crates/node/src/state/settlement.rs`, `crates/node/src/network/mempool.rs` — the two gates that read a breaker.
- `crates/node/tests/exploit_replays.rs` — the attack replays.

---

## The one idea to read first

**A block that creates value is invalid. A block that is merely alarming is
valid, and stops the module it alarmed.**

Those are two different claims and conflating them produces a system that is
wrong in both directions. Refuse everything unusual and a threshold becomes a
consensus rule, so tuning it forks the chain and a false positive is a halt
nobody chose. Refuse nothing and a soundness break drains a pool before anyone
reads a dashboard.

So the guard has exactly two outcomes:

| Kind | Example | Outcome |
|---|---|---|
| Conservation | a block creates a unit of an asset from nothing | the block is **invalid**; nothing commits, no breaker is written |
| Anomaly | a pool is suddenly worth less per share | the block **commits**, and the module is halted for 100 blocks |

A conservation failure has no innocent reading, so it is refused exactly as a
wrong state root is refused (invariant 24). An anomaly does have one — a
threshold is a judgement about what is *unusual*, not about what is *legal* —
so the block stands and the affected module stops accepting new work while a
human looks.

---

## Where it runs

`StateDB::stage_block` is the single place a block's overlay is produced. All
four commit paths funnel through it:

| Path | What it is |
|---|---|
| `apply_block` | the plain apply |
| `apply_block_checked` | via `stage_checked` |
| `apply_block_journaled` | via `stage_checked`; the chain's only apply path |
| `preview_root` | what a miner runs while assembling a candidate |

One hook at the end of `stage_block` therefore covers all four, and the fourth
is what makes this more than a check: a miner assembling a candidate through
`Chain::candidate_block` runs the same guard, so an honest miner refuses to
*build* a block that breaks an invariant rather than minting one the network
then rejects.

It runs **last**, after the sealed, trading, oracle and governance settlement
passes. Running it earlier would check a half-built block: the trading pass
moves reserves and the governance pass returns deposits, and a conservation
equation evaluated between the two is not the block's equation.

---

## Conservation

For every asset, native and registered, what a block moves must equal what it
issues:

```text
Σ holdings delta  ==  supply delta
```

The left side is every place on this chain that can hold value, which is a
closed list:

| Asset | Held in |
|---|---|
| Native | `acct:` balances, `chan:` capacity, the shielded pool's public balance, `g:lock:` stakes, `g:prop:` deposits |
| Registered | `d:bal:` balances, `d:pool:` reserves, `d:ord:` escrow |
| LP share | `d:bal:` balances |

The right side is zero for the native coin — there is no mint operation and no
block reward, so the genesis allocation is the supply forever. For a registered
asset it is the change in `d:asset:<id>.total_supply`, non-zero exactly once at
registration. For a share asset it is the change in its pool's `total_shares`.

### Why a delta and not a sum

Summing every account each block would make block execution O(state), so
execution would slow down for the life of the chain. The overlay already holds
exactly what the block touched, so every term is a difference between an
overlay value and its committed one: O(touched), which for a normal block is a
handful of keys.

It is also not weaker. Conservation over deltas, applied to every block from
genesis, is an induction: if the total was right before the block and the block
moved a net zero, the total is right after it. An absolute `MAX_SUPPLY` scan
would check the same fact more slowly, and only for the assets it remembered to
look at.

### What it buys

Each of the five native holding places is checked today only by its own local
rules — channel escrow by `settlement.rs`, pool reserves by the curve, stakes
and deposits by `governance_exec.rs`, the pool balance by the joinsplit
verifier. A bug in any one of them is invisible to the other four. The
conservation equation is the only thing in the tree that sees all five at once.

---

## The anomaly checks

Two, and both are stated with the reason the threshold has the value it has.

### Pool value per share — `POOL_VALUE_TOLERANCE_BPS = 0`

The quantity is `k / s²`, where `k` is the product of the reserves and `s` the
issued share count: the value one share redeems for, up to the curve. Under
every operation this chain has, it may only rise:

- a swap adds the LP fee to the reserves and issues no shares;
- a deposit rounds the shares it issues **down**;
- a withdrawal rounds the reserves it pays out **down**.

So a fall is not a rounding artefact of the intended rules. It means value left
a pool without a matching share being burned — a curve bug, a fee accounted the
wrong way, or a sequence of operations that leaves the pool poorer than it
started. That is the whole AMM-manipulation class, named by mechanism-free
effect rather than by attack.

The tolerance is **zero**, because there is no legitimate fall to tolerate. Any
non-zero value here is a budget an attacker can spend once per block.

The comparison is exact: `k` and `s²` are both already 128-bit, so
`anomaly.rs` carries a hand-written 256-bit multiply and cross-multiplies
rather than dividing. There is no rounding for two nodes to disagree about, and
no dependency whose rounding decisions would become consensus.

A pool being created or fully redeemed is skipped rather than tolerated: with
no shares on one side there is no value per share to compare, and conservation
already covers where the reserves went.

### Shielded drain rate — `SHIELDED_DRAIN_BPS = 5_000`

Conservation cannot see inside the pool. A joinsplit that mints hidden value
and then withdraws it balances perfectly at the boundary, because the pool's
public balance falls by exactly what the transparent side gains. The only thing
the transparent chain can observe about a Groth16 soundness break is the *rate*
at which the pool empties.

So this is a rate limit, not a correctness rule, and it is set high on purpose.
Half the pool in one block is far beyond ordinary traffic —
`MAX_SHIELDED_PER_BLOCK` is 64 joinsplits — while still leaving a large
legitimate exit room to clear. The cost of a false positive is that an unusually
large withdrawal waits 100 blocks; the cost of not having it is that a break
drains the pool in one block and the first anyone knows is the balance.

#### And the floor under it — `SHIELDED_DRAIN_FLOOR = 1_000_000`

A rate with no floor is not a safety rule, it is a lever. `before` is the pool's
balance immediately prior to the block, so on a thin pool "half of it" is a
small absolute number — and anybody holding that much can trip the breaker at
will, halting **every other user's** shielded transactions for a hundred blocks,
for the price of one fee, every window, indefinitely. The same arithmetic makes
an honest large exit from a young pool a false positive whose blast radius is
the whole module rather than the withdrawer.

The rule the rate implements is "a soundness break should not empty the pool
before anyone notices". Below the floor there is nothing worth noticing: the
entire loss is bounded by the pool, and halting the subsystem costs more than
the value at risk. So the check applies only once the pool is large enough for
that trade to go the other way.

The number is a policy choice and is stated as one. This codebase has no
decimals constant — every amount is base units — so it is anchored to the
largest ordinary amount the tree names: `MAYA_FAUCET_DAILY_CAP` defaults to
1,000,000 base units. A shielded pool holding less than a testnet faucet hands
out in a day is not a pool worth halting the network over. Revisit it if the
coin's scale is ever fixed.

`a_thin_shielded_pool_can_be_emptied_without_halting_anyone` empties 90% of a
pool under the floor and asserts no breaker is written and the module still
works.

### What is not checked yet

Two detectors, and they cover two modules. `Channels`, `Oracle`, `Governance`,
`Vm` and `Sealed` are gateable — `require_module` gates all seven — but nothing
in the tree trips them today, so a breaker for those five is machinery with no
trigger attached. They are reachable only through a conservation failure, which
invalidates the whole block rather than halting a module.

That is stated rather than left to be discovered, because the failure it
prevents is somebody reading the module list and concluding the governance
subsystem has anomaly protection it does not have. Adding a detector for one of
them is the same shape of work as the two here: name the quantity that may only
move one way, state why the threshold is the value it is, and pin it with a test
that fails without the guard.

---

## The circuit breaker

### Modules, and the one that does not exist

The breaker halts a **module**, deliberately coarser than a `TxKind`. An
operator reading an alert wants to know that "the DEX is halted", and a breaker
per transaction kind would let an attacker halt one narrow operation while
leaving the rest of a broken subsystem running.

| Module | Covers |
|---|---|
| `Channels` | open, close, dispute, penalty, batch settle |
| `Dex` | assets, pools, liquidity, swaps, routes, the order book |
| `Shielded` | the shielded pool |
| `Oracle` | price feeds and the randomness beacon |
| `Governance` | stake, proposals, votes, work claims |
| `Vm` | contract deploy and call |
| `Sealed` | the sealed mempool: envelopes and decryption shares |

There is **no `Transfer` module**, and that is the design rather than an
omission. `Module::of(TxKind::Transfer)` returns `None`, so a plain
peer-to-peer payment is ungated by construction and no future edit can
accidentally gate it. The match in `Module::of` is exhaustive with no wildcard
arm, so adding a `TxKind` is a compile error until somebody decides which
module owns it — which is what "basic peer transfers keep working" has to mean
if it is to survive somebody adding a module.

### The record

```text
g:guard:<module tag>  ->  module | invariant | tripped_at | until
```

`until = tripped_at + 100`, saturating. A hundred blocks is roughly twenty-five
minutes at this chain's target spacing — long enough that a human sees the
alert, short enough that a false positive is an outage measured in minutes
rather than a governance emergency. Nothing re-arms it automatically: it
expires, and if the condition still holds the next block trips it again.

A module that trips while already halted has its window extended from the new
block rather than expiring on the old schedule.

The prefix is under governance's `g:`, asserted at compile time in
`breaker.rs`, so the record is covered by the state root through the existing
`StateLayer::Governance` mapping and needs no new entry in
`state::commitments` (invariant 25).

### The two gates

| Gate | Where | Reads |
|---|---|---|
| Consensus | `settlement.rs`, per transaction, before it is applied | the overlay, at the block's height |
| Admission | `mempool.rs`, on submit | committed state, at the tip height |

The mempool gate is admission, not consensus — the block executor applies the
same gate, and this only stops the pool accumulating transactions for a block
that would refuse them. It reads the tip height rather than the height the
transaction will land at, which is the closest thing the pool has and is off by
at most the breaker's last block. A transaction whose module is halted is
rejected rather than held: the sender can resubmit after the breaker clears,
and a pool that held everything for a hundred blocks would be a queue an
attacker can fill.

---

## What this guard does not claim to catch

The brief that produced it named re-entrancy, integer overflow and flash-loan
manipulation. Two of those three have no surface on this chain and the third is
not what the phrase usually means. `crates/node/tests/exploit_replays.rs` demonstrates that
rather than asserting it:

- **Re-entrancy** needs a contract-to-contract call. The VM exposes nine host
  functions, none of which calls a contract or moves value, and `Vm::validate`
  now refuses an unknown import at **deploy** rather than at first call — so a
  module importing `env.call` never reaches the chain.
- **Integer overflow** is already refused. `ledger-math` is checked arithmetic,
  Kani-verified, and every call site turns `None` into `NodeError::BalanceOverflow`,
  which fails the whole block.
- **Flash loans** do not exist. `SwapRoute` is self-funded — no borrow, no
  callback — and `l2-flash` is a payment-channel network, not a lending one.

What the guard is actually for is the class underneath all three: value created
from nothing by *any* future bug, across all five holding places at once.

---

## Determinism

The guard reads only committed state and the block. No clock, no configuration,
no node-local value, ever. A breaker that trips on one node and not another
changes which transactions are valid on each, which is a chain split — the same
reason invariant 24's state-root check reads only what execution produced.

Two consequences that look like fussiness and are not:

- The anomaly checks run in a **fixed order**. A block that trips two must trip
  them in the same order on every node, or two nodes write two different sets of
  records and the state roots differ.
- The pool comparison does no division and takes no dependency. See above.

---

## Tests

`crates/node/tests/exploit_replays.rs`, eleven replays:

| Test | What it pins |
|---|---|
| `a_module_importing_a_contract_call_or_a_value_transfer_cannot_deploy` | re-entrancy has no surface, checked at deploy |
| `the_only_imports_that_resolve_are_the_nine_that_move_no_value` | the host surface, against what the linker registers |
| `a_contract_that_re_enters_its_own_storage_moves_no_balance` | self-re-entry through storage is inert |
| `a_transfer_that_would_overflow_the_recipient_is_refused_and_changes_nothing` | overflow fails the block, atomically |
| `outputs_that_sum_past_u64_are_refused_before_any_balance_is_read` | overflow is refused at validation, not after a partial debit |
| `a_deposit_that_would_overflow_a_reserve_is_refused_and_changes_nothing` | the same for pool reserves |
| `a_round_trip_route_cannot_end_ahead_of_where_it_started` | the nearest thing to a flash loan loses to fees |
| `a_sandwich_around_a_batch_clears_at_one_price` | uniform-price clearing defeats the sandwich |
| `every_legitimate_subsystem_block_satisfies_conservation` | the guard does not refuse honest blocks |
| `draining_the_shielded_pool_trips_the_breaker_and_transfers_keep_working` | the breaker fires, and transfers survive it |
| `a_thin_shielded_pool_can_be_emptied_without_halting_anyone` | the floor: a pool too small to matter is not a lever |

Plus 18 unit tests in `conservation.rs` covering each holding place in both
directions (a credit from nowhere and a burn), ten in `breaker.rs` for the
record's wire format and the expiry boundary, and three in `anomaly.rs` for the
256-bit product.

The last row is the one that matters most. A circuit breaker that halts the
chain is not a safety feature, it is the outage the attacker wanted; the test
trips the shielded breaker and then asserts a plain transfer still applies in
the same block range.
