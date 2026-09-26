---
title: 'Real-World Assets'
editUrl: false
# GENERATED from docs/rwa.md by scripts/ingest.mjs. Edit the source, not this.
---
Tokenised assets, atomic settlement, jurisdictional rules, and paying ten
thousand holders in one block.

- `crates/rwa/` — the records. Chain-free, so the decoder fuzzes alone.
- `ledger-math::distribute` — the pro-rata split, Kani-checked.
- `crates/node/src/state/rwa.rs` — prefixes and their place in the state root.
- `crates/node/src/state/rwa_exec.rs` — issuance, DvP, eligibility, revenue.
- `crates/node/tests/rwa_tests.rs`.

---

## Three collisions with invariants already in the tree

### 1. A DvP cannot return an error — invariant 7

> *A trade that merely loses is a no-op, never an `Err`. A failing transaction
> fails its whole block here, so making any of these an error hands every trader
> a way to void a block.*

Delivery-versus-payment means both legs or neither, and the obvious
implementation errors when a leg cannot settle. On this chain that voids the
block — so **anyone could kill any block** by submitting a DvP they know will
fail. The counterparty need not be involved.

So `settle_dvp` returns a `DvpOutcome` and never an error. Every reason a swap
does not happen — the buyer is short, the seller does not hold the units, the
buyer is not eligible, the asset does not exist, the page is full — is a no-op.
The nonce advances, the fee is spent, nothing moves.

Atomicity comes from **reading both sides before writing either**: nothing is
written until every check has passed, so a half-applied swap is not reachable.
It does not come from an error unwinding one.

`a_dvp_that_cannot_settle_leaves_the_block_valid` walks four failure modes and
asserts the block applies each time and the state is byte-identical after.

### 2. "Dynamic regulatory rule hooks" collides with invariant 13

> *No governance key's value is a program. Native code is never fetched from
> chain state and run.*

So an issuer cannot supply logic. A `TransferRule` names three things — a claim
schema, a predicate, and the credential issuers whose word counts — and the
binary evaluates them. The issuer **selects** a rule; it does not write one.

The predicate shapes are exactly the three `maya_zk_stark::credential::Predicate`
proves. A rule the disclosure circuit cannot evaluate is a rule no holder can
satisfy privately, and therefore one they must satisfy by revealing the value.

### 3. Pro-rata dust would fail the invariant guard

Splitting revenue across ten thousand holders by integer division leaves a
remainder. `Σ payouts < total` is value destroyed, and the invariant guard
refuses any block whose value deltas do not balance — so a rounding rule that
lost a base unit is not a small unfairness. It is a distribution nobody can
mine.

---

## The distribution rule, and why the tie-break forks the chain

`ledger_math::distribute` is largest remainder — Hamilton's method. Everyone
gets `floor(total × weight / Σ weight)`; the `k` base units left over go to the
`k` largest fractional remainders, so every holder lands within one base unit of
their exact share.

Two holders can have the **same** remainder. If the order between them were
unspecified, two validators would hand the spare unit to different accounts,
produce different state roots, and the chain would split over one base unit. So
the sort key is `(remainder, index)` — the index breaks every tie, total and
identical on every machine.

It lives in `ledger-math` because that crate is dependency-free and Kani can
compile it. Two proofs: the payouts sum to exactly the total, and no holder is
paid more than one unit above their exact share. The second matters because a
rule that paid one holder everything would satisfy the first.

The function allocates nothing — the caller supplies the output and scratch
buffers. `no_std` here means there is no allocator to reach for even by
accident, which is the execution directive about consensus loops made
mechanical rather than aspirational.

---

## Ten thousand dividends in one block

Not ten thousand transactions. At ~13 KB each — hybrid signature plus key —
that would be 130 MB of block. It is **one** transaction that fans out over the
cap table.

Consequences that are real rather than incidental:

- **The cap table is paged**, 256 holders each, like the revocation bitmap. A
  single unbounded table is a record nobody wrote a limit for, and every write
  journals the whole of it.
- **`MAX_DISTRIBUTION_PAGES` is 40** — 10,240 holders. A bound, because the
  alternative is a transaction whose cost is whatever an issuer's cap table
  happens to be, and a block whose validation time nobody can predict.
- **The undo journal for that block holds ten thousand prior account values.**
  Invariant 8 requires it and it is not free.
- **A round cannot be settled twice.** The round number is part of the key, and
  a round already written is refused — otherwise a replay pays every holder
  again out of an issuer who agreed to pay once.

What the test asserts is conservation: the holders' balances rise by exactly
what the issuer's fell. Not "roughly", not "within dust".

---

## Why eligibility is cached rather than proved per transfer

A disclosure proof is a STARK (ADR-008); the joinsplit, the nearest measured
one, is ~383 KB. On a block with ten thousand transfers, one per transfer would
be **gigabytes** every node downloads and verifies, forever, for a check whose
answer changes rarely.

So the proof is verified once in its own transaction and cached in `r:elg:`; the
transfer path reads a record and a height. The cache expires — an eligibility
that never went stale would be a jurisdiction check answered by a proof from
years ago — and the expiry is in **block height**, never a timestamp
(invariant 9). A miner may write any `header.timestamp`, so a freshness rule
measured in seconds would read as safety and provide none.

A stale record fails the same way an absent one does: the DvP is a no-op.

---

## What no record can hold

A prospectus. `LegalAttestation` carries a document **hash** and a reference to
where the document lives. A chain is permanent and public, so a prospectus
written to it cannot be corrected, and it likely names people.

The same reasoning as the identity records: the type has no field for it.

---

## State

One prefix, `r:`, one `StateLayer::Rwa`, one `RECORD_LAYERS` entry. Five
sub-prefixes: `r:tok:`, `r:cap:`, `r:leg:`, `r:dst:`, `r:elg:`. Separate because
they change at different rates — a cap table page moves on every transfer, a
token record never after issuance — and folding them together would journal the
token on every trade.

The layer folds only when non-empty, so a chain with no tokenised assets
produces the root it would have produced before the subsystem existed.

Adding the five `TxKind` variants produced compile errors at `Module::of` and
`apply_kind_for` until each was assigned a module. That is the exhaustive match
in the invariant guard doing its job for the third time this week.

---

## Status

**RESEARCH.** Nothing else in consensus reads an RWA record; a chain with no
issuance is byte-identical to one built before the subsystem.

Promoting it means: a decision about who may issue (today anyone can), the
disclosure-proof verification wired into `RecordEligibility` rather than
recorded on the issuer's word, and a view on whether `MAX_DISTRIBUTION_PAGES`
belongs in the governed parameter table rather than a `const` — invariant 17
says a governed value is read from state, and 40 is currently neither governed
nor obviously permanent.
