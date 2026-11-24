# ADR-154: One clearing per pool per block, and a batch that stays open to a better bid

**Status:** Accepted, 2026-11-24. Eric decided both items on 2026-11-24
(decisions 9 and 10 of the second round: "batch all routes into one clearing
step per block" and "fee-weighted eviction + per-sender share cap").
**Date:** 2026-11-24
**Related:** [ADR-144](ADR-144-state-and-fee-rule-hardening.md) (governed DEX
rate), [docs/dex.md](../dex.md), invariants 6, 7, 8 and 17,
`crates/dex/src/clearing.rs`, `crates/node/src/state/dex_exec.rs`.

## Context

Plain swaps on a pool already settled at one uniform price per block
(`maya_dex::batch`), which removes the sandwich for them. Two gaps remained.

1. **Routes executed in place.** `SwapRoute` ran leg after leg where it sat in
   the block. A miner could put one route in front of a victim's route and a
   second behind it, and the victim's own price impact paid for the round
   trip. `docs/dex.md` said this could not happen because routes ran before
   the batches, so nothing could unwind a front-run in the same block. That
   was wrong twice: a second route *after* the victim unwinds it, and so does
   the plain-swap batch, which ran after every route.
2. **A full pair locked everyone else out.** A pair's batch held 256 intents
   and refused the 257th with a block-level error. Whoever filled a pair first,
   from one account or many, kept every other trader off it for that block, and
   paid nothing beyond the transaction fees to do it.

## Decision

### 1. Every pool clears exactly once per block, routes included

A route is staged like a swap: its input goes into escrow when the transaction
is applied, and nothing is priced. At the end of the block
`maya_dex::clearing::clear_block` clears every pool once. A pool's batch holds
its plain swaps and one leg of every route that crosses it; all of them settle
at that batch's single price.

- **Order of pools.** A route's later leg needs its earlier leg's output, so
  each route adds edges `hop[i] -> hop[i+1]` to a precedence graph, and pools
  clear in topological order, smallest pair identifier first among the ready
  ones. The order is a function of the set of intents, never of where a
  transaction sat in the block.
- **Conflicting routes.** Two routes may ask for opposite orders. The one
  that joins first (below) fixes the order; the other closes a cycle and is a
  no-op (invariant 7): its input comes back and nothing moves. Opposite flows
  through a hub pair in one block (`X->N->Y` beside `Y->N->X`) conflict this
  way, so one of the two waits a block. That is the stated cost of one
  clearing per pool.
- **A route visits each pool at most once.** A revisiting route would need its
  pool to clear twice, so it is refused at staging as malformed, like a route
  whose legs do not join up.
- **All or nothing, in fee order.** Routes join the clearing one at a time,
  heaviest first: the fee their transaction paid, then the smaller
  identifier, the weight eviction uses (section 2), a sealed reveal above
  every plaintext route. A route joins only if its path keeps the pool order
  acyclic and every route already in, and itself, then settles in a clearing
  recomputed from the original pools. Otherwise it is refunded whole and the
  set is unchanged. Pools no route touches are cleared once.
- **Bounds.** At most `MAX_BLOCK_ROUTES` = 64 routes per block (each costs
  one recomputation), at most 4 hops each, so a pool's batch holds at most
  `MAX_CLEARING_INTENTS` = 256 + 64 intents. Compiled DoS bounds, liftable by a
  height-keyed scheduled upgrade rather than a hard fork.
- **Sealed routes** reveal before trading and join the same clearing.

**The guarantee:** within a block, no trade on a pool executes before another
trade on the same pool, so nobody can trade in front of a route. A sandwich
needs two executions on one pool with the victim between them; a pool has one.
A trader who joins the victim's batch gets the victim's price, not a better
one.

**The guarantee, for refunds:** a refunded route leaves no trace. The block
clears exactly the routes that joined, so a route that misses its bound, on
purpose or not, cannot have moved a pool another route settled against, and
cannot have kept another route out. Only a route ahead of it in fee order that
settles can keep a route out; an identifier ground to be small wins only a tie
between equal fees.

*Amended 2026-11-24 after review of #699.* The first version removed every
failing route in one pass and admitted routes in identifier order once, before
any route was priced. Both let a route that was going to be refunded anyway
void an honest one: a large first leg that skews a pool, then an impossible
bound (`clearing_tests.rs`
`a_doomed_route_cannot_void_an_honest_route_on_a_pool_it_skewed`,
`a_doomed_route_cannot_void_an_honest_route_that_ends_after_it`), or a
one-unit reverse route with a smaller identifier
(`a_failing_reverse_route_with_a_smaller_id_cannot_void_a_route`,
`a_higher_fee_wins_an_order_conflict_over_a_smaller_id`), with node-level
replays in `dex_fairness_tests.rs`. Removing one route per pass, the one that
fails at the latest pool, was considered and rejected: a victim whose last leg
sits after the attacker's failing pool is then removed first (the second test
above).

