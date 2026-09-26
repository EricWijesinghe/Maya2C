---
title: 'Batch references and deterministic sharded execution'
editUrl: false
# GENERATED from docs/blockgraph.md by scripts/ingest.mjs. Edit the source, not this.
---
**Status: research branch. No consensus path reaches this code.**

Two things live here. `maya-blockgraph` carries the bounds, the reference-set
rules, and the scheduler; `core::batch` in the node carries the digest
derivation and the access-set computation. Neither is wired to an activation
height, and no header field references a batch.

---

## What this takes from Narwhal, and what it does not

Narwhal's contribution is **separating data availability from consensus**. A
block orders *references* to batches that were already disseminated, so block
propagation costs one 32-byte digest per batch instead of every transaction
byte, and the transaction bytes travelled earlier — off the critical path, while
miners were still searching. That half transfers to proof of work unchanged, and
it is the half implemented here.

The other half does not transfer, and the reason is not a detail.

Narwhal's availability guarantee **is** the quorum certificate: a batch is
available because `2f + 1` of a **known committee** signed for it. Tusk's leader
election needs a shared coin over that same committee. Maya2C has no committee,
no `f`, and no stake weight — membership is open and churns, which is what
proof of work is for.

So there are no certificates here. Availability is a *local* question: a
validator that cannot fetch a referenced batch does not build on the block that
referenced it. That is strictly weaker than Narwhal, and it is written down
rather than implied away.

Adopting Narwhal **and** Tusk would mean replacing Nakamoto sybil resistance
with a BFT committee — a different chain, with deterministic instead of
probabilistic finality, after milestones 14–16 built post-quantum signatures for
open participation and 22 built on-chain governance. That is a decision somebody
writes down, in the sense of invariant 11.

## Naming

`crates/node/src/crypto/dag/` is the **Ethash-style memory-hard proof-of-work dataset** —
`cache.rs`, `dataset.rs`, `hashimoto.rs`, `DagConfig`, `CacheRegistry`,
`DAG_ACTIVATION_HEIGHT`, `docs/dag-pow.md`, `crates/node/tests/dag_tests.rs`,
`crates/node/benches/dag.rs`, `hal/cuda-miner/tests/dag_parity.rs`.

It has nothing to do with a block DAG. Anything about the transaction graph is
called `blockgraph`, and `dag` is left alone.

## The throughput ceiling

The point of the exercise is a number, so here it is:

```text
MAX_BATCH_TRANSACTIONS  x  MAX_REFS_PER_BLOCK  =  512 x 256 = 131,072 tx/block
at TARGET_BLOCK_TIME = 15 s                    ->  ~8,738 tx/s
```

That is **structural, not measured** — the ceiling the constants impose, before
anyone checks whether a node can reach it.

It is about a sixth of the 50,000/s this work was originally scoped against, and
the binding constraint is not this crate:

| | |
|---|---|
| ML-DSA-65 signature (FIPS 204) | 3,309 B |
| SLH-DSA-SHA2-128s signature (FIPS 205) | 7,856 B |
| **Hybrid, per transaction** | **11,165 B** |
| 50,000 tx/s | **558 MB/s = 4.47 Gbit/s** of signature bytes |

Both signatures must verify and there is no fallback path
(`docs/hybrid-signatures.md`). 4.47 Gbit/s of signatures alone, before payloads
and before batch dissemination traffic, is not a 5-node-on-one-host number.

A throughput test should therefore **measure and report two figures** — one with
real hybrid signatures, one with mock unsigned transactions to isolate mempool
cost — and assert neither. A test asserting 50,000/s would either fail or be
measuring something that is not this chain.

Raising `MAX_REFS_PER_BLOCK` raises the ceiling linearly and raises the work one
block can demand of every node: a validator must fetch and verify every
referenced batch before it can check the state root.

## Deterministic execution instead of two-phase commit

### Why not 2PC

Two-phase commit is **blocking** — a coordinator crash leaves participants
holding locks — and it is not Byzantine-safe. Neither objection is the main one.

