# ADR-112: Pull oracle — signed batch reports carried by the transaction that uses them

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-46, [prior-art/pull-oracle.md](../prior-art/pull-oracle.md), [ADR-024](ADR-024-oracle-choice.md), [ADR-057](ADR-057-hybrid-oracle-pc-testnet.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), `docs/oracle.md`; invariants 7, 8, 9, 11, 17, 28, 33

## Context

The oracle is a push feed. A `FeedSubmission { feed_id, round,
observed_height, observations }` carries up to `MAX_OBSERVATIONS = 21`
`FeedObservation { value: U256, public_key, signature }`
(`crates/node/src/core/oracle_payload.rs:50-80`). `submit_feed`
(`crates/node/src/state/oracle_exec.rs:266-370`) refuses a round at or below
the recorded one (`:292-298`), verifies every observation under its hybrid
key (`:325-334`), needs a registry quorum (`:339`), takes the lower median
(`:346`, `oracle/feed.rs:3-15`), and stamps `updated_height` with the chain's
height, not the authorities' (`:352-357`, invariant 9). Nothing checks the
**sender**: anyone holding a quorum's signatures may relay them. Per block,
`OracleMaxFeedSubmissionsPerBlock` (`params.rs:72`, default
`MAX_FEED_SUBMISSIONS_PER_BLOCK = 8`, `oracle_exec.rs:70`).

One observation is `32 + 1_984 + 11_165 = 13_181` bytes
(`OBSERVATION_SIZE`, `oracle_payload.rs:53`; `HYBRID_PUBLIC_KEY_LEN`
`crypto/hybrid.rs:98`; `ML_DSA_65_SIGNATURE_LEN = 3_309`
`crypto-pq/src/suite/ml_dsa.rs:22`; SLH-DSA `SIGNATURE_LEN = 7_856`
`crypto-pq/src/sig.rs:68`). A quorum of three is about 40 KB per feed per
round; each observation signs one feed (`feed.rs:25-31`). Contracts read
`OracleValue { value, updated_height, round, observations }`
(`crates/vm/src/host.rs:64-74`) through `oracle_read`, `oracle_read_u256`
and `oracle_feed_age` (`crates/vm/src/runtime.rs:889-927`). The oracle is
optional and absent by default (invariant 11).

What is missing against Pyth and Chainlink Data Streams (prior-art file):
(1) a transaction cannot bring its own fresh price and use it in the same
step — a swap or call that needs a round must trust that a separate
`SubmitFeed` landed earlier in the block; (2) one signature covers one feed,
so a stream of many feeds costs a quorum of hybrid signatures per feed; (3)
no off-chain delivery is defined.

**Finding that contradicts the brief.** The parity row says "none". The
relay half of a pull oracle exists: `submit_feed` has no sender check, so a
user may already pull a quorum's signatures off chain and push them. What
is absent is atomic use and batch signing. This ADR builds those two.

## Decision

**Two changes. An authority signs a Merkle root over many feeds once, and a
transaction may carry such reports and have them applied before its own
kind.**

### Batch reports (the stream unit)

`FeedObservation` v2: an authority signs `report_root` over leaves
`blake3::derive_key("maya2c/oracle/leaf/v2", feed_id ‖ value(32 LE) ‖ round(8) ‖ observed_height(8))`
built by `state::merkle` (the same tree as `tx_root`, invariant 24). A
`FeedReport` is:

```
FeedReport { observed_height: u64, roots: Vec<(public_key, signature over report_root)>, entries: Vec<FeedEntry { feed_id, round, values: Vec<(authority_index: u8, value: U256, proof: Vec<[u8;32]>)> }> }
```

For each feed entry the node verifies each `(authority_index, value)`
against that authority's signed root by its Merkle proof, then applies the
existing rules unchanged: registry membership, no duplicate authority, round
strictly above the record, quorum, lower median, `updated_height =
context.height`. One hybrid verification per authority per report instead
of per feed; a report over 32 feeds by a quorum of 3 costs 3 signatures and
96 proofs of `log2(32) = 5` hashes. A v1 `FeedSubmission` stays valid.

### Atomic use

A new kind `TxKind::WithReports { reports: Vec<FeedReport>, inner:
Box<TxKind> }` in the existing v6 payload frame (no new wire version). Apply
order: each report in order, then `inner` as if it were the transaction's
kind. Rules:

