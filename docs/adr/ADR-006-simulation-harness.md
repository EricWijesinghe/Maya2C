# ADR-006: A hand-rolled deterministic simulator, not `madsim` or `turmoil`

**Status:** Accepted
**Date:** 2026-09-20

## Context

Phase C of the foundation brief asked for:

> Seeded, deterministic multi-node simulator: fake clock, fake network
> (latency, loss, partitions, reordering), fake disk (corruption, slow
> writes). Model it on FoundationDB / TigerBeetle simulation testing;
> evaluate madsim or turmoil. Every failure prints its seed so it can be
> replayed exactly. All later chaos, latency, partition, Earth–Mars, subsea,
> and Byzantine tests use this harness.

The tree already has pieces of this and they do not compose:

- `crates/node/src/network/sim.rs` models latency, loss and partitions, but
  only for the node's own gossip, and it is tagged SHIPPED in
  `docs/architecture-vision.md`.
- `crates/node/tests/chaos_simulator.rs` replays a specific attack.
- `crates/node/tests/latency_sim_tests.rs` measures propagation.
- None of them is seed-replayable in the sense the brief means: a failure
  does not hand you an integer that reproduces it.

## Decision

A new dependency-free crate, `sim/` (`maya-sim`), with five modules:

| Module | What it decides |
|---|---|
| `rng` | every random choice, from one seed (SplitMix64, integer parts-per-million probabilities) |
| `clock` | virtual time as integer nanoseconds, moving only when the scheduler moves it |
| `net` | latency bounds, loss, reordering, partitions |
| `disk` | slow, corrupt, torn, lost-on-crash and full writes |
| `world` | the discrete-event queue tying them together |

Three properties are load-bearing:

**One seed, one run.** `maya_sim::replay(seed, body)` prints the seed,
virtual time and delivery counts when the body panics, then re-raises. A
failure that does not name its seed is a failure nobody can reproduce.

**Each model has its own stream.** `World::new` forks the root seed into a
network stream, a disk stream and a stream for the test's own choices. Adding
a draw in one does not shift the others — otherwise every recorded seed
silently changes meaning the next time somebody adds a `rng` call, which is
the failure mode that makes teams stop trusting seeds.

**Ties break on a sequence number, not on a hash.** Two events at the same
virtual nanosecond come out in insertion order on every machine. A
`BinaryHeap` keyed only on time would be free to reorder them, and a replay
that reorders is not a replay.

The crate has **no dependencies**, for the same class of reason
`maya-ledger-math` and `maya-dex` have none: a dependency is a second source
of decisions — its own RNG, its own hashing, its own iteration order — and any
one of them can make a replay a different run.

## Alternatives considered

**`madsim`.** The strongest option on paper: it replaces `tokio`, so the
node's *real* async code becomes deterministic rather than code written
against a model. Rejected for now on three counts. It takes ownership of the
runtime, which means every crate under test compiles against `madsim::tokio`
rather than `tokio` — a `cfg`-switched dependency threaded through a
45-member workspace, including `libp2p`, which madsim does not intercept. It
is a dependency with its own RNG inside the determinism boundary. And it
would land as part of a foundation phase whose subject is workspace layout,
where a failure could not be attributed.

This is the option to revisit. The reason to take it is the one thing this
ADR does not deliver: determinism of the node's *actual* concurrency, not of
a model of it.

**`turmoil`.** Lighter, and a good fit for the network half — but it models
the network only. The disk faults are half the value here: the undo journal,
the block store and the state batch all rest on "written, therefore durable"
(invariants 25, 26, 27), and the interesting failures are where that is
false. Using turmoil would mean a second harness for the disk and two
independent notions of "the seed".

**Extend `crates/node/src/network/sim.rs`.** Rejected: it lives inside the
node, so anything using it links RocksDB, libp2p and the whole chain. A
harness that cannot be used by `dex` or `stateless-core` is not the shared
harness the brief asks for.

## Consequences

- `maya-sim` models the *environment*. Code under test is written against the
  model, not against a socket — which is a real limitation and the reason
  madsim stays on the table.
- The existing simulation tests are not ported in this phase. They work;
  porting them is a change to test behaviour and belongs in its own commit.
  `PROGRESS.md` tracks it.
- Anything that changes a draw order in `net::deliver` or `disk::write`
  invalidates every recorded seed. Both functions say so, and both fix their
  draw order in a comment for that reason.
- Probabilities are integers in parts per million. A float probability would
  be one more thing for two platforms to disagree about, for a precision
  nobody needs.

## Revisit when

A test needs determinism of the node's real `tokio` scheduling rather than of
a model — at which point `madsim` is the answer and this crate becomes the
environment models it drives.