### 2. A full batch evicts its cheapest intent for a better one

- **Weight.** An intent's weight is what its transaction paid the fee
  collector, then the reverse of its identifier as a tiebreak. A sealed
  reveal ranks above every plaintext intent, because it was ordered blocks ago
  and cannot rebid.
- **Eviction.** When a pair's batch is full (256 plain swaps), a new intent
  that outranks the cheapest staged one takes its place, and the evicted
  intent's input is refunded. One that does not outrank it is a no-op: nothing
  taken, the nonce spent. Never a block-level error (invariant 7). Routes have
  the same rule against `MAX_BLOCK_ROUTES`.
- **Share cap.** One sender holds at most a sixteenth: 16 intents of a pair's
  batch, 4 of a block's routes. A sender past the cap has sent a malformed
  transaction (`TooManySwaps`); a miner leaves it out.

**The guarantee:** a pair's batch is never closed to a better-paying plaintext
trader unless the batch is full of sealed reveals, and filling a pair takes at
least sixteen funded accounts, each of which pays for every slot it holds.
Sixteen accounts with sixteen sealed swaps each can fill a pair for one block;
each of those swaps was committed and paid for blocks earlier.

## What was rejected

- **A canonical per-block route order** (sort by hash, by a beacon) as the
  execution order. The miner can grind its own route's identifier into a
  front position, and an ordered sequence of in-place executions is still a
  sequence to be in front of. The clearing uses an identifier only to break a
  tie between equal fees when deciding which of two conflicting routes joins;
  it never decides who trades first, and a ground identifier buys nothing
  against a route that pays more.
- **Identifier-order admission, decided once.** The first version of this
  ADR. A blocker with a smaller identifier fixed a pool order even when it
  was itself refunded (amendment above).
- **Hop-index rounds** (every first leg clears together, then every second
  leg). A trade at round 0 on a pool sits in front of every second leg on it.
- **A simultaneous multi-pool solver.** Correct but iterative with no
  termination proof in integer arithmetic. The topological order terminates
  and is exact.
- **Amount-weighted eviction.** Free to grief with balanced capital: buys and
  sells that net inside the batch pay no fee and move no price.

## Consequences

- **Consensus change.** State roots differ from the in-place behaviour for any
  block with a route. Pre-genesis re-freeze; the running testnet needs a
  re-genesis to carry it.
- **Behaviour now refused:** a route revisiting a pool; a seventeenth swap from
  one sender on one pair in one block; a fifth route from one sender in one
  block.
- **Latency for arbitrage.** A route now settles at the batch price, not at the
  price it saw; arbitrageurs compete on the same terms as everyone else.
- **Cost.** Each route that joins, or tries to, recomputes the route-carrying
  pools once: at most `routes` passes. The worst case at 64 routes is NOT MEASURED;
  `cargo bench --bench dex_matching` has no clearing case yet.
- **Tests.** `crates/dex/tests/clearing_tests.rs` (engine) and
  `crates/node/tests/dex_fairness_tests.rs` (block execution: the sandwich
  attempt with a route and with a swap as the back leg, order independence of
  the state root, one sender filling a pair, a sixteen-account flood met by a
  better and a worse bid). Spec rules DEX-1 to DEX-4.