- A report whose every entry is stale (round at or below the record) is a
  **no-op**, not a refusal: two pullers racing for the same round must not
  hand either a way to void the block (invariant 7's reason). The inner
  kind then reads the record another transaction updated, which is at
  least as fresh.
- A report with a bad signature, a non-authority, or a short quorum refuses
  the whole transaction, as `submit_feed` does today. A lie is not a race.
- `inner` may not be `WithReports` (one level), `SubmitFeed`, or
  `RotateAuthorities`.
- Reports count toward `OracleMaxFeedSubmissionsPerBlock`.
- `Module::of(WithReports)` is `Module::of(inner)`; the report part is
  gated by `Module::Oracle`. A breaker on `Oracle` makes every report a
  no-op (the inner kind still runs on the committed record), never a
  refusal of a transfer (invariant 28).

### Freshness for the consumer

`ContractCall` and `Swap` do not change. A contract that needs a bound
reads `oracle_feed_age` and traps if too old, as today; `WithReports`
makes "age 0" reachable in one transaction. Freshness is a height, never a
timestamp (invariant 9; `feed.rs:93-99`).

### Off-chain delivery (not consensus)

Authorities publish signed `FeedReport`s at a cadence over a gossip topic
`maya2c/oracle/report/v2` and the gateway (`crates/api-gateway`) serves the
latest per feed. A puller fetches, assembles and submits. This is a
service; the chain verifies and never fetches (invariant 11; CATALOG P-42).

### 256-bit arithmetic

Values are `U256` compared and sorted for the median; no multiplication.
Merkle leaves are hashes. No 512-bit intermediate.

## Alternatives considered

- **Signature aggregation.** No aggregate for ML-DSA or SLH-DSA exists;
  the batch root is the available way to make one signature cover many
  values.
- **Verify reports in a contract (Pyth, Chainlink verifier proxy).** The
  VM has no signature host function, and adding one for a 7,856-byte
  SLH-DSA signature is a fuel-pricing decision (invariant 21) on its own.
  The node verifies natively, as it does today.
- **A Wormhole-style second attestation layer.** A second trusted set on
  top of the first; rejected (`docs/oracle.md` "one trusted party is one
  too many already").
- **Timestamps in reports for staleness.** Invariant 9. Signed heights are
  "what they meant"; the chain's height is what counts.

## Consequences

- One transaction can update and consume a price; low-latency streams are
  possible at the cost the puller pays in bytes.
- Verification cost per report is bounded by the registry size (21) and
  the proofs; the per-block ceiling is the existing governed one.
- Two new payload shapes (`FeedReport`, `WithReports`), one arm in
  `Module::of`, `describe.rs`, the vault door list (invariant 32: a
  `WithReports` wrapping a `Transfer` is a transfer door and counts toward
  the window; wrapping anything else is refused from a vault).
- `o:` records stay in the undo journal (invariant 8).

## Invariants touched

- **9**: freshness by height; signed heights are not used for staleness.
- **11**: no registry, no reports: `WithReports` refused on a chain without
  an oracle, and the state root is what it would have been.
- **7**: a stale report is a no-op.
- **8**: feed records in the journal, unchanged.
- **17**: the per-block ceiling from state.
- **28**: `Module::of(WithReports) = Module::of(inner)`, stated in the match.
- **33**: reports are transaction bytes under the cap; a 21-authority report
  of 64 feeds is measured and reported with the implementation.

## Activation

`FeatureSet::PULL_ORACLE = 1 << 20` (claimed in `adr-claims.txt`), compiled
`PULL_ORACLE_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082 with the
v4 bundle at **genesis + 15,552,000**. Before activation `WithReports` and a
v2 observation are refused on admission and on apply.

## Test plan

Red tests, `crates/node/tests/pull_oracle_tests.rs`:

- `a_report_updates_the_record_and_the_inner_swap_reads_age_zero`
- `a_stale_report_is_a_no_op_and_the_inner_kind_still_runs`
- `a_report_with_one_bad_proof_refuses_the_whole_transaction`
- `a_non_authority_root_is_refused`
- `one_root_signature_covers_sixty_four_feeds`
- `reports_count_toward_the_per_block_ceiling`
- `with_reports_on_a_chain_without_an_oracle_is_refused_and_moves_no_root`
- `a_reorg_restores_every_feed_record_touched_by_a_report`

Vectors (`spec/03-state.md` OR-5..OR-8, checked by `cargo xtask
spec-coverage`): a leaf, a root over four feeds, a report's bytes, a
`WithReports` transaction's bytes and txid. A benchmark of report
verifications per second per core with commit and hardware (Standing
Order 2).

## Open questions for Eric (economics and pricing only)

1. Does a report pay only its bytes, or a fixed fee per feed updated,
   with some share rebated to the signing authorities (today authorities
   earn nothing)?
2. Should the puller's fee be lower when a report is a no-op (lost race),
   or is the byte fee simply the price of racing?
