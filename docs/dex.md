# The native DEX

Two venues, one asset layer, and a batch auction in place of a mempool race.

- `dex/` — the engine. Dependency-free, `no_std`, model-checkable.
- `src/state/asset.rs` — assets other than the native coin.
- `src/state/dex.rs` — how a pool and an order are stored.
- `src/state/dex_exec.rs` — what happens when a trading transaction executes.
- `src/core/dex_payload.rs` — the wire forms.

---

## Why the engine is its own crate

The same argument that produced `ledger-math`, applied to the other half of the
code where a rounding error mints value. Kani compiles a crate *together with
its whole dependency graph*, so a crate that reaches RocksDB's C++ or the
`ark-*` stack cannot be model-checked at all. `maya-dex` has no dependencies and
never will.

The visible cost is that nothing in it hashes. Pair identifiers, order
identifiers, and share-asset identifiers arrive as opaque 32-byte values, and
deriving them is `custom-l1-node`'s job. That is the price of the boundary and
it is worth paying.

---

## The asset layer

`Account` holds one balance, every existing transfer moves it, and the state
root is computed over those records. The native coin therefore keeps the home it
already has: `NATIVE_ASSET` is all zeros and every access routes back to
`Account::balance`. Everything else lives under `d:bal:<asset><address>`.

A chain that has never registered an asset has exactly the state root it always
had. The trading layer folds into the root only when there is something in it —
the same rule the channel and shielded layers follow.

**Supply is fixed at registration.** There is no mint operation and no authority
field. A mint authority is an account that can dilute every holder, and
introducing one is a governance decision rather than a data-model one.

**Liquidity shares are assets.** A pool's shares are an ordinary asset with a
derived identifier, so they transfer through the ordinary path and can
themselves be pooled. The alternative — a per-pool table of provider balances —
is the same data structure written twice.

---

## Venue one: the constant-product pool

`x · y ≥ k`, with fees retained in the reserves.

There is no fee accumulator and no claim transaction. A swap's LP fee stays
inside the pool, `k` grows, and every outstanding share redeems for slightly
more than it did before. An accumulator would be a second representation of a
number the reserves already carry, kept in step by hand, drifting by a unit
every time a division was inexact.

The consequence worth knowing: a provider's share count never changes, so "how
much have I earned" is `shares · reserves / supply` now, minus what it was at
deposit — and nothing on chain records the second term. That is a wallet's job.

The protocol's cut, when there is one, comes off the *input* before the curve
sees it rather than being skimmed from the reserves afterwards. Skimming
afterwards lowers `k`, and then "did this swap decrease `k`" stops being a usable
invariant, because the honest answer becomes "yes, by exactly the amount we
intended".

`PROTOCOL_FEE_BPS` is **zero**. The plumbing exists end to end; nothing takes a
cut, because there is no governance process that could have decided to.

### Rounding

One rule: when a division is inexact, the remainder goes to the pool. A trader
receiving output gets a floor; a trader supplying input pays a ceiling. The
alternative is not fairer, it is a leak a bot can drain in a loop.

### The locked minimum

`MINIMUM_LIQUIDITY` shares are minted to nobody on the first deposit. Without
it, the first depositor can mint one share, donate directly into the reserves,
and make one share worth more than a later depositor's whole contribution —
which then rounds to zero shares and is absorbed.

The lock does not eliminate the rounding loss; it bounds it. A depositor can
still lose up to one share's worth, and the lock is what caps how expensive a
share can be made. The attack stops paying because the attacker recovers only
their fraction of what the victim lost, having spent the whole donation to
arrange it. `dex/tests/amm_tests.rs` checks exactly that.

---

## Venue two: the order book

Price-time priority, cleared at the **maker's** price — the order that arrived
first, identified by the lower sequence number. That single rule is what makes
queue position worth having.

The ordering *is* the storage key. An order files under
`d:ord:<pair><side><price><sequence>` with price and sequence big-endian.
RocksDB iterates lexicographically, so a prefix scan yields one side of one book
already in priority order: no sort step, and no opportunity for two nodes to
disagree about the queue because neither of them chose it. Bids store the
complement of their price so both sides are best-first ascending.

Only the protocol's rate applies to a book trade, and only to the taker. The LP
rate pays providers for the use of pooled capital, and a book trade uses none —
the counterparty is another trader.

### Escrow

A resting order holds its own assets: a bid escrows quote (rounded **up**, so it
can always cover every fill it can produce), an ask escrows the base it is
selling. The remaining escrow is carried on the record rather than recomputed,
because recomputing means reproducing the placement's rounding exactly and being
right both times. A bid that executes at a maker's better price gets the
difference back when it leaves the book.

---

## MEV: what is done and what is not

### The attack

Executed one at a time in the order a miner chooses, swaps against a curve are
trivially extractable: buy in front of a victim, sell behind them, and the
victim's own price impact pays for the round trip. It needs nothing but the
ability to decide where in a block a transaction goes.

Slippage bounds do not fix this. A bound caps the loss; it does not remove the
incentive, and a bound tight enough to remove it fails constantly on ordinary
volatility.

### What happens instead

**Every swap on a pool in one block settles at one price.** Position within the
block stops being a variable. An attacker who front-runs is in the same batch as
their victim and gets the price they engineered; the sandwich costs them the fee
and returns nothing.

The price is found by netting. Buyers and sellers in the same batch cross
against each other, and only the imbalance reaches the curve; the curve's
execution price on that imbalance becomes everyone's price. A perfectly balanced
batch clears at spot and pays no fee at all — coincidence of wants is free,
because the pool lent no capital.