The main one is that 2PC exists to agree an order *at commit time*, and here the
order is already agreed. Running a lock protocol on top of a system that
eliminated the need for one buys nothing.

### The scheduling rule

For transaction `i`, with `conflicts(i, j)` meaning their shard sets intersect:

```text
wave(i) = 0                                    if no j < i conflicts with i
wave(i) = 1 + max{ wave(j) : j < i, conflict } otherwise
```

**Within a wave, nothing conflicts.** If `i < j` are both in wave `w` and share
a shard, then when `j` was assigned that shard was already claimed at wave `w`,
so `wave(j) >= w + 1`. Contradiction.

**Conflicting transactions keep their relative order.** If `i < j` conflict then
`wave(j) > wave(i)`, directly from the rule.

So waves execute in sequence, transactions within a wave in any order on any
number of threads, and the result is the state the serial order would have
reached. One pass, `O(n)` with a 64-bit mask per transaction; the `last_claimed`
table is 64 words on the stack.

### Shards are not threads

A *shard* is a partition of state; a *thread* is an execution resource. There are
64 shards because `shard_of` takes the high six bits of the address's first byte
— a prefix, so a shard is a contiguous RocksDB key range rather than a scattered
one. How many threads serve a wave changes no result.

### The one way this goes wrong

The scheduler is handed access sets and trusts them. A set that is too **wide**
costs parallelism. A set that is too **narrow** is a consensus failure:
contending transactions land in one wave, run concurrently, and the state root
depends on which thread won.

The errors are not symmetric, so `access_for_transaction` resolves every doubt
towards the wider set. Anything added to `core::payload` touching state not named
by an input or output must be reflected there, and the failure mode of
forgetting is silent.

## What is tested

| | |
|---|---|
| `blockgraph` unit tests | 25 |
| `serial_equivalence_tests.rs` | 8 |
| `core::batch` unit tests | 8 |

The equivalence tests run an executable model — 64 slots, one per shard — and
compare scheduled execution against serial execution across 64 seeds, with each
wave applied forward, reversed, and rotated. The fold is deliberately
**order-sensitive**; a commutative one would pass even for a scheduler that
reordered conflicting transactions, which is the bug worth catching.
`the_model_would_notice_a_reordering` asserts that property of the model itself.

The structural properties are also asserted directly, not only inferred from
matching state: no wave holds two conflicting transactions, and conflicting
transactions land in strictly increasing waves.

## What is proved

`crates/blockgraph/src/proofs.rs`, under `cargo kani -p maya-blockgraph`:

| Harness | Bound |
|---|---|
| `every_address_lands_in_a_partition_that_exists` | **unbounded** |
| `an_access_set_is_refused_exactly_when_it_is_empty` | **unbounded** over `u64` |
| `conflict_is_symmetric_and_reflexive` | **unbounded** over both masks |
| `a_reference_set_is_accepted_exactly_when_it_is_well_formed` | 3 identifiers |
| `a_batch_is_accepted_exactly_when_it_is_well_formed` | 3 transactions |
| `no_wave_holds_two_conflicting_transactions` | 3 transactions, 3 shards |
| `conflicts_are_ordered_and_nothing_is_dropped` | 3 transactions, 3 shards |

**These harnesses have not been executed.** Kani is not installed on the
development host, and `#[cfg(kani)]` means `cargo check` does not compile them
either. They are unverified in both senses until someone runs them.

## Not built

Phases 2, 3, and 5 of the agreed plan:

- **Batch dissemination** — the `crates/node/src/network/` worker that broadcasts batches,
  answers fetches, and evicts. Needs a libp2p protocol and a fuzz target for
  batch decode.
- **Header batch references** — `BlockHeader` is fixed-width (`[u8; 144]`, with
  `serialize()` writing constant offsets). Adding a reference list makes it
  variable-length: new codec, new fuzz target, and since the proof-of-work digest
  covers the header, a hard fork gated at an activation height.
- **The 5-node cluster test** — measure-and-report, `#[ignore]`d like the
  existing soak tiers, with the host's 31.4 GB in mind.
