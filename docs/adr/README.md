# Architecture Decision Records

One file per decision that would otherwise be re-litigated by whoever arrives
next. An ADR records what was decided, what it cost, and what would have to be
true to revisit it — not what the code does. For what the code does, read
`CLAUDE.md` and `docs/architecture-vision.md`.

A record is written **when the decision is made**, not afterwards. If a
decision turns out to be wrong it gets a new ADR that supersedes the old one;
the old one stays, with its status changed, because the reasoning that was
wrong is the useful part.

## Format

```
# ADR-NNN: <title>

**Status:** Proposed | Accepted | Superseded by ADR-NNN
**Date:** YYYY-MM-DD

## Context
What made a decision necessary. Measurements, not impressions.

## Decision
What was chosen, stated so that a reader can tell whether the code obeys it.

## Alternatives considered
Each with the reason it was not chosen.

## Consequences
What this costs, including the parts that are worse than the alternative.

## Revisit when
The observation that would make this worth reopening.
```

## Index

| ADR | Title | Status |
|---|---|---|
| [001](ADR-001-workspace-layout.md) | Workspace layout, and why the directory move is deferred | Accepted |
| [002](ADR-002-feature-tiers.md) | Feature tiers gate services, never consensus | Accepted |
| [003](ADR-003-build-profiles.md) | Build profiles and the disk ceiling | Accepted |
| [004](ADR-004-reality-ledger.md) | `features.toml` as the reality ledger | Accepted |
| [005](ADR-005-dependency-unification.md) | One version per dependency, and the duplication that remains | Accepted |
| [006](ADR-006-simulation-harness.md) | A hand-rolled deterministic simulator, not `madsim` or `turmoil` | Accepted |
| [007](ADR-007-signature-suites.md) | Signature suites, a registry, and a suite-tagged envelope | Accepted |
| [008](ADR-008-transparent-zk.md) | Plonky3 STARKs as the only proof system | Accepted |
| [009](ADR-009-kem-suites.md) | KEM suites and the two combiners | Accepted |
| [010](ADR-010-entropy.md) | Entropy — every source health-tested, all mixed into one HMAC-DRBG | Accepted |
| [011](ADR-011-threshold-custody.md) | Threshold custody — on-chain multisig now, threshold lattice signing later | Accepted |
| [012](ADR-012-hash-lock-htlcs.md) | Hash-lock HTLCs are live; lattice locks stay RESEARCH | Accepted |
| [013](ADR-013-suite-envelope-activation.md) | The suite envelope and multisig are live from genesis | Accepted |
| [014](ADR-014-threshold-raccoon.md) | Threshold Raccoon for threshold lattice custody (RESEARCH) | Accepted |
| [015](ADR-015-consensus-design.md) | One consensus design — DAG-BFT orders, work never does | Accepted |
| [016](ADR-016-launch-scope.md) | Launch scope freeze — what mainnet v1 is, and what waits | Accepted |
| [017](ADR-017-parallel-execution.md) | Parallel execution — optimistic with in-order validation first | Accepted |
| [018](ADR-018-async-execution.md) | Synchronous execution for v1; asynchronous (D = 2) next | Accepted |
| [019](ADR-019-storage-engine.md) | Stay on RocksDB until it is the measured bottleneck | Accepted |
| [020](ADR-020-spec-conformance-and-upgrades.md) | A spec with an independent reference; upgrades halt instead of fork | Accepted |
| [021](ADR-021-vote-certificates.md) | Vote certificates — a signature list for v1, a 100-validator cap from its measured cost | Accepted |
| [022](ADR-022-remote-signer-backends.md) | Remote signer — keystore backend now, HSM/KMS when one can be tested | Accepted |