`dex/tests/batch_tests.rs` runs the same three trades twice: once as a sequence,
where the sandwich is asserted to *actually pay*, and once as a batch, where it
is asserted not to. A test that only checked the batch would prove nothing.

### What is not solved

- **A miner spanning consecutive blocks.** Batching is per block. A miner who
  mines two in a row can trade in the first and unwind in the third. Defeating
  that needs commit–reveal, which costs a block of latency on every trade. Not
  implemented.
- **Censorship.** A miner who drops a transaction rather than reordering it is
  unaffected by any of this.
- **Routes.** They execute in place — see below.

---

## Two failure classes

A transaction can be **wrong**: it names a pool that does not exist, its route's
legs do not join up, its symbol has a control character in it. That is an
`Err`, and since a failing transaction fails its whole block, it is also a claim
that no honest miner would have included it.

A transaction can also merely **lose**: its bound was missed because somebody
traded first, its arbitrage was taken by someone faster. That is a **no-op** —
the nonce advances, nothing moves. It must never be an error, because two
traders racing one opportunity is the ordinary case, and if the loser took the
block down with them, every block carrying a competitive trade would be invalid.

This is the single most load-bearing decision in the subsystem. `dex_tests.rs`
tests it directly for both swaps and routes.

Deposits and withdrawals are the exception: they execute in place, so their
bounds *are* errors. There is no later settlement stage at which they could be
dropped, and the alternative is taking the assets anyway.

---

## Routes, and what "flash swap" does and does not mean

`TxKind::SwapRoute` is an atomic multi-hop swap. Every leg executes or none
does; the whole path is computed against copies and nothing is written until the
final bound is met. A route may revisit a pool, and the second visit is priced
against the first — which is what makes a two-leg round trip lose money rather
than being a free option.

That is the arbitrage instrument, and it is what `dex_tests.rs` exercises as the
flash-arbitrage case: two pools at different fee rates seeded at different
ratios, closed in one transaction or not at all.

**It is not an uncollateralised flash loan.** The sender supplies `amount_in`
from their own balance. Lending the first leg against a repayment check requires
re-entry into the borrower's code, which means the contract host ABI in
`vm/src/host.rs`. That is a larger change than this one and is not pretended at.

Routes execute in place rather than in a batch, because there is nothing to net
a multi-hop path against. This does not reopen the sandwich: routes run *before*
the block's batches, so a miner can put a trade in front of a victim's batch but
has nowhere left in the same block to unwind it, and a front-run that cannot be
unwound is just a position.

---

## What is deliberately not built

**Routing between the two venues.** A swap reaches the pool; a limit order rests
in the book; the block-end pass crosses the book against itself. Taking book
liquidity up to the pool's marginal price and the remainder from the curve is a
real feature and is *not* implemented, because a half-built router that silently
picks the worse venue is worse than no router.

**Expiry reaping on untouched books.** An expired order is skipped by the matcher
and reaped when its pair is next traded. A book nobody touches keeps its expired
orders' escrow locked. The owner can always cancel; a sweeper is not worth an
unbounded per-block scan.

**A fee market.** There is none on this chain, so the cost of resting an order is
one transaction plus its escrow. Escrow is the real deterrent, but it does not
bound the *count*, which is what every node pays for in storage. Hence
`MAX_ORDERS_PER_BOOK`.

---

## Bounds

| Constant | Value | What it stops | Measured cost at the bound |
|---|---|---|---|
| `MAX_BATCH_INTENTS` | 256 | The clearing loop is quadratic in the worst case | 4.1 µs normally; **30.6 µs** when every intent misses its bound and the loop removes one per round |
| `MAX_FILLS_PER_BLOCK` | 1024 | One deep book crossed by one order is work every node redoes | ~0.75 µs per fill, so **~0.8 ms** for a full block |
| `MAX_ORDERS_PER_BOOK` | 4096 | Storage and book reconstruction cost | 117 µs to rebuild 1024 orders |
| `MAX_ROUTE_LEGS` | 4 | Pools loaded per transaction, and the search space a miner could grind | 22.6 ns per curve evaluation |
| `MAX_TOTAL_FEE_BPS` | 500 | How badly a pool creator can misconfigure one | — |

Figures from `cargo bench --bench dex_matching`. They are validation costs, not
miner costs: every node redoes a block's matching pass and its batch clearing.

The one worth reading twice is the adversarial batch. The removal loop is
quadratic and the worst case is reachable on purpose — fill a batch with bounds
that fail in cascade — so the number that justifies the ceiling is 30.6 µs, not
the 4.1 µs of the ordinary case. At 256 intents that is comfortable; the
quadratic term is what makes it a ceiling rather than a preference.

---

## Reorgs

Every trading record's prior value goes in the undo journal. Without it a reorg
leaves a pool holding the abandoned chain's reserves — which is not a detectable
corruption, because the reserves are still two plausible numbers and the pool
goes on quoting prices from them.

A pool created by a reverted block is deleted rather than restored as an empty
record, for the same reason an account that did not exist is deleted rather than
restored as a zero: a spurious record changes the state root.

---

## Concurrency, honestly

Block execution is single-threaded and deterministic: every mutation lands in
one overlay, flushed as one write batch. There is no concurrent state mutation
to race.

What is concurrent is everything in front of it. Transactions arrive on many
tasks at once into a shared `Mempool`, and come back out in hash-map order — so
two nodes assembling a block from the same set will not agree on a sequence. The
property that matters is therefore **confluence**: one set of transactions, one
state root, whatever the ordering. `tests/dex_concurrency_tests.rs` checks that,
and separately checks that no position in a block pays a trader better than any
other — which is the executable form of the batch's whole claim.
