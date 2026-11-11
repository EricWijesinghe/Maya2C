# ADR-124: Inclusion and fair ordering under DAG-BFT — what is already structural, and a sender-keyed order inside the batch

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled. **Consensus-critical: this ADR needs two human reviewers before it is accepted** (CLAUDE.md, Production Standing Orders), and the ordering change needs a spec update and conformance vectors.
**Date:** 2026-11-12
**Related:** parity row P-49, [prior-art/inclusion-lists.md](../prior-art/inclusion-lists.md), [ADR-025](ADR-025-mev-position.md), [ADR-027](ADR-027-dag-bft-in-the-node.md), [ADR-072](ADR-072-max-block-bytes.md), [ADR-115](ADR-115-post-quantum-vrf.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 11, 24, 33; Standing Order 4

## Context

Ethereum's inclusion lists (EIP-7547, now FOCIL/EIP-7805) exist because one
proposer builds each block and can leave transactions out; a committee
publishes lists the next block must honour (prior-art file).

**Finding that contradicts the brief.** Maya2C has no single proposer. Every
validator authors a vertex each round carrying its own batch
(`crates/dag-bft/src/vertex.rs:168-171`), the committed sub-DAG is ordered by
`(round, author)` as a pure function of the DAG (`commit.rs:11-12`,
`:136-160`), and the block's admitted set is a deficit round-robin over
authors with a quantum each (ADR-072 CON-13, `builder.rs:62-99`). A
transaction that any honest author holds is in that author's next vertex;
under `f < n/3` faults the honest authors' first-turn admission is at least
one full block cap (ADR-072's derivation). **Each validator's vertex is its
inclusion list**, enforced by the commit rule rather than a fork-choice
filter, and censorship needs every author that holds the transaction to
drop it. The inclusion half of P-49 is structural today; what it lacks is a
test that pins it as an invariant, and this ADR adds that.

The ordering half is real. Inside one author's batch the order is the
author's choice (`validator.rs:97-114`, FIFO from its queue, which it
controls), and execution is commit order restricted to the admitted set
(ADR-072 "Commit order"). So an author can place its own transaction
before a victim's that it carries: a sandwich *within its own batch*. ADR-025
states what is in core against that: uniform-price batch settlement for DEX
swaps (so a same-block sandwich clears at one price) and the optional sealed
mempool for everything else, with validators forbidden to sell ordering. It
leaves plain transfers and contract calls ordered by the carrying author.

Aequitas and Themis order by receive-time across validators, which needs
timestamps consensus does not have here (Standing Order 4, no clock);
Ethereum's answer is encrypted mempools (prior-art file).

## Decision

**Two things. (1) An invariant, pinned by a test: a valid transaction in
any honest author's committed vertex is admitted within a bounded number of
walks, whatever the other authors do. (2) An ordering rule: within a block,
transactions are executed in commit order of their *author*, and inside
each author's batch in the order of `blake3(anchor_certificate_digest ‖
sender)` then `nonce`, instead of the author's FIFO. The author chooses what
it carries, not where in its batch it lands.**

### The inclusion invariant (no new code path)

Stated for `docs/invariants.md` once the test exists (invariant 35 is not
claimed here; the number is given when the test lands): "A transaction in a
certified vertex of an honest author is admitted by `build_block` within
`⌈L / Q⌉` walks, where `L` is its length and `Q` the quantum, regardless of
other authors' payloads." That is ADR-072's termination bound, restated as a
property of inclusion and pinned by
`a_transaction_an_honest_author_carries_is_admitted_within_the_bound`
against a flood from `f` junk authors in `builder.rs`'s harness. Nothing is
built; a rule nobody checks is a comment, and this gives it a check.

### The ordering rule

In `build_block` (`builder.rs:145`), after `decode_fair_payloads` chooses the
admitted set, the admitted transactions of each author's vertex are sorted by
`(key, nonce)` where `key = blake3::derive_key("maya2c fair order v1",
anchor.digest ‖ sender_address)`, with `sender_address` the signer per the
frame (`public_key` / `suite_auth` / `multisig`). Vertices keep commit order
between them. Properties:

- **Deterministic**: a pure function of the sub-DAG (invariant 24; the
  anchor's certificate digest is in it).
- **Nonce-safe**: one sender's transactions stay contiguous and ascending,
  so a `k, k+1` pair in one batch still applies.
- **Author-blind at pack time**: the anchor certificate of round `r` is
  formed after every vertex of rounds `≤ r` is sealed, from `n − f` votes
  the batch author does not control (`vertex.rs:166`, `:230`), so an author
  cannot compute its batch's order when packing it.
- **Residual, stated plainly**: the anchor's *author* (the leader of that
  even round) assembles the anchor vertex after seeing the DAG and could try
  parents or payload variations to steer the digest. The space it controls
  is its own vertex's content; each try costs it a full vertex and the
  votes must still arrive. This is grinding bounded by one leader's options
  per two rounds, not a prevention. Mixing in ADR-115's beacon randomness
  does not help: the beacon as of the previous block is known before the
  batch is packed. The reviewers are asked to judge whether this residual
  is acceptable or whether the sealed mempool (ADR-025) must be the stated
  answer for order-sensitive transactions. The ADR's position is the
  latter: this rule removes the *carrying author's* free choice, which is
  the common case; the sealed mempool removes the rest.

### Interaction with the sealed mempool

Sealed envelopes are executed by `settle_sealed` in big-endian height key
order (`sealed/mod.rs:15-26`), after transactions; this rule does not touch
them. ADR-112's `WithReports` and ADR-121's `WithLane` wrappers are ordered
by their inner signer like any transaction.

### What this is not

- Not a receive-time order (Aequitas): no clock.
- Not a committee inclusion list (FOCIL): `n` authors already are one.
- Not a fix for cross-block MEV: ADR-025 leaves that to the market.
- Not a change to `(round, author)` between vertices: that order is what
  ADR-072's fairness proof is written over.

## Alternatives considered

- **Do nothing; cite ADR-025.** The sealed mempool is per-network and
  trusts a committee setup; the within-batch order is free to fix.
- **Sort the whole block by `(key, nonce)` across authors.** Breaks
  ADR-072's commit-order emission and its fair-share derivation, which is
  stated per author prefix. Rejected.
- **Sort by txid.** Separates one sender's `k, k+1`, so the second fails
  and the sender resubmits. The sender key keeps them together.
- **Key on the previous block's beacon** (ADR-115). Known at pack time:
  the author can order its batch to the key. Rejected.
- **Threshold-encrypt every transaction (LUCID-style).** The sealed mempool
  already exists as an option; making it mandatory is an economics and
  trust decision this ADR does not take.
- **Receive-time ordering with validator timestamps.** Standing Order 4.

## Consequences

- One sort in `build_block`, one derive-key context, one invariant test;
  no state, no wire change.
- The spec's block-derivation section changes (conformance vectors: a
  sub-DAG, its anchor digest, the admitted set, the executed order), and
  the `spec-ref` oracle `consensus::decode_fair` gains the sort.
- A wallet submitting `k, k+1` to one validator still sees them apply in
  order; one submitting to two validators sees commit order between the
  vertices, as today.
- Two human reviewers, by name, in the PR that implements it.

## Invariants touched

- **24**: execution order is a function of the sub-DAG; the declared
  `state_root` must reproduce it; `decode_fair` oracle updated.
- **11**: no trusted party; no committee beyond the one that exists.
- **33**: unchanged; the sort does not change the admitted set.
- Standing Order 4: no clock, no map order, BLAKE3 keys.

## Activation

`FeatureSet::FAIR_ORDER = 1 << 31` (claimed in `adr-claims.txt`), compiled
`FAIR_ORDER_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082 with the
v4 bundle at **genesis + 15,552,000**. The inclusion invariant test is not
gated: it pins behaviour that holds today and lands with the ADR's PR.
Before activation `build_block` emits commit order, as now; a block built
with the sort before the height has a wrong `state_root` and is refused
(invariant 24), which is how a feature bit on ordering is enforced.

## Test plan

Red tests, `crates/node/src/consensus/bft/builder.rs` and
`crates/node/tests/fair_order_tests.rs`:

- `a_transaction_an_honest_author_carries_is_admitted_within_the_bound`
  (the invariant; passes today, must keep passing)
- `an_authors_batch_is_executed_in_sender_key_order_not_fifo`
- `one_senders_nonces_stay_contiguous_and_ascending`
- `two_authors_batches_keep_commit_order_between_them`
- `the_order_changes_when_the_anchor_digest_changes` (author-blindness)
- `the_order_is_the_same_on_every_node_for_one_sub_dag` (under `maya-sim`)
- `before_activation_a_sorted_block_is_refused_by_root` (invariant 24)
- `exploit_replays::a_within_batch_sandwich_lands_behind_the_victim_half_the_time`
  — a statistical replay over 1,000 anchors, reported with its real count.

Spec: `spec/04-consensus.md` block derivation + vectors FO-1..FO-2;
`cargo xtask spec-coverage` lists them.

## Open questions for Eric (economics only)

None. Ordering changes no fee or parameter. The non-economic question — is
the leader-grinding residual acceptable, or must the sealed mempool be the
stated answer for order-sensitive transactions — is for the two human
reviewers, and the ADR records the position above.
